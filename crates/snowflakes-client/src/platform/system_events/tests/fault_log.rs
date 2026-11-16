use super::*;
use crate::error_log::Recorder;

fn power(ac: u8, saver: u8) -> Result<SYSTEM_POWER_STATUS> {
    Ok(SYSTEM_POWER_STATUS {
        ACLineStatus: ac,
        SystemStatusFlag: saver,
        ..Default::default()
    })
}
fn failure(code: WIN32_ERROR) -> windows::core::Error {
    windows::core::Error::new(code.to_hresult(), "synthetic-private-system-detail")
}
fn update(
    poll: &mut Poll,
    state: &WindowState,
    start: Instant,
    millis: u64,
    power: Result<SYSTEM_POWER_STATUS>,
    session: Result<u32>,
) {
    poll.update(
        start + Duration::from_millis(millis),
        state,
        || power,
        || session,
    );
}
fn history(directory: &std::path::Path, expected: &[(&str, WIN32_ERROR)]) {
    let text = std::fs::read_to_string(directory.join("errors.jsonl")).unwrap();
    assert!(!text.contains("synthetic-private-system-detail"));
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
fn system_query_journal_preserves_session_failure_episodes_and_poll_deadlines() {
    let dir = tempfile::tempdir().unwrap();
    let log = Recorder::start(dir.path().into());
    let start = Instant::now();
    let mut poll = Poll::new(start);
    poll.errors = Some(log.clone());
    let state = state();
    let mut session = session();
    session.set_condition(PauseReason::Manual, true).unwrap();
    let settings = session.local_settings().clone();
    state.session_event(WTS_REMOTE_CONNECT, 42);
    state.session_event(WTS_SESSION_LOCK, 42);
    state.power_event(PBT_APMSUSPEND);
    state.display_event(0);
    let bits = state.conditions.bits.load(Ordering::Acquire);
    let revision = state.conditions.revision();
    update(
        &mut poll,
        &state,
        start,
        0,
        power(1, 0),
        Err(failure(ERROR_ACCESS_DENIED)),
    );
    assert!(!state.conditions.ready());
    assert!(state.conditions.paused(Some(&settings)));
    assert_eq!(state.conditions.bits.load(Ordering::Acquire), bits);
    assert_eq!(state.conditions.revision(), revision);
    assert_eq!(
        log.snapshot().enqueued,
        1,
        "session query error was not journaled"
    );
    poll.update(
        start + Duration::from_millis(4999),
        &state,
        || panic!("power query before its deadline"),
        || panic!("session query before its deadline"),
    );
    update(
        &mut poll,
        &state,
        start,
        5000,
        power(1, 0),
        Err(failure(ERROR_ACCESS_DENIED)),
    );
    assert_eq!(log.snapshot().enqueued, 1);
    state.power_event(PBT_APMPOWERSTATUSCHANGE);
    update(
        &mut poll,
        &state,
        start,
        5001,
        power(1, 0),
        Err(failure(ERROR_ACCESS_DENIED)),
    );
    assert!(!state.refresh.get());
    assert_eq!(log.snapshot().enqueued, 1);
    poll.update(
        start + Duration::from_millis(10000),
        &state,
        || panic!("notification did not preserve the five-second poll interval"),
        || panic!("notification did not preserve the five-second poll interval"),
    );
    update(
        &mut poll,
        &state,
        start,
        10001,
        power(1, 0),
        Err(failure(ERROR_INVALID_DATA)),
    );
    assert_eq!(log.snapshot().enqueued, 2);
    update(
        &mut poll,
        &state,
        start,
        15001,
        power(1, 0),
        Ok(LOCKED | REMOTE),
    );
    assert!(state.conditions.ready());
    assert_eq!(state.conditions.bits.load(Ordering::Acquire), bits);
    assert_eq!(state.conditions.revision(), revision);
    // A consumer can see only this successful snapshot; earlier faults survive.
    assert_eq!(log.snapshot().enqueued, 2);
    update(
        &mut poll,
        &state,
        start,
        20001,
        power(1, 0),
        Err(failure(ERROR_ACCESS_DENIED)),
    );
    assert!(!state.conditions.ready());
    assert_eq!(log.snapshot().enqueued, 3);
    state.conditions.apply_to(&mut session);
    assert_eq!(session.local_settings(), &settings);
    let pauses = session.snapshot(start).pauses;
    for reason in [
        PauseReason::Manual,
        PauseReason::Locked,
        PauseReason::Suspended,
        PauseReason::DisplaysOff,
        PauseReason::RemoteSession,
    ] {
        assert!(pauses.contains(&reason));
    }
    assert_eq!(log.snapshot().dropped, 0);
    drop(poll);
    drop(log);
    history(
        dir.path(),
        &[
            ("session_query", ERROR_ACCESS_DENIED),
            ("session_query", ERROR_INVALID_DATA),
            ("session_query", ERROR_ACCESS_DENIED),
        ],
    );
}

#[test]
fn system_query_journal_keeps_power_and_session_failures_independent() {
    let dir = tempfile::tempdir().unwrap();
    let log = Recorder::start(dir.path().into());
    let start = Instant::now();
    let mut poll = Poll::new(start);
    poll.errors = Some(log.clone());
    let state = state();
    let mut session = session();
    session.set_condition(PauseReason::Manual, true).unwrap();
    let settings = session.local_settings().clone();
    state.conditions.power_snapshot(0, 1);
    let revision = state.conditions.revision();
    update(
        &mut poll,
        &state,
        start,
        0,
        Err(failure(ERROR_RETRY)),
        Ok(0),
    );
    assert!(state.conditions.ready());
    assert!(!state.conditions.paused(Some(&settings)));
    assert!(state.conditions.low_power());
    assert_eq!(state.conditions.revision(), revision);
    assert_eq!(
        log.snapshot().enqueued,
        1,
        "power query error was not journaled"
    );
    update(
        &mut poll,
        &state,
        start,
        5000,
        Err(failure(ERROR_RETRY)),
        Err(failure(ERROR_RETRY)),
    );
    assert!(!state.conditions.ready());
    assert!(state.conditions.low_power());
    assert_eq!(log.snapshot().enqueued, 2);
    // Unknown successful readings preserve quality but end only the power fault.
    update(
        &mut poll,
        &state,
        start,
        10000,
        power(255, 255),
        Err(failure(ERROR_RETRY)),
    );
    assert!(!state.conditions.ready());
    assert!(state.conditions.low_power());
    assert_eq!(log.snapshot().enqueued, 2);
    update(
        &mut poll,
        &state,
        start,
        15000,
        Err(failure(ERROR_RETRY)),
        Err(failure(ERROR_INVALID_DATA)),
    );
    assert_eq!(log.snapshot().enqueued, 4);
    update(
        &mut poll,
        &state,
        start,
        20000,
        Err(failure(ERROR_INVALID_DATA)),
        Ok(REMOTE),
    );
    assert!(state.conditions.ready());
    assert!(state.conditions.paused(Some(&settings)));
    assert!(state.conditions.low_power());
    assert_eq!(log.snapshot().enqueued, 5);
    update(
        &mut poll,
        &state,
        start,
        25000,
        power(1, 0),
        Err(failure(ERROR_INVALID_DATA)),
    );
    assert!(!state.conditions.ready());
    assert!(!state.conditions.low_power());
    assert_eq!(log.snapshot().enqueued, 6);
    state.conditions.apply_to(&mut session);
    assert!(
        session
            .snapshot(start)
            .pauses
            .contains(&PauseReason::Manual)
    );
    assert_eq!(session.local_settings(), &settings);
    assert_eq!(log.snapshot().dropped, 0);
    drop(poll);
    drop(log);
    history(
        dir.path(),
        &[
            ("power_query", ERROR_RETRY),
            ("session_query", ERROR_RETRY),
            ("power_query", ERROR_RETRY),
            ("session_query", ERROR_INVALID_DATA),
            ("power_query", ERROR_INVALID_DATA),
            ("session_query", ERROR_INVALID_DATA),
        ],
    );
}

#[test]
fn system_query_journal_success_keeps_history_lazy_and_ending_pause_authoritative() {
    let dir = tempfile::tempdir().unwrap();
    let directory = dir.path().join("history");
    let log = Recorder::start(directory.clone());
    let start = Instant::now();
    let mut poll = Poll::new(start);
    poll.errors = Some(log.clone());
    let state = state();
    let mut session = session();
    session.set_condition(PauseReason::Manual, true).unwrap();
    let settings = session.local_settings().clone();
    update(&mut poll, &state, start, 0, power(1, 0), Ok(0));
    assert!(state.conditions.ready());
    assert!(!state.conditions.paused(Some(&settings)));
    assert!(!directory.exists());
    state.session_event(WTS_SESSION_LOGOFF, 42);
    state.power_event(PBT_APMSUSPEND);
    state.display_event(0);
    update(&mut poll, &state, start, 5000, power(255, 255), Ok(0));
    assert!(state.conditions.ready());
    assert!(state.conditions.paused(Some(&settings)));
    assert_eq!(
        state.conditions.bits.load(Ordering::Acquire),
        DISCONNECTED | SUSPENDED | DISPLAYS_OFF
    );
    state.conditions.apply_to(&mut session);
    assert_eq!(session.local_settings(), &settings);
    assert_eq!(
        session.snapshot(start).pauses,
        [
            PauseReason::Manual,
            PauseReason::Disconnected,
            PauseReason::Suspended,
            PauseReason::DisplaysOff
        ]
        .into()
    );
    let snapshot = log.snapshot();
    assert_eq!(
        (snapshot.enqueued, snapshot.dropped, snapshot.write_errors),
        (0, 0, 0)
    );
    drop(poll);
    drop(log);
    assert!(!directory.exists());
}
