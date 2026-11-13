//! Modeless native controls. Rc/Cell owns callback state; no mutable borrow is
//! held across a Win32 call that can reenter this window procedure.
use super::native::wide;
use crate::{
    control::{Command, OK, Response, Setting},
    session::{PauseReason, ProfileSource},
    ui::{self, Pending, View},
};
use snowflakes_protocol::WeatherMode;
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
};
use windows::{
    Win32::{
        Foundation::*,
        Graphics::Gdi::*,
        System::LibraryLoader::GetModuleHandleW,
        UI::{Controls::*, HiDpi::*, Input::KeyboardAndMouse::*, WindowsAndMessaging::*},
    },
    core::{PCWSTR, Result, w},
};

const FOLLOW: i32 = 101;
const ENDPOINT: i32 = 102;
const SAVE: i32 = 1; // standard dialog default action
const CLOSE: i32 = 2;
const SYNC: i32 = 104;
const MODE: i32 = 105;
const EFFECTS: i32 = 106;
const INFO: i32 = 107;
const HINT: i32 = 108;
const FEEDBACK: i32 = 109;
const PAUSE: i32 = 110;
const CLEAR: i32 = 111;
const SLIDERS: [i32; 4] = [201, 202, 203, 204];
const VALUES: [i32; 4] = [301, 302, 303, 304];
// Trackbar position message from commctrl.h (omitted by windows-rs metadata).
const TBM_GETPOS: u32 = WM_USER;
const CLASS: PCWSTR = w!("SnowflakesSettingsPanel");

#[derive(Clone, Copy)]
struct Child {
    handle: HWND,
    id: i32,
    x: i32,
    y: i32,
    width: i32,
    height: i32,
    stretch: bool,
    right: bool,
}
struct State {
    children: RefCell<Vec<Child>>,
    font: Cell<HFONT>,
    dpi: Cell<u32>,
    scroll: Cell<i32>,
    view: RefCell<View>,
    pending: RefCell<Pending>,
    inflight: Cell<Option<u8>>,
    inflight_endpoint: RefCell<Option<String>>,
    last_focus: Cell<HWND>,
    updating: Cell<bool>,
    tracking: Cell<i32>,
    endpoint_dirty: Cell<bool>,
    font_dirty: Cell<bool>,
    modes: Cell<Option<(bool, bool)>>,
}
impl State {
    fn control(&self, id: i32) -> Option<HWND> {
        self.children
            .borrow()
            .iter()
            .find(|c| c.id == id)
            .map(|c| c.handle)
    }
    fn busy(&self, key: u8) -> bool {
        self.inflight.get() == Some(key) || self.pending.borrow().contains(key)
    }
    fn enqueue(&self, command: Command) {
        let accepted = self.pending.borrow_mut().push(command);
        if !accepted && let Some(feedback) = self.control(FEEDBACK) {
            set_text(
                feedback,
                "Очередь занята. Дождитесь выполнения команд и повторите.",
            );
        }
    }
}

