use snowflakes_client::{
    config::LocalConfig,
    control::*,
    session::{Limits, PauseReason, ProfileSource},
    storage::{CacheClock, UserStore},
};
use snowflakes_protocol::{Display, MonitorSelection, Settings, WeatherMode};
use std::time::Instant;
fn app(store: UserStore) -> Controller {
    Controller::open(
        store,
        1,
        CacheClock {
            unix_ms: 2_000_000_000_000,
            uptime_ms: 100_000,
        },
        Instant::now(),
    )
    .unwrap()
}
#[test]
fn snow_presets_only_set_intensity_and_keep_requested_values_under_policy() {
    use snowflakes_client::config::SnowPreset;
    let dir = tempfile::tempdir().unwrap();
    let store = UserStore::at(dir.path());
    let mut app = app(store.clone());
    let now = Instant::now();
    let command = |value: &str| parse_command(&["--set".into(), value.into()]).unwrap();
    assert_eq!(app.handle(command("snow_preset=light"), now).code, INVALID);
    app.handle(Command::FollowServer { enabled: false }, now);
    app.handle(Command::Pause {}, now);
    app.handle(command("snow_wind=-45"), now);
    app.handle(command("opacity=35"), now);
    for (name, preset, intensity) in [
        ("light", SnowPreset::Light, 20),
        ("normal", SnowPreset::Normal, 50),
        ("heavy", SnowPreset::Heavy, 85),
    ] {
        let mut expected = app.config().clone();
        expected.settings.snow.intensity_percent = intensity;
        expected.snow_preset = preset;
        assert_eq!(
            app.handle(command(&format!("snow_preset={name}")), now)
                .code,
            OK
        );
        assert_eq!(app.config(), &expected);
        assert_eq!(store.load_config().unwrap().config, expected);
        assert_eq!(app.status(now).snow_preset, Some(preset));
    }
    app.session
        .set_limits(
            Limits {
                max_intensity_percent: 25,
                ..Default::default()
            },
            now,
        )
        .unwrap();
    assert_eq!(app.handle(command("snow_preset=heavy"), now).code, OK);
    assert_eq!(app.status(now).settings.unwrap().snow.intensity_percent, 25);
    assert_eq!(app.config().settings.snow.intensity_percent, 85);
    app.handle(command("snow_wind=-100"), now);
    assert_eq!(app.status(now).snow_preset, Some(SnowPreset::Custom));
    app.handle(command("snow_preset=light"), now);
    app.handle(command("snow_intensity=20"), now);
    assert_eq!(app.status(now).snow_preset, Some(SnowPreset::Custom));
    app.handle(Command::FollowServer { enabled: true }, now);
    assert_eq!(app.status(now).snow_preset, None);
    app.handle(Command::FollowServer { enabled: false }, now);
    assert_eq!(app.status(now).snow_preset, Some(SnowPreset::Custom));
    assert_eq!(app.status(now).pauses, [PauseReason::Manual]);
    app.handle(command("mode=rain"), now);
    let before = app.config().clone();
    assert_eq!(app.handle(command("snow_preset=light"), now).code, INVALID);
    assert_eq!(app.config(), &before);
    assert!(parse_command(&["--set".into(), "snow_preset=unknown".into()]).is_err());
}
#[test]
fn unsuccessful_preset_save_keeps_both_label_and_weather_unchanged() {
    let dir = tempfile::tempdir().unwrap();
    let store = UserStore::at(dir.path());
    let mut app = app(store.clone());
    let now = Instant::now();
    app.handle(Command::FollowServer { enabled: false }, now);
    let before = app.config().clone();
    std::fs::remove_file(store.config_path()).unwrap();
    std::fs::create_dir(store.config_path()).unwrap();
    let command = parse_command(&["--set".into(), "snow_preset=heavy".into()]).unwrap();
    assert_eq!(app.handle(command, now).code, INTERNAL);
    assert_eq!(app.config(), &before);
}
#[test]
fn snow_options_persist_independently_and_preserve_manual_pause_and_source_rules() {
    let dir = tempfile::tempdir().unwrap();
    let store = UserStore::at(dir.path());
    let mut controller = app(store.clone());
    let now = Instant::now();
    let command = |text: &str| parse_command(&["--set".into(), text.into()]).unwrap();
    assert_eq!(
        controller
            .handle(command("snow_on_windows=false"), now)
            .code,
        INVALID
    );
    controller.handle(Command::FollowServer { enabled: false }, now);
    controller.handle(Command::Pause {}, now);
    let mut expected = controller.config().clone();
    for key in [
        "snow_on_windows",
        "snow_on_ground",
        "snow_melting",
        "cursor_enabled",
    ] {
        assert_eq!(
            controller
                .handle(command(&format!("{key}=false")), now)
                .code,
            OK
        );
    }
    expected.settings.snow.on_windows = false;
    expected.settings.snow.on_ground = false;
    expected.settings.snow.melting = false;
    expected.settings.interaction.cursor_enabled = false;
    assert_eq!(store.load_config().unwrap().config, expected);
    assert_eq!(app(store.clone()).config(), &expected);
    assert_eq!(controller.status(now).pauses, [PauseReason::Manual]);
    assert!(parse_command(&["--set".into(), "snow_melting=yes".into()]).is_err());
    std::fs::remove_file(store.config_path()).unwrap();
    std::fs::create_dir(store.config_path()).unwrap();
    assert_eq!(
        controller.handle(command("snow_on_ground=true"), now).code,
        INTERNAL
    );
    assert_eq!(controller.config(), &expected);
}
#[test]
fn performance_preferences_validate_persist_and_leave_weather_and_pause_untouched() {
    let directory = tempfile::tempdir().unwrap();
    let store = UserStore::at(directory.path());
    let mut controller = app(store.clone());
    let now = Instant::now();
    let command = |value: &str| parse_command(&["--set".into(), value.into()]).unwrap();
    assert_eq!(
        controller.handle(command("battery_saver=false"), now).code,
        INVALID
    );
    controller.handle(Command::FollowServer { enabled: false }, now);
    controller.handle(Command::Pause {}, now);
    let mut expected = controller.config().clone();
    for value in ["target_fps=30", "battery_saver=false"] {
        assert_eq!(controller.handle(command(value), now).code, OK);
    }
    expected.settings.performance.target_fps = 30;
    expected.settings.performance.battery_saver = false;
    assert_eq!(store.load_config().unwrap().config, expected);
    assert_eq!(app(store.clone()).config(), &expected);
    assert_eq!(controller.status(now).pauses, [PauseReason::Manual]);
    for value in [
        "target_fps=0",
        "target_fps=45",
        "target_fps=61",
        "battery_saver=maybe",
    ] {
        assert!(parse_command(&["--set".into(), value.into()]).is_err());
    }
    assert_eq!(
        controller
            .handle(
                Command::Set {
                    setting: Setting::TargetFps(0)
                },
                now
            )
            .code,
        INVALID
    );
    assert_eq!(store.load_config().unwrap().config, expected);
    controller
        .session
        .set_limits(
            Limits {
                max_fps: 30,
                ..Default::default()
            },
            now,
        )
        .unwrap();
    assert_eq!(controller.handle(command("target_fps=60"), now).code, OK);
    assert_eq!(controller.config().settings.performance.target_fps, 60);
    assert_eq!(
        controller
            .status(now)
            .settings
            .unwrap()
            .performance
            .target_fps,
        30
    );
}
#[test]
fn commands_preserve_independent_values_and_pause() {
    let dir = tempfile::tempdir().unwrap();
    let store = UserStore::at(dir.path());
    let mut app = app(store.clone());
    let now = Instant::now();
    assert_eq!(
        app.handle(
            Command::Set {
                setting: Setting::Mode(WeatherMode::Rain)
            },
            now
        )
        .code,
        INVALID
    );
    assert!(!store.config_path().exists());
    assert_eq!(
        app.handle(Command::FollowServer { enabled: false }, now)
            .code,
        OK
    );
    app.handle(Command::Pause {}, now);
    for setting in [
        Setting::Mode(WeatherMode::Rain),
        Setting::SnowIntensity(5),
        Setting::RainIntensity(85),
        Setting::DriftSize(0),
        Setting::PuddleSize(100),
    ] {
        assert_eq!(app.handle(Command::Set { setting }, now).code, OK);
    }
    let saved = store.load_config().unwrap().config;
    assert_eq!(saved.settings.rain.intensity_percent, 85);
    assert_eq!(saved.settings.snow.intensity_percent, 5);
    assert_eq!(saved.settings.snow.drift_size_percent, 0);
    assert_eq!(saved.settings.rain.puddle_size_percent, 100);
    let status = app.status(now);
    assert_eq!(status.source, ProfileSource::Local);
    assert!(status.pauses.contains(&PauseReason::Manual));
    assert!(!status.rendering);
    app.handle(Command::Clear {}, now);
    assert_ne!(app.status(now).clear_epoch, status.clear_epoch);
    assert_eq!(store.load_config().unwrap().config, saved);
    app.handle(Command::Resume {}, now);
    assert!(app.status(now).pauses.is_empty());
}
#[test]
fn failed_save_never_changes_applied_preferences() {
    let dir = tempfile::tempdir().unwrap();
    let store = UserStore::at(dir.path());
    let mut prefs = LocalConfig::default();
    prefs.network.follow_server = false;
    store.save_config(&prefs).unwrap();
    let mut app = app(store.clone());
    std::fs::remove_file(store.config_path()).unwrap();
    std::fs::create_dir(store.config_path()).unwrap();
    assert_eq!(
        app.handle(Command::FollowServer { enabled: true }, Instant::now())
            .code,
        INTERNAL
    );
    assert_eq!(
        app.handle(
            Command::Set {
                setting: Setting::RainIntensity(80)
            },
            Instant::now()
        )
        .code,
        INTERNAL
    );
    assert!(!app.session.follows_server());
    assert_eq!(app.session.local_settings(), &prefs.settings);
}
#[test]
fn policy_limits_do_not_overwrite_saved_values_and_exit_is_final() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = app(UserStore::at(dir.path()));
    let now = Instant::now();
    app.handle(Command::FollowServer { enabled: false }, now);
    app.session
        .set_limits(
            Limits {
                max_intensity_percent: 10,
                ..Default::default()
            },
            now,
        )
        .unwrap();
    app.handle(
        Command::Set {
            setting: Setting::RainIntensity(80),
        },
        now,
    );
    assert_eq!(app.status(now).settings.unwrap().rain.intensity_percent, 10);
    assert_eq!(app.config().settings.rain.intensity_percent, 80);
    app.handle(Command::Quit {}, now);
    assert_eq!(app.handle(Command::Resume {}, now).code, ABSENT);
}
#[test]
fn commands_reject_unknown_fields_types_and_out_of_range_values() {
    for text in [
        r#"{"command":"run","shell":"ignored"}"#,
        r#"{"command":"pause","extra":true}"#,
        r#"{"command":"set","setting":{"setting":"unknown","value":1}}"#,
        r#"{"command":"set","setting":{"setting":"rain_intensity","value":"50"}}"#,
        r#"{"command":"follow_server","enabled":1}"#,
    ] {
        assert!(serde_json::from_str::<Command>(text).is_err());
    }
    for args in [
        vec!["--set", "mode=other"],
        vec!["--set", "shell=test"],
        vec!["--set", "snow_intensity=-1"],
        vec!["--quit", "extra"],
    ] {
        assert!(parse_command(&args.into_iter().map(str::to_owned).collect::<Vec<_>>()).is_err());
    }
    let dir = tempfile::tempdir().unwrap();
    let mut app = app(UserStore::at(dir.path()));
    let now = Instant::now();
    app.handle(Command::FollowServer { enabled: false }, now);
    assert_eq!(
        app.handle(
            Command::Set {
                setting: Setting::RainIntensity(101)
            },
            now
        )
        .code,
        INVALID
    );
    assert_eq!(app.session.local_settings(), &Settings::default());
}
#[test]
fn status_has_no_endpoint_credentials_or_personal_identity() {
    let dir = tempfile::tempdir().unwrap();
    let app = app(UserStore::at(dir.path()));
    let text = serde_json::to_string(&app.status(Instant::now())).unwrap();
    for forbidden in ["server_url", "\"sid\"", "token", "hostname"] {
        assert!(!text.contains(forbidden));
    }
    let command = parse_command(&["--set".into(), "puddle_size=35".into()]).unwrap();
    let copy: Command = serde_json::from_slice(&serde_json::to_vec(&command).unwrap()).unwrap();
    assert!(matches!(
        copy,
        Command::Set {
            setting: Setting::PuddleSize(35)
        }
    ));
}

