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
    let child = Command::new(env!("CARGO_BIN_EXE_snowflakes"))
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
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
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
    for value in ["target_fps=30", "battery_saver=false"] {
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
    let preferences = quality.settings.unwrap().performance;
    assert_eq!(preferences.target_fps, 30);
    assert!(!preferences.battery_saver);
    assert_eq!(request(&namespace, &["--set", "target_fps=45"]).0, INVALID);
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
