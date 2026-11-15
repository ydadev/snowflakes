use super::*;

fn clock() -> CacheClock {
    CacheClock {
        unix_ms: 1000,
        uptime_ms: 1000,
    }
}
fn records(directory: &std::path::Path) -> Vec<serde_json::Value> {
    std::fs::read_to_string(directory.join("errors.jsonl"))
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

#[test]
fn recovered_configuration_logs_only_a_coded_fault_and_keeps_existing_status_shape() {
    let dir = tempfile::tempdir().unwrap();
    let store = UserStore::at(dir.path());
    std::fs::write(
        store.config_path(),
        b"TEST-PRIVATE invalid configuration text",
    )
    .unwrap();
    let directory = store.error_log_directory();
    let log = ErrorRecorder::start(directory.clone());
    let controller =
        Controller::open_with_log(store, 1, clock(), Instant::now(), Some(log.clone())).unwrap();
    let status = controller.status(Instant::now());
    assert_eq!(status.configuration_warnings, 1);
    assert!(status.follow_server);
    let value = serde_json::to_value(&status).unwrap();
    assert!(value.get("error_log").is_none());
    drop(controller);
    drop(log);
    let events = records(&directory);
    assert_eq!(events.len(), 1);
    assert_eq!(events[0]["fault"], "configuration_recovered");
    assert_eq!(
        events[0]["code"],
        serde_json::json!({"kind":"storage","value":"configuration"})
    );
    assert!(
        !serde_json::to_string(&events)
            .unwrap()
            .contains("TEST-PRIVATE")
    );
}

#[cfg(windows)]
#[test]
fn failed_profile_write_is_logged_without_changing_settings_disk_or_manual_pause() {
    use std::os::windows::fs::OpenOptionsExt;
    let dir = tempfile::tempdir().unwrap();
    let store = UserStore::at(dir.path());
    let mut config = LocalConfig::default();
    config.network.follow_server = false;
    store.save_config(&config).unwrap();
    let original = std::fs::read(store.config_path()).unwrap();
    let directory = store.error_log_directory();
    let log = ErrorRecorder::start(directory.clone());
    let mut controller =
        Controller::open_with_log(store.clone(), 1, clock(), Instant::now(), Some(log.clone()))
            .unwrap();
    assert_eq!(
        controller.handle(Command::Pause {}, Instant::now()).code,
        OK
    );
    // Allow reading our own profile, but deny delete/replace for atomic save.
    let held = std::fs::OpenOptions::new()
        .read(true)
        .share_mode(1)
        .open(store.config_path())
        .unwrap();
    let before = serde_json::to_value(controller.status(Instant::now())).unwrap();
    let response = controller.handle(
        Command::Set {
            setting: Setting::SnowIntensity(83),
        },
        Instant::now(),
    );
    assert_eq!(
        (response.code, response.message.as_str()),
        (INTERNAL, "preferences_not_saved")
    );
    assert_eq!(
        serde_json::to_value(controller.status(Instant::now())).unwrap(),
        before
    );
    assert_eq!(std::fs::read(store.config_path()).unwrap(), original);
    drop(held);
    drop(controller);
    drop(log);
    let events = records(&directory);
    assert_eq!(events.len(), 1);
    assert_eq!(events[0]["fault"], "configuration_write");
    assert_eq!(events[0]["code"]["kind"], "os");
    assert!(events[0]["code"]["value"].as_i64().is_some());
    assert!(
        !serde_json::to_string(&events)
            .unwrap()
            .contains(dir.path().to_str().unwrap())
    );
}
