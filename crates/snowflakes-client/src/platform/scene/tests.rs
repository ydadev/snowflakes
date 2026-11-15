use super::*;

mod events;
pub(in crate::platform) use events::Host;

struct Desktop {
    calls: usize,
    reads: usize,
    fail: bool,
    fail_screens: bool,
    shift: f32,
    floor: f32,
}
impl Source for Desktop {
    type Error = windows::core::Error;
    fn order(&mut self) -> Result<Vec<usize>> {
        self.calls += 1;
        if self.fail {
            Err(super::super::native::failure(ERROR_RETRY))
        } else {
            Ok(vec![1])
        }
    }
    fn read(&mut self, handle: usize) -> Option<WindowInfo> {
        self.reads += 1;
        Some(WindowInfo {
            handle,
            process: 100,
            thread: 1,
            rect: crate::geometry::Rect {
                left: self.shift,
                top: 0.0,
                right: 800.0 + self.shift,
                bottom: 600.0,
            },
            visible: true,
            collect: true,
            maximized: false,
            taskbar: false,
        })
    }
    fn screens(&mut self) -> Result<Vec<Screen>> {
        if self.fail_screens {
            return Err(super::super::native::failure(ERROR_GEN_FAILURE));
        }
        Ok(vec![Screen {
            key: 1,
            persistent_id: true,
            rect: crate::geometry::Rect {
                left: 0.0,
                top: 0.0,
                right: 800.0,
                bottom: 600.0,
            },
            floor: self.floor,
            scale: 1.0,
        }])
    }
    fn foreground(&mut self) -> usize {
        1
    }
}

#[test]
fn unstable_order_is_nonfatal_throttled_and_recovers_without_rereading_stable_geometry() {
    let start = Instant::now();
    let mut tracker = Tracker {
        hooks: vec![],
        registry: Registry::default(),
        started: start,
        ready: false,
        initialized: false,
        retry_at: None,
        fullscreen: BTreeSet::new(),
    };
    let mut desktop = Desktop {
        calls: 0,
        reads: 0,
        fail: true,
        fail_screens: false,
        shift: 0.0,
        floor: 600.0,
    };
    assert!(tracker.poll_source(start, &mut desktop).unwrap().is_none());
    assert!(!tracker.ready());
    assert_eq!(desktop.calls, 1);
    desktop.fail = false;
    assert!(
        tracker
            .poll_source(start + Duration::from_millis(99), &mut desktop)
            .unwrap()
            .is_none()
    );
    assert_eq!(desktop.calls, 1);
    let scene = tracker
        .poll_source(start + Duration::from_millis(100), &mut desktop)
        .unwrap()
        .unwrap();
    assert!(tracker.ready());
    assert_eq!(scene.windows.len(), 1);
    assert_eq!(scene.screens.len(), 1);
    assert_eq!(desktop.reads, 1);
    let key = scene.windows[0].key;
    desktop.fail = true;
    tracker.registry.changes.order();
    assert!(
        tracker
            .poll_source(start + Duration::from_millis(130), &mut desktop)
            .unwrap()
            .is_none()
    );
    assert!(!tracker.ready());
    desktop.fail = false;
    let scene = tracker
        .poll_source(start + Duration::from_millis(230), &mut desktop)
        .unwrap()
        .unwrap();
    assert!(tracker.ready());
    assert_eq!(scene.windows[0].key, key);
    assert_eq!(desktop.reads, 1);
    desktop.fail_screens = true;
    tracker.refresh();
    assert!(
        tracker
            .poll_source(start + Duration::from_millis(260), &mut desktop)
            .unwrap()
            .is_none()
    );
    assert!(!tracker.ready());
    let calls = desktop.calls;
    desktop.fail_screens = false;
    assert!(
        tracker
            .poll_source(start + Duration::from_millis(359), &mut desktop)
            .unwrap()
            .is_none()
    );
    assert_eq!(desktop.calls, calls);
    assert!(
        tracker
            .poll_source(start + Duration::from_millis(360), &mut desktop)
            .unwrap()
            .is_some()
    );
    assert!(tracker.ready());
}

#[test]
fn periodic_failure_retries_all_geometry_before_scene_becomes_ready() {
    for order_failure in [true, false] {
        let start = Instant::now();
        let mut tracker = Tracker {
            hooks: vec![],
            registry: Registry::default(),
            started: start,
            ready: false,
            initialized: false,
            retry_at: None,
            fullscreen: BTreeSet::new(),
        };
        let mut desktop = Desktop {
            calls: 0,
            reads: 0,
            fail: false,
            fail_screens: false,
            shift: 0.0,
            floor: 600.0,
        };
        let initial = tracker.poll_source(start, &mut desktop).unwrap().unwrap();
        assert!(tracker.ready());
        desktop.shift = 25.0;
        desktop.floor = 580.0;
        desktop.fail = order_failure;
        desktop.fail_screens = !order_failure;
        // No events: these changes can only be recovered by full reconciliation.
        for millis in [1000, 1100] {
            assert!(
                tracker
                    .poll_source(start + Duration::from_millis(millis), &mut desktop)
                    .unwrap()
                    .is_none()
            );
            assert!(!tracker.ready());
            assert_eq!(desktop.reads, 1);
            let calls = desktop.calls;
            assert!(
                tracker
                    .poll_source(start + Duration::from_millis(millis + 99), &mut desktop)
                    .unwrap()
                    .is_none()
            );
            assert_eq!(
                desktop.calls, calls,
                "source retries must respect the backoff"
            );
        }
        desktop.fail = false;
        desktop.fail_screens = false;
        let recovered = tracker
            .poll_source(start + Duration::from_millis(1200), &mut desktop)
            .unwrap()
            .unwrap();
        assert!(tracker.ready());
        assert_eq!(recovered.windows[0].rect.left, 25.0);
        assert_eq!(recovered.screens[0].floor, 580.0);
        assert_eq!(recovered.windows[0].key, initial.windows[0].key);
        assert_eq!(desktop.reads, 2);
        let calls = desktop.calls;
        assert!(
            tracker
                .poll_source(start + Duration::from_millis(2199), &mut desktop)
                .unwrap()
                .is_none()
        );
        assert_eq!(desktop.calls, calls);
        assert!(
            tracker
                .poll_source(start + Duration::from_millis(2200), &mut desktop)
                .unwrap()
                .is_some()
        );
        assert_eq!(desktop.reads, 3);
    }
}
