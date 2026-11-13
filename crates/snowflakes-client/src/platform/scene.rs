//! Native geometry reads stay on the graphics thread. Callbacks only coalesce IDs.
use crate::{
    geometry::{Rect, Scene, Screen},
    scene_registry::{Changes, MAX_WINDOWS, Registry, Source, WindowInfo},
};
use std::{cell::RefCell, collections::BTreeSet, time::Instant};
use windows::{
    Win32::{
        Foundation::*,
        Graphics::{Dwm::*, Gdi::*},
        UI::{Accessibility::*, WindowsAndMessaging::*},
    },
    core::{BOOL, Result},
};

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
        let mut info = MONITORINFO {
            cbSize: size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        if GetMonitorInfoW(h, &mut info).as_bool() {
            (&mut *(data.0 as *mut Vec<Screen>)).push(Screen {
                key: h.0 as usize as u64,
                rect: rect(info.rcMonitor),
                floor: info.rcWork.bottom as f32,
                scale: 1.0,
            });
        }
    }
    BOOL(1)
}
unsafe extern "system" fn window(hwnd: HWND, data: LPARAM) -> BOOL {
    let output = unsafe { &mut *(data.0 as *mut Vec<usize>) };
    if output.len() >= MAX_WINDOWS {
        return BOOL(0);
    }
    output.push(hwnd.0 as usize);
    BOOL(1)
}
struct Native;
impl Source for Native {
    type Error = windows::core::Error;
    fn order(&mut self) -> Result<Vec<usize>> {
        let mut handles = Vec::<usize>::new();
        let result = unsafe { EnumWindows(Some(window), LPARAM(&mut handles as *mut _ as isize)) };
        if result.is_err() && handles.len() < MAX_WINDOWS {
            return Err(windows::core::Error::from_thread());
        }
        Ok(handles)
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
                taskbar,
            })
        }
    }
}
pub struct Tracker {
    hooks: Vec<HWINEVENTHOOK>,
    registry: Registry,
    started: Instant,
    pub fullscreen: BTreeSet<u64>,
}
impl Tracker {
    pub fn new() -> Result<Self> {
        CHANGES.with_borrow_mut(|changes| *changes = Changes::default());
        let mut tracker = Self {
            hooks: Vec::new(),
            registry: Registry::default(),
            started: Instant::now(),
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
        let scene = self
            .registry
            .poll(now.duration_since(self.started), &mut Native)?;
        self.fullscreen.clone_from(&self.registry.fullscreen);
        Ok(scene)
    }
    pub fn refresh(&mut self) {
        self.registry.changes.all();
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