#[test]
fn endpoint_changes_are_atomic_normalized_and_private() {
    let dir = tempfile::tempdir().unwrap();
    let store = UserStore::at(dir.path());
    let mut app = app(store.clone());
    let now = Instant::now();
    let bytes = include_bytes!("../../../protocol/fixtures/configuration.json");
    let ticket = app.session.begin_sync(now).unwrap().unwrap();
    app.session.receive(ticket, bytes, now).unwrap();
    let old = app.session.begin_sync(now).unwrap().unwrap();
    let command = parse_command(&["--server-url".into(), " http://127.0.0.1:805 ".into()]).unwrap();
    let response = app.handle(command, now);
    assert_eq!(response.code, OK);
    assert_eq!(app.config().network.server_url, "http://127.0.0.1:805/");
    assert_eq!(store.load_config().unwrap().config, *app.config());
    assert_eq!(app.ui_view(now).server_url, "http://127.0.0.1:805/");
    assert_eq!(app.status(now).source, ProfileSource::WaitingForServer);
    assert!(app.session.receive(old, bytes, now).is_err());
    assert!(
        !serde_json::to_string(&response)
            .unwrap()
            .contains("127.0.0.1")
    );
    let ticket = app.session.begin_sync(now).unwrap().unwrap();
    app.session.receive(ticket, bytes, now).unwrap();
    let same = app.config().network.server_url.clone();
    assert_eq!(app.handle(Command::ServerUrl { value: same }, now).code, OK);
    assert_eq!(app.status(now).source, ProfileSource::Server);
    for value in [
        "file:///tmp/test",
        "http://user:secret@127.0.0.1/",
        "http://127.0.0.1/?token=test",
    ] {
        let response = app.handle(
            Command::ServerUrl {
                value: value.into(),
            },
            now,
        );
        assert_eq!(response.code, INVALID);
        assert!(!response.message.contains(value));
    }
    std::fs::remove_file(store.config_path()).unwrap();
    std::fs::create_dir(store.config_path()).unwrap();
    assert_eq!(
        app.handle(
            Command::ServerUrl {
                value: String::new()
            },
            now
        )
        .code,
        INTERNAL
    );
    assert_eq!(app.status(now).source, ProfileSource::Server);
    assert!(!app.config().network.server_url.is_empty());
    std::fs::remove_dir(store.config_path()).unwrap();
    assert_eq!(
        app.handle(
            Command::ServerUrl {
                value: String::new()
            },
            now
        )
        .code,
        OK
    );
    assert!(app.config().network.server_url.is_empty());
    assert_eq!(app.session.local_settings(), &Settings::default());
}