pub struct Panel {
    hwnd: HWND,
    state: Rc<State>,
}
impl Panel {
    pub fn create(view: &View, owner: HWND) -> Result<Self> {
        unsafe {
            InitCommonControlsEx(&INITCOMMONCONTROLSEX {
                dwSize: size_of::<INITCOMMONCONTROLSEX>() as u32,
                dwICC: ICC_BAR_CLASSES,
            })
            .ok()?;
            let instance = HINSTANCE(GetModuleHandleW(None)?.0);
            let class = WNDCLASSW {
                lpfnWndProc: Some(window_proc),
                hInstance: instance,
                hCursor: LoadCursorW(None, IDC_ARROW)?,
                hbrBackground: HBRUSH((COLOR_WINDOW.0 + 1) as *mut core::ffi::c_void),
                lpszClassName: CLASS,
                ..Default::default()
            };
            if RegisterClassW(&class) == 0 && GetLastError() != ERROR_CLASS_ALREADY_EXISTS {
                return Err(windows::core::Error::from_thread());
            }
            let state = Rc::new(State {
                children: RefCell::new(Vec::new()),
                font: Cell::new(HFONT::default()),
                dpi: Cell::new(96),
                scroll: Cell::new(0),
                view: RefCell::new(view.clone()),
                pending: RefCell::new(Pending::default()),
                inflight: Cell::new(None),
                inflight_endpoint: RefCell::new(None),
                last_focus: Cell::new(HWND::default()),
                updating: Cell::new(true),
                tracking: Cell::new(0),
                endpoint_dirty: Cell::new(false),
                font_dirty: Cell::new(true),
                modes: Cell::new(None),
            });
            let hwnd = CreateWindowExW(
                WS_EX_CONTROLPARENT | WS_EX_APPWINDOW,
                CLASS,
                w!("Snowflakes — настройки"),
                WS_OVERLAPPEDWINDOW | WS_VSCROLL,
                CW_USEDEFAULT,
                CW_USEDEFAULT,
                640,
                720,
                Some(owner),
                None,
                Some(instance),
                Some(Rc::as_ptr(&state).cast()),
            )?;
            let panel = Self { hwnd, state };
            panel.state.dpi.set(GetDpiForWindow(hwnd).max(96));
            panel.child(
                FOLLOW,
                "BUTTON",
                "&Слушать сервер",
                BS_AUTOCHECKBOX as u32,
                true,
                20,
                18,
                0,
                26,
                true,
                false,
            )?;
            panel.child(
                120,
                "STATIC",
                "&Адрес сервера",
                0,
                false,
                20,
                52,
                0,
                18,
                true,
                false,
            )?;
            panel.child(
                ENDPOINT,
                "EDIT",
                "",
                ES_AUTOHSCROLL as u32 | WS_BORDER.0,
                true,
                20,
                74,
                150,
                28,
                true,
                false,
            )?;
            panel.child(
                SAVE,
                "BUTTON",
                "Со&хранить",
                BS_DEFPUSHBUTTON as u32,
                true,
                130,
                74,
                110,
                28,
                false,
                true,
            )?;
            panel.child(INFO, "STATIC", "", 0, false, 20, 116, 0, 60, true, false)?;
            panel.child(
                SYNC,
                "BUTTON",
                "Проверить се&йчас",
                0,
                true,
                20,
                178,
                180,
                28,
                false,
                false,
            )?;
            panel.child(
                121,
                "STATIC",
                "&Режим",
                0,
                false,
                20,
                216,
                75,
                24,
                false,
                false,
            )?;
            panel.child(
                MODE,
                "COMBOBOX",
                "",
                CBS_DROPDOWNLIST as u32 | WS_VSCROLL.0,
                true,
                100,
                212,
                125,
                120,
                false,
                false,
            )?;
            panel.child(
                EFFECTS,
                "BUTTON",
                "Эффекты &включены",
                BS_AUTOCHECKBOX as u32,
                true,
                245,
                212,
                0,
                28,
                true,
                false,
            )?;
            for (index, title) in [
                "&Интенсивность снегопада",
                "Интенсивность &дождя",
                "Размер с&угробов",
                "Размер &луж",
            ]
            .iter()
            .enumerate()
            {
                let y = 256 + index as i32 * 70;
                panel.child(
                    130 + index as i32,
                    "STATIC",
                    title,
                    0,
                    false,
                    20,
                    y,
                    85,
                    20,
                    true,
                    false,
                )?;
                panel.child(
                    VALUES[index],
                    "STATIC",
                    "—",
                    2,
                    false,
                    75,
                    y,
                    55,
                    20,
                    false,
                    true,
                )?;
                panel.child(
                    SLIDERS[index],
                    "msctls_trackbar32",
                    &title.replace('&', ""),
                    TBS_NOTICKS,
                    true,
                    20,
                    y + 24,
                    0,
                    34,
                    true,
                    false,
                )?;
            }
            panel.child(HINT, "STATIC", "", 0, false, 20, 540, 0, 38, true, false)?;
            panel.child(
                FEEDBACK, "STATIC", "", 0, false, 20, 582, 0, 32, true, false,
            )?;
            panel.child(
                PAUSE,
                "BUTTON",
                "&Пауза",
                0,
                true,
                20,
                630,
                130,
                28,
                false,
                false,
            )?;
            panel.child(
                CLEAR,
                "BUTTON",
                "О&чистить",
                0,
                true,
                160,
                630,
                130,
                28,
                false,
                false,
            )?;
            panel.child(
                CLOSE,
                "BUTTON",
                "&Закрыть",
                0,
                true,
                130,
                630,
                110,
                28,
                false,
                true,
            )?;
            if let Some(edit) = panel.state.control(ENDPOINT) {
                send(edit, EM_LIMITTEXT, WPARAM(2048), LPARAM(0));
            }
            panel.state.updating.set(false);
            panel.font();
            panel.fit_to_monitor();
            panel.sync(view);
            Ok(panel)
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn child(
        &self,
        id: i32,
        class: &str,
        text: &str,
        style: u32,
        tab: bool,
        x: i32,
        y: i32,
        width: i32,
        height: i32,
        stretch: bool,
        right: bool,
    ) -> Result<()> {
        let class = wide(class);
        let text = wide(text);
        let handle = unsafe {
            CreateWindowExW(
                WINDOW_EX_STYLE(0),
                PCWSTR(class.as_ptr()),
                PCWSTR(text.as_ptr()),
                WS_CHILD
                    | WS_VISIBLE
                    | WINDOW_STYLE(style)
                    | if tab { WS_TABSTOP } else { WINDOW_STYLE(0) },
                0,
                0,
                1,
                1,
                Some(self.hwnd),
                Some(HMENU(id as usize as *mut core::ffi::c_void)),
                Some(HINSTANCE(GetModuleHandleW(None)?.0)),
                None,
            )?
        };
        self.state.children.borrow_mut().push(Child {
            handle,
            id,
            x,
            y,
            width,
            height,
            stretch,
            right,
        });
        Ok(())
    }
    pub fn hwnd(&self) -> HWND {
        self.hwnd
    }
    pub fn show(&self) {
        unsafe {
            // The first ShowWindow may honor a launcher's STARTF_USESHOWWINDOW
            // (e.g. Hidden). The explicit restoration then shows this request.
            let _ = ShowWindow(self.hwnd, SW_SHOW);
            let _ = ShowWindow(self.hwnd, SW_RESTORE);
            let _ = SetForegroundWindow(self.hwnd);
            let _ = SetFocus(self.state.control(FOLLOW));
        }
    }
    pub fn dialog_message(&self, message: &MSG) -> bool {
        unsafe {
            IsWindowVisible(self.hwnd).as_bool() && IsDialogMessageW(self.hwnd, message).as_bool()
        }
    }
    pub fn take_command(&self) -> Option<Command> {
        let command = self.state.pending.borrow_mut().pop();
        if let Some(command) = &command {
            self.state.inflight.set(Some(ui::command_key(command)));
            if let Command::ServerUrl { value } = command {
                self.state.inflight_endpoint.replace(Some(value.clone()));
            }
        }
        command
    }
    pub fn complete(&self, response: &Response) {
        let key = self.state.inflight.replace(None);
        if key == Some(11) {
            let submitted = self.state.inflight_endpoint.take();
            if response.code == OK
                && !self.state.pending.borrow().contains(11)
                && self
                    .state
                    .control(ENDPOINT)
                    .is_some_and(|edit| Some(text(edit)) == submitted)
            {
                self.state.endpoint_dirty.set(false);
            }
        }
        if response.code != OK {
            self.state.tracking.set(0);
            let text = match response.message.as_str() {
                "invalid_server_url" => {
                    "Введите корректный адрес HTTP/HTTPS без пароля и параметров запроса."
                }
                "local_profile_read_only" => {
                    "Для местной настройки снимите галочку «Слушать сервер»."
                }
                "preferences_not_saved" => {
                    "Не удалось сохранить настройки. Проверьте доступ к каталогу."
                }
                "dispatch_timeout" => {
                    "Нет подтверждения команды. Состояние обновится после ответа."
                }
                _ => "Не удалось выполнить команду.",
            };
            self.feedback(text);
        } else {
            self.feedback("Сохранено.");
        }
    }
    fn feedback(&self, value: &str) {
        if let Some(control) = self.state.control(FEEDBACK) {
            set_text(control, value);
        }
    }

    pub fn sync(&self, view: &View) {
        self.state.view.replace(view.clone());
        if self.state.font_dirty.replace(false) {
            self.font();
        }
        self.state.updating.set(true);
        let local = !view.status.follow_server;
        let settings = &view.status.settings;
        unsafe {
            if !self.state.busy(10) {
                checked(self.state.control(FOLLOW), view.status.follow_server);
            }
            if !self.state.endpoint_dirty.get()
                && !self.state.busy(11)
                && let Some(edit) = self.state.control(ENDPOINT)
            {
                set_text(edit, &view.server_url);
            }
            if let Some(sync) = self.state.control(SYNC) {
                let _ = EnableWindow(
                    sync,
                    view.status.follow_server && view.status.network_state != "not_connected",
                );
            }
            let source = match view.status.source {
                ProfileSource::Local => "местный профиль",
                ProfileSource::Server => "серверный профиль",
                ProfileSource::WaitingForServer => "ожидание серверного профиля",
            };
            let pause = if view.status.pauses.is_empty() {
                ""
            } else {
                " | Пауза"
            };
            if let Some(info) = self.state.control(INFO) {
                set_text(
                    info,
                    &format!(
                        "Источник: {source}{pause}\r\nСвязь: сетевой клиент в разработке\r\nПоследняя проверка: сведений пока нет"
                    ),
                );
            }
            if let Some(hint) = self.state.control(HINT) {
                set_text(
                    hint,
                    if view.status.follow_server {
                        "Погодой управляет сервер. Для местной настройки снимите галочку выше."
                    } else if view.status.limited {
                        "Действуют ограничения политики Windows."
                    } else {
                        "Интенсивность осадков и размер накопления изменяются независимо."
                    },
                );
            }
            if let Some(pause) = self.state.control(PAUSE) {
                set_text(
                    pause,
                    if view.status.pauses.contains(&PauseReason::Manual) {
                        "&Продолжить"
                    } else {
                        "&Пауза"
                    },
                );
            }
            if let Some(combo) = self.state.control(MODE) {
                if !self.state.busy(5) {
                    let allowed = (view.limits.allow_snow, view.limits.allow_rain);
                    if self.state.modes.get() != Some(allowed) {
                        send(combo, CB_RESETCONTENT, WPARAM(0), LPARAM(0));
                        for (allowed, caption, value) in
                            [(allowed.0, "Снег", 1), (allowed.1, "Дождь", 2)]
                        {
                            if allowed {
                                let text = wide(caption);
                                let index = send(
                                    combo,
                                    CB_ADDSTRING,
                                    WPARAM(0),
                                    LPARAM(text.as_ptr() as isize),
                                )
                                .0;
                                send(combo, CB_SETITEMDATA, WPARAM(index as usize), LPARAM(value));
                            }
                        }
                        self.state.modes.set(Some(allowed));
                    }
                    if send(combo, CB_GETDROPPEDSTATE, WPARAM(0), LPARAM(0)).0 == 0 {
                        let count = send(combo, CB_GETCOUNT, WPARAM(0), LPARAM(0)).0;
                        let desired = settings.as_ref().map(|s| match s.weather.mode {
                            WeatherMode::Snow => 1,
                            WeatherMode::Rain => 2,
                        });
                        let selected = (0..count).find(|&index| {
                            Some(send(combo, CB_GETITEMDATA, WPARAM(index as usize), LPARAM(0)).0)
                                == desired
                        });
                        let selected = selected.unwrap_or(-1);
                        if send(combo, CB_GETCURSEL, WPARAM(0), LPARAM(0)).0 != selected {
                            send(combo, CB_SETCURSEL, WPARAM(selected as usize), LPARAM(0));
                        }
                    }
                }
                let _ = EnableWindow(combo, local && settings.is_some());
            }
            if let Some(effects) = self.state.control(EFFECTS) {
                if !self.state.busy(6) {
                    checked(
                        Some(effects),
                        settings.as_ref().is_some_and(|s| s.effects_enabled),
                    );
                }
                let _ = EnableWindow(effects, local && settings.is_some());
            }
            let values = settings.as_ref().map(|s| {
                [
                    s.snow.intensity_percent,
                    s.rain.intensity_percent,
                    s.snow.drift_size_percent,
                    s.rain.puddle_size_percent,
                ]
            });
            let caps = [
                view.limits.max_intensity_percent,
                view.limits.max_intensity_percent,
                view.limits.max_drift_size_percent,
                view.limits.max_puddle_size_percent,
            ];
            for index in 0..4 {
                if let Some(slider) = self.state.control(SLIDERS[index]) {
                    send(slider, TBM_SETRANGEMIN, WPARAM(0), LPARAM(0));
                    send(
                        slider,
                        TBM_SETRANGEMAX,
                        WPARAM(1),
                        LPARAM(caps[index] as isize),
                    );
                    send(slider, TBM_SETPAGESIZE, WPARAM(0), LPARAM(10));
                    if self.state.tracking.get() != SLIDERS[index]
                        && !self.state.busy(index as u8 + 1)
                    {
                        let value = values.map(|v| v[index]).unwrap_or(0);
                        send(slider, TBM_SETPOS, WPARAM(1), LPARAM(value as isize));
                        if let Some(label) = self.state.control(VALUES[index]) {
                            set_text(
                                label,
                                &values
                                    .map(|v| format!("{}%", v[index]))
                                    .unwrap_or_else(|| "—".into()),
                            );
                        }
                    }
                    let _ = EnableWindow(slider, local && settings.is_some() && caps[index] > 0);
                }
            }
        }
        self.state.updating.set(false);
        self.reveal_focus();
    }

    fn reveal_focus(&self) {
        let focus = unsafe { GetFocus() };
        if self.state.last_focus.replace(focus) == focus {
            return;
        }
        let child = self
            .state
            .children
            .borrow()
            .iter()
            .find(|child| child.handle == focus)
            .copied();
        if let Some(child) = child {
            let mut rect = RECT::default();
            if unsafe { GetClientRect(self.hwnd, &mut rect) }.is_err() {
                return;
            }
            let dpi = self.state.dpi.get();
            let top = ui::scale(child.y - 8, dpi);
            let bottom = ui::scale(child.y + child.height.min(34) + 8, dpi);
            let scroll = self.state.scroll.get();
            if top < scroll {
                self.state.scroll.set(top.max(0));
            } else if bottom > scroll + rect.bottom {
                self.state.scroll.set(bottom - rect.bottom);
            }
            layout(self.hwnd, &self.state);
        }
    }

    fn fit_to_monitor(&self) {
        unsafe {
            let monitor = MonitorFromWindow(self.hwnd, MONITOR_DEFAULTTONEAREST);
            let mut info = MONITORINFO {
                cbSize: size_of::<MONITORINFO>() as u32,
                ..Default::default()
            };
            if !GetMonitorInfoW(monitor, &mut info).as_bool() {
                return;
            }
            let dpi = self.state.dpi.get();
            let mut rect = RECT {
                left: 0,
                top: 0,
                right: ui::scale(620, dpi),
                bottom: ui::scale(ui::CONTENT_HEIGHT, dpi),
            };
            let _ = AdjustWindowRectExForDpi(
                &mut rect,
                WS_OVERLAPPEDWINDOW | WS_VSCROLL,
                false,
                WS_EX_CONTROLPARENT | WS_EX_APPWINDOW,
                dpi,
            );
            let width = (rect.right - rect.left).min(info.rcWork.right - info.rcWork.left - 32);
            let height = (rect.bottom - rect.top).min(info.rcWork.bottom - info.rcWork.top - 32);
            let _ = SetWindowPos(
                self.hwnd,
                None,
                info.rcWork.left + 16,
                info.rcWork.top + 16,
                width,
                height,
                SWP_NOACTIVATE | SWP_NOZORDER,
            );
            layout(self.hwnd, &self.state);
        }
    }
    fn font(&self) {
        unsafe {
            let font = CreateFontW(
                -(((9 * self.state.dpi.get() + 36) / 72) as i32),
                0,
                0,
                0,
                400,
                0,
                0,
                0,
                DEFAULT_CHARSET,
                OUT_DEFAULT_PRECIS,
                CLIP_DEFAULT_PRECIS,
                CLEARTYPE_QUALITY,
                0,
                w!("Segoe UI"),
            );
            if font.is_invalid() {
                return;
            }
            let children = self.state.children.borrow().clone();
            for child in children {
                send(child.handle, WM_SETFONT, WPARAM(font.0 as usize), LPARAM(1));
            }
            let old = self.state.font.replace(font);
            if !old.is_invalid() {
                let _ = DeleteObject(old.into());
            }
        }
    }
}
impl Drop for Panel {
    fn drop(&mut self) {
        unsafe {
            SetWindowLongPtrW(self.hwnd, GWLP_USERDATA, 0);
            let _ = DestroyWindow(self.hwnd);
            let font = self.state.font.get();
            if !font.is_invalid() {
                let _ = DeleteObject(font.into());
            }
            if let Ok(module) = GetModuleHandleW(None) {
                let _ = UnregisterClassW(CLASS, Some(HINSTANCE(module.0)));
            }
        }
    }
}

unsafe fn send(hwnd: HWND, message: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    unsafe { SendMessageW(hwnd, message, Some(wparam), Some(lparam)) }
}

fn text(handle: HWND) -> String {
    unsafe {
        let length = GetWindowTextLengthW(handle).clamp(0, 4096);
        let mut buffer = vec![0_u16; length as usize + 1];
        let count = GetWindowTextW(handle, &mut buffer);
        String::from_utf16_lossy(&buffer[..count.max(0) as usize])
    }
}
fn set_text(handle: HWND, value: &str) {
    if text(handle) != value {
        let wide = wide(value);
        unsafe {
            let _ = SetWindowTextW(handle, PCWSTR(wide.as_ptr()));
        }
    }
}
unsafe fn checked(handle: Option<HWND>, value: bool) {
    if let Some(handle) = handle {
        unsafe {
            send(handle, BM_SETCHECK, WPARAM(usize::from(value)), LPARAM(0));
        }
    }
}
fn layout(hwnd: HWND, state: &State) {
    unsafe {
        let mut rect = RECT::default();
        if GetClientRect(hwnd, &mut rect).is_err() {
            return;
        }
        let dpi = state.dpi.get();
        let maximum = ui::scroll_limit(rect.bottom, dpi);
        state.scroll.set(state.scroll.get().clamp(0, maximum));
        let info = SCROLLINFO {
            cbSize: size_of::<SCROLLINFO>() as u32,
            fMask: SIF_RANGE | SIF_PAGE | SIF_POS,
            nMin: 0,
            nMax: ui::scale(ui::CONTENT_HEIGHT, dpi) - 1,
            nPage: rect.bottom.max(1) as u32,
            nPos: state.scroll.get(),
            ..Default::default()
        };
        SetScrollInfo(hwnd, SB_VERT, &info, true);
        let children = state.children.borrow().clone();
        for child in children {
            let x = if child.right {
                rect.right - ui::scale(child.x, dpi)
            } else {
                ui::scale(child.x, dpi)
            };
            let width = if child.stretch {
                rect.right - x - ui::scale(20 + child.width, dpi)
            } else {
                ui::scale(child.width, dpi)
            };
            let _ = MoveWindow(
                child.handle,
                x,
                ui::scale(child.y, dpi) - state.scroll.get(),
                width.max(1),
                ui::scale(child.height, dpi),
                true,
            );
        }
    }
}
unsafe extern "system" fn window_proc(
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
        let pointer = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *const State;
        if pointer.is_null() {
            return DefWindowProcW(hwnd, message, wparam, lparam);
        }
        let state = &*pointer;
        match message {
            WM_CLOSE => {
                state.endpoint_dirty.set(false);
                let _ = ShowWindow(hwnd, SW_HIDE);
                LRESULT(0)
            }
            WM_GETMINMAXINFO => {
                let limits = &mut *(lparam.0 as *mut MINMAXINFO);
                limits.ptMinTrackSize.x = ui::scale(460, state.dpi.get());
                limits.ptMinTrackSize.y = ui::scale(320, state.dpi.get());
                LRESULT(0)
            }
            WM_SIZE => {
                layout(hwnd, state);
                LRESULT(0)
            }
            WM_DPICHANGED => {
                state.dpi.set((wparam.0 & 0xffff) as u32);
                state.font_dirty.set(true);
                let rect = &*(lparam.0 as *const RECT);
                let _ = SetWindowPos(
                    hwnd,
                    None,
                    rect.left,
                    rect.top,
                    rect.right - rect.left,
                    rect.bottom - rect.top,
                    SWP_NOZORDER | SWP_NOACTIVATE,
                );
                layout(hwnd, state);
                LRESULT(0)
            }
            WM_MOUSEWHEEL => {
                let delta = (wparam.0 >> 16) as i16 as i32;
                state.scroll.set(
                    (state.scroll.get() - delta / 120 * ui::scale(60, state.dpi.get())).max(0),
                );
                layout(hwnd, state);
                LRESULT(0)
            }
            WM_VSCROLL => {
                let mut info = SCROLLINFO {
                    cbSize: size_of::<SCROLLINFO>() as u32,
                    fMask: SIF_ALL,
                    ..Default::default()
                };
                let _ = GetScrollInfo(hwnd, SB_VERT, &mut info);
                let position = match (wparam.0 & 0xffff) as i32 {
                    0 => info.nPos - ui::scale(24, state.dpi.get()),
                    1 => info.nPos + ui::scale(24, state.dpi.get()),
                    2 => info.nPos - info.nPage as i32,
                    3 => info.nPos + info.nPage as i32,
                    4 | 5 => info.nTrackPos,
                    6 => 0,
                    7 => info.nMax,
                    _ => info.nPos,
                };
                state.scroll.set(position.max(0));
                layout(hwnd, state);
                LRESULT(0)
            }
            WM_HSCROLL if !state.updating.get() => {
                let control = HWND(lparam.0 as *mut core::ffi::c_void);
                let id = GetDlgCtrlID(control);
                if let Some(index) = SLIDERS.iter().position(|&item| item == id) {
                    let value = send(control, TBM_GETPOS, WPARAM(0), LPARAM(0))
                        .0
                        .clamp(0, 100) as u8;
                    let notification = (wparam.0 & 0xffff) as u32;
                    state
                        .tracking
                        .set(if notification == TB_THUMBTRACK { id } else { 0 });
                    if let Some(label) = state.control(VALUES[index]) {
                        set_text(label, &format!("{value}%"));
                    }
                    let setting = match index {
                        0 => Setting::SnowIntensity(value),
                        1 => Setting::RainIntensity(value),
                        2 => Setting::DriftSize(value),
                        _ => Setting::PuddleSize(value),
                    };
                    state.enqueue(Command::Set { setting });
                }
                LRESULT(0)
            }
            WM_COMMAND if !state.updating.get() => {
                let id = (wparam.0 & 0xffff) as i32;
                let code = (wparam.0 >> 16) as u32;
                match (id, code) {
                    (ENDPOINT, EN_CHANGE) => state.endpoint_dirty.set(true),
                    (CLOSE, BN_CLICKED) => {
                        let _ = send(hwnd, WM_CLOSE, WPARAM(0), LPARAM(0));
                    }
                    (SAVE, BN_CLICKED) => {
                        if let Some(edit) = state.control(ENDPOINT) {
                            state.enqueue(Command::ServerUrl { value: text(edit) });
                        }
                    }
                    (FOLLOW, BN_CLICKED) => {
                        if let Some(check) = state.control(FOLLOW) {
                            state.enqueue(Command::FollowServer {
                                enabled: send(check, BM_GETCHECK, WPARAM(0), LPARAM(0)).0 != 0,
                            });
                        }
                    }
                    (EFFECTS, BN_CLICKED) => {
                        if let Some(check) = state.control(EFFECTS) {
                            state.enqueue(Command::Set {
                                setting: Setting::EffectsEnabled(
                                    send(check, BM_GETCHECK, WPARAM(0), LPARAM(0)).0 != 0,
                                ),
                            });
                        }
                    }
                    (MODE, CBN_SELCHANGE) => {
                        if let Some(combo) = state.control(MODE) {
                            let selected = send(combo, CB_GETCURSEL, WPARAM(0), LPARAM(0)).0;
                            if selected >= 0 {
                                let value = send(
                                    combo,
                                    CB_GETITEMDATA,
                                    WPARAM(selected as usize),
                                    LPARAM(0),
                                )
                                .0;
                                state.enqueue(Command::Set {
                                    setting: Setting::Mode(if value == 1 {
                                        WeatherMode::Snow
                                    } else {
                                        WeatherMode::Rain
                                    }),
                                });
                            }
                        }
                    }
                    (SYNC, BN_CLICKED) => state.enqueue(Command::SyncNow {}),
                    (CLEAR, BN_CLICKED) => state.enqueue(Command::Clear {}),
                    (PAUSE, BN_CLICKED) => {
                        let paused = state
                            .view
                            .borrow()
                            .status
                            .pauses
                            .contains(&PauseReason::Manual);
                        state.enqueue(if paused {
                            Command::Resume {}
                        } else {
                            Command::Pause {}
                        });
                    }
                    _ => (),
                }
                LRESULT(0)
            }
            WM_NCDESTROY => {
                SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
                DefWindowProcW(hwnd, message, wparam, lparam)
            }
            _ => DefWindowProcW(hwnd, message, wparam, lparam),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        control::Controller,
        session::Limits,
        storage::{CacheClock, UserStore},
    };

    // Hidden controls test the real Win32 ABI/creation path, not visual input.
    #[test]
    fn hidden_panel_projects_source_and_policy_without_overwriting_endpoint_edits() {
        let dir = tempfile::tempdir().unwrap();
        let now = std::time::Instant::now();
        let mut controller = Controller::open(
            UserStore::at(dir.path()),
            1,
            CacheClock {
                unix_ms: 2_000_000_000_000,
                uptime_ms: 100_000,
            },
            now,
        )
        .unwrap();
        let panel = Panel::create(&controller.ui_view(now), HWND::default()).unwrap();
        let slider = panel.state.control(SLIDERS[0]).unwrap();
        unsafe {
            assert!(!IsWindowEnabled(slider).as_bool());
        }
        controller.handle(Command::FollowServer { enabled: false }, now);
        controller
            .session
            .set_limits(
                Limits {
                    max_intensity_percent: 10,
                    ..Default::default()
                },
                now,
            )
            .unwrap();
        panel.sync(&controller.ui_view(now));
        unsafe {
            assert!(IsWindowEnabled(slider).as_bool());
            assert_eq!(send(slider, TBM_GETPOS, WPARAM(0), LPARAM(0)).0, 10);
        }
        let edit = panel.state.control(ENDPOINT).unwrap();
        set_text(edit, "http://127.0.0.1:805/");
        panel
            .state
            .enqueue(Command::ServerUrl { value: text(edit) });
        let response = controller.handle(panel.take_command().unwrap(), now);
        set_text(edit, "http://127.0.0.1:806/");
        panel.complete(&response);
        panel.sync(&controller.ui_view(now));
        assert_eq!(text(edit), "http://127.0.0.1:806/");
        assert!(panel.state.endpoint_dirty.get());
        let response = controller.handle(Command::FollowServer { enabled: true }, now);
        assert_eq!(response.code, OK);
        panel.sync(&controller.ui_view(now));
        unsafe {
            assert!(!IsWindowEnabled(slider).as_bool());
        }
    }
}
