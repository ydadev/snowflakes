#![cfg(windows)]
use snowflakes_client::control::{ABSENT, INVALID, NO_SESSION, OK, Response};
use std::{
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};
struct Host {
    child: Child,
    namespace: String,
    directory: tempfile::TempDir,
}
impl Drop for Host {
    fn drop(&mut self) {
        if self.child.try_wait().ok().flatten().is_none() {
            let _ = self.child.kill();
        }
        let _ = self.child.wait();
    }
}
fn request_output(namespace: &str, args: &[&str]) -> std::process::Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_snowflakes"))
        .args(["--test-namespace", namespace])
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(8);
    while child.try_wait().unwrap().is_none() {
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            panic!("CLI request timed out");
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    child.wait_with_output().unwrap()
}
fn request(namespace: &str, args: &[&str]) -> (i32, Response) {
    let output = request_output(namespace, args);
    let code = output.status.code().unwrap();
    let response: Response = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(code, response.code);
    (code, response)
}
fn discover(namespace: &str) -> Vec<String> {
    let output = request_output(namespace, &["--list-monitors", "--json"]);
    assert_eq!(output.status.code(), Some(OK));
    let response: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(response["code"], OK);
    assert_eq!(response["message"], "monitors");
    let monitors = response["monitors"].as_array().unwrap();
    assert!(!monitors.is_empty());
    assert_eq!(monitors.iter().filter(|m| m["primary"] == true).count(), 1);
    let mut ids = Vec::new();
    for monitor in monitors {
        let id = monitor["id"].as_str().unwrap();
        let key = u64::from_str_radix(id.strip_prefix("display-").unwrap(), 16).unwrap();
        assert!(key > 0 && key < (1 << 63));
        assert!(monitor["persistent"].is_boolean());
        assert!(monitor["width"].as_f64().unwrap() > 0.0);
        assert!(monitor["height"].as_f64().unwrap() > 0.0);
        // Only the documented, non-identifying geometry fields leave discovery.
        assert_eq!(monitor.as_object().unwrap().len(), 7);
        ids.push(id.to_owned());
    }
    ids.sort_unstable();
    let mut unique = ids.clone();
    unique.dedup();
    assert_eq!(unique.len(), ids.len());
    ids
}
#[test]
fn monitor_discovery_is_repeatable_and_does_not_start_an_instance() {
    let namespace = format!("discovery-{}", std::process::id());
    assert_eq!(request(&namespace, &["--status", "--json"]).0, ABSENT);
    assert_eq!(discover(&namespace), discover(&namespace));
    assert_eq!(request(&namespace, &["--status", "--json"]).0, ABSENT);
}
fn start(namespace: String, directory: tempfile::TempDir) -> Host {
    start_with_trace(namespace, directory, None)
}
fn start_with_trace(
    namespace: String,
    directory: tempfile::TempDir,
    trace: Option<&std::path::Path>,
) -> Host {
    start_with_diagnostics(namespace, directory, trace, false)
}
fn start_with_diagnostics(
    namespace: String,
    directory: tempfile::TempDir,
    trace: Option<&std::path::Path>,
    frame_metrics: bool,
) -> Host {
    let mut command = Command::new(env!("CARGO_BIN_EXE_snowflakes"));
    command
        .args([
            "--ensure-running",
            "--test-headless",
            "--test-namespace",
            &namespace,
            "--data-dir",
        ])
        .arg(directory.path())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    if let Some(trace) = trace {
        command.arg("--trace-latency").arg(trace);
    }
    if frame_metrics {
        command.arg("--frame-metrics");
    }
    let child = command.spawn().unwrap();
    let mut host = Host {
        child,
        namespace,
        directory,
    };
    let deadline = Instant::now() + Duration::from_secs(8);
    loop {
        if request(&host.namespace, &["--status", "--json"]).0 == OK {
            return host;
        }
        assert!(
            host.child.try_wait().unwrap().is_none(),
            "test host exited during startup"
        );
        assert!(Instant::now() < deadline, "test host startup timed out");
        std::thread::sleep(Duration::from_millis(30));
    }
}
fn await_exit(host: &mut Host) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while host.child.try_wait().unwrap().is_none() {
        assert!(Instant::now() < deadline, "quit timed out");
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn auto_pause_settings_round_trip_over_ipc_without_resuming_the_headless_client() {
    let namespace = format!("auto-pause-control-{}", std::process::id());
    assert_eq!(
        request(&namespace, &["--set", "pause_remote_session=false"]).0,
        ABSENT
    );
    let mut host = start(namespace.clone(), tempfile::tempdir().unwrap());
    assert_eq!(request(&namespace, &["--pause"]).0, OK);
    assert_eq!(
        request(&namespace, &["--set", "pause_fullscreen=false"]).0,
        INVALID
    );
    assert_eq!(request(&namespace, &["--follow-server", "off"]).0, OK);
    assert_eq!(request(&namespace, &["--set", "snow_preset=normal"]).0, OK);
    let epoch = request(&namespace, &["--status"])
        .1
        .status
        .unwrap()
        .clear_epoch;
    for (entry, remote, fullscreen) in [
        ("pause_remote_session=false", false, true),
        ("pause_fullscreen=false", false, false),
        ("pause_remote_session=true", true, false),
        ("pause_fullscreen=true", true, true),
    ] {
        let (code, response) = request(&namespace, &["--set", entry]);
        assert_eq!(code, OK);
        let status = response.status.unwrap();
        assert!(!status.rendering);
        assert!(
            status
                .pauses
                .contains(&snowflakes_client::session::PauseReason::Manual)
        );
        assert_eq!(status.clear_epoch, epoch);
        let actual = &status.settings.unwrap().performance;
        for (effective, requested) in [
            (actual.pause_fullscreen, fullscreen),
            (actual.pause_remote_session, remote),
        ] {
            if requested {
                assert!(effective);
            } else if effective {
                assert!(status.limited);
            }
        }
        let config = snowflakes_client::storage::UserStore::at(host.directory.path())
            .load_config()
            .unwrap()
            .config;
        assert_eq!(config.settings.performance.pause_remote_session, remote);
        assert_eq!(config.settings.performance.pause_fullscreen, fullscreen);
        assert_eq!(
            config.snow_preset,
            snowflakes_client::config::SnowPreset::Normal
        );
    }
    assert_eq!(request(&namespace, &["--quit"]).0, OK);
    await_exit(&mut host);
}

#[test]
fn capture_preference_round_trips_over_ipc_without_starting_a_renderer() {
    let namespace = format!("capture-control-{}", std::process::id());
    assert_eq!(
        request(&namespace, &["--exclude-from-capture", "on"]).0,
        ABSENT
    );
    let mut host = start(namespace.clone(), tempfile::tempdir().unwrap());
    assert_eq!(request(&namespace, &["--pause"]).0, OK);
    for (value, enabled) in [("on", true), ("off", false)] {
        let (code, response) = request(&namespace, &["--exclude-from-capture", value]);
        assert_eq!(code, OK);
        let status = response.status.unwrap();
        assert_eq!(status.exclude_from_capture, enabled);
        assert!(status.follow_server);
        assert!(
            status
                .pauses
                .contains(&snowflakes_client::session::PauseReason::Manual)
        );
        assert!(!status.rendering);
        assert_eq!(status.capture_exclusion.windows, 0);
        assert_eq!(status.capture_exclusion.applied, 0);
        assert_eq!(
            snowflakes_client::storage::UserStore::at(host.directory.path())
                .load_config()
                .unwrap()
                .config
                .exclude_from_capture,
            enabled
        );
    }
    assert_eq!(request(&namespace, &["--quit"]).0, OK);
    await_exit(&mut host);
}

#[test]
fn frame_metrics_flag_does_not_invent_a_headless_renderer() {
    let namespace = format!("frame-headless-{}", std::process::id());
    let mut host =
        start_with_diagnostics(namespace.clone(), tempfile::tempdir().unwrap(), None, true);
    let status = request(&namespace, &["--status", "--json"])
        .1
        .status
        .unwrap();
    assert!(!status.rendering);
    assert!(status.frame_metrics.is_none());
    let wire = request_output(&namespace, &["--status", "--json"]);
    let json: serde_json::Value = serde_json::from_slice(&wire.stdout).unwrap();
    assert!(json["status"].get("frame_metrics").is_none());
    assert_eq!(request(&namespace, &["--quit"]).0, OK);
    await_exit(&mut host);
}

#[test]
fn latency_cli_is_opt_in_new_instance_only_and_never_overwrites() {
    let directory = tempfile::tempdir().unwrap();
    let namespace = format!("latency-{}", std::process::id());
    let trace = directory.path().join("trace.ndjson");
    let trace_arg = trace.to_str().unwrap();
    for args in [
        vec!["--trace-latency"],
        vec!["--ensure-running", "--trace-latency", "relative.ndjson"],
        vec![
            "--ensure-running",
            "--trace-latency",
            trace_arg,
            "--trace-latency",
            trace_arg,
        ],
        vec!["--status", "--json", "--trace-latency", trace_arg],
        vec!["--list-monitors", "--json", "--trace-latency", trace_arg],
        vec!["--status", "--json", "--frame-metrics"],
        vec!["--list-monitors", "--json", "--frame-metrics"],
        vec!["--ensure-running", "--frame-metrics", "--frame-metrics"],
    ] {
        assert_eq!(request(&namespace, &args).0, INVALID);
        assert!(!trace.exists());
    }
    let existing = directory.path().join("existing.ndjson");
    std::fs::write(&existing, b"keep").unwrap();
    assert_eq!(
        request(
            &namespace,
            &[
                "--ensure-running",
                "--test-headless",
                "--data-dir",
                directory.path().to_str().unwrap(),
                "--trace-latency",
                existing.to_str().unwrap()
            ]
        )
        .1
        .message,
        "diagnostics_output_unavailable"
    );
    assert_eq!(std::fs::read(existing).unwrap(), b"keep");
    assert_eq!(request(&namespace, &["--status", "--json"]).0, ABSENT);
    let mut host = start_with_trace(namespace.clone(), directory, Some(&trace));
    assert_eq!(request(&namespace, &["--pause"]).0, OK);
    assert_eq!(request(&namespace, &["--follow-server", "off"]).0, OK);
    assert_eq!(request(&namespace, &["--set", "flake_size=150"]).0, OK);
    let other_trace = host.directory.path().join("other.ndjson");
    assert_eq!(
        request(
            &namespace,
            &[
                "--ensure-running",
                "--trace-latency",
                other_trace.to_str().unwrap()
            ]
        )
        .1
        .message,
        "diagnostics_require_new_instance"
    );
    assert!(!other_trace.exists());
    assert_eq!(
        request(&namespace, &["--ensure-running", "--frame-metrics"])
            .1
            .message,
        "diagnostics_require_new_instance"
    );
    let status = request(&namespace, &["--status", "--json"])
        .1
        .status
        .unwrap();
    assert!(
        status
            .pauses
            .contains(&snowflakes_client::session::PauseReason::Manual)
    );
    assert!(!status.rendering);
    assert_eq!(status.settings.unwrap().snow.flake_size_percent, 150);
    assert_eq!(request(&namespace, &["--quit"]).0, OK);
    await_exit(&mut host);
    let events: Vec<serde_json::Value> = std::fs::read_to_string(trace)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(events.len(), 2); // IPC never pretends to be local widget input.
    assert_eq!(events[0]["kind"], "header");
    assert_eq!(
        events[1],
        serde_json::json!({"kind":"summary", "dropped":0, "write_errors":0, "unfinished":0})
    );
}
#[test]
fn native_process_singleton_commands_exit_and_restart() {
    let directory = tempfile::tempdir().unwrap();
    let namespace = format!(
        "test-{}-{}",
        std::process::id(),
        directory
            .path()
            .file_name()
            .unwrap()
            .to_string_lossy()
            .replace('.', "-")
    );
    assert_eq!(request(&namespace, &["--status", "--json"]).0, ABSENT);
    let mut host = start(namespace.clone(), directory);
    let initial = request(&namespace, &["--status", "--json"])
        .1
        .status
        .unwrap();
    assert!(initial.follow_server);
    assert!(!initial.rendering);
    assert_eq!(request(&namespace, &["--settings"]).0, NO_SESSION);
    let endpoint = request(&namespace, &["--server-url", "http://127.0.0.1:805"]);
    assert_eq!(endpoint.0, OK);
    assert!(
        !serde_json::to_string(&endpoint.1)
            .unwrap()
            .contains("127.0.0.1")
    );
    assert_eq!(request(&namespace, &["--follow-server", "off"]).0, OK);
    for arg in [
        "mode=rain",
        "snow_intensity=7",
        "rain_intensity=85",
        "drift_size=0",
        "puddle_size=100",
    ] {
        assert_eq!(request(&namespace, &["--set", arg]).0, OK);
    }
    assert_eq!(
        request(&namespace, &["--set", "rain_intensity=101"]).0,
        INVALID
    );
    let paused = request(&namespace, &["--pause"]).1.status.unwrap();
    for value in [
        "target_fps=30",
        "battery_saver=false",
        "snow_on_windows=false",
        "snow_on_ground=false",
        "snow_melting=false",
        "cursor_enabled=false",
    ] {
        let result = request(&namespace, &["--set", value]);
        assert_eq!(result.0, OK);
        assert_eq!(result.1.status.unwrap().pauses, paused.pauses);
    }
    let quality = request(&namespace, &["--status", "--json"])
        .1
        .status
        .unwrap();
    assert_eq!(
        quality.effective_target_fps, 0,
        "headless host has no renderer"
    );
    assert!(!quality.reduced_detail);
    let applied = quality.settings.as_ref().unwrap();
    assert!(
        !applied.snow.on_windows
            && !applied.snow.on_ground
            && !applied.snow.melting
            && !applied.interaction.cursor_enabled
    );
    let preferences = quality.settings.unwrap().performance;
    assert_eq!(preferences.target_fps, 30);
    assert!(!preferences.battery_saver);
    assert_eq!(request(&namespace, &["--set", "target_fps=45"]).0, INVALID);
    assert_eq!(
        request(&namespace, &["--set", "snow_preset=light"]).0,
        INVALID
    ); // Active mode is rain.
    assert_eq!(request(&namespace, &["--set", "mode=snow"]).0, OK);
    let preset_result = request(&namespace, &["--set", "snow_preset=normal"]);
    assert_eq!(preset_result.0, OK);
    let preset_status = preset_result.1.status.unwrap();
    assert_eq!(preset_status.settings.unwrap().snow.intensity_percent, 50);
    assert_eq!(
        preset_status.snow_preset,
        Some(snowflakes_client::config::SnowPreset::Normal)
    );
    assert_eq!(preset_status.pauses, paused.pauses);
    let selected = request(
        &namespace,
        &[
            "--monitors",
            "selected",
            "display-synthetic-a",
            "display-synthetic-b",
        ],
    );
    assert_eq!(selected.0, OK);
    let selected = selected.1.status.unwrap();
    assert_eq!(
        selected.settings.unwrap().display.monitor_ids,
        ["display-synthetic-a", "display-synthetic-b"]
    );
    assert_eq!(selected.pauses, paused.pauses);
    let same = request(&namespace, &["--ensure-running"]).1.status.unwrap();
    assert_eq!(same.pid, initial.pid);
    assert_eq!(same.pauses, paused.pauses);
    assert!(!same.pauses.is_empty());
    discover(&namespace);
    let after_discovery = request(&namespace, &["--status", "--json"])
        .1
        .status
        .unwrap();
    assert_eq!(after_discovery.pid, paused.pid);
    assert_eq!(after_discovery.pauses, paused.pauses);
    assert!(!after_discovery.rendering);
    assert_ne!(
        request(&namespace, &["--clear"])
            .1
            .status
            .unwrap()
            .clear_epoch,
        paused.clear_epoch
    );
    assert_eq!(request(&namespace, &["--resume"]).0, OK);
    assert_eq!(request(&namespace, &["--quit"]).0, OK);
    await_exit(&mut host);
    assert_eq!(request(&namespace, &["--resume"]).0, ABSENT);
    let store = snowflakes_client::storage::UserStore::at(host.directory.path());
    let mut prefs = store.load_config().unwrap().config;
    prefs.startup.enabled = true;
    store.save_config(&prefs).unwrap();
    let path = host.directory.path().to_str().unwrap();
    let suppressed = request(
        &namespace,
        &[
            "--ensure-running",
            "--from-logon",
            "--test-headless",
            "--data-dir",
            path,
        ],
    );
    assert_eq!(suppressed.0, OK);
    assert_eq!(suppressed.1.message, "start_suppressed");
    assert_eq!(request(&namespace, &["--status", "--json"]).0, ABSENT);
    let directory = std::mem::replace(&mut host.directory, tempfile::tempdir().unwrap());
    let mut restarted = start(namespace.clone(), directory);
    let saved = request(&namespace, &["--status", "--json"])
        .1
        .status
        .unwrap();
    assert!(!saved.follow_server);
    assert_eq!(saved.settings.unwrap().rain.intensity_percent, 85);
    assert_eq!(request(&namespace, &["--quit"]).0, OK);
    await_exit(&mut restarted);
}
