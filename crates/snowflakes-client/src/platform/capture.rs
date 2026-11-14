//! Display affinity belongs to this process's top-level overlay windows.
use crate::capture::Failure;
use windows::Win32::{Foundation::HWND, UI::WindowsAndMessaging::*};

pub(super) fn apply(hwnd: HWND, excluded: bool) -> Result<(), Failure> {
    let expected = if excluded {
        WDA_EXCLUDEFROMCAPTURE
    } else {
        WDA_NONE
    };
    // Windows validates ownership and window kind. The readback distinguishes
    // successful exclusion from an older/unsupported affinity interpretation.
    unsafe { SetWindowDisplayAffinity(hwnd, expected) }
        .map_err(|error| Failure::Api(error.code().0))?;
    let mut observed = 0;
    unsafe { GetWindowDisplayAffinity(hwnd, &mut observed) }
        .map_err(|error| Failure::Api(error.code().0))?;
    if observed != expected.0 {
        return Err(Failure::DifferentAffinity(observed));
    }
    Ok(())
}
