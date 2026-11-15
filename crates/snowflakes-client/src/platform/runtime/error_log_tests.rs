use super::*;

fn wait(mut ready: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(4);
    while !ready() {
        assert!(Instant::now() < deadline, "fault report was not processed");
        thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn graphics_fault_journal_survives_coalesced_reports_and_keeps_binding_after_policy_reset() {
    let fixture = policy::tests::Fixture::new();
    let directory = tempfile::tempdir().unwrap();
    let store = UserStore::at(directory.path());
    let log_directory = store.error_log_directory();
    let errors = ErrorRecorder::start(log_directory.clone());
    let mut config = crate::config::LocalConfig::default();
    config.network.follow_server = false;
    store.save_config(&config).unwrap();
    let mut controller = Controller::open_with_log(
        store,
        1,
        CacheClock {
            unix_ms: 2_000_000_000_000,
            uptime_ms: 100_000,
        },
        Instant::now(),
        Some(errors.clone()),
    )
    .unwrap();
    controller.handle(Command::Pause {}, Instant::now());
    let owner = Owner::start_with_policy(
        controller,
        Limits::default(),
        directory.path().join("exit"),
        false,
        None,
        fixture.source(),
    );
    const CODE: i32 = 0x80004005_u32 as i32;
    let now = Instant::now();
    let mut recovery = super::super::graphics_state::Recovery::new(now);
    recovery.set_error_log(Some(errors.clone()));
    recovery.failed(now, CODE);
    recovery.failed(now + Duration::from_secs(2), CODE);
    recovery.submitted(false); // An occluded/skipped submission leaves the issue unresolved.
    assert_eq!(recovery.error, Some(CODE));
    assert!(recovery.can_create(now + Duration::from_secs(6)));
    // The UI sees only the latest report; all intermediate fault reports may
    // have been replaced before the control thread polls them.
    recovery.submitted(true);
    assert_eq!(recovery.error, None);
    wait(|| errors.snapshot().written == 1);
    for _ in 0..20 {
        let status = owner.request(Command::Status {}).status.unwrap();
        assert!(status.pauses.contains(&PauseReason::Manual));
        assert!(!status.rendering);
        assert_eq!(status.graphics_error, None);
    }
    assert_eq!(errors.snapshot().enqueued, 1);
    recovery.failed(now + Duration::from_secs(7), CODE);
    wait(|| errors.snapshot().written == 2);
    assert_eq!(errors.snapshot().enqueued, 2);
    recovery.reset(now + Duration::from_secs(8));
    assert!(recovery.can_create(now + Duration::from_secs(8)));
    recovery.failed(now + Duration::from_secs(8), CODE);
    wait(|| errors.snapshot().written == 3);
    assert_eq!(errors.snapshot().enqueued, 3);
    drop(recovery);
    drop(owner);
    drop(errors);
    let text = std::fs::read_to_string(log_directory.join("errors.jsonl")).unwrap();
    let events: Vec<serde_json::Value> = text
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(events.len(), 3);
    assert!(events.iter().all(|event| event["fault"] == "graphics"
        && event["code"] == serde_json::json!({"kind":"hresult", "value":CODE})));
}

#[test]
fn startup_fault_uses_the_supplied_store_without_reading_or_changing_a_profile() {
    let directory = tempfile::tempdir().unwrap();
    let root = Some(directory.path().to_path_buf());
    record_startup_fault(
        &root,
        Fault::InstanceGuard,
        LogCode::Hresult(0x80070005_u32 as i32),
    );
    assert!(!UserStore::at(directory.path()).config_path().exists());
    let text = std::fs::read_to_string(directory.path().join("logs/errors.jsonl")).unwrap();
    let events: Vec<serde_json::Value> = text
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0]["fault"], "instance_guard");
    assert_eq!(events[0]["code"]["kind"], "hresult");
    assert!(!text.contains(directory.path().to_str().unwrap()));
}
