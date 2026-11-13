use snowflakes_client::{
    config::{ConfigError, FILE_LIMIT, LocalConfig, WarningKind, normalize_endpoint},
    session::{CACHE_LIFETIME, ProfileSource, Session, SessionError},
    storage::{CacheClock, CachedProfile, StoreError, UserStore, atomic_write},
};
use snowflakes_protocol::{Configuration, Settings, WeatherMode};
use std::{
    fs,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};
use tempfile::tempdir;

const ENDPOINT: &str = "http://127.0.0.1:805/";
fn profile() -> Configuration {
    Configuration::from_json(include_bytes!(
        "../../../protocol/fixtures/configuration.json"
    ))
    .unwrap()
}
fn stamp(age_seconds: u64) -> CacheClock {
    CacheClock {
        unix_ms: 2_000_000_000_000 + age_seconds * 1000,
        uptime_ms: 100_000 + age_seconds * 1000,
    }
}

#[test]
fn local_snow_preset_round_trips_and_recovers_old_unknown_or_inconsistent_values() {
    use snowflakes_client::config::SnowPreset;
    let (old, warnings) = LocalConfig::parse(b"schema_version = 2").unwrap();
    assert_eq!(old.snow_preset, SnowPreset::Custom);
    assert!(warnings.is_empty());
    let mut config = old;
    config.settings.snow.intensity_percent = 20;
    config.snow_preset = SnowPreset::Light;
    let (loaded, warnings) = LocalConfig::parse(&config.encode().unwrap()).unwrap();
    assert_eq!(loaded, config);
    assert!(warnings.is_empty());
    let (unknown, warnings) = LocalConfig::parse(b"snow_preset = 'unexpected'").unwrap();
    assert_eq!(unknown.snow_preset, SnowPreset::Custom);
    assert!(
        warnings
            .iter()
            .any(|w| w.field == "snow_preset" && w.kind == WarningKind::InvalidValue)
    );
    let (inconsistent, warnings) =
        LocalConfig::parse(b"snow_preset = 'heavy'\n[snow]\nintensity_percent = 20").unwrap();
    assert_eq!(inconsistent.snow_preset, SnowPreset::Custom);
    assert_eq!(inconsistent.settings.snow.intensity_percent, 20);
    assert!(
        warnings
            .iter()
            .any(|w| w.field == "snow_preset" && w.kind == WarningKind::Adjusted)
    );
    config.settings.snow.intensity_percent = 21;
    assert_eq!(
        LocalConfig::parse(&config.encode().unwrap())
            .unwrap()
            .0
            .snow_preset,
        SnowPreset::Custom
    );
}
#[test]
fn defaults_and_full_toml_round_trip() {
    let (defaults, warnings) = LocalConfig::parse(b"").unwrap();
    assert_eq!(defaults, LocalConfig::default());
    assert!(warnings.is_empty());
    assert!(defaults.network.follow_server);
    assert!(defaults.network.server_url.is_empty());
    let mut config = defaults;
    config.network.follow_server = false;
    config.network.server_url = ENDPOINT.into();
    config.settings.weather.mode = WeatherMode::Rain;
    config.settings.rain.intensity_percent = 79;
    config.settings.rain.puddle_size_percent = 2;
    config.startup.enabled = true;
    let bytes = config.encode().unwrap();
    let (copy, warnings) = LocalConfig::parse(&bytes).unwrap();
    assert_eq!(copy, config);
    assert!(warnings.is_empty());
}

#[test]
fn published_example_matches_local_defaults_and_loopback_endpoint() {
    let (config, warnings) =
        LocalConfig::parse(include_bytes!("../../../examples/config-v2.example.toml")).unwrap();
    assert_eq!(config.settings, Settings::default());
    assert_eq!(config.network.server_url, ENDPOINT);
    assert!(config.network.follow_server);
    assert!(warnings.is_empty());
}