#[test]
fn open_settings_is_a_consumable_request_without_a_preferences_write() {
    let dir = tempfile::tempdir().unwrap();
    let store = UserStore::at(dir.path());
    let mut app = app(store.clone());
    assert!(!app.take_settings_request());
    let command = parse_command(&["--settings".into()]).unwrap();
    assert_eq!(app.handle(command, Instant::now()).code, OK);
    assert!(app.take_settings_request());
    assert!(!app.take_settings_request());
    assert!(!store.config_path().exists());
}

#[test]
fn percentage_appearance_controls_persist_without_changing_precipitation_or_drift_limit() {
    let dir = tempfile::tempdir().unwrap();
    let store = UserStore::at(dir.path());
    let mut app = app(store.clone());
    let now = Instant::now();
    app.handle(Command::FollowServer { enabled: false }, now);
    for argument in ["flake_size=150", "flake_visibility=175"] {
        let command = parse_command(&["--set".into(), argument.into()]).unwrap();
        assert_eq!(app.handle(command, now).code, OK);
    }
    let settings = store.load_config().unwrap().config.settings;
    assert_eq!(settings.snow.flake_size_percent, 150);
    assert_eq!(settings.snow.flake_visibility_percent, 175);
    assert_eq!(settings.snow.intensity_percent, 35);
    assert_eq!(settings.snow.drift_size_percent, 35);
    for setting in [Setting::FlakeSize(24), Setting::FlakeVisibility(201)] {
        assert_eq!(app.handle(Command::Set { setting }, now).code, INVALID);
    }
}

