//! Native queries produce data-only geometry. WinEvent callbacks only coalesce IDs.
use crate::{
    geometry::{Rect, Scene, Screen},
    scene_registry::{Changes, Registry, Source, WindowInfo},
};
use std::{
    cell::RefCell,
    collections::{BTreeMap, BTreeSet},
    time::{Duration, Instant},
};
use windows::{
    Win32::{
        Foundation::*,
        Graphics::{Dwm::*, Gdi::*},
        UI::{Accessibility::*, WindowsAndMessaging::*},
    },
    core::{BOOL, PCWSTR, Result},
};
mod order;

thread_local! {
    static CHANGES: RefCell<Changes> = RefCell::new(Changes::default());
}
unsafe extern "system" fn event(
    _: HWINEVENTHOOK,
    event: u32,
    hwnd: HWND,
    object: i32,
    child: i32,
    _: u32,
    _: u32,
) {
    CHANGES.with_borrow_mut(|changes| {
        // Reorder notifications may identify a parent's client object, and
        // desktop/menu events may have no window at all.
        match event {
            EVENT_SYSTEM_DESKTOPSWITCH => changes.all(),
            EVENT_OBJECT_REORDER
            | EVENT_SYSTEM_FOREGROUND
            | EVENT_SYSTEM_MENUSTART
            | EVENT_SYSTEM_MENUEND
            | EVENT_SYSTEM_MENUPOPUPSTART
            | EVENT_SYSTEM_MENUPOPUPEND => changes.order(),
            _ => {}
        }
        if object != OBJID_WINDOW.0 || child != 0 || hwnd.is_invalid() {
            return;
        }
        match event {
            EVENT_OBJECT_DESTROY => changes.window(hwnd.0 as usize, true),
            EVENT_OBJECT_CREATE
            | EVENT_OBJECT_SHOW
            | EVENT_OBJECT_HIDE
            | EVENT_OBJECT_LOCATIONCHANGE
            | EVENT_OBJECT_STATECHANGE
            | EVENT_OBJECT_CLOAKED
            | EVENT_OBJECT_UNCLOAKED
            | EVENT_SYSTEM_FOREGROUND
            | EVENT_SYSTEM_MINIMIZESTART
            | EVENT_SYSTEM_MINIMIZEEND => {
                changes.window(hwnd.0 as usize, false);
            }
            _ => {}
        }
    });
}
fn rect(value: RECT) -> Rect {
    Rect {
        left: value.left as f32,
        top: value.top as f32,
        right: value.right as f32,
        bottom: value.bottom as f32,
    }
}
unsafe extern "system" fn monitor(h: HMONITOR, _: HDC, _: *mut RECT, data: LPARAM) -> BOOL {
    unsafe {
        let mut info = MONITORINFOEXW {
            monitorInfo: MONITORINFO {
                cbSize: size_of::<MONITORINFOEXW>() as u32,
                ..Default::default()
            },
            ..Default::default()
        };
        if GetMonitorInfoW(h, &mut info.monitorInfo).as_bool() {
            let mut paths = Vec::new();
            for index in 0..32 {
                let mut device = DISPLAY_DEVICEW {
                    cb: size_of::<DISPLAY_DEVICEW>() as u32,
                    ..Default::default()
                };
                const EDD_GET_DEVICE_INTERFACE_NAME: u32 = 1;
                if !EnumDisplayDevicesW(
                    PCWSTR(info.szDevice.as_ptr()),
                    index,
                    &mut device,
                    EDD_GET_DEVICE_INTERFACE_NAME,
                )
                .as_bool()
                {
                    break;
                }
                if device.StateFlags.0 & DISPLAY_DEVICE_ACTIVE.0 != 0 {
                    let length = device
                        .DeviceID
                        .iter()
                        .position(|&c| c == 0)
                        .unwrap_or(device.DeviceID.len());
                    if length > 0 {
                        paths.push(String::from_utf16_lossy(&device.DeviceID[..length]));
                    }
                }
            }
            let length = info
                .szDevice
                .iter()
                .position(|&c| c == 0)
                .unwrap_or(info.szDevice.len());
            let device_name = String::from_utf16_lossy(&info.szDevice[..length]);
            let (key, persistent_id) = crate::display_identity::key(&paths, &device_name);
            (&mut *(data.0 as *mut Vec<Screen>)).push(Screen {
                key,
                persistent_id,
                rect: rect(info.monitorInfo.rcMonitor),
                floor: info.monitorInfo.rcWork.bottom as f32,
                scale: 1.0,
            });
        }
    }
    BOOL(1)
}
#[derive(Default)]
struct Native {
    owners: BTreeMap<usize, (u32, u32)>,
}
pub fn screens() -> Result<Vec<Screen>> {
    Native::default().screens()
}
impl Source for Native {
    type Error = windows::core::Error;
    fn order(&mut self) -> Result<Vec<usize>> {
        self.owners.clear();
        let marks = order::capture()?;
        self.owners = marks
            .iter()
            .map(|mark| (mark.handle, (mark.owner.process, mark.owner.thread)))
            .collect();
        Ok(marks.into_iter().map(|mark| mark.handle).collect())
    }
    fn owner(&self, handle: usize) -> Option<(u32, u32)> {
        self.owners.get(&handle).copied()
    }
    fn screens(&mut self) -> Result<Vec<Screen>> {
        let mut screens = Vec::<Screen>::new();
        unsafe {
            EnumDisplayMonitors(
                None,
                None,
                Some(monitor),
                LPARAM(&mut screens as *mut _ as isize),
            )
            .ok()?;
        }
        Ok(screens)
    }
    fn foreground(&mut self) -> usize {
        unsafe { GetForegroundWindow() }.0 as usize
    }
    fn read(&mut self, handle: usize) -> Option<WindowInfo> {
        let hwnd = HWND(handle as *mut _);
        unsafe {
            let mut process = 0;
            let thread = GetWindowThreadProcessId(hwnd, Some(&mut process));
            if process == std::process::id()
                || thread == 0
                || !IsWindowVisible(hwnd).as_bool()
                || IsIconic(hwnd).as_bool()
            {
                return None;
            }
            let style = GetWindowLongW(hwnd, GWL_STYLE) as u32;
            if style & WS_CHILD.0 != 0 {
                return None;
            }
            let extended = GetWindowLongW(hwnd, GWL_EXSTYLE) as u32;
            let mut cloaked = 0_u32;
            if DwmGetWindowAttribute(hwnd, DWMWA_CLOAKED, (&mut cloaked as *mut u32).cast(), 4)
                .is_err()
            {
                return None;
            }
            let mut bounds = RECT::default();
            if DwmGetWindowAttribute(
                hwnd,
                DWMWA_EXTENDED_FRAME_BOUNDS,
                (&mut bounds as *mut RECT).cast(),
                size_of::<RECT>() as u32,
            )
            .is_err()
                && GetWindowRect(hwnd, &mut bounds).is_err()
            {
                return None;
            }
            if bounds.right <= bounds.left || bounds.bottom <= bounds.top {
                return None;
            }
            let mut name = [0_u16; 128];
            let length = GetClassNameW(hwnd, &mut name).max(0) as usize;
            let class = String::from_utf16_lossy(&name[..length]);
            if matches!(class.as_str(), "Progman" | "WorkerW") {
                return None;
            }
            let taskbar = matches!(class.as_str(), "Shell_TrayWnd" | "Shell_SecondaryTrayWnd");
            let popup = matches!(class.as_str(), "#32768" | "tooltips_class32");
            let collect = style & WS_CAPTION.0 == WS_CAPTION.0
                && extended & WS_EX_TOOLWINDOW.0 == 0
                && !taskbar
                && !popup;
            let maximized = IsZoomed(hwnd).as_bool();
            // Exclude a disappearing/reused HWND observed midway through the query.
            let mut final_process = 0;
            if GetWindowThreadProcessId(hwnd, Some(&mut final_process)) != thread
                || final_process != process
            {
                return None;
            }
            Some(WindowInfo {
                handle,
                process,
                thread,
                rect: rect(bounds),
                visible: cloaked == 0,
                collect,
                maximized,
                taskbar,
            })
        }
    }
}
pub struct Tracker {
    hooks: Vec<HWINEVENTHOOK>,
    registry: Registry,
    started: Instant,
    ready: bool,
    initialized: bool,
    retry_at: Option<Instant>,
    pub fullscreen: BTreeSet<u64>,
}
impl Tracker {
    pub fn new() -> Result<Self> {
        CHANGES.with_borrow_mut(|changes| *changes = Changes::default());
        let mut tracker = Self {
            hooks: Vec::new(),
            registry: Registry::default(),
            started: Instant::now(),
            ready: false,
            initialized: false,
            retry_at: None,
            fullscreen: BTreeSet::new(),
        };
        for (first, last) in [
            (EVENT_OBJECT_CREATE, EVENT_OBJECT_LOCATIONCHANGE),
            (EVENT_OBJECT_CLOAKED, EVENT_OBJECT_UNCLOAKED),
            (EVENT_SYSTEM_FOREGROUND, EVENT_SYSTEM_MINIMIZEEND),
            (EVENT_SYSTEM_DESKTOPSWITCH, EVENT_SYSTEM_DESKTOPSWITCH),
        ] {
            let hook = unsafe {
                SetWinEventHook(
                    first,
                    last,
                    None,
                    Some(event),
                    0,
                    0,
                    WINEVENT_OUTOFCONTEXT | WINEVENT_SKIPOWNPROCESS,
                )
            };
            if hook.is_invalid() {
                return Err(windows::core::Error::from_thread());
            }
            tracker.hooks.push(hook);
        }
        Ok(tracker)
    }
    pub fn poll(&mut self, now: Instant) -> Result<Option<Scene>> {
        // Release the callback cell before invoking Win32, which may reenter hooks.
        self.registry
            .changes
            .merge(CHANGES.with_borrow_mut(std::mem::take));
        self.poll_source(now, &mut Native::default())
    }
    fn poll_source<S: Source<Error = windows::core::Error>>(
        &mut self,
        now: Instant,
        source: &mut S,
    ) -> Result<Option<Scene>> {
        if self.retry_at.is_some_and(|deadline| now < deadline) {
            return Ok(None);
        }
        let scene = match self.registry.poll(now.duration_since(self.started), source) {
            Ok(scene) => {
                self.retry_at = None;
                scene
            }
            // Source queries are geometry, not GPU operations. A transient order
            // or display-query failure must not spend the graphics retry budget.
            Err(_) => {
                self.defer(now);
                return Ok(None);
            }
        };
        if scene.is_some() {
            self.ready = true;
            self.initialized = true;
        }
        self.fullscreen.clone_from(&self.registry.fullscreen);
        Ok(scene)
    }
    pub fn refresh(&mut self) {
        self.ready = false;
        self.registry.changes.all();
    }
    pub fn ready(&self) -> bool {
        self.ready
    }
    fn defer(&mut self, now: Instant) {
        self.ready = false;
        self.retry_at = Some(now + Duration::from_millis(100));
        self.registry.changes.order();
        if !self.initialized {
            self.registry.changes.all();
        }
    }
    #[cfg(test)]
    pub(super) fn defer_for_test(&mut self, now: Instant) {
        self.defer(now);
    }
}
impl Drop for Tracker {
    fn drop(&mut self) {
        for hook in self.hooks.drain(..) {
            unsafe {
                let _ = UnhookWinEvent(hook);
            }
        }
    }
}

#[cfg(test)]
mod tests;
