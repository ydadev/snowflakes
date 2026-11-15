//! Bounded test-thread diagnostics. Never read captions or destroy unknown windows.
use std::{cell::RefCell, collections::BTreeSet};
use windows::{
    Win32::{
        Foundation::*,
        System::{StationsAndDesktops::*, Threading::*},
        UI::{Accessibility::HWINEVENTHOOK, WindowsAndMessaging::*},
    },
    core::BOOL,
};

#[derive(Clone, Default, serde::Serialize)]
pub(in crate::platform::scene) struct Hooks {
    pub installed: usize,
    pub removed: usize,
    pub failed: usize,
    pub wrong_owner: usize,
    pub remaining_tracked: usize,
}
#[derive(Clone, Default, serde::Serialize)]
pub(in crate::platform::scene) struct Windows {
    pub total: usize,
    visible: usize,
    pub static_class: usize,
    ime: usize,
    text_services: usize,
    ole: usize,
    other: usize,
    enumeration_error: u32,
    message_only_error: u32,
    truncated: bool,
}
#[derive(Clone, Default, serde::Serialize)]
pub(in crate::platform::scene) struct Snapshot {
    pub phase: String,
    pub private_desktop: bool,
    pub windows: Windows,
    pub hooks: Hooks,
}
#[derive(Clone, Default, serde::Serialize)]
pub(in crate::platform::scene) struct Report {
    pub snapshots: Vec<Snapshot>,
    pub restore_hresult: Option<i32>,
    pub close_hresult: Option<i32>,
    pub hooks: Hooks,
    unwinding: bool,
}
#[derive(Default)]
struct State {
    detailed: bool,
    handles: BTreeSet<isize>,
    hooks: Hooks,
    snapshots: Vec<Snapshot>,
    report: Option<Report>,
}
thread_local! { static STATE: RefCell<State> = RefCell::new(State::default()); }
pub(in crate::platform::scene) fn detailed(enabled: bool) {
    STATE.with_borrow_mut(|state| state.detailed = enabled);
}
pub(in crate::platform::scene) fn begin() {
    STATE.with_borrow_mut(|state| {
        if state.handles.is_empty() {
            state.hooks = Hooks::default();
        }
        state.snapshots.clear();
        state.report = None;
    });
}
pub(in crate::platform::scene) fn installed(hook: HWINEVENTHOOK) {
    STATE.with_borrow_mut(|state| {
        state.handles.insert(hook.0 as isize);
        state.hooks.installed += 1;
    });
}
pub(in crate::platform::scene) fn unhooked(hook: HWINEVENTHOOK, owner: u32, removed: bool) {
    STATE.with_borrow_mut(|state| {
        let same = owner == unsafe { GetCurrentThreadId() };
        state.hooks.wrong_owner += usize::from(!same);
        if removed {
            state.hooks.removed += 1;
            if same {
                state.handles.remove(&(hook.0 as isize));
            }
        } else {
            state.hooks.failed += 1;
        }
    });
}
struct Inventory {
    owner: u32,
    process: u32,
    seen: BTreeSet<isize>,
    result: Windows,
}
impl Inventory {
    fn read(&mut self, hwnd: HWND) {
        let mut pid = 0;
        let thread = unsafe { GetWindowThreadProcessId(hwnd, Some(&mut pid)) };
        if thread != self.owner || pid != self.process || self.seen.contains(&(hwnd.0 as isize)) {
            return;
        }
        if self.seen.len() >= 128 {
            self.result.truncated = true;
            return;
        }
        self.seen.insert(hwnd.0 as isize);
        self.result.total += 1;
        self.result.visible += usize::from(unsafe { IsWindowVisible(hwnd) }.as_bool());
        let mut class = [0u16; 128];
        let length = unsafe { GetClassNameW(hwnd, &mut class) }.max(0) as usize;
        let name = String::from_utf16_lossy(&class[..length]);
        match name.as_str() {
            "Static" | "STATIC" => self.result.static_class += 1,
            "IME" => self.result.ime += 1,
            "MSCTFIME UI" => self.result.text_services += 1,
            "OleMainThreadWndClass" => self.result.ole += 1,
            _ => self.result.other += 1,
        }
    }
}
unsafe extern "system" fn window(hwnd: HWND, data: LPARAM) -> BOOL {
    // Synchronous enumeration; only a bounded owned-thread class category escapes.
    let inventory = unsafe { &mut *(data.0 as *mut Inventory) };
    if inventory.seen.len() >= 128 {
        inventory.result.truncated = true;
        return BOOL(0);
    }
    inventory.read(hwnd);
    BOOL(1)
}
fn private_desktop() -> bool {
    unsafe {
        let Ok(desktop) = GetThreadDesktop(GetCurrentThreadId()) else {
            return false;
        };
        let mut name = [0u16; 256];
        let mut input = BOOL(1);
        if GetUserObjectInformationW(
            HANDLE(desktop.0),
            UOI_NAME,
            Some(name.as_mut_ptr().cast()),
            std::mem::size_of_val(&name) as u32,
            None,
        )
        .is_err()
            || GetUserObjectInformationW(
                HANDLE(desktop.0),
                UOI_IO,
                Some((&mut input as *mut BOOL).cast()),
                std::mem::size_of::<BOOL>() as u32,
                None,
            )
            .is_err()
        {
            return false;
        }
        let end = name.iter().position(|&c| c == 0).unwrap_or(name.len());
        !input.as_bool() && String::from_utf16_lossy(&name[..end]).starts_with("Snowflakes")
    }
}
pub(in crate::platform::scene) fn phase(label: &str) {
    if !STATE.with_borrow(|state| state.detailed) {
        return;
    }
    failure_phase(label);
}
pub(in crate::platform::scene) fn failure_phase(label: &str) {
    let private = private_desktop();
    let mut inventory = Inventory {
        owner: unsafe { GetCurrentThreadId() },
        process: unsafe { GetCurrentProcessId() },
        seen: BTreeSet::new(),
        result: Windows::default(),
    };
    if private {
        unsafe {
            SetLastError(ERROR_SUCCESS);
            let enumerated = EnumThreadWindows(
                inventory.owner,
                Some(window),
                LPARAM((&mut inventory as *mut Inventory) as isize),
            );
            if !enumerated.as_bool() && !inventory.result.truncated {
                inventory.result.enumeration_error = GetLastError().0;
            }
            // Message-only HWNDs are inspected separately and filtered by PID/TID.
            let mut after = None;
            for i in 0..512 {
                SetLastError(ERROR_SUCCESS);
                let Ok(hwnd) = FindWindowExW(Some(HWND_MESSAGE), after, None, None) else {
                    inventory.result.message_only_error = GetLastError().0;
                    break;
                };
                inventory.read(hwnd);
                if inventory.result.truncated {
                    break;
                }
                after = Some(hwnd);
                if i == 511 {
                    inventory.result.truncated = true;
                }
            }
        }
    }
    STATE.with_borrow_mut(|state| {
        let mut hooks = state.hooks.clone();
        hooks.remaining_tracked = state.handles.len();
        if state.snapshots.len() < 16 {
            state.snapshots.push(Snapshot {
                phase: label.into(),
                private_desktop: private,
                windows: inventory.result,
                hooks,
            });
        }
    });
}
pub(in crate::platform::scene) fn finish(
    restore: Option<i32>,
    close: Option<i32>,
    unwinding: bool,
) -> Report {
    STATE.with_borrow_mut(|state| {
        let report = Report {
            snapshots: std::mem::take(&mut state.snapshots),
            restore_hresult: restore,
            close_hresult: close,
            hooks: {
                let mut hooks = state.hooks.clone();
                hooks.remaining_tracked = state.handles.len();
                hooks
            },
            unwinding,
        };
        state.report = Some(report.clone());
        report
    })
}
pub(in crate::platform::scene) fn last() -> Option<Report> {
    STATE.with_borrow(|state| state.report.clone())
}
