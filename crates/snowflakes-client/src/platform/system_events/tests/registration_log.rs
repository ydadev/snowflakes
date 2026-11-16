use super::*;

fn failure(code: WIN32_ERROR) -> windows::core::Error {
    windows::core::Error::new(code.to_hresult(), "synthetic-private-registration-detail")
}
fn history(directory: &std::path::Path, expected: &[(&str, WIN32_ERROR)]) {
    if expected.is_empty() {
        assert!(!directory.exists());
        return;
    }
    let text = std::fs::read_to_string(directory.join("errors.jsonl")).unwrap();
    assert!(!text.contains("synthetic-private-registration-detail"));
    assert!(!text.contains(directory.to_string_lossy().as_ref()));
    let records: Vec<serde_json::Value> = text
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(records.len(), expected.len());
    for (record, &(fault, code)) in records.iter().zip(expected) {
        assert_eq!(record.as_object().unwrap().len(), 5);
        assert_eq!(record["fault"], fault);
        assert_eq!(
            record["code"],
            serde_json::json!({"kind":"hresult","value":code.to_hresult().0})
        );
    }
}

#[test]
fn quality_notification_journal_retains_successful_handles_for_all_failure_combinations() {
    for failed in 0_u32..4 {
        let dir = tempfile::tempdir().unwrap();
        let directory = dir.path().join("history");
        let log = Recorder::start(directory.clone());
        // Opaque values exercise the common registration path without fake HWNDs
        // or notification handles being sent to Windows.
        let mut handles = vec![99_u32];
        let mut calls = Vec::new();
        register_quality_notifications(&mut handles, Some(&log), |guid| {
            let index = calls.len();
            calls.push(*guid);
            if failed & (1 << index) != 0 {
                Err(failure(if index == 0 {
                    ERROR_ACCESS_DENIED
                } else {
                    ERROR_NOT_SUPPORTED
                }))
            } else {
                Ok(index as u32 + 1)
            }
        });
        assert_eq!(calls, [GUID_ACDC_POWER_SOURCE, GUID_POWER_SAVING_STATUS]);
        let mut expected_handles = vec![99];
        let mut expected_events = Vec::new();
        if failed & 1 == 0 {
            expected_handles.push(1);
        } else {
            expected_events.push(("power_source_notification", ERROR_ACCESS_DENIED));
        }
        if failed & 2 == 0 {
            expected_handles.push(2);
        } else {
            expected_events.push(("power_saving_notification", ERROR_NOT_SUPPORTED));
        }
        assert_eq!(handles, expected_handles);
        let snapshot = log.snapshot();
        assert_eq!(
            snapshot.enqueued,
            failed.count_ones() as u64,
            "optional registration failure was not journaled"
        );
        assert_eq!(snapshot.dropped, 0);
        drop(log);
        history(&directory, &expected_events);
    }
}

#[test]
fn quality_notification_journal_keeps_periodic_fallback_and_manual_pause_with_or_without_log() {
    for logging in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let directory = dir.path().join("history");
        let log = Recorder::start(directory.clone());
        let start = Instant::now();
        let mut poll = Poll::new(start);
        poll.errors = logging.then(|| log.clone());
        let mut handles = vec![99_u32];
        let mut calls = 0;
        register_quality_notifications(&mut handles, poll.errors.as_deref(), |_| {
            calls += 1;
            Err(failure(ERROR_ACCESS_DENIED))
        });
        assert_eq!(calls, 2);
        assert_eq!(handles, [99]);
        let state = state();
        state.conditions.ready.store(false, Ordering::Release);
        let mut session = session();
        session.set_condition(PauseReason::Manual, true).unwrap();
        let settings = session.local_settings().clone();
        let revision = state.conditions.revision();
        poll.update(
            start,
            &state,
            || {
                Ok(SYSTEM_POWER_STATUS {
                    ACLineStatus: 0,
                    SystemStatusFlag: 1,
                    ..Default::default()
                })
            },
            || Ok(0),
        );
        assert!(state.conditions.ready());
        assert!(state.conditions.low_power());
        assert!(!state.conditions.paused(Some(&settings)));
        poll.update(
            start + Duration::from_millis(4999),
            &state,
            || panic!("fallback polled power before five seconds"),
            || panic!("fallback polled session before five seconds"),
        );
        poll.update(
            start + Duration::from_secs(5),
            &state,
            || {
                Ok(SYSTEM_POWER_STATUS {
                    ACLineStatus: 1,
                    SystemStatusFlag: 0,
                    ..Default::default()
                })
            },
            || Ok(0),
        );
        assert!(state.conditions.ready());
        assert!(!state.conditions.low_power());
        assert_eq!(state.conditions.revision(), revision);
        state.conditions.apply_to(&mut session);
        assert_eq!(session.local_settings(), &settings);
        assert_eq!(session.snapshot(start).pauses, [PauseReason::Manual].into());
        assert!(!session.snapshot(start).should_render);
        let snapshot = log.snapshot();
        assert_eq!(
            snapshot.enqueued,
            if logging { 2 } else { 0 },
            "optional errors must remain visible through successful polling"
        );
        assert_eq!((snapshot.dropped, snapshot.write_errors), (0, 0));
        drop(poll);
        drop(log);
        history(
            &directory,
            if logging {
                &[
                    ("power_source_notification", ERROR_ACCESS_DENIED),
                    ("power_saving_notification", ERROR_ACCESS_DENIED),
                ]
            } else {
                &[]
            },
        );
    }
}
