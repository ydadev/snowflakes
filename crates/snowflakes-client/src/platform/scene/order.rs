//! Win32 inventory and explicit Z links. No geometry, captions or process handles are queried.
use crate::{
    scene_registry::MAX_WINDOWS,
    window_order::{self, Links, Mark, Owner},
};
use windows::{
    Win32::{Foundation::*, UI::WindowsAndMessaging::*},
    core::{BOOL, Result},
};

#[derive(Default)]
struct Inventory {
    handles: Vec<usize>,
    capped: bool,
}
unsafe extern "system" fn enumerated(hwnd: HWND, data: LPARAM) -> BOOL {
    // EnumWindows is synchronous; the stack inventory outlives every callback.
    let inventory = unsafe { &mut *(data.0 as *mut Inventory) };
    if inventory.handles.len() == MAX_WINDOWS {
        inventory.capped = true;
        unsafe {
            SetLastError(ERROR_SUCCESS);
        }
        return BOOL(0);
    }
    inventory.handles.push(hwnd.0 as usize);
    BOOL(1)
}
fn owner(handle: usize) -> Option<Owner> {
    let mut process = 0;
    let thread = unsafe { GetWindowThreadProcessId(HWND(handle as *mut _), Some(&mut process)) };
    (thread != 0 && process != 0).then_some(Owner { process, thread })
}
fn link(call: impl FnOnce() -> Result<HWND>) -> Result<Option<usize>> {
    unsafe {
        SetLastError(ERROR_SUCCESS);
    }
    match call() {
        Ok(handle) => Ok(Some(handle.0 as usize)),
        // The bindings represent NULL as Err, even when it means no neighbour.
        Err(_) if unsafe { GetLastError() } == ERROR_SUCCESS => Ok(None),
        Err(error) => Err(error),
    }
}
struct NativeLinks;
impl Links for NativeLinks {
    type Error = windows::core::Error;
    fn top(&mut self) -> Result<Option<usize>> {
        link(|| unsafe { GetTopWindow(None) })
    }
    fn next(&mut self, handle: usize) -> Result<Option<usize>> {
        link(|| unsafe { GetWindow(HWND(handle as *mut _), GW_HWNDNEXT) })
    }
    fn owner(&mut self, handle: usize) -> Option<Owner> {
        owner(handle)
    }
}
fn once() -> Result<Vec<Mark>> {
    let mut inventory = Inventory::default();
    let result =
        unsafe { EnumWindows(Some(enumerated), LPARAM(&mut inventory as *mut _ as isize)) };
    if let Err(error) = result
        && !inventory.capped
    {
        return Err(error);
    }
    let marks: Vec<_> = inventory
        .handles
        .into_iter()
        .filter_map(|handle| owner(handle).map(|owner| Mark { handle, owner }))
        .collect();
    window_order::collect(&marks, &mut NativeLinks)
        .map_err(|_| super::super::native::failure(ERROR_RETRY))
}
pub(super) fn capture() -> Result<Vec<Mark>> {
    // One fresh retry, never an unbounded desktop walk or an enumeration fallback.
    for _ in 0..2 {
        if let Ok(marks) = once() {
            return Ok(marks);
        }
    }
    Err(super::super::native::failure(ERROR_RETRY))
}

#[cfg(test)]
mod tests;
