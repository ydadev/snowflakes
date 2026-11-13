//! Out-of-context notifications only mark the scene dirty. Snapshots never read
//! captions or executable names. Native queries and callbacks stay on this thread.
use crate::geometry::{Rect, Scene, Screen, Window};
use std::{
    cell::{Cell, RefCell},
    collections::{BTreeMap, BTreeSet},
    time::{Duration, Instant},
};
use windows::{
    Win32::{
        Foundation::*,
        Graphics::{Dwm::*, Gdi::*},
        UI::{Accessibility::*, WindowsAndMessaging::*},
    },
    core::{BOOL, Result},
};

thread_local! {
    static DIRTY: Cell<bool> = const { Cell::new(true) };
    static DESTROYED: RefCell<Vec<usize>> = const { RefCell::new(Vec::new()) };
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
    if object != 0 || child != 0 || hwnd.is_invalid() {
        return;
    }
    DIRTY.set(true);
    if event == EVENT_OBJECT_DESTROY {
        DESTROYED.with_borrow_mut(|ids| {
            if ids.len() < 2048 {
                ids.push(hwnd.0 as usize);
            }
        });
    }
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
struct Raw {
    hwnd: usize,
    pid: u32,
    thread: u32,
    rect: Rect,
    visible: bool,
    collect: bool,
    taskbar: bool,
}
unsafe extern "system" fn window(hwnd: HWND, data: LPARAM) -> BOOL {
    unsafe {
        let output = &mut *(data.0 as *mut Vec<Raw>);
        if output.len() >= 512 {
            return BOOL(0);
        }
        let mut pid = 0;
        let thread = GetWindowThreadProcessId(hwnd, Some(&mut pid));
        if pid == std::process::id()
            || thread == 0
            || !IsWindowVisible(hwnd).as_bool()
            || IsIconic(hwnd).as_bool()
        {
            return BOOL(1);
        }
        let mut cloaked = 0_u32;
        if DwmGetWindowAttribute(hwnd, DWMWA_CLOAKED, (&mut cloaked as *mut u32).cast(), 4).is_err()
        {
            return BOOL(1);
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
            return BOOL(1);
        }
        if bounds.right <= bounds.left || bounds.bottom <= bounds.top {
            return BOOL(1);
        }
        let style = GetWindowLongW(hwnd, GWL_STYLE) as u32;
        let extended = GetWindowLongW(hwnd, GWL_EXSTYLE) as u32;
        let mut name = [0_u16; 128];
        let length = GetClassNameW(hwnd, &mut name).max(0) as usize;
        let class = String::from_utf16_lossy(&name[..length]);
        if matches!(class.as_str(), "Progman" | "WorkerW") {
            return BOOL(1);
        }
        let taskbar = matches!(class.as_str(), "Shell_TrayWnd" | "Shell_SecondaryTrayWnd");
        let collect = style & WS_CAPTION.0 != 0 && extended & WS_EX_TOOLWINDOW.0 == 0 && !taskbar;
        output.push(Raw {
            hwnd: hwnd.0 as usize,
            pid,
            thread,
            rect: rect(bounds),
            visible: cloaked == 0,
            collect,
            taskbar,
        });
    }
    BOOL(1)
}
pub struct Tracker {
    hooks: Vec<HWINEVENTHOOK>,
    keys: BTreeMap<usize, (u32, u32, u64)>,
    serial: u64,
    last: Instant,
    pub fullscreen: BTreeSet<u64>,
}
impl Tracker {
    pub fn new() -> Result<Self> {
        let mut tracker = Self {
            hooks: Vec::new(),
            keys: BTreeMap::new(),
            serial: 1,
            last: Instant::now() - Duration::from_secs(2),
            fullscreen: BTreeSet::new(),
        };
        for (first, last) in [
            (EVENT_OBJECT_CREATE, EVENT_OBJECT_LOCATIONCHANGE),
            (EVENT_SYSTEM_FOREGROUND, EVENT_SYSTEM_MINIMIZEEND),
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
        let elapsed = now.duration_since(self.last);
        if elapsed < Duration::from_secs(1) && (!DIRTY.get() || elapsed < Duration::from_millis(30))
        {
            return Ok(None);
        }
        DIRTY.set(false);
        self.last = now;
        DESTROYED.with_borrow_mut(|ids| {
            for id in ids.drain(..) {
                self.keys.remove(&id);
            }
        });
        let mut scene = Scene::default();
        let mut raw = Vec::<Raw>::new();
        unsafe {
            EnumDisplayMonitors(
                None,
                None,
                Some(monitor),
                LPARAM(&mut scene.screens as *mut _ as isize),
            )
            .ok()?;
            // FALSE from the callback is a deliberate enumeration budget, not an API error.
            let result = EnumWindows(Some(window), LPARAM(&mut raw as *mut _ as isize));
            if result.is_err() && raw.len() < 512 {
                return Err(windows::core::Error::from_thread());
            }
        }
        let foreground = unsafe { GetForegroundWindow() }.0 as usize;
        self.fullscreen.clear();
        for item in &raw {
            if item.hwnd == foreground && !item.collect && item.visible {
                for screen in &scene.screens {
                    if item.rect.left <= screen.rect.left + 1.0
                        && item.rect.top <= screen.rect.top + 1.0
                        && item.rect.right >= screen.rect.right - 1.0
                        && item.rect.bottom >= screen.rect.bottom - 1.0
                    {
                        self.fullscreen.insert(screen.key);
                    }
                }
            }
        }
        let seen: BTreeSet<_> = raw.iter().map(|r| r.hwnd).collect();
        self.keys.retain(|handle, _| seen.contains(handle));
        for item in raw {
            if item.taskbar {
                if item.visible && item.rect.width() > item.rect.height() {
                    for screen in &mut scene.screens {
                        if item.rect.right > screen.rect.left
                            && item.rect.left < screen.rect.right
                            && item.rect.top > screen.rect.top + screen.rect.height() * 0.5
                            && item.rect.bottom >= screen.rect.bottom - 4.0
                            && (screen.rect.bottom - item.rect.top) > 8.0
                        {
                            screen.floor = screen.floor.min(item.rect.top);
                        }
                    }
                }
                continue;
            }
            let entry = self.keys.entry(item.hwnd).or_insert_with(|| {
                let key = self.serial;
                self.serial += 1;
                (item.pid, item.thread, key)
            });
            if entry.0 != item.pid || entry.1 != item.thread {
                let key = self.serial;
                self.serial += 1;
                *entry = (item.pid, item.thread, key);
            }
            scene.windows.push(Window {
                key: entry.2,
                rect: item.rect,
                visible: item.visible,
                collect: item.collect,
            });
        }
        // Screen scale is replaced with GetDpiForWindow on each layer before use.
        Ok(Some(scene))
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
