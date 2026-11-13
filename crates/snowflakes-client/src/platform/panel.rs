//! Modeless native controls. Rc/Cell owns callback state; no mutable borrow is
//! held across a Win32 call that can reenter this window procedure.
use super::native::wide;
use crate::{
    config::SnowPreset,
    control::{Command, OK, Response, Setting},
    session::{PauseReason, ProfileSource},
    ui::{self, Pending, View},
};
use snowflakes_protocol::{Display, MonitorSelection, Settings, WeatherMode};
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
const DISPLAY_MODE: i32 = 112;
const DISPLAY_LIST: i32 = 113;
const DISPLAY_APPLY: i32 = 114;
const DISPLAY_HINT: i32 = 115;
const TARGET_FPS: i32 = 116;
const BATTERY_SAVER: i32 = 117;
const QUALITY_INFO: i32 = 118;
const SNOW_OPTIONS: [i32; 4] = [151, 152, 153, 154];
const SNOW_OPTION_KEYS: [u8; 4] = [23, 24, 25, 26];
const SNOW_OPTIONS_TOP: i32 = 884;
const SNOW_OPTIONS_HEIGHT: i32 = 144;
const EXTRA_SETTINGS_HEIGHT: i32 = 204;
const PRESET: i32 = 155;
const PRESET_LABEL: i32 = 156;
const SLIDERS: [i32; 8] = [201, 205, 206, 202, 203, 204, 207, 208];
const SLIDER_KEYS: [u8; 8] = [1, 18, 19, 2, 3, 4, 7, 8];
const MINIMUM: [i16; 8] = [0, 25, 0, 0, 0, 0, 10, -100];
const VALUES: [i32; 8] = [301, 305, 306, 302, 303, 304, 307, 308];
const PRESETS: [SnowPreset; 4] = [
    SnowPreset::Custom,
    SnowPreset::Light,
    SnowPreset::Normal,
    SnowPreset::Heavy,
];
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
    inflight_display: RefCell<Option<Display>>,
    display: RefCell<Option<Display>>,
    display_dirty: Cell<bool>,
    monitor_rows: RefCell<Vec<ui::MonitorRow>>,
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
    fn display_editable(&self) -> bool {
        let view = self.view.borrow();
        !view.status.follow_server && view.status.settings.is_some()
    }
    fn capture_display(&self) {
        if !self.display_editable() {
            return;
        }
        let (Some(combo), Some(list)) = (self.control(DISPLAY_MODE), self.control(DISPLAY_LIST))
        else {
            return;
        };
        let selection = match unsafe { send(combo, CB_GETCURSEL, WPARAM(0), LPARAM(0)).0 } {
            0 => MonitorSelection::All,
            1 => MonitorSelection::Primary,
            2 => MonitorSelection::Selected,
            _ => return,
        };
        let rows = self.monitor_rows.borrow().clone();
        let monitor_ids = if selection == MonitorSelection::Selected {
            rows.iter()
                .enumerate()
                .filter(|(i, _)| unsafe { send(list, LB_GETSEL, WPARAM(*i), LPARAM(0)).0 > 0 })
                .map(|(_, r)| r.id.clone())
                .collect()
        } else {
            Vec::new()
        };
        self.display.replace(Some(Display {
            selection,
            monitor_ids,
        }));
        self.display_dirty.set(true);
    }
    fn apply_display(&self) {
        if !self.display_editable() || self.busy(ui::DISPLAY_COMMAND) {
            return;
        }
        let Some(display) = self.display.borrow().clone() else {
            return;
        };
        let valid = Settings {
            display: display.clone(),
            ..Default::default()
        }
        .validate()
        .is_ok();
        if !valid {
            if let Some(hint) = self.control(DISPLAY_HINT) {
                set_text(
                    hint,
                    "Выберите от 1 до 16 экранов и нажмите «Применить экраны».",
                );
            }
            return;
        }
        self.enqueue(Command::Set {
            setting: Setting::Display(display),
        });
    }
    fn sync_display(&self) {
        let view = self.view.borrow().clone();
        let canonical = view.status.settings.as_ref().map(|s| s.display.clone());
        let editable = !view.status.follow_server && canonical.is_some();
        if !editable || (!self.display_dirty.get() && !self.busy(ui::DISPLAY_COMMAND)) {
            self.display.replace(canonical.clone());
            self.display_dirty.set(false);
        }
        let draft = self.display.borrow().clone();
        let ids = draft
            .iter()
            .chain(canonical.iter())
            .flat_map(|d| d.monitor_ids.iter().cloned());
        let rows = ui::monitor_rows(view.monitors.as_deref().unwrap_or_default(), ids);
        let updating = self.updating.replace(true);
        unsafe {
            if let Some(combo) = self.control(DISPLAY_MODE) {
                if send(combo, CB_GETDROPPEDSTATE, WPARAM(0), LPARAM(0)).0 == 0 {
                    let selected = draft.as_ref().map_or(-1, |d| match d.selection {
                        MonitorSelection::All => 0,
                        MonitorSelection::Primary => 1,
                        MonitorSelection::Selected => 2,
                    });
                    send(combo, CB_SETCURSEL, WPARAM(selected as usize), LPARAM(0));
                }
                let _ = EnableWindow(combo, editable);
            }
            if let Some(list) = self.control(DISPLAY_LIST) {
                let previous = self.monitor_rows.borrow().clone();
                if previous != rows {
                    let focused_id =
                        usize::try_from(send(list, LB_GETCARETINDEX, WPARAM(0), LPARAM(0)).0)
                            .ok()
                            .and_then(|i| previous.get(i))
                            .map(|r| r.id.clone());
                    let top_id =
                        usize::try_from(send(list, LB_GETTOPINDEX, WPARAM(0), LPARAM(0)).0)
                            .ok()
                            .and_then(|i| previous.get(i))
                            .map(|r| r.id.clone());
                    send(list, LB_RESETCONTENT, WPARAM(0), LPARAM(0));
                    for row in &rows {
                        let caption = wide(&row.label);
                        send(
                            list,
                            LB_ADDSTRING,
                            WPARAM(0),
                            LPARAM(caption.as_ptr() as isize),
                        );
                    }
                    if let Some(index) =
                        rows.iter().position(|r| Some(&r.id) == focused_id.as_ref())
                    {
                        send(list, LB_SETCARETINDEX, WPARAM(index), LPARAM(0));
                    }
                    if let Some(index) = rows.iter().position(|r| Some(&r.id) == top_id.as_ref()) {
                        send(list, LB_SETTOPINDEX, WPARAM(index), LPARAM(0));
                    }
                    self.monitor_rows.replace(rows.clone());
                }
                let extent = rows
                    .iter()
                    .map(|r| r.label.chars().count())
                    .max()
                    .unwrap_or(0) as i32
                    * 7;
                send(
                    list,
                    LB_SETHORIZONTALEXTENT,
                    WPARAM(ui::scale(extent, self.dpi.get()).max(0) as usize),
                    LPARAM(0),
                );
                for (index, row) in rows.iter().enumerate() {
                    let selected = draft.as_ref().is_some_and(|d| row.selected(d));
                    if (send(list, LB_GETSEL, WPARAM(index), LPARAM(0)).0 > 0) != selected {
                        send(
                            list,
                            LB_SETSEL,
                            WPARAM(usize::from(selected)),
                            LPARAM(index as isize),
                        );
                    }
                }
                let _ = EnableWindow(
                    list,
                    editable
                        && view.monitors.is_some()
                        && draft
                            .as_ref()
                            .is_some_and(|d| d.selection == MonitorSelection::Selected),
                );
            }
            if let Some(apply) = self.control(DISPLAY_APPLY) {
                let _ = EnableWindow(
                    apply,
                    editable && self.display_dirty.get() && !self.busy(ui::DISPLAY_COMMAND),
                );
            }
        }
        if let Some(hint) = self.control(DISPLAY_HINT) {
            let selected = draft
                .as_ref()
                .is_some_and(|d| d.selection == MonitorSelection::Selected);
            let count = draft.as_ref().map_or(0, |d| d.monitor_ids.len());
            let any_connected = draft
                .as_ref()
                .is_some_and(|d| rows.iter().any(|r| r.connected && r.selected(d)));
            set_text(
                hint,
                if view.monitors.is_none() {
                    "Список экранов пока недоступен. Сохранённый выбор не меняется."
                } else if selected && (count == 0 || count > 16) {
                    "Выберите от 1 до 16 экранов и нажмите «Применить экраны»."
                } else if selected && !any_connected {
                    "Выбранные экраны недоступны. Подключите их или выберите другие; сохранённый список не удаляется."
                } else {
                    "В режиме «Выбранные» щелчок или Пробел переключает выделение. Затем нажмите «Применить экраны»."
                },
            );
        }
        self.updating.set(updating);
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
                inflight_display: RefCell::new(None),
                display: RefCell::new(None),
                display_dirty: Cell::new(false),
                monitor_rows: RefCell::new(Vec::new()),
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
                "Размер сне&жинок",
                "Заметность снеж&инок",
                "Интенсивность &дождя",
                "Размер с&угробов",
                "Размер &луж",
                "Общая неп&розрачность",
                "Ветер &снега",
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
            panel.child(
                PRESET_LABEL,
                "STATIC",
                "Пресет интенсивности сне&га",
                0,
                false,
                20,
                816,
                0,
                20,
                true,
                false,
            )?;
            panel.child(
                PRESET,
                "COMBOBOX",
                "",
                CBS_DROPDOWNLIST as u32,
                true,
                20,
                842,
                0,
                180,
                true,
                false,
            )?;
            if let Some(combo) = panel.state.control(PRESET) {
                for title in [
                    "Свои настройки",
                    "Лёгкий — 20%",
                    "Обычный — 50%",
                    "Сильный — 85%",
                ] {
                    let title = wide(title);
                    send(
                        combo,
                        CB_ADDSTRING,
                        WPARAM(0),
                        LPARAM(title.as_ptr() as isize),
                    );
                }
            }
            for (index, title) in [
                "Снег на &окнах",
                "Снег внизу &экрана",
                "&Таяние снега",
                "Смахивание &курсором",
            ]
            .iter()
            .enumerate()
            {
                panel.child(
                    SNOW_OPTIONS[index],
                    "BUTTON",
                    title,
                    BS_AUTOCHECKBOX as u32,
                    true,
                    20,
                    SNOW_OPTIONS_TOP + index as i32 * 32,
                    0,
                    26,
                    true,
                    false,
                )?;
            }
            panel.child(
                140,
                "STATIC",
                "Экра&ны",
                0,
                false,
                20,
                680,
                0,
                20,
                true,
                false,
            )?;
            panel.child(
                DISPLAY_MODE,
                "COMBOBOX",
                "",
                CBS_DROPDOWNLIST as u32 | WS_VSCROLL.0,
                true,
                20,
                706,
                210,
                140,
                false,
                false,
            )?;
            panel.child(
                DISPLAY_APPLY,
                "BUTTON",
                "Применить &экраны",
                0,
                true,
                250,
                706,
                0,
                28,
                true,
                false,
            )?;
            panel.child(
                141,
                "STATIC",
                "Вы&бранные экраны",
                0,
                false,
                20,
                746,
                0,
                20,
                true,
                false,
            )?;
            panel.child(
                DISPLAY_LIST,
                "LISTBOX",
                "",
                (LBS_MULTIPLESEL | LBS_NOTIFY | LBS_NOINTEGRALHEIGHT) as u32
                    | WS_BORDER.0
                    | WS_VSCROLL.0
                    | WS_HSCROLL.0,
                true,
                20,
                774,
                0,
                104,
                true,
                false,
            )?;
            panel.child(
                DISPLAY_HINT,
                "STATIC",
                "",
                0,
                false,
                20,
                888,
                0,
                56,
                true,
                false,
            )?;
            if let Some(combo) = panel.state.control(DISPLAY_MODE) {
                for title in ["Все экраны", "Основной экран", "Выбранные"]
                {
                    let title = wide(title);
                    send(
                        combo,
                        CB_ADDSTRING,
                        WPARAM(0),
                        LPARAM(title.as_ptr() as isize),
                    );
                }
            }
            panel.child(
                142,
                "STATIC",
                "Частота &кадров",
                0,
                false,
                20,
                952,
                0,
                20,
                true,
                false,
            )?;
            panel.child(
                TARGET_FPS,
                "COMBOBOX",
                "",
                CBS_DROPDOWNLIST as u32,
                true,
                20,
                978,
                210,
                120,
                false,
                false,
            )?;
            if let Some(combo) = panel.state.control(TARGET_FPS) {
                for title in ["30 FPS", "60 FPS"] {
                    let title = wide(title);
                    send(
                        combo,
                        CB_ADDSTRING,
                        WPARAM(0),
                        LPARAM(title.as_ptr() as isize),
                    );
                }
            }
            panel.child(
                BATTERY_SAVER,
                "BUTTON",
                "&Экономить заряд батареи",
                BS_AUTOCHECKBOX as u32,
                true,
                20,
                1018,
                0,
                26,
                true,
                false,
            )?;
            panel.child(
                QUALITY_INFO,
                "STATIC",
                "",
                0,
                false,
                20,
                1052,
                0,
                40,
                true,
                false,
            )?;
            panel.child(HINT, "STATIC", "", 0, false, 20, 1100, 0, 38, true, false)?;
            panel.child(
                FEEDBACK, "STATIC", "", 0, false, 20, 1142, 0, 32, true, false,
            )?;
            panel.child(
                PAUSE,
                "BUTTON",
                "&Пауза",
                0,
                true,
                20,
                1190,
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
                1190,
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
                1190,
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
        let new_setting = SNOW_OPTIONS.contains(&id)
            || [PRESET, PRESET_LABEL].contains(&id)
            || SLIDERS.contains(&id)
            || VALUES.contains(&id)
            || (130..138).contains(&id);
        let y = if y >= 680 && !new_setting {
            y + SNOW_OPTIONS_HEIGHT + EXTRA_SETTINGS_HEIGHT
        } else {
            y
        };
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
            if let Command::Set {
                setting: Setting::Display(value),
            } = command
            {
                self.state.inflight_display.replace(Some(value.clone()));
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
        if key == Some(ui::DISPLAY_COMMAND) {
            let submitted = self.state.inflight_display.take();
            if response.code == OK
                && !self.state.pending.borrow().contains(ui::DISPLAY_COMMAND)
                && *self.state.display.borrow() == submitted
            {
                self.state.display_dirty.set(false);
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
                    } else if settings
                        .as_ref()
                        .is_some_and(|s| s.weather.mode == WeatherMode::Rain)
                    {
                        "Капли и брызги доступны. Накопление воды ещё в разработке."
                    } else {
                        "100% — текущий размер и заметность; разница между снежинками сохраняется."
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
            let options = settings.as_ref().map(|s| {
                [
                    s.snow.on_windows,
                    s.snow.on_ground,
                    s.snow.melting,
                    s.interaction.cursor_enabled,
                ]
            });
            for (index, &id) in SNOW_OPTIONS.iter().enumerate() {
                if let Some(check) = self.state.control(id) {
                    if !self.state.busy(SNOW_OPTION_KEYS[index]) {
                        checked(Some(check), options.is_some_and(|values| values[index]));
                    }
                    let _ = EnableWindow(check, local && settings.is_some());
                }
            }
            if let Some(combo) = self.state.control(TARGET_FPS) {
                if !self.state.busy(21)
                    && send(combo, CB_GETDROPPEDSTATE, WPARAM(0), LPARAM(0)).0 == 0
                {
                    let selection = settings
                        .as_ref()
                        .map_or(-1, |s| if s.performance.target_fps == 30 { 0 } else { 1 });
                    send(combo, CB_SETCURSEL, WPARAM(selection as usize), LPARAM(0));
                }
                let _ = EnableWindow(combo, local && settings.is_some());
            }
            if let Some(check) = self.state.control(BATTERY_SAVER) {
                if !self.state.busy(22) {
                    checked(
                        Some(check),
                        settings
                            .as_ref()
                            .is_some_and(|s| s.performance.battery_saver),
                    );
                }
                let _ = EnableWindow(check, local && settings.is_some());
            }
            if let Some(info) = self.state.control(QUALITY_INFO) {
                set_text(
                    info,
                    &if view.status.effective_target_fps == 0 {
                        "На батарее: до 30 FPS и меньше теней. Количество осадков сохраняется."
                            .into()
                    } else {
                        format!(
                            "Цель: {} FPS. {}",
                            view.status.effective_target_fps,
                            if view.status.reduced_detail {
                                "Экономия заряда активна."
                            } else {
                                "Полная детализация."
                            }
                        )
                    },
                );
            }
            if let Some(combo) = self.state.control(PRESET) {
                if !self.state.busy(27)
                    && !self.state.busy(1)
                    && send(combo, CB_GETDROPPEDSTATE, WPARAM(0), LPARAM(0)).0 == 0
                {
                    let selected = view
                        .status
                        .snow_preset
                        .and_then(|preset| PRESETS.iter().position(|p| *p == preset))
                        .map_or(-1, |index| index as isize);
                    send(combo, CB_SETCURSEL, WPARAM(selected as usize), LPARAM(0));
                }
                let _ = EnableWindow(
                    combo,
                    local
                        && settings
                            .as_ref()
                            .is_some_and(|s| s.weather.mode == WeatherMode::Snow)
                        && view.limits.max_intensity_percent > 0
                        && view.limits.allow_snow,
                );
            }
            let values = settings.as_ref().map(|s| {
                [
                    i16::from(s.snow.intensity_percent),
                    i16::from(s.snow.flake_size_percent),
                    i16::from(s.snow.flake_visibility_percent),
                    i16::from(s.rain.intensity_percent),
                    i16::from(s.snow.drift_size_percent),
                    i16::from(s.rain.puddle_size_percent),
                    i16::from(s.weather.opacity_percent),
                    s.snow.wind_percent,
                ]
            });
            let caps = [
                view.limits.max_intensity_percent,
                200,
                200,
                view.limits.max_intensity_percent,
                view.limits.max_drift_size_percent,
                view.limits.max_puddle_size_percent,
                100,
                100,
            ];
            for index in 0..SLIDERS.len() {
                if let Some(slider) = self.state.control(SLIDERS[index]) {
                    send(
                        slider,
                        TBM_SETRANGEMIN,
                        WPARAM(0),
                        LPARAM(MINIMUM[index] as isize),
                    );
                    send(
                        slider,
                        TBM_SETRANGEMAX,
                        WPARAM(1),
                        LPARAM(caps[index] as isize),
                    );
                    send(slider, TBM_SETPAGESIZE, WPARAM(0), LPARAM(10));
                    if self.state.tracking.get() != SLIDERS[index]
                        && !self.state.busy(SLIDER_KEYS[index])
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
        self.state.sync_display();
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
                state.display_dirty.set(false);
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
                    let value = send(control, TBM_GETPOS, WPARAM(0), LPARAM(0)).0.clamp(
                        MINIMUM[index] as isize,
                        if index == 1 || index == 2 { 200 } else { 100 },
                    ) as i16;
                    let notification = (wparam.0 & 0xffff) as u32;
                    state
                        .tracking
                        .set(if notification == TB_THUMBTRACK { id } else { 0 });
                    if let Some(label) = state.control(VALUES[index]) {
                        set_text(label, &format!("{value}%"));
                    }
                    let setting = match index {
                        0 => Setting::SnowIntensity(value as u8),
                        1 => Setting::FlakeSize(value as u8),
                        2 => Setting::FlakeVisibility(value as u8),
                        3 => Setting::RainIntensity(value as u8),
                        4 => Setting::DriftSize(value as u8),
                        5 => Setting::PuddleSize(value as u8),
                        6 => Setting::Opacity(value as u8),
                        _ => Setting::SnowWind(value),
                    };
                    state.enqueue(Command::Set { setting });
                }
                LRESULT(0)
            }
            WM_COMMAND if !state.updating.get() => {
                let id = (wparam.0 & 0xffff) as i32;
                let code = (wparam.0 >> 16) as u32;
                match (id, code) {
                    (PRESET, CBN_SELCHANGE) => {
                        if let Some(combo) = state.control(PRESET) {
                            let index = send(combo, CB_GETCURSEL, WPARAM(0), LPARAM(0)).0;
                            if let Some(&preset) = PRESETS.get(index as usize) {
                                state.enqueue(Command::Set {
                                    setting: Setting::SnowPreset(preset),
                                });
                            }
                        }
                    }
                    (DISPLAY_MODE, CBN_SELCHANGE) => {
                        state.capture_display();
                        state.sync_display();
                    }
                    (DISPLAY_LIST, code) if code == LBN_SELCHANGE => {
                        state.capture_display();
                    }
                    (DISPLAY_APPLY, BN_CLICKED) => state.apply_display(),
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
                    (BATTERY_SAVER, BN_CLICKED) => {
                        if let Some(check) = state.control(BATTERY_SAVER) {
                            state.enqueue(Command::Set {
                                setting: Setting::BatterySaver(
                                    send(check, BM_GETCHECK, WPARAM(0), LPARAM(0)).0 != 0,
                                ),
                            });
                        }
                    }
                    (id, BN_CLICKED) if SNOW_OPTIONS.contains(&id) => {
                        let index = SNOW_OPTIONS.iter().position(|&item| item == id).unwrap();
                        if let Some(check) = state.control(id) {
                            let enabled = send(check, BM_GETCHECK, WPARAM(0), LPARAM(0)).0 != 0;
                            let setting = match index {
                                0 => Setting::SnowOnWindows(enabled),
                                1 => Setting::SnowOnGround(enabled),
                                2 => Setting::SnowMelting(enabled),
                                _ => Setting::CursorEnabled(enabled),
                            };
                            state.enqueue(Command::Set { setting });
                        }
                    }
                    (TARGET_FPS, CBN_SELCHANGE) => {
                        if let Some(combo) = state.control(TARGET_FPS) {
                            let selected = send(combo, CB_GETCURSEL, WPARAM(0), LPARAM(0)).0;
                            if (0..=1).contains(&selected) {
                                state.enqueue(Command::Set {
                                    setting: Setting::TargetFps(if selected == 0 {
                                        30
                                    } else {
                                        60
                                    }),
                                });
                            }
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
    static PANEL_TEST: std::sync::Mutex<()> = std::sync::Mutex::new(());

    // Hidden controls test the real Win32 ABI/creation path, not visual input.
    #[test]
    fn hidden_panel_projects_source_and_policy_without_overwriting_endpoint_edits() {
        let _serial = PANEL_TEST.lock().unwrap();
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
        let child_ids: std::collections::BTreeSet<_> = panel
            .state
            .children
            .borrow()
            .iter()
            .map(|child| child.id)
            .collect();
        assert_eq!(child_ids.len(), panel.state.children.borrow().len());
        let slider = panel.state.control(SLIDERS[0]).unwrap();
        let fps = panel.state.control(TARGET_FPS).unwrap();
        let saver = panel.state.control(BATTERY_SAVER).unwrap();
        let snow_checks: Vec<_> = SNOW_OPTIONS
            .iter()
            .map(|&id| panel.state.control(id).unwrap())
            .collect();
        unsafe {
            assert!(!IsWindowEnabled(slider).as_bool());
            assert!(!IsWindowEnabled(fps).as_bool());
            assert!(!IsWindowEnabled(saver).as_bool());
            for &check in &snow_checks {
                assert!(!IsWindowEnabled(check).as_bool());
            }
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
            assert!(IsWindowEnabled(fps).as_bool());
            assert!(IsWindowEnabled(saver).as_bool());
            assert_eq!(send(fps, CB_GETCURSEL, WPARAM(0), LPARAM(0)).0, 1);
            assert_eq!(send(saver, BM_GETCHECK, WPARAM(0), LPARAM(0)).0, 1);
            send(fps, CB_SETCURSEL, WPARAM(0), LPARAM(0));
            send(
                panel.hwnd,
                WM_COMMAND,
                WPARAM(((CBN_SELCHANGE as usize) << 16) | TARGET_FPS as usize),
                LPARAM(0),
            );
            send(saver, BM_SETCHECK, WPARAM(0), LPARAM(0));
            send(
                panel.hwnd,
                WM_COMMAND,
                WPARAM(BATTERY_SAVER as usize),
                LPARAM(0),
            );
        }
        controller.handle(Command::Pause {}, now);
        for _ in 0..2 {
            let response = controller.handle(panel.take_command().unwrap(), now);
            assert_eq!(response.code, OK);
            panel.complete(&response);
            panel.sync(&controller.ui_view(now));
        }
        assert_eq!(controller.config().settings.performance.target_fps, 30);
        assert!(!controller.config().settings.performance.battery_saver);
        assert_eq!(controller.status(now).pauses, [PauseReason::Manual]);
        assert!(panel.take_command().is_none());
        for (index, &check) in snow_checks.iter().enumerate() {
            unsafe {
                assert!(IsWindowEnabled(check).as_bool());
                assert_eq!(send(check, BM_GETCHECK, WPARAM(0), LPARAM(0)).0, 1);
                send(check, BM_SETCHECK, WPARAM(0), LPARAM(0));
                send(
                    panel.hwnd,
                    WM_COMMAND,
                    WPARAM(SNOW_OPTIONS[index] as usize),
                    LPARAM(0),
                );
            }
        }
        for _ in 0..4 {
            let response = controller.handle(panel.take_command().unwrap(), now);
            assert_eq!(response.code, OK);
            panel.complete(&response);
            panel.sync(&controller.ui_view(now));
            for &check in &snow_checks {
                assert_eq!(
                    unsafe { send(check, BM_GETCHECK, WPARAM(0), LPARAM(0)).0 },
                    0
                );
            }
        }
        let saved = &controller.config().settings;
        assert!(
            !saved.snow.on_windows
                && !saved.snow.on_ground
                && !saved.snow.melting
                && !saved.interaction.cursor_enabled
        );
        assert_eq!(controller.status(now).pauses, [PauseReason::Manual]);
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
            assert!(!IsWindowEnabled(fps).as_bool());
            assert!(!IsWindowEnabled(saver).as_bool());
            for &check in &snow_checks {
                assert!(!IsWindowEnabled(check).as_bool());
            }
        }
    }

    #[test]
    fn hidden_snow_controls_keep_signed_wind_opacity_and_preset_state() {
        let _serial = PANEL_TEST.lock().unwrap();
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
        let wind = panel.state.control(SLIDERS[7]).unwrap();
        let opacity = panel.state.control(SLIDERS[6]).unwrap();
        let preset = panel.state.control(PRESET).unwrap();
        fn dispatch(panel: &Panel, controller: &mut Controller, now: std::time::Instant) {
            let result = controller.handle(panel.take_command().unwrap(), now);
            assert_eq!(result.code, OK);
            panel.complete(&result);
            panel.sync(&controller.ui_view(now));
        }
        unsafe {
            for handle in [wind, opacity, preset] {
                assert!(!IsWindowEnabled(handle).as_bool());
            }
        }
        controller.handle(Command::FollowServer { enabled: false }, now);
        controller.handle(Command::Pause {}, now);
        panel.sync(&controller.ui_view(now));
        for (slider, value) in [
            (wind, -100),
            (wind, 100),
            (wind, -45),
            (opacity, 10),
            (opacity, 100),
            (opacity, 63),
        ] {
            unsafe {
                assert!(IsWindowEnabled(slider).as_bool());
                send(slider, TBM_SETPOS, WPARAM(1), LPARAM(value));
                send(
                    panel.hwnd,
                    WM_HSCROLL,
                    WPARAM(TB_ENDTRACK as usize),
                    LPARAM(slider.0 as isize),
                );
            }
            panel.sync(&controller.ui_view(now)); // In-flight input must survive an older snapshot.
            assert_eq!(
                unsafe { send(slider, TBM_GETPOS, WPARAM(0), LPARAM(0)).0 },
                value
            );
            dispatch(&panel, &mut controller, now);
            let settings = &controller.config().settings;
            assert_eq!(
                if slider == wind {
                    settings.snow.wind_percent as isize
                } else {
                    settings.weather.opacity_percent as isize
                },
                value
            );
        }
        controller
            .session
            .set_limits(
                Limits {
                    max_intensity_percent: 25,
                    ..Default::default()
                },
                now,
            )
            .unwrap();
        panel.sync(&controller.ui_view(now));
        unsafe {
            send(preset, CB_SETCURSEL, WPARAM(3), LPARAM(0));
            send(
                panel.hwnd,
                WM_COMMAND,
                WPARAM(((CBN_SELCHANGE as usize) << 16) | PRESET as usize),
                LPARAM(0),
            );
        }
        dispatch(&panel, &mut controller, now);
        assert_eq!(controller.config().settings.snow.intensity_percent, 85);
        assert_eq!(
            controller
                .status(now)
                .settings
                .unwrap()
                .snow
                .intensity_percent,
            25
        );
        assert_eq!(controller.config().settings.snow.wind_percent, -45);
        assert_eq!(controller.config().settings.weather.opacity_percent, 63);
        assert_eq!(
            unsafe { send(preset, CB_GETCURSEL, WPARAM(0), LPARAM(0)).0 },
            3
        );
        unsafe {
            send(wind, TBM_SETPOS, WPARAM(1), LPARAM(-20));
            send(
                panel.hwnd,
                WM_HSCROLL,
                WPARAM(TB_ENDTRACK as usize),
                LPARAM(wind.0 as isize),
            );
        }
        dispatch(&panel, &mut controller, now);
        assert_eq!(
            unsafe { send(preset, CB_GETCURSEL, WPARAM(0), LPARAM(0)).0 },
            0
        );
        assert_eq!(controller.status(now).pauses, [PauseReason::Manual]);
        controller.handle(Command::FollowServer { enabled: true }, now);
        panel.sync(&controller.ui_view(now));
        unsafe {
            for handle in [wind, opacity, preset] {
                assert!(!IsWindowEnabled(handle).as_bool());
            }
        }
    }
    #[test]
    fn hidden_monitor_picker_preserves_drafts_offline_choices_and_manual_pause() {
        let _serial = PANEL_TEST.lock().unwrap();
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
        controller.handle(Command::FollowServer { enabled: false }, now);
        controller.handle(Command::Pause {}, now);
        let monitors: Vec<_> = (0..2)
            .map(|index| ui::Monitor {
                id: format!("display-{}", index + 1),
                rect: crate::geometry::Rect {
                    left: index as f32 * 800.0,
                    top: 0.0,
                    right: (index + 1) as f32 * 800.0,
                    bottom: 600.0,
                },
                primary: index == 0,
                persistent: true,
            })
            .collect();
        let view = |controller: &Controller, devices: Option<Vec<ui::Monitor>>| {
            let mut view = controller.ui_view(now);
            view.monitors = devices;
            view
        };
        let panel =
            Panel::create(&view(&controller, Some(monitors.clone())), HWND::default()).unwrap();
        let combo = panel.state.control(DISPLAY_MODE).unwrap();
        let list = panel.state.control(DISPLAY_LIST).unwrap();
        let notify = |id: i32, code: u32| unsafe {
            send(
                panel.hwnd,
                WM_COMMAND,
                WPARAM(((code as usize) << 16) | id as usize),
                LPARAM(0),
            );
        };
        let check = |index: usize, selected: bool| unsafe {
            send(
                list,
                LB_SETSEL,
                WPARAM(usize::from(selected)),
                LPARAM(index as isize),
            );
            notify(DISPLAY_LIST, LBN_SELCHANGE);
        };
        unsafe {
            assert!(!IsWindowVisible(panel.hwnd).as_bool());
            assert!(!IsWindowEnabled(list).as_bool());
            assert_eq!(send(list, LB_GETCOUNT, WPARAM(0), LPARAM(0)).0, 2);
            send(combo, CB_SETCURSEL, WPARAM(2), LPARAM(0));
        }
        notify(DISPLAY_MODE, CBN_SELCHANGE);
        check(0, false);
        assert!(panel.take_command().is_none());
        notify(DISPLAY_APPLY, BN_CLICKED);
        let response = controller.handle(panel.take_command().unwrap(), now);
        assert_eq!(response.code, OK);
        assert_eq!(
            controller.config().settings.display.monitor_ids,
            ["display-2"]
        );
        // A later edit must survive acknowledgement of the submitted selection.
        check(0, true);
        check(1, false);
        panel.complete(&response);
        panel.sync(&view(&controller, Some(monitors.clone())));
        assert!(panel.state.display_dirty.get());
        assert_eq!(
            panel.state.display.borrow().as_ref().unwrap().monitor_ids,
            ["display-1"]
        );
        unsafe {
            assert_eq!(send(list, LB_GETSEL, WPARAM(0), LPARAM(0)).0, 1);
            assert_eq!(send(list, LB_GETSEL, WPARAM(1), LPARAM(0)).0, 0);
        }
        notify(DISPLAY_APPLY, BN_CLICKED);
        let response = controller.handle(panel.take_command().unwrap(), now);
        panel.complete(&response);
        panel.sync(&view(&controller, Some(monitors.clone())));
        assert!(!panel.state.display_dirty.get());
        assert!(controller.status(now).pauses.contains(&PauseReason::Manual));
        assert_eq!(
            controller.config().settings.display.monitor_ids,
            ["display-1"]
        );
        panel.sync(&view(&controller, Some(vec![monitors[1].clone()])));
        assert!(!panel.state.monitor_rows.borrow()[1].connected);
        assert_eq!(panel.state.monitor_rows.borrow()[1].id, "display-1");
        unsafe {
            assert_eq!(send(list, LB_GETSEL, WPARAM(0), LPARAM(0)).0, 0);
            assert_eq!(send(list, LB_GETSEL, WPARAM(1), LPARAM(0)).0, 1);
        }
        panel.sync(&view(&controller, None));
        assert!(panel.take_command().is_none());
        assert_eq!(
            controller.config().settings.display.monitor_ids,
            ["display-1"]
        );
        unsafe {
            assert!(!IsWindowEnabled(list).as_bool());
        }
        controller.handle(Command::FollowServer { enabled: true }, now);
        panel.sync(&view(&controller, Some(monitors.clone())));
        unsafe {
            assert!(!IsWindowEnabled(combo).as_bool());
            assert!(!IsWindowEnabled(list).as_bool());
            assert!(!IsWindowEnabled(panel.state.control(DISPLAY_APPLY).unwrap()).as_bool());
        }
        controller.handle(Command::FollowServer { enabled: false }, now);
        panel.sync(&view(&controller, Some(monitors)));
        check(0, false);
        notify(DISPLAY_APPLY, BN_CLICKED);
        assert!(panel.take_command().is_none());
        assert!(text(panel.state.control(DISPLAY_HINT).unwrap()).contains("от 1 до 16"));
        assert_eq!(
            controller.config().settings.display.monitor_ids,
            ["display-1"]
        );
    }
}