#[test]
fn forgiving_local_fields_do_not_weaken_strict_network_schema() {
    let text = br#"
schema_version = 2
unknown = "not logged"
[network]
follow_server = false
server_url = "http://127.0.0.1:805"
poll_interval_seconds = 1
[weather]
mode = "rain"
opacity_percent = 0
[snow]
intensity_percent = -1000
drift_size_percent = 101
wind_percent = 200
[rain]
intensity_percent = "forty"
puddle_size_percent = 0
[performance]
target_fps = 144
[interaction]
cursor_enabled = "yes"
"#;
    let (config, warnings) = LocalConfig::parse(text).unwrap();
    assert!(!config.network.follow_server);
    assert_eq!(config.network.server_url, ENDPOINT);
    assert_eq!(config.network.poll_interval_seconds, 600);
    assert_eq!(config.settings.weather.opacity_percent, 10);
    assert_eq!(config.settings.snow.intensity_percent, 0);
    assert_eq!(config.settings.snow.drift_size_percent, 100);
    assert_eq!(config.settings.snow.wind_percent, 100);
    assert_eq!(config.settings.rain.intensity_percent, 40);
    assert_eq!(config.settings.rain.puddle_size_percent, 0);
    assert!(config.settings.interaction.cursor_enabled);
    assert!(warnings.iter().any(|w| w.kind == WarningKind::Unknown));
    assert!(warnings.iter().any(|w| w.kind == WarningKind::WrongType));
    assert!(warnings.iter().any(|w| w.kind == WarningKind::Adjusted));
    let mut network = serde_json::to_value(profile()).unwrap();
    network["settings"]["snow"]["intensity_percent"] = 101.into();
    assert!(Configuration::from_json(&serde_json::to_vec(&network).unwrap()).is_err());
}

#[test]
fn unknown_nested_fields_and_wrong_sections_use_defaults_without_raw_values_in_errors() {
    let (config, warnings) = LocalConfig::parse(
        br#"
[network]
registration_token = "synthetic-sensitive-value"
[weather]
mode = "hail"
[snow]
melting = 1
[startup]
enabled = true
[unused]
arbitrary = "ignored"
"#,
    )
    .unwrap();
    assert_eq!(config.settings, Settings::default());
    assert!(config.startup.enabled);
    assert!(!format!("{warnings:?}").contains("synthetic-sensitive-value"));
    assert!(LocalConfig::parse(b"schema_version = 99").is_err());
    assert_eq!(
        LocalConfig::parse(b"[weather\n").unwrap_err(),
        ConfigError::InvalidSyntax
    );
    assert_eq!(
        LocalConfig::parse(&vec![b' '; FILE_LIMIT + 1]).unwrap_err(),
        ConfigError::TooLarge
    );
    assert_eq!(
        LocalConfig::parse(&[0xff]).unwrap_err(),
        ConfigError::Encoding
    );
}

#[test]
fn urls_are_normalized_without_credentials_or_url_secrets() {
    assert_eq!(
        normalize_endpoint("HTTP://LOCALHOST:805/site").unwrap(),
        "http://localhost:805/site/"
    );
    assert_eq!(
        normalize_endpoint("http://127.0.0.1:805/some%20site").unwrap(),
        "http://127.0.0.1:805/some%20site/"
    );
    for value in [
        "file:///test",
        "http://user:pass@localhost/",
        "http://localhost/?token=x",
        "http://localhost/#secret",
        "http://localhost/\n",
        "not a url",
    ] {
        assert!(normalize_endpoint(value).is_err());
    }
    let (config, warnings) = LocalConfig::parse(b"[network]\nserver_url = 'file:///test'").unwrap();
    assert!(config.network.server_url.is_empty());
    assert!(warnings.iter().any(|w| w.field == "network.server_url"));
}

#[test]
fn selected_monitor_identifiers_are_preserved_without_picking_a_replacement() {
    let (config, _) = LocalConfig::parse(
        br#"
[display]
selection = "selected"
monitor_ids = ["synthetic-unavailable-monitor"]
"#,
    )
    .unwrap();
    assert_eq!(
        config.settings.display.monitor_ids,
        ["synthetic-unavailable-monitor"]
    );
    assert!(LocalConfig::parse(b"[display]\nselection = 'selected'\nmonitor_ids = []").is_err());
}

