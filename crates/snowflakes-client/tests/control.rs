use snowflakes_client::{
    config::LocalConfig,
    control::*,
    session::{Limits, PauseReason, ProfileSource},
    storage::{CacheClock, UserStore},
};
use snowflakes_protocol::{Settings, WeatherMode};
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