#[test]
fn display_selection_persists_independently_of_server_settings_and_manual_pause() {
    let dir = tempfile::tempdir().unwrap();
    let store = UserStore::at(dir.path());
    let mut controller = app(store.clone());
    let now = Instant::now();
    let choose = || {
        parse_command(&[
            "--monitors".into(),
            "selected".into(),
            "display-a".into(),
            "display-offline".into(),
        ])
        .unwrap()
    };
    assert_eq!(controller.handle(choose(), now).code, INVALID);
    assert!(!store.config_path().exists());
    controller.handle(Command::FollowServer { enabled: false }, now);
    controller.handle(Command::Pause {}, now);
    assert_eq!(controller.handle(choose(), now).code, OK);
    let saved = store.load_config().unwrap().config;
    assert_eq!(
        saved.settings.display.monitor_ids,
        ["display-a", "display-offline"]
    );
    assert_eq!(saved.settings.display.selection, MonitorSelection::Selected);
    assert!(controller.status(now).pauses.contains(&PauseReason::Manual));
    assert_eq!(saved.settings.snow, Settings::default().snow);
    controller.handle(Command::FollowServer { enabled: true }, now);
    let ticket = controller.session.begin_sync(now).unwrap().unwrap();
    controller
        .session
        .receive(
            ticket,
            include_bytes!("../../../protocol/fixtures/configuration.json"),
            now,
        )
        .unwrap();
    assert_eq!(
        controller.status(now).settings.unwrap().display.selection,
        MonitorSelection::All
    );
    let primary = || parse_command(&["--monitors".into(), "primary".into()]).unwrap();
    assert_eq!(controller.handle(primary(), now).code, INVALID);
    assert_eq!(
        store.load_config().unwrap().config.settings.display,
        saved.settings.display
    );
    controller.handle(Command::FollowServer { enabled: false }, now);
    assert_eq!(
        controller.status(now).settings.unwrap().display,
        saved.settings.display
    );
    assert!(controller.status(now).pauses.contains(&PauseReason::Manual));
    drop(controller);
    let mut restarted = app(store.clone());
    assert_eq!(
        restarted.status(now).settings.unwrap().display,
        saved.settings.display
    );
    for mode in ["primary", "all"] {
        assert_eq!(
            restarted
                .handle(
                    parse_command(&["--monitors".into(), mode.into()]).unwrap(),
                    now
                )
                .code,
            OK
        );
        assert!(
            store
                .load_config()
                .unwrap()
                .config
                .settings
                .display
                .monitor_ids
                .is_empty()
        );
    }
}