#[test]
fn missing_config_has_no_side_effects_and_saving_replaces_the_complete_file() {
    let directory = tempdir().unwrap();
    let store = UserStore::at(directory.path().join("new"));
    assert_eq!(store.load_config().unwrap().config, LocalConfig::default());
    assert!(!store.config_path().exists());
    let mut config = LocalConfig::default();
    store.save_config(&config).unwrap();
    config.network.follow_server = false;
    config.settings.snow.drift_size_percent = 0;
    store.save_config(&config).unwrap();
    assert_eq!(store.load_config().unwrap().config, config);
    assert_eq!(
        fs::read_dir(store.config_path().parent().unwrap())
            .unwrap()
            .count(),
        1
    );
}

#[test]
fn corruption_is_backed_up_exactly_before_defaults_are_written() {
    let directory = tempdir().unwrap();
    let store = UserStore::at(directory.path());
    let corrupt = b"schema_version = 2\n[weather\n";
    fs::write(store.config_path(), corrupt).unwrap();
    let loaded = store.load_config().unwrap();
    assert_eq!(loaded.config, LocalConfig::default());
    assert_eq!(fs::read(loaded.recovered_backup.unwrap()).unwrap(), corrupt);
    assert!(
        loaded
            .warnings
            .iter()
            .any(|w| w.kind == WarningKind::Recovered)
    );
    assert_eq!(
        LocalConfig::parse(&fs::read(store.config_path()).unwrap())
            .unwrap()
            .0,
        LocalConfig::default()
    );
}

#[test]
fn unsupported_version_oversize_and_invalid_save_never_destroy_the_original() {
    let directory = tempdir().unwrap();
    let store = UserStore::at(directory.path());
    let future = b"schema_version = 99\n";
    fs::write(store.config_path(), future).unwrap();
    assert!(matches!(
        store.load_config(),
        Err(StoreError::Config(ConfigError::UnsupportedVersion))
    ));
    assert_eq!(fs::read(store.config_path()).unwrap(), future);
    assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 1);
    fs::write(store.config_path(), vec![b'x'; FILE_LIMIT + 1]).unwrap();
    assert!(matches!(
        store.load_config(),
        Err(StoreError::Config(ConfigError::TooLarge))
    ));
    assert_eq!(
        fs::metadata(store.config_path()).unwrap().len(),
        (FILE_LIMIT + 1) as u64
    );
    store.save_config(&LocalConfig::default()).unwrap();
    let original = fs::read(store.config_path()).unwrap();
    let mut invalid = LocalConfig::default();
    invalid.settings.snow.intensity_percent = 101;
    assert!(store.save_config(&invalid).is_err());
    assert_eq!(fs::read(store.config_path()).unwrap(), original);
}

#[test]
fn reader_never_observes_a_missing_or_partially_written_file() {
    let directory = tempdir().unwrap();
    let path = Arc::new(directory.path().join("atomic.txt"));
    let first = vec![b'a'; 32_768];
    let second = vec![b'b'; 32_768];
    atomic_write(&path, &first).unwrap();
    let running = Arc::new(AtomicBool::new(true));
    let (ready, started) = std::sync::mpsc::channel();
    let reader = {
        let path = path.clone();
        let running = running.clone();
        let first = first.clone();
        let second = second.clone();
        std::thread::spawn(move || {
            let content = fs::read(&*path).unwrap();
            assert!(content == first || content == second);
            ready.send(()).unwrap();
            while running.load(Ordering::Acquire) {
                let content = fs::read(&*path).unwrap();
                assert!(content == first || content == second);
            }
        })
    };
    started.recv().unwrap();
    let writes = (0..20)
        .try_for_each(|index| atomic_write(&path, if index % 2 == 0 { &second } else { &first }));
    running.store(false, Ordering::Release);
    reader.join().unwrap();
    writes.unwrap();
}

#[cfg(windows)]
#[test]
fn locked_destination_keeps_old_config_and_leaves_no_temporary_file() {
    use std::os::windows::fs::OpenOptionsExt;
    let directory = tempdir().unwrap();
    let path = directory.path().join("locked.txt");
    fs::write(&path, b"original").unwrap();
    let lock = fs::OpenOptions::new()
        .read(true)
        .share_mode(1)
        .open(&path)
        .unwrap();
    assert!(atomic_write(&path, b"replacement").is_err());
    drop(lock);
    assert_eq!(fs::read(&path).unwrap(), b"original");
    assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 1);
}

