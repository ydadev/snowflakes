//! Explicit matched repetitions; ordinary successful tests do not enumerate windows.
use super::production_native_tracker_reads_dwm_geometry_and_updates_move_close_without_fixture_source;
use crate::platform::scene::tests::cleanup;

#[test]
fn native_cleanup_observes_controlled_owned_window_denial_and_then_restores_strictly() {
    use super::{DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, DpiScope, NativeWindow, TestDesktop};
    use windows::Win32::{
        Foundation::ERROR_BUSY,
        System::{
            StationsAndDesktops::{HDESK, SetThreadDesktop},
            Threading::GetCurrentThreadId,
        },
    };
    let name = format!(
        "SnowflakesNative-{}-{}-cleanup",
        std::process::id(),
        unsafe { GetCurrentThreadId() }
    );
    cleanup::detailed(false);
    let desktop = TestDesktop::attach(&name, true);
    let owned = desktop.owned.0 as usize;
    let original = desktop.original.0 as usize;
    // The parent owns both desktop lifetimes. scope joins this dedicated UI
    // thread even on panic, before the parent restores/closes its desktop.
    let failure = std::thread::scope(|scope| {
        scope
            .spawn(move || {
                let _dpi = DpiScope::set(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
                unsafe {
                    SetThreadDesktop(HDESK(owned as *mut _)).unwrap();
                }
                cleanup::begin();
                let own = NativeWindow::new(&name, 550);
                // This intentional denial has a known live resource. It is not a retry
                // of the intermittent failure, and no unknown window is destroyed.
                let error = unsafe { SetThreadDesktop(HDESK(original as *mut _)) }
                    .expect_err("Windows allowed a thread with its live window to change desktop");
                assert_eq!(
                    error.code(),
                    windows::core::HRESULT::from_win32(ERROR_BUSY.0)
                );
                cleanup::phase("ordinary_phase_disabled");
                cleanup::failure_phase("controlled_owned_window_failure");
                let failure = cleanup::finish(Some(error.code().0), None, false);
                assert_eq!(failure.restore_hresult, Some(error.code().0));
                drop(own); // Explicitly destroy our HWND on its creator thread.
                // Thread exit bounds any lazily created Windows runtime windows. No
                // attempt is made to destroy them or force desktop reassignment.
                failure
            })
            .join()
            .unwrap()
    });
    assert_eq!(failure.snapshots.len(), 1);
    let snapshot = &failure.snapshots[0];
    assert_eq!(snapshot.phase, "controlled_owned_window_failure");
    assert!(snapshot.private_desktop && snapshot.windows.static_class > 0);
    drop(desktop);
    let completed = cleanup::last().expect("final cleanup report missing");
    assert!(completed.restore_hresult.is_none() && completed.close_hresult.is_none());
    assert!(completed.snapshots.is_empty());
}

#[derive(serde::Serialize)]
struct Case {
    passed: bool,
    report: Option<cleanup::Report>,
}
#[derive(serde::Serialize)]
struct Mode {
    detailed: bool,
    elapsed_ms: u128,
    cases: Vec<Case>,
}

fn run(detailed: bool) -> Mode {
    let started = std::time::Instant::now();
    let workers: Vec<_> = (0..4)
        .map(|_| {
            std::thread::spawn(move || {
                cleanup::detailed(detailed);
                let mut cases = Vec::new();
                for _ in 0..4 {
                    let result = std::panic::catch_unwind(
                        production_native_tracker_reads_dwm_geometry_and_updates_move_close_without_fixture_source,
                    );
                    cases.push(Case {
                        passed: result.is_ok(),
                        report: cleanup::last(),
                    });
                    // A failed restore is not retried and its thread is not reused.
                    if result.is_err() {
                        break;
                    }
                }
                cleanup::detailed(false);
                cases
            })
        })
        .collect();
    let cases = workers
        .into_iter()
        .flat_map(|worker| worker.join().unwrap())
        .collect();
    Mode {
        detailed,
        elapsed_ms: started.elapsed().as_millis(),
        cases,
    }
}

#[test]
#[ignore = "explicit bounded parallel investigation of private desktop restoration"]
fn native_cleanup_parallel_reproduction_records_owned_windows_and_hook_release() {
    let modes = [run(false), run(true)];
    println!(
        "native_cleanup_audit {}",
        serde_json::to_string(&modes).unwrap()
    );
    for mode in modes {
        assert_eq!(mode.cases.len(), 16);
        for case in mode.cases {
            assert!(case.passed);
            let report = case.report.expect("desktop cleanup report missing");
            assert!(report.restore_hresult.is_none() && report.close_hresult.is_none());
            assert!(report.hooks.installed > 0);
            assert_eq!(report.hooks.installed, report.hooks.removed);
            assert_eq!(report.hooks.failed, 0);
            assert_eq!(report.hooks.wrong_owner, 0);
            assert_eq!(report.hooks.remaining_tracked, 0);
            if mode.detailed {
                assert_eq!(report.snapshots.len(), 4);
                assert!(
                    report
                        .snapshots
                        .iter()
                        .all(|snapshot| snapshot.private_desktop)
                );
                let alive = report
                    .snapshots
                    .iter()
                    .find(|s| s.phase == "owned_window_alive")
                    .unwrap();
                assert!(alive.windows.static_class > 0);
                let destroyed = report
                    .snapshots
                    .iter()
                    .find(|s| s.phase == "owned_window_destroyed")
                    .unwrap();
                assert_eq!(destroyed.windows.static_class, 0);
                let restored = report
                    .snapshots
                    .iter()
                    .find(|s| s.phase == "before_restore")
                    .unwrap();
                assert_eq!(restored.hooks.remaining_tracked, 0);
            } else {
                assert!(report.snapshots.is_empty());
            }
        }
    }
}
