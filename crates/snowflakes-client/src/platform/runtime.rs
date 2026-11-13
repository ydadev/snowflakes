//! Windows process lifetime. The control thread owns Session and disk operations;
//! the GUI thread owns HWND/HICON and only reads immutable status snapshots.
use super::{
    native::{self, Envelope, Identity, Instance, Server},
    panel::Panel,
    system_events::Conditions,
    weather::{self, Report},
};
use crate::{
    control::{self, Command, Controller, Response, Setting, Status},
    session::{ExitReason, Limits, PauseReason},
    storage::{CacheClock, UserStore},
    ui::View,
};
use snowflakes_protocol::WeatherMode;
use std::{
    cell::Cell,
    path::PathBuf,
    rc::Rc,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicIsize, Ordering},
        mpsc::{self, Receiver, SyncSender},
    },
    thread,
    time::{Duration, Instant},
};
use windows::{
    Win32::{
        Foundation::*,
        System::{LibraryLoader::GetModuleHandleW, Registry::*, StationsAndDesktops::*},
        UI::{HiDpi::*, Shell::*, WindowsAndMessaging::*},
    },
    core::{PCWSTR, Result, w},
};

const CALLBACK: u32 = WM_APP + 1;
const MENU_SNOW: usize = 1;
const MENU_RAIN: usize = 2;
const MENU_PAUSE: usize = 3;
const MENU_CLEAR: usize = 4;
const MENU_FOLLOW: usize = 5;
const MENU_ABOUT: usize = 6;
const MENU_QUIT: usize = 7;
const MENU_SETTINGS: usize = 8;

fn reg(name: &str) -> Result<Option<u32>> {
    let name = native::wide(name);
    let mut value = 0_u32;
    let mut size = 4;
    let result = unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            w!("Software\\Policies\\Snowflakes"),
            PCWSTR(name.as_ptr()),
            RRF_RT_REG_DWORD | RRF_SUBKEY_WOW6464KEY,
            None,
            Some((&mut value as *mut u32).cast()),
            Some(&mut size),
        )
    };
    match result {
        ERROR_SUCCESS => Ok(Some(value)),
        ERROR_FILE_NOT_FOUND | ERROR_PATH_NOT_FOUND => Ok(None),
        other => Err(native::failure(other)),
    }
}
fn policy() -> Result<Limits> {
    let mut limits = Limits::default();
    if let Some(v) = reg("Enabled")? {
        limits.enabled = v != 0;
    }
    if let Some(v) = reg("AllowedModes")? {
        limits.allow_snow = v & 1 != 0;
        limits.allow_rain = v & 2 != 0;
    }
    if let Some(v) = reg("MaxIntensityPercent")? {
        limits.max_intensity_percent = v.min(100) as u8;
    }
    if let Some(v) = reg("MaxDriftSizePercent")? {
        limits.max_drift_size_percent = v.min(100) as u8;
    }
    if let Some(v) = reg("MaxPuddleSizePercent")? {
        limits.max_puddle_size_percent = v.min(100) as u8;
    }
    if let Some(v) = reg("MaxFps")? {
        limits.max_fps = if v < 60 { 30 } else { 60 };
    }
    if let Some(v) = reg("PauseOnFullscreen")? {
        limits.pause_on_fullscreen = v != 0;
    }
    if let Some(v) = reg("AllowRemoteSession")? {
        limits.allow_remote_session = v != 0;
    }
    Ok(limits)
}

fn apply_report(status: &mut Status, report: &Report) {
    status.renderer = report.renderer.into();
    status.rendering = report.rendering;
    status.particle_count = report.particles;
    status.monitor_count = report.monitors;
    status.effective_target_fps = report.effective_target_fps;
    status.reduced_detail = report.reduced_detail;
}