#[test]
fn invalid_or_unsaved_display_changes_leave_the_complete_previous_selection_intact() {
    let dir = tempfile::tempdir().unwrap();
    let store = UserStore::at(dir.path());
    let mut controller = app(store.clone());
    let now = Instant::now();
    controller.handle(Command::FollowServer { enabled: false }, now);
    let before = std::fs::read(store.config_path()).unwrap();
    for display in [
        Display {
            selection: MonitorSelection::Selected,
            monitor_ids: vec![],
        },
        Display {
            selection: MonitorSelection::Selected,
            monitor_ids: vec!["display-a".into(); 2],
        },
        Display {
            selection: MonitorSelection::Selected,
            monitor_ids: (0..17).map(|i| format!("display-{i}")).collect(),
        },
        Display {
            selection: MonitorSelection::All,
            monitor_ids: vec!["display-a".into()],
        },
        Display {
            selection: MonitorSelection::Selected,
            monitor_ids: vec!["bad\nvalue".into()],
        },
    ] {
        assert_eq!(
            controller
                .handle(
                    Command::Set {
                        setting: Setting::Display(display)
                    },
                    now
                )
                .code,
            INVALID
        );
        assert_eq!(std::fs::read(store.config_path()).unwrap(), before);
        assert_eq!(
            controller.session.local_settings().display,
            Settings::default().display
        );
    }
    for args in [
        vec!["--monitors", "selected"],
        vec!["--monitors", "all", "display-a"],
        vec!["--monitors", "unknown"],
    ] {
        assert!(parse_command(&args.into_iter().map(str::to_owned).collect::<Vec<_>>()).is_err());
    }
    std::fs::remove_file(store.config_path()).unwrap();
    std::fs::create_dir(store.config_path()).unwrap();
    let command =
        parse_command(&["--monitors".into(), "selected".into(), "display-a".into()]).unwrap();
    assert_eq!(controller.handle(command, now).code, INTERNAL);
    assert_eq!(
        controller.session.local_settings().display,
        Settings::default().display
    );
}
