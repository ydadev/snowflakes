use super::*;
use crate::platform::weather;

fn fixture() -> (Tracker, Desktop) {
    CHANGES.with_borrow_mut(|changes| *changes = Changes::default());
    (
        Tracker {
            hooks: vec![],
            hook_owner: 0, // Managed Source: no HWND, hooks or native DPI change.
            registry: Registry::default(),
            started: Instant::now(),
            ready: false,
            initialized: false,
            retry_at: None,
            errors: None,
            scene_error: None,
            fullscreen: BTreeSet::new(),
        },
        Desktop {
            calls: 0,
            reads: 0,
            fail: false,
            fail_screens: false,
            shift: 0.0,
            floor: 600.0,
        },
    )
}
fn poll(tracker: &mut Tracker, source: &mut Desktop, millis: u64) -> Option<Scene> {
    tracker
        .poll_changes(tracker.started + Duration::from_millis(millis), source)
        .unwrap()
}
fn notify() {
    // Exercise the actual callback branch with managed arguments. This is not
    // native delivery, and neither this DPI nor this suggested rect is applied.
    let suggested = RECT {
        left: -9000,
        top: -8000,
        right: -7000,
        bottom: -6000,
    };
    assert_eq!(
        unsafe {
            weather::proc(
                HWND::default(),
                WM_DPICHANGED,
                WPARAM(192 | (192 << 16)),
                LPARAM(&suggested as *const RECT as isize),
            )
        },
        LRESULT(0)
    );
}

#[test]
fn layer_dpi_notifications_coalesce_full_geometry_before_periodic_refresh() {
    let (mut tracker, mut source) = fixture();
    let first = poll(&mut tracker, &mut source, 0).unwrap();
    source.shift = 25.0;
    source.floor = 550.0;
    for _ in 0..4096 {
        notify();
    }
    for millis in [10, 29] {
        assert!(poll(&mut tracker, &mut source, millis).is_none());
        assert_eq!((source.calls, source.reads), (1, 1));
    }
    let changed = poll(&mut tracker, &mut source, 30)
        .expect("layer DPI notification did not request full geometry");
    assert_eq!((source.calls, source.reads), (2, 2));
    assert_eq!(changed.windows[0].key, first.windows[0].key);
    assert_eq!(changed.windows[0].rect.left, 25.0);
    assert_eq!(changed.screens[0].floor, 550.0);
    assert_eq!(changed.screens[0].rect, first.screens[0].rect);
    assert_eq!(changed.screens[0].scale, 1.0); // Source, not synthetic WPARAM.
    assert!(tracker.ready());
    assert!(poll(&mut tracker, &mut source, 60).is_none());
    assert_eq!((source.calls, source.reads), (2, 2));
}

#[test]
fn layer_dpi_notifications_preserve_failed_geometry_and_existing_retry_deadline() {
    let (mut tracker, mut source) = fixture();
    let first = poll(&mut tracker, &mut source, 0).unwrap();
    source.fail_screens = true;
    notify();
    assert!(poll(&mut tracker, &mut source, 30).is_none());
    assert!(!tracker.ready(), "stale scene remained ready after failure");
    assert_eq!((source.calls, source.reads), (2, 1));
    source.fail_screens = false;
    source.shift = 40.0;
    source.floor = 500.0;
    for millis in [31, 60, 129] {
        notify();
        assert!(poll(&mut tracker, &mut source, millis).is_none());
        assert_eq!((source.calls, source.reads), (2, 1));
        assert!(!tracker.ready());
    }
    let recovered = poll(&mut tracker, &mut source, 130).unwrap();
    assert!(tracker.ready());
    assert_eq!((source.calls, source.reads), (3, 2));
    assert_eq!(recovered.windows[0].key, first.windows[0].key);
    assert_eq!(recovered.windows[0].rect.left, 40.0);
    assert_eq!(recovered.screens[0].floor, 500.0);
    assert!(poll(&mut tracker, &mut source, 160).is_none());
}

#[test]
fn layer_dpi_notifications_stay_on_the_render_thread_and_preserve_input_transparency() {
    let (mut tracker, mut source) = fixture();
    poll(&mut tracker, &mut source, 0).unwrap();
    std::thread::spawn(|| {
        for _ in 0..100 {
            notify();
        }
    })
    .join()
    .unwrap();
    for (message, expected) in [
        (WM_NCHITTEST, HTTRANSPARENT as isize),
        (WM_MOUSEACTIVATE, MA_NOACTIVATE as isize),
        (WM_ERASEBKGND, 1),
    ] {
        assert_eq!(
            unsafe { weather::proc(HWND::default(), message, WPARAM(0), LPARAM(0)) },
            LRESULT(expected)
        );
    }
    assert!(poll(&mut tracker, &mut source, 100).is_none());
    assert_eq!((source.calls, source.reads), (1, 1));
    notify();
    assert!(poll(&mut tracker, &mut source, 100).is_some());
    assert_eq!((source.calls, source.reads), (2, 2));
}