struct Owner {
    sender: SyncSender<Envelope>,
    view: Arc<Mutex<View>>,
    graphics: Arc<Mutex<Report>>,
    conditions: Arc<Conditions>,
    stop: Arc<AtomicBool>,
    ui_window: Arc<AtomicIsize>,
    panel_window: Arc<AtomicIsize>,
    show_settings: Arc<AtomicBool>,
    worker: Option<thread::JoinHandle<()>>,
}
impl Owner {
    fn start(
        mut controller: Controller,
        limits: Limits,
        exit_marker: PathBuf,
        gui_available: bool,
    ) -> Self {
        let _ = controller.session.set_limits(limits, Instant::now());
        let view = Arc::new(Mutex::new(controller.ui_view(Instant::now())));
        let graphics = Arc::new(Mutex::new(Report::default()));
        let conditions = Arc::new(Conditions::default());
        let worker_conditions = conditions.clone();
        let worker_graphics = graphics.clone();
        let stop = Arc::new(AtomicBool::new(false));
        let ui_window = Arc::new(AtomicIsize::new(0));
        let panel_window = Arc::new(AtomicIsize::new(0));
        let worker_panel = panel_window.clone();
        let show_settings = Arc::new(AtomicBool::new(false));
        let worker_settings = show_settings.clone();
        let worker_window = ui_window.clone();
        let (sender, receiver) = mpsc::sync_channel::<Envelope>(8);
        let worker_view = view.clone();
        let worker_stop = stop.clone();
        let worker = thread::spawn(move || {
            if gui_available {
                unsafe {
                    let _ =
                        SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
                }
            }
            let mut next_policy = Instant::now() + Duration::from_secs(60);
            let mut next_discovery = Instant::now();
            let mut monitors = None;
            while !worker_stop.load(Ordering::Acquire) {
                if let Ok(envelope) = receiver.recv_timeout(Duration::from_millis(100)) {
                    worker_conditions.apply_to(&mut controller.session);
                    let mut response = if Instant::now() > envelope.deadline {
                        Response::error(control::TIMEOUT, "dispatch_timeout")
                    } else if matches!(&envelope.command, Command::Settings {}) && !gui_available {
                        Response::error(control::NO_SESSION, "interactive_session_required")
                    } else {
                        let quitting = matches!(&envelope.command, Command::Quit {});
                        let marker_failed = quitting
                            && crate::storage::atomic_write(&exit_marker, b"closed\n").is_err();
                        let response = controller.handle(envelope.command, Instant::now());
                        if marker_failed {
                            Response::error(control::INTERNAL, "closed_exit_marker_not_saved")
                        } else {
                            response
                        }
                    };
                    if let (Some(status), Ok(report)) =
                        (&mut response.status, worker_graphics.lock())
                    {
                        apply_report(status, &report);
                    }
                    if controller.take_settings_request() {
                        worker_settings.store(true, Ordering::Release);
                    }
                    if let Ok(mut current) = worker_view.lock() {
                        *current = controller.ui_view(Instant::now());
                        current.monitors = monitors.clone();
                        if let Ok(report) = worker_graphics.lock() {
                            apply_report(&mut current.status, &report);
                        }
                    }
                    let _ = envelope.reply.send(response);
                }
                let now = Instant::now();
                if gui_available && now >= next_discovery {
                    monitors = super::scene::screens()
                        .ok()
                        .map(|screens| screens.iter().map(crate::ui::Monitor::from).collect());
                    next_discovery = now + Duration::from_secs(2);
                }
                worker_conditions.apply_to(&mut controller.session);
                let _ = controller.session.tick(now);
                if now >= next_policy {
                    match policy() {
                        Ok(limits) => {
                            let _ = controller.session.set_limits(limits, now);
                        }
                        Err(_) => {
                            controller.session.close(ExitReason::Policy);
                        }
                    }
                    next_policy = now + Duration::from_secs(60);
                }
                if let Ok(mut current) = worker_view.lock() {
                    *current = controller.ui_view(now);
                    current.monitors = monitors.clone();
                    if let Ok(report) = worker_graphics.lock() {
                        apply_report(&mut current.status, &report);
                    }
                }
                if controller.session.snapshot(now).exit.is_some() {
                    if controller.session.snapshot(now).exit == Some(ExitReason::Policy) {
                        let _ = crate::storage::atomic_write(&exit_marker, b"policy\n");
                    }
                    worker_stop.store(true, Ordering::Release);
                    for window in [&worker_window, &worker_panel] {
                        let raw = window.load(Ordering::Acquire);
                        if raw != 0 {
                            unsafe {
                                let hwnd = Some(HWND(raw as *mut core::ffi::c_void));
                                let _ = PostMessageW(hwnd, WM_CANCELMODE, WPARAM(0), LPARAM(0));
                                if Arc::ptr_eq(window, &worker_panel) {
                                    let _ = PostMessageW(hwnd, WM_CLOSE, WPARAM(0), LPARAM(0));
                                }
                            }
                        }
                    }
                }
            }
        });
        Self {
            sender,
            view,
            graphics,
            conditions,
            stop,
            ui_window,
            panel_window,
            show_settings,
            worker: Some(worker),
        }
    }
    fn submit(&self, command: Command) -> std::result::Result<Receiver<Response>, Box<Response>> {
        let (reply, response) = mpsc::sync_channel(1);
        let envelope = Envelope {
            command,
            reply,
            deadline: Instant::now() + Duration::from_millis(1500),
        };
        if self.sender.try_send(envelope).is_err() {
            return Err(Box::new(Response::error(
                control::INTERNAL,
                "instance_busy",
            )));
        }
        Ok(response)
    }
    fn request(&self, command: Command) -> Response {
        let response = match self.submit(command) {
            Ok(response) => response,
            Err(error) => return *error,
        };
        response
            .recv_timeout(Duration::from_secs(2))
            .unwrap_or_else(|_| Response::error(control::TIMEOUT, "dispatch_timeout"))
    }
}
impl Drop for Owner {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

struct TrayState {
    settings_requested: Cell<bool>,
    follow: Cell<bool>,
    paused: Cell<bool>,
    pending: Cell<Option<Command>>,
    taskbar_created: u32,
    restore: Cell<bool>,
    menu_open: Cell<bool>,
}
struct Tray {
    hwnd: HWND,
    icon: HICON,
    state: Rc<TrayState>,
}
impl Tray {
    fn create() -> Result<Self> {
        unsafe {
            let instance = HINSTANCE(GetModuleHandleW(None)?.0);
            let class = WNDCLASSW {
                lpfnWndProc: Some(tray_proc),
                hInstance: instance,
                lpszClassName: w!("SnowflakesControlWindow"),
                ..Default::default()
            };
            if RegisterClassW(&class) == 0 {
                return Err(windows::core::Error::from_thread());
            }
            let state = Rc::new(TrayState {
                settings_requested: Cell::new(false),
                follow: Cell::new(true),
                paused: Cell::new(false),
                pending: Cell::new(None),
                taskbar_created: RegisterWindowMessageW(w!("TaskbarCreated")),
                restore: Cell::new(false),
                menu_open: Cell::new(false),
            });
            let hwnd = CreateWindowExW(
                WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
                w!("SnowflakesControlWindow"),
                w!("Snowflakes"),
                WS_OVERLAPPED,
                0,
                0,
                0,
                0,
                None,
                None,
                Some(instance),
                Some((&*state as *const TrayState).cast()),
            )?;
            // Original procedural 32px icon; no external assets.
            let mut pixels = vec![0_u8; 32 * 32 * 4];
            for y in 0..32_i32 {
                for x in 0..32_i32 {
                    let i = (y * 32 + x) as usize * 4;
                    let dx = (x - 16).abs();
                    let dy = (y - 16).abs();
                    let flake = (dx <= 1 || dy <= 1 || (dx - dy).abs() <= 1) && dx.max(dy) <= 11;
                    pixels[i..i + 4].copy_from_slice(if flake {
                        &[245, 245, 245, 255]
                    } else {
                        &[110, 60, 25, 255]
                    });
                }
            }
            let icon = match CreateIcon(
                Some(instance),
                32,
                32,
                1,
                32,
                [0_u8; 128].as_ptr(),
                pixels.as_ptr(),
            ) {
                Ok(icon) => icon,
                Err(error) => {
                    let _ = DestroyWindow(hwnd);
                    return Err(error);
                }
            };
            let tray = Self { hwnd, icon, state };
            tray.notify(NIM_ADD)?;
            Ok(tray)
        }
    }
    fn notify(&self, action: NOTIFY_ICON_MESSAGE) -> Result<()> {
        unsafe {
            let mut data = NOTIFYICONDATAW {
                cbSize: size_of::<NOTIFYICONDATAW>() as u32,
                hWnd: self.hwnd,
                uID: 1,
                uFlags: NIF_MESSAGE | NIF_ICON | NIF_TIP,
                uCallbackMessage: CALLBACK,
                hIcon: self.icon,
                ..Default::default()
            };
            let text = native::wide("Snowflakes — эффекты в разработке");
            data.szTip[..text.len()].copy_from_slice(&text);
            if !Shell_NotifyIconW(action, &data).as_bool() {
                return Err(windows::core::Error::from_thread());
            }
            if action == NIM_ADD {
                data.Anonymous.uVersion = NOTIFYICON_VERSION_4;
                if !Shell_NotifyIconW(NIM_SETVERSION, &data).as_bool() {
                    return Err(windows::core::Error::from_thread());
                }
            }
            Ok(())
        }
    }
    fn update(&self, status: &Status) {
        self.state.follow.set(status.follow_server);
        self.state
            .paused
            .set(status.pauses.contains(&PauseReason::Manual));
        if self.state.restore.replace(false) {
            let _ = self.notify(NIM_ADD);
        }
    }
    fn error_notice(&self) {
        unsafe {
            let mut data = NOTIFYICONDATAW {
                cbSize: size_of::<NOTIFYICONDATAW>() as u32,
                hWnd: self.hwnd,
                uID: 1,
                uFlags: NIF_INFO,
                dwInfoFlags: NIIF_ERROR,
                ..Default::default()
            };
            let title = native::wide("Snowflakes");
            let text =
                native::wide("Не удалось выполнить команду. Проверьте настройки и повторите.");
            data.szInfoTitle[..title.len()].copy_from_slice(&title);
            data.szInfo[..text.len()].copy_from_slice(&text);
            let _ = Shell_NotifyIconW(NIM_MODIFY, &data);
        }
    }
}
impl Drop for Tray {
    fn drop(&mut self) {
        let _ = self.notify(NIM_DELETE);
        unsafe {
            SetWindowLongPtrW(self.hwnd, GWLP_USERDATA, 0);
            let _ = DestroyWindow(self.hwnd);
            let _ = DestroyIcon(self.icon);
        }
    }
}

unsafe extern "system" fn tray_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    unsafe {
        if message == WM_NCCREATE {
            let create = &*(lparam.0 as *const CREATESTRUCTW);
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, create.lpCreateParams as isize);
        }
        let pointer = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut TrayState;
        if !pointer.is_null() {
            let state = &*pointer;
            if message == state.taskbar_created {
                state.restore.set(true);
                return LRESULT(0);
            }
            if message == CALLBACK {
                let event = (lparam.0 & 0xffff) as u32;
                if event == NIN_SELECT || event == (NIN_SELECT | 1) {
                    state.settings_requested.set(true);
                    return LRESULT(0);
                }
                if event == WM_CONTEXTMENU {
                    if !state.menu_open.replace(true) {
                        let _ = tray_menu(hwnd, state);
                        state.menu_open.set(false);
                    }
                    return LRESULT(0);
                }
            }
        }
        if message == WM_NCDESTROY {
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
        }
        DefWindowProcW(hwnd, message, wparam, lparam)
    }
}
unsafe fn tray_menu(hwnd: HWND, state: &TrayState) -> Result<()> {
    unsafe {
        let menu = CreatePopupMenu()?;
        let result = (|| {
            let weather_flags = MF_STRING
                | if state.follow.get() {
                    MF_GRAYED
                } else {
                    MF_ENABLED
                };
            AppendMenuW(menu, MF_STRING, MENU_SETTINGS, w!("&Настройки…"))?;
            AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null())?;
            AppendMenuW(menu, weather_flags, MENU_SNOW, w!("Снег"))?;
            AppendMenuW(menu, weather_flags, MENU_RAIN, w!("Дождь"))?;
            AppendMenuW(
                menu,
                MF_STRING,
                MENU_PAUSE,
                if state.paused.get() {
                    w!("Продолжить")
                } else {
                    w!("Пауза")
                },
            )?;
            AppendMenuW(menu, MF_STRING, MENU_CLEAR, w!("Очистить накопление"))?;
            AppendMenuW(
                menu,
                MF_STRING
                    | if state.follow.get() {
                        MF_CHECKED
                    } else {
                        MF_UNCHECKED
                    },
                MENU_FOLLOW,
                w!("Слушать сервер"),
            )?;
            AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null())?;
            AppendMenuW(
                menu,
                MF_STRING | MF_GRAYED,
                MENU_ABOUT,
                w!("Snowflakes 0.1.0 — разработка"),
            )?;
            AppendMenuW(menu, MF_STRING, MENU_QUIT, w!("Выход"))?;
            let mut point = POINT::default();
            GetCursorPos(&mut point)?;
            let _ = SetForegroundWindow(hwnd);
            let selected = TrackPopupMenu(
                menu,
                TPM_RETURNCMD | TPM_NONOTIFY | TPM_RIGHTBUTTON,
                point.x,
                point.y,
                None,
                hwnd,
                None,
            )
            .0 as usize;
            if selected == MENU_SETTINGS {
                state.settings_requested.set(true);
            }
            state.pending.set(match selected {
                MENU_SNOW => Some(Command::Set {
                    setting: Setting::Mode(WeatherMode::Snow),
                }),
                MENU_RAIN => Some(Command::Set {
                    setting: Setting::Mode(WeatherMode::Rain),
                }),
                MENU_PAUSE => Some(if state.paused.get() {
                    Command::Resume {}
                } else {
                    Command::Pause {}
                }),
                MENU_CLEAR => Some(Command::Clear {}),
                MENU_FOLLOW => Some(Command::FollowServer {
                    enabled: !state.follow.get(),
                }),
                MENU_QUIT => Some(Command::Quit {}),
                _ => None,
            });
            let _ = PostMessageW(Some(hwnd), WM_NULL, WPARAM(0), LPARAM(0));
            Ok(())
        })();
        let _ = DestroyMenu(menu);
        result
    }
}