#[test]
fn repeated_restarts_keep_the_original_validation_deadline() {
    let directory = tempdir().unwrap();
    let store = UserStore::at(directory.path());
    store
        .save_cache(&CachedProfile::verified(ENDPOINT, profile(), stamp(0)).unwrap())
        .unwrap();
    for age in [1, 60, 600, 43_200, 86_399] {
        let restored = store.load_cache(ENDPOINT, stamp(age)).unwrap().unwrap();
        assert_eq!(
            restored.remaining,
            CACHE_LIFETIME - Duration::from_secs(age)
        );
    }
    assert!(matches!(
        store.load_cache(ENDPOINT, stamp(86_400)),
        Err(StoreError::CacheExpired)
    ));
}

#[test]
fn rollback_frozen_clock_reboot_and_endpoint_change_are_checked() {
    let directory = tempdir().unwrap();
    let store = UserStore::at(directory.path());
    store
        .save_cache(&CachedProfile::verified(ENDPOINT, profile(), stamp(0)).unwrap())
        .unwrap();
    store.load_cache(ENDPOINT, stamp(600)).unwrap();
    assert!(matches!(
        store.load_cache(ENDPOINT, stamp(599)),
        Err(StoreError::ClockRollback)
    ));
    let frozen = CacheClock {
        unix_ms: stamp(600).unix_ms,
        uptime_ms: stamp(700).uptime_ms,
    };
    assert!(matches!(
        store.load_cache(ENDPOINT, frozen),
        Err(StoreError::ClockRollback)
    ));
    assert!(matches!(
        store.load_cache("http://localhost:805/", stamp(700)),
        Err(StoreError::WrongEndpoint)
    ));
    let after_reboot = CacheClock {
        unix_ms: stamp(3600).unix_ms,
        uptime_ms: 20_000,
    };
    assert_eq!(
        store
            .load_cache(ENDPOINT, after_reboot)
            .unwrap()
            .unwrap()
            .remaining,
        Duration::from_secs(82_800)
    );
}

#[test]
fn restored_profile_expires_after_remaining_time_and_does_not_overwrite_local_values() {
    let directory = tempdir().unwrap();
    let store = UserStore::at(directory.path());
    let mut config = LocalConfig::default();
    config.network.server_url = ENDPOINT.into();
    config.settings.snow.intensity_percent = 3;
    store.save_config(&config).unwrap();
    store
        .save_cache(&CachedProfile::verified(ENDPOINT, profile(), stamp(0)).unwrap())
        .unwrap();
    let now = Instant::now();
    let mut loaded = store.open_session(stamp(86_390), now).unwrap();
    assert!(loaded.cache_error.is_none());
    assert!(loaded.session.snapshot(now).should_render);
    assert_eq!(loaded.session.local_settings(), &config.settings);
    assert_eq!(
        loaded
            .session
            .snapshot(now + Duration::from_secs(10))
            .source,
        ProfileSource::WaitingForServer
    );
    loaded.session.set_follow_server(false, now).unwrap();
    assert_eq!(
        loaded.session.snapshot(now).settings.as_ref(),
        Some(&config.settings)
    );
    let restarted = store.open_session(stamp(86_400), now).unwrap();
    assert!(matches!(
        restarted.cache_error,
        Some(StoreError::CacheExpired)
    ));
    assert!(!restarted.session.snapshot(now).should_render);
}

#[test]
fn local_source_choice_survives_restart_even_with_a_server_cache() {
    let directory = tempdir().unwrap();
    let store = UserStore::at(directory.path());
    let mut config = LocalConfig::default();
    config.network.follow_server = false;
    config.network.server_url = ENDPOINT.into();
    config.settings.snow.intensity_percent = 9;
    store.save_config(&config).unwrap();
    store
        .save_cache(&CachedProfile::verified(ENDPOINT, profile(), stamp(0)).unwrap())
        .unwrap();
    let now = Instant::now();
    let mut loaded = store.open_session(stamp(60), now).unwrap();
    assert_eq!(loaded.session.snapshot(now).source, ProfileSource::Local);
    assert_eq!(loaded.session.local_settings(), &config.settings);
    assert!(loaded.session.begin_sync(now).unwrap().is_none());
}

