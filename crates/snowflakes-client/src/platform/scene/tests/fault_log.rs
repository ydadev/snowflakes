use super::*;
use crate::error_log::Recorder;
use std::sync::Arc;

fn tracker(started: Instant, log: Arc<Recorder>) -> Tracker {
    let mut tracker = Tracker {
        hooks: vec![],
        hook_owner: 0, // Managed Source owns no native hooks or windows.
        registry: Registry::default(),
        started,
        ready: false,
        initialized: false,
        retry_at: None,
        errors: None,
        scene_error: None,
        fullscreen: BTreeSet::new(),
    };
    tracker.set_error_log(Some(log));
    tracker
}
fn desktop() -> Desktop {
    Desktop {
        calls: 0,
        reads: 0,
        fail: false,
        fail_screens: false,
        shift: 0.0,
        floor: 600.0,
    }
}
fn poll(tracker: &mut Tracker, source: &mut Desktop, millis: u64) -> Option<Scene> {
    tracker
        .poll_source(tracker.started + Duration::from_millis(millis), source)
        .unwrap()
}
fn events(directory: &std::path::Path) -> Vec<serde_json::Value> {
    std::fs::read_to_string(directory.join("errors.jsonl"))
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

#[test]
fn scene_fault_journal_keeps_unobserved_episodes_and_deduplicates_without_shortening_retry() {
    let dir = tempfile::tempdir().unwrap();
    let log = Recorder::start(dir.path().into());
    let mut tracker = tracker(Instant::now(), log.clone());
    let mut source = desktop();
    source.fail = true;
    assert!(poll(&mut tracker, &mut source, 0).is_none());
    assert!(!tracker.ready());
    assert_eq!(
        log.snapshot().enqueued,
        1,
        "scene failure was not journaled"
    );
    assert!(poll(&mut tracker, &mut source, 99).is_none());
    assert_eq!(source.calls, 1);
    assert!(poll(&mut tracker, &mut source, 100).is_none());
    assert_eq!(log.snapshot().enqueued, 1);
    tracker.refresh(); // A pending refresh must not begin a new failure episode.
    assert!(poll(&mut tracker, &mut source, 199).is_none());
    assert_eq!(source.calls, 2);
    assert!(poll(&mut tracker, &mut source, 200).is_none());
    assert_eq!(log.snapshot().enqueued, 1);
    source.fail = false;
    source.fail_screens = true;
    assert!(poll(&mut tracker, &mut source, 300).is_none());
    assert_eq!(log.snapshot().enqueued, 2);
    assert!(poll(&mut tracker, &mut source, 400).is_none());
    assert_eq!(log.snapshot().enqueued, 2);
    source.fail_screens = false;
    assert!(poll(&mut tracker, &mut source, 500).is_some());
    assert!(tracker.ready());
    // A consumer may see only this final successful scene. Earlier faults must
    // still exist independently of Report/status sampling and coalescing.
    assert!(poll(&mut tracker, &mut source, 501).is_none());
    assert_eq!(log.snapshot().enqueued, 2);
    source.fail = true;
    tracker.registry.changes.order();
    assert!(poll(&mut tracker, &mut source, 530).is_none());
    assert!(!tracker.ready());
    assert_eq!(log.snapshot().enqueued, 3);
    drop(tracker);
    drop(log);
    let events = events(dir.path());
    assert_eq!(events.len(), 3);
    let expected = [ERROR_RETRY, ERROR_GEN_FAILURE, ERROR_RETRY];
    for (event, code) in events.iter().zip(expected) {
        assert_eq!(event["fault"], "scene_query");
        assert_eq!(
            event["code"],
            serde_json::json!({"kind":"hresult","value":windows::core::HRESULT::from_win32(code.0).0})
        );
    }
}

#[test]
fn scene_fault_journal_preserves_periodic_refresh_geometry_and_generation() {
    for order_failure in [true, false] {
        let dir = tempfile::tempdir().unwrap();
        let log = Recorder::start(dir.path().into());
        let mut tracker = tracker(Instant::now(), log.clone());
        let mut source = desktop();
        let initial = poll(&mut tracker, &mut source, 0).unwrap();
        assert_eq!(log.snapshot().enqueued, 0);
        source.shift = 25.0;
        source.floor = 580.0;
        source.fail = order_failure;
        source.fail_screens = !order_failure;
        for millis in [1000, 1100] {
            assert!(poll(&mut tracker, &mut source, millis).is_none());
            assert!(!tracker.ready());
            assert_eq!(source.reads, 1);
            assert_eq!(log.snapshot().enqueued, 1);
            let calls = source.calls;
            assert!(poll(&mut tracker, &mut source, millis + 99).is_none());
            assert_eq!(source.calls, calls);
            assert_eq!(log.snapshot().enqueued, 1);
        }
        source.fail = false;
        source.fail_screens = false;
        let scene = poll(&mut tracker, &mut source, 1200).unwrap();
        assert!(tracker.ready());
        assert_eq!(scene.windows[0].key, initial.windows[0].key);
        assert_eq!(scene.windows[0].rect.left, 25.0);
        assert_eq!(scene.screens[0].floor, 580.0);
        assert_eq!(source.reads, 2);
        let calls = source.calls;
        assert!(poll(&mut tracker, &mut source, 2199).is_none());
        assert_eq!(source.calls, calls);
        assert!(poll(&mut tracker, &mut source, 2200).is_some());
        assert_eq!(source.reads, 3);
        drop(tracker);
        drop(log);
        let events = events(dir.path());
        assert_eq!(events.len(), 1);
        assert_eq!(events[0]["fault"], "scene_query");
    }
}

struct PrivateError;
impl Source for PrivateError {
    type Error = windows::core::Error;
    fn order(&mut self) -> Result<Vec<usize>> {
        Err(windows::core::Error::new(
            windows::core::HRESULT(0x80004005_u32 as i32),
            "TEST-PRIVATE scene path and window text",
        ))
    }
    fn read(&mut self, _: usize) -> Option<WindowInfo> {
        unreachable!()
    }
    fn screens(&mut self) -> Result<Vec<Screen>> {
        unreachable!()
    }
    fn foreground(&mut self) -> usize {
        unreachable!()
    }
}

#[test]
fn scene_fault_journal_serializes_only_the_code_and_clean_queries_create_no_history() {
    for failed in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let directory = dir.path().join("logs");
        let log = Recorder::start(directory.clone());
        let mut tracker = tracker(Instant::now(), log.clone());
        if failed {
            assert!(
                tracker
                    .poll_source(tracker.started, &mut PrivateError)
                    .unwrap()
                    .is_none()
            );
            assert_eq!(log.snapshot().enqueued, 1);
        } else {
            let mut source = desktop();
            assert!(poll(&mut tracker, &mut source, 0).is_some());
            assert!(poll(&mut tracker, &mut source, 1000).is_some());
            assert_eq!(log.snapshot().enqueued, 0);
        }
        drop(tracker);
        drop(log);
        if failed {
            let text = std::fs::read_to_string(directory.join("errors.jsonl")).unwrap();
            assert!(
                !text.contains("TEST-PRIVATE")
                    && !text.contains("window text")
                    && !text.contains(dir.path().to_str().unwrap())
            );
            let events = events(&directory);
            assert_eq!(events.len(), 1);
            assert_eq!(
                events[0]["code"],
                serde_json::json!({"kind":"hresult","value":0x80004005_u32 as i32})
            );
        } else {
            assert!(!directory.exists());
        }
    }
}