fn interactive(identity: &Identity) -> bool {
    if identity.session_id == 0 {
        return false;
    }
    unsafe {
        let Ok(station) = GetProcessWindowStation() else {
            return false;
        };
        let mut flags = USEROBJECTFLAGS::default();
        if GetUserObjectInformationW(
            HANDLE(station.0),
            UOI_FLAGS,
            Some((&mut flags as *mut USEROBJECTFLAGS).cast()),
            size_of::<USEROBJECTFLAGS>() as u32,
            None,
        )
        .is_err()
        {
            return false;
        }
        flags.dwFlags & 1 != 0 // WSF_VISIBLE
    }
}

pub fn run(args: &[String]) -> i32 {
    let mut args = args.to_vec();
    let mut directory: Option<PathBuf> = None;
    let mut namespace = "main".to_owned();
    let mut headless = false;
    let mut index = 0;
    while index < args.len() {
        if args[index] == "--data-dir" {
            if index + 1 >= args.len() {
                return output(Response::error(control::INVALID, "missing_data_directory"));
            }
            directory = Some(PathBuf::from(args.remove(index + 1)));
            args.remove(index);
        } else if cfg!(debug_assertions) && args[index] == "--test-namespace" {
            if index + 1 >= args.len() {
                return output(Response::error(control::INVALID, "missing_test_namespace"));
            }
            namespace = args.remove(index + 1);
            args.remove(index);
        } else if cfg!(debug_assertions) && args[index] == "--test-headless" {
            headless = true;
            args.remove(index);
        } else {
            index += 1;
        }
    }
    if args == ["--list-monitors", "--json"] {
        unsafe {
            let _ = SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        }
        return match super::scene::screens() {
            Ok(screens) => {
                let monitors: Vec<_> = screens.iter().map(|s| serde_json::json!({
                    "id": s.id(), "persistent": s.persistent_id,
                    "primary": s.rect.contains(crate::geometry::Point::new(0.0, 0.0)),
                    "left": s.rect.left, "top": s.rect.top, "width": s.rect.width(), "height": s.rect.height(),
                })).collect();
                println!(
                    "{}",
                    serde_json::json!({ "code": 0, "message": "monitors", "monitors": monitors })
                );
                control::OK
            }
            Err(_) => output(Response::error(control::INTERNAL, "monitor_query_failed")),
        };
    }
    let identity = match Identity::current(&namespace) {
        Ok(identity) => identity,
        Err(_) => return output(Response::error(control::INTERNAL, "identity_unavailable")),
    };
    let launch = args.is_empty()
        || args == ["--ensure-running"]
        || args == ["--ensure-running", "--from-logon"];
    if !launch {
        let command = match control::parse_command(&args) {
            Ok(command) => command,
            Err(error) => return output(Response::error(control::INVALID, error)),
        };
        return match native::send(&identity, &command) {
            Ok(response) => output(response),
            Err(error) => output(Response::error(
                if error.code() == windows::core::HRESULT::from_win32(ERROR_FILE_NOT_FOUND.0) {
                    control::ABSENT
                } else if error.code() == windows::core::HRESULT::from_win32(ERROR_TIMEOUT.0) {
                    control::TIMEOUT
                } else {
                    control::INTERNAL
                },
                "ipc_unavailable",
            )),
        };
    }
    let limits = match policy() {
        Ok(limits) => limits,
        Err(_) => return output(Response::error(control::POLICY, "policy_unreadable")),
    };
    if !limits.enabled {
        return output(Response::error(control::POLICY, "application_disabled"));
    }
    let instance = match Instance::acquire(&identity) {
        Ok(instance) => instance,
        Err(_) => return output(Response::error(control::INTERNAL, "instance_guard_failed")),
    };
    if !instance.owns() {
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            let command = if args.is_empty() && !headless {
                Command::Settings {}
            } else {
                Command::Status {}
            };
            if let Ok(response) = native::send(&identity, &command) {
                return output(response);
            }
            if Instant::now() >= deadline {
                return output(Response::error(control::TIMEOUT, "instance_starting"));
            }
            thread::sleep(Duration::from_millis(25));
        }
    }
    if !headless && !interactive(&identity) {
        return output(Response::error(
            control::NO_SESSION,
            "interactive_session_required",
        ));
    }
    let store = match directory {
        Some(path) if path.is_absolute() => UserStore::at(path),
        Some(_) => {
            return output(Response::error(
                control::INVALID,
                "data_directory_must_be_absolute",
            ));
        }
        None => match UserStore::for_current_user() {
            Ok(store) => store,
            Err(_) => return output(Response::error(control::INTERNAL, "storage_unavailable")),
        },
    };
    // Invalid wall time rejects disk cache, but local operation stays available.
    let clock = CacheClock::current().unwrap_or(CacheClock {
        unix_ms: 0,
        uptime_ms: 0,
    });
    let from_logon = args.last().is_some_and(|arg| arg == "--from-logon");
    let exit_marker = store.config_path().with_file_name(format!(
        "session-exit-{}-{}.marker",
        identity.session_id, identity.logon_key
    ));
    let marker_exists = match exit_marker.try_exists() {
        Ok(value) => value,
        Err(_) => {
            return output(Response::error(
                control::INTERNAL,
                "exit_marker_unavailable",
            ));
        }
    };
    if from_logon && marker_exists {
        return output(Response::error(control::OK, "start_suppressed"));
    }
    if !from_logon && marker_exists && std::fs::remove_file(&exit_marker).is_err() {
        return output(Response::error(
            control::INTERNAL,
            "exit_marker_unavailable",
        ));
    }
    let controller = match Controller::open(store, identity.session_id, clock, Instant::now()) {
        Ok(controller) => controller,
        Err(_) => {
            return output(Response::error(
                control::INTERNAL,
                "configuration_unavailable",
            ));
        }
    };
    if args.last().is_some_and(|arg| arg == "--from-logon") {
        let allowed = match reg("StartAtLogon") {
            Ok(Some(value)) => value != 0,
            Ok(None) => controller.config().startup.enabled,
            Err(_) => false,
        };
        if !allowed {
            return output(Response::error(control::POLICY, "logon_start_disabled"));
        }
    }
    let owner = Owner::start(controller, limits, exit_marker, !headless);
    if !headless {
        unsafe {
            let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        }
    }
    let server = match Server::start(&identity, owner.sender.clone()) {
        Ok(server) => server,
        Err(_) => return output(Response::error(control::INTERNAL, "ipc_start_failed")),
    };
    let mut tray = if headless {
        None
    } else {
        match Tray::create() {
            Ok(tray) => Some(tray),
            Err(_) => return output(Response::error(control::INTERNAL, "tray_unavailable")),
        }
    };
    if let Some(tray) = &tray {
        owner
            .ui_window
            .store(tray.hwnd.0 as isize, Ordering::Release);
    }
    let weather = (!headless).then(|| {
        weather::Worker::start(
            owner.view.clone(),
            owner.graphics.clone(),
            owner.conditions.clone(),
            identity.session_id,
        )
    });
    let mut panel: Option<Panel> = None;
    let mut pending: Option<(Receiver<Response>, Instant, bool)> = None;
    if args.is_empty() && !headless {
        owner.show_settings.store(true, Ordering::Release);
    }
    while !owner.stop.load(Ordering::Acquire) {
        let view = owner.view.lock().ok().map(|view| view.clone());
        if let Some(tray) = &mut tray {
            if let Some(view) = &view {
                tray.update(&view.status);
            }
            let requested = tray.state.settings_requested.replace(false)
                | owner.show_settings.swap(false, Ordering::AcqRel);
            if requested && let Some(view) = &view {
                if panel.is_none() {
                    match Panel::create(view, tray.hwnd) {
                        Ok(created) => {
                            owner
                                .panel_window
                                .store(created.hwnd().0 as isize, Ordering::Release);
                            panel = Some(created);
                        }
                        Err(error) => {
                            eprintln!("settings_panel_unavailable: {:08x}", error.code().0);
                            tray.error_notice();
                        }
                    }
                }
                if let Some(panel) = &panel {
                    panel.show();
                    #[cfg(debug_assertions)]
                    eprintln!("settings_panel_shown: {}", unsafe {
                        IsWindowVisible(panel.hwnd()).as_bool()
                    });
                }
            }
        }
        if let (Some(panel), Some(view)) = (&panel, &view) {
            panel.sync(view);
        }
        let completed =
            pending
                .as_ref()
                .and_then(
                    |(receiver, deadline, from_panel)| match receiver.try_recv() {
                        Ok(response) => Some((response, *from_panel)),
                        Err(mpsc::TryRecvError::Disconnected) => Some((
                            Response::error(control::INTERNAL, "instance_closing"),
                            *from_panel,
                        )),
                        Err(mpsc::TryRecvError::Empty) if Instant::now() >= *deadline => Some((
                            Response::error(control::TIMEOUT, "dispatch_timeout"),
                            *from_panel,
                        )),
                        Err(mpsc::TryRecvError::Empty) => None,
                    },
                );
        if let Some((response, from_panel)) = completed {
            pending = None;
            if from_panel {
                if let Some(panel) = &panel {
                    panel.complete(&response);
                }
            } else if response.code != control::OK
                && let Some(tray) = &tray
            {
                tray.error_notice();
            }
        }
        if pending.is_none() {
            let command = tray
                .as_ref()
                .and_then(|tray| tray.state.pending.take())
                .map(|c| (c, false))
                .or_else(|| {
                    panel
                        .as_ref()
                        .and_then(Panel::take_command)
                        .map(|c| (c, true))
                });
            if let Some((command, from_panel)) = command {
                match owner.submit(command) {
                    Ok(receiver) => {
                        pending = Some((
                            receiver,
                            Instant::now() + Duration::from_secs(2),
                            from_panel,
                        ))
                    }
                    Err(error) => {
                        if from_panel {
                            if let Some(panel) = &panel {
                                panel.complete(&error);
                            }
                        } else if let Some(tray) = &tray {
                            tray.error_notice();
                        }
                    }
                }
            }
        }
        unsafe {
            let mut message = MSG::default();
            while PeekMessageW(&mut message, None, 0, 0, PM_REMOVE).as_bool() {
                if message.message == WM_QUIT {
                    let _ = owner.request(Command::Quit {});
                    break;
                }
                if !panel
                    .as_ref()
                    .is_some_and(|panel| panel.dialog_message(&message))
                {
                    let _ = TranslateMessage(&message);
                    DispatchMessageW(&message);
                }
            }
            MsgWaitForMultipleObjectsEx(None, 100, QS_ALLINPUT, MWMO_INPUTAVAILABLE);
        }
    }
    drop(server);
    drop(weather);
    drop(owner);
    drop(panel);
    drop(tray);
    drop(instance);
    control::OK
}

fn output(response: Response) -> i32 {
    let code = response.code;
    if let Ok(json) = serde_json::to_string(&response) {
        println!("{json}");
    }
    code
}