#[test]
fn invalid_cache_does_not_prevent_opening_local_settings() {
    let directory = tempdir().unwrap();
    let store = UserStore::at(directory.path());
    let mut config = LocalConfig::default();
    config.network.server_url = ENDPOINT.into();
    store.save_config(&config).unwrap();
    fs::write(store.cache_path(), b"not json").unwrap();
    let loaded = store.open_session(stamp(1), Instant::now()).unwrap();
    assert!(matches!(loaded.cache_error, Some(StoreError::InvalidCache)));
    assert_eq!(loaded.local.config, config);
    assert!(!loaded.session.snapshot(Instant::now()).should_render);
    assert!(fs::read_dir(directory.path()).unwrap().any(|entry| {
        entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with("cache-corrupt-")
    }));
}

#[test]
fn startup_cache_cannot_extend_lifetime_or_replace_an_active_session() {
    let now = Instant::now();
    let mut session = Session::new(Settings::default()).unwrap();
    assert_eq!(
        session.restore_cache(profile(), CACHE_LIFETIME + Duration::from_secs(1), now),
        Err(SessionError::RestoreUnavailable)
    );
    assert_eq!(
        session.restore_cache(profile(), Duration::ZERO, now),
        Err(SessionError::RestoreUnavailable)
    );
    session
        .restore_cache(profile(), Duration::from_secs(5), now)
        .unwrap();
    assert_eq!(
        session.restore_cache(profile(), CACHE_LIFETIME, now),
        Err(SessionError::RestoreUnavailable)
    );
    let mut active = Session::new(Settings::default()).unwrap();
    active.begin_sync(now).unwrap();
    assert_eq!(
        active.restore_cache(profile(), CACHE_LIFETIME, now),
        Err(SessionError::RestoreUnavailable)
    );
}

#[cfg(windows)]
#[test]
fn platform_cache_clock_is_positive_and_monotonic() {
    let first = CacheClock::current().unwrap();
    let next = CacheClock::current().unwrap();
    assert!(first.unix_ms > 0);
    assert!(next.uptime_ms >= first.uptime_ms);
}

#[test]
fn reboot_with_frozen_time_and_nonadvancing_restore_clocks_are_rejected() {
    let directory = tempdir().unwrap();
    let store = UserStore::at(directory.path());
    store
        .save_cache(&CachedProfile::verified(ENDPOINT, profile(), stamp(0)).unwrap())
        .unwrap();
    assert!(matches!(
        store.load_cache(ENDPOINT, stamp(0)),
        Err(StoreError::ClockRollback)
    ));
    store.load_cache(ENDPOINT, stamp(600)).unwrap();
    let reboot = CacheClock {
        unix_ms: stamp(600).unix_ms,
        uptime_ms: 20_000,
    };
    assert!(matches!(
        store.load_cache(ENDPOINT, reboot),
        Err(StoreError::ClockRollback)
    ));
}

#[test]
fn semantically_invalid_cache_is_backed_up_and_rejected() {
    let directory = tempdir().unwrap();
    let store = UserStore::at(directory.path());
    let cache = CachedProfile::verified(ENDPOINT, profile(), stamp(0)).unwrap();
    let mut document = serde_json::to_value(cache).unwrap();
    document["configuration"]["settings"]["rain"]["intensity_percent"] = 101.into();
    let bytes = serde_json::to_vec(&document).unwrap();
    fs::write(store.cache_path(), &bytes).unwrap();
    assert!(matches!(
        store.load_cache(ENDPOINT, stamp(1)),
        Err(StoreError::InvalidCache)
    ));
    let backup = fs::read_dir(directory.path())
        .unwrap()
        .filter_map(Result::ok)
        .find(|entry| {
            entry
                .file_name()
                .to_string_lossy()
                .starts_with("cache-corrupt-")
        })
        .unwrap();
    assert_eq!(fs::read(backup.path()).unwrap(), bytes);
}
