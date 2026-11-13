//! Windows process lifetime. The control thread owns Session and disk operations;
//! the GUI thread owns HWND/HICON and only reads immutable status snapshots.
use super::native::{self, Envelope, Identity, Instance, Server};
use crate::{
    control::{self, Command, Controller, Response, Setting, Status},
    session::{ExitReason, Limits, PauseReason},
    storage::{CacheClock, UserStore},
};
use snowflakes_protocol::WeatherMode;
use std::{
    cell::Cell,
    path::PathBuf,
    rc::Rc,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, SyncSender},
    },
    thread,
    time::{Duration, Instant},
};
use windows::{
    Win32::{
        Foundation::*,
        System::{LibraryLoader::GetModuleHandleW, Registry::*, StationsAndDesktops::*},
        UI::{Shell::*, WindowsAndMessaging::*},
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

struct Owner {
    sender: SyncSender<Envelope>,
    status: Arc<Mutex<Status>>,
    stop: Arc<AtomicBool>,
    ui_window: Arc<std::sync::atomic::AtomicIsize>,
    worker: Option<thread::JoinHandle<()>>,
}
impl Owner {
    fn start(mut controller: Controller, limits: Limits, exit_marker: PathBuf) -> Self {
        let _ = controller.session.set_limits(limits, Instant::now());
        let status = Arc::new(Mutex::new(controller.status(Instant::now())));
        let stop = Arc::new(AtomicBool::new(false));
        let ui_window = Arc::new(std::sync::atomic::AtomicIsize::new(0));
        let worker_window = ui_window.clone();
        let (sender, receiver) = mpsc::sync_channel::<Envelope>(8);
        let worker_status = status.clone();
        let worker_stop = stop.clone();
        let worker = thread::spawn(move || {
            let mut next_policy = Instant::now() + Duration::from_secs(60);
            while !worker_stop.load(Ordering::Acquire) {
                if let Ok(envelope) = receiver.recv_timeout(Duration::from_millis(100)) {
                    let response = if Instant::now() > envelope.deadline {
                        Response::error(control::TIMEOUT, "dispatch_timeout")
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
                    let _ = envelope.reply.send(response);
                }
                let now = Instant::now();
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
                if let Ok(mut current) = worker_status.lock() {
                    *current = controller.status(now);
                }
                if controller.session.snapshot(now).exit.is_some() {
                    if controller.session.snapshot(now).exit == Some(ExitReason::Policy) {
                        let _ = crate::storage::atomic_write(&exit_marker, b"policy\n");
                    }
                    worker_stop.store(true, Ordering::Release);
                    let raw = worker_window.load(Ordering::Acquire);
                    if raw != 0 {
                        unsafe {
                            let _ = PostMessageW(
                                Some(HWND(raw as *mut core::ffi::c_void)),
                                WM_CANCELMODE,
                                WPARAM(0),
                                LPARAM(0),
                            );
                        }
                    }
                }
            }
        });
        Self {
            sender,
            status,
            stop,
            ui_window,
            worker: Some(worker),
        }
    }
    fn request(&self, command: Command) -> Response {
        let (reply, response) = mpsc::sync_channel(1);
        let envelope = Envelope {
            command,
            reply,
            deadline: Instant::now() + Duration::from_millis(1500),
        };
        if self.sender.try_send(envelope).is_err() {
            return Response::error(control::INTERNAL, "instance_busy");
        }
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
                if event == WM_CONTEXTMENU || event == NIN_SELECT || event == (NIN_SELECT | 1) {
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
            if let Ok(response) = native::send(&identity, &Command::Status {}) {
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
    let owner = Owner::start(controller, limits, exit_marker);
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
    while !owner.stop.load(Ordering::Acquire) {
        if let Some(tray) = &mut tray {
            if let Ok(status) = owner.status.lock() {
                tray.update(&status);
            }
            if let Some(command) = tray.state.pending.take() {
                let response = owner.request(command);
                if response.code != control::OK {
                    tray.error_notice();
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
                let _ = TranslateMessage(&message);
                DispatchMessageW(&message);
            }
            MsgWaitForMultipleObjectsEx(None, 100, QS_ALLINPUT, MWMO_INPUTAVAILABLE);
        }
    }
    drop(server);
    drop(owner);
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
