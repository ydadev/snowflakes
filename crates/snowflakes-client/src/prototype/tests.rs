use super::*;
use windows::Win32::Foundation::{E_ACCESSDENIED, E_INVALIDARG};

#[test]
fn dpi_initialization_binary_thread_starts_in_manifest_pmv2_context() {
    // Read only: no HWND, SetProcess/SetThread call, input or desktop changes.
    assert!(unsafe {
        AreDpiAwarenessContextsEqual(
            GetThreadDpiAwarenessContext(),
            DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
        )
        .as_bool()
    });
}

#[test]
fn dpi_initialization_accepts_manifest_denial_only_with_confirmed_pmv2() {
    let denied = || windows::core::Error::from_hresult(E_ACCESSDENIED);
    assert!(dpi_initialization_result(Err(denied()), || true).is_ok());
    assert_eq!(
        dpi_initialization_result(Err(denied()), || false)
            .unwrap_err()
            .code(),
        E_ACCESSDENIED
    );
}
#[test]
fn dpi_initialization_preserves_success_and_other_errors_without_querying_context() {
    assert!(dpi_initialization_result(Ok(()), || panic!("unnecessary context query")).is_ok());
    assert_eq!(
        dpi_initialization_result(
            Err(windows::core::Error::from_hresult(E_INVALIDARG)),
            || panic!("unrelated error masked by context")
        )
        .unwrap_err()
        .code(),
        E_INVALIDARG
    );
}
