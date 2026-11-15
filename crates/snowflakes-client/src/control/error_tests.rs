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
fn configuration_warning_categories_are_journaled_without_private_fields_or_profile_rewrite() {
    let dir = tempfile::tempdir().unwrap();
    let store = UserStore::at(dir.path());
    let input = br#"
"TEST-PRIVATE-KEY" = "TEST-PRIVATE-VALUE"
"TEST-PRIVATE-SECOND" = { content = "TEST-PRIVATE-NESTED" }
[weather]
mode = "TEST-PRIVATE-MODE"
opacity_percent = 999
[network]
follow_server = false
server_url = "https://example.invalid/TEST-PRIVATE-ENDPOINT?private=value"
poll_interval_seconds = 1
max_cache_age_seconds = 1
[snow]
"TEST-PRIVATE-SNOW" = "TEST-PRIVATE-TEXT"
on_windows = "TEST-PRIVATE-TYPE"
intensity_percent = 999
wind_percent = -999
flake_size_percent = 999
flake_visibility_percent = -999
"#;
    std::fs::write(store.config_path(), input).unwrap();
    let directory = store.error_log_directory();
    let log = ErrorRecorder::start(directory.clone());
    let mut controller =
        Controller::open_with_log(store.clone(), 1, clock(), Instant::now(), Some(log.clone()))
            .unwrap();
    let config = controller.config();
    assert_eq!(config.settings.weather.mode, WeatherMode::Snow);
    assert_eq!(config.settings.weather.opacity_percent, 100);
    assert_eq!(config.settings.snow.intensity_percent, 100);
    assert_eq!(config.settings.snow.wind_percent, -100);
    assert_eq!(config.settings.snow.flake_size_percent, 200);
    assert_eq!(config.settings.snow.flake_visibility_percent, 0);
    assert!(config.settings.snow.on_windows);
    assert!(config.network.server_url.is_empty());
    assert_eq!(config.network.poll_interval_seconds, 600);
    assert_eq!(config.network.max_cache_age_seconds, 86_400);
    assert!(!config.network.follow_server);
    controller.handle(Command::Pause {}, Instant::now());
    for _ in 0..20 {
        let status = controller
            .handle(Command::Status {}, Instant::now())
            .status
            .unwrap();
        assert_eq!(status.configuration_warnings, 13);
        assert!(status.pauses.contains(&PauseReason::Manual));
        assert!(!status.rendering);
    }
    assert_eq!(std::fs::read(store.config_path()).unwrap(), input);
    drop(controller);
    drop(log);
    assert!(
        directory.join("errors.jsonl").is_file(),
        "configuration warnings were not journaled"
    );
    let events = records(&directory);
    let actual: std::collections::BTreeMap<_, _> = events
        .iter()
        .map(|event| {
            assert_eq!(event["code"]["kind"], "count");
            (
                event["fault"].as_str().unwrap(),
                event["code"]["value"].as_u64().unwrap(),
            )
        })
        .collect();
    assert_eq!(events.len(), 4);
    assert_eq!(
        actual,
        std::collections::BTreeMap::from([
            ("configuration_unknown", 3),
            ("configuration_wrong_type", 1),
            ("configuration_adjusted", 7),
            ("configuration_invalid_value", 2),
        ])
    );
    let text = std::fs::read_to_string(directory.join("errors.jsonl")).unwrap();
    for private in [
        "TEST-PRIVATE",
        "example.invalid",
        "private=value",
        dir.path().to_str().unwrap(),
    ] {
        assert!(!text.contains(private));
    }
}

#[test]
fn many_unknown_configuration_keys_use_one_bounded_journal_event() {
    let dir = tempfile::tempdir().unwrap();
    let store = UserStore::at(dir.path());
    let mut input = String::from("[snow]\n");
    for index in 0..2500 {
        use std::fmt::Write;
        writeln!(input, "private_key_{index}=1").unwrap();
    }
    assert!(input.len() < crate::config::FILE_LIMIT);
    std::fs::write(store.config_path(), &input).unwrap();
    let directory = store.error_log_directory();
    let log = ErrorRecorder::start(directory.clone());
    let controller =
        Controller::open_with_log(store, 1, clock(), Instant::now(), Some(log.clone())).unwrap();
    assert_eq!(
        controller.status(Instant::now()).configuration_warnings,
        2500
    );
    assert_eq!(log.snapshot().enqueued, 1);
    assert_eq!(log.snapshot().dropped, 0);
    drop(controller);
    drop(log);
    let events = records(&directory);
    assert_eq!(events.len(), 1);
    assert_eq!(events[0]["fault"], "configuration_unknown");
    assert_eq!(
        events[0]["code"],
        serde_json::json!({"kind":"count","value":2500})
    );
    assert!(
        std::fs::metadata(directory.join("errors.jsonl"))
            .unwrap()
            .len()
            < 512
    );
    assert!(
        !serde_json::to_string(&events)
            .unwrap()
            .contains("private_key")
    );
}

#[test]
fn missing_and_clean_configurations_create_no_error_history() {
    for existing in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let store = UserStore::at(dir.path());
        if existing {
            store.save_config(&LocalConfig::default()).unwrap();
        }
        let directory = store.error_log_directory();
        let log = ErrorRecorder::start(directory.clone());
        let controller =
            Controller::open_with_log(store.clone(), 1, clock(), Instant::now(), Some(log.clone()))
                .unwrap();
        assert_eq!(controller.status(Instant::now()).configuration_warnings, 0);
        assert_eq!(log.snapshot().enqueued, 0);
        drop(controller);
        drop(log);
        assert!(!directory.exists());
        assert_eq!(store.config_path().exists(), existing);
    }
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
