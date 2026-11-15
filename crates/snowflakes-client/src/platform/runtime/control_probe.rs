//! Real hidden Panel and Owner control thread; no graphics Worker or Observer.
use super::*;
use snowflakes_protocol::{Display, MonitorSelection, Settings};

#[derive(Default, serde::Serialize, serde::Deserialize)]
pub(in crate::platform) struct Summary {
    pub input: usize,
    pub dispatch: usize,
    pub received: usize,
    pub applied: usize,
    pub superseded: usize,
    pub observed: usize,
    pub submitted: usize,
    pub rejected: usize,
    pub dropped: u64,
    pub unfinished: u64,
    pub panel_destroyed: bool,
    pub control_joined: bool,
}
pub(in crate::platform) struct Applied {
    pub trace_id: u64,
    pub key: u8,
}
pub(in crate::platform) struct Probe {
    owner: Option<Owner>,
    panel: Option<Panel>,
    recorder: Option<Arc<Recorder>>,
    dir: tempfile::TempDir,
    _policy: policy::tests::Fixture,
}
fn guard() {
    super::super::weather::worker_probe::guard_control_probe();
}
impl Probe {
    pub fn new() -> Self {
        guard();
        let dir = tempfile::tempdir().unwrap();
        let mut controller = Controller::open(
            UserStore::at(dir.path()),
            1,
            CacheClock {
                unix_ms: 2_000_000_000_000,
                uptime_ms: 100_000,
            },
            Instant::now(),
        )
        .unwrap();
        assert_eq!(
            controller
                .handle(Command::FollowServer { enabled: false }, Instant::now())
                .code,
            control::OK
        );
        for setting in [
            Setting::SnowIntensity(0),
            Setting::SnowMelting(false),
            Setting::CursorEnabled(false),
            Setting::PauseFullscreen(false),
            Setting::Display(Display {
                selection: MonitorSelection::Primary,
                monitor_ids: Vec::new(),
            }),
        ] {
            assert_eq!(
                controller
                    .handle(Command::Set { setting }, Instant::now())
                    .code,
                control::OK
            );
        }
        assert_eq!(
            controller.handle(Command::Pause {}, Instant::now()).code,
            control::OK
        );
        let recorder = Recorder::open(&dir.path().join("control.ndjson")).unwrap();
        // Existing isolated, absent HKCU fixture: never read or alter machine policy.
        let isolated_policy = policy::tests::Fixture::absent();
        let owner = Owner::start_with_policy(
            controller,
            Limits::default(),
            dir.path().join("exit"),
            false,
            Some(recorder.clone()),
            isolated_policy.source(),
        );
        let panel = Panel::create(
            &owner.view.lock().unwrap(),
            HWND::default(),
            Some(recorder.clone()),
        )
        .unwrap();
        assert!(!unsafe { IsWindowVisible(panel.hwnd()) }.as_bool());
        owner
            .ui_window
            .store(panel.hwnd().0 as isize, Ordering::Release);
        let probe = Self {
            owner: Some(owner),
            panel: Some(panel),
            recorder: Some(recorder),
            dir,
            _policy: isolated_policy,
        };
        probe.check_gates();
        probe
    }
    fn check_gates(&self) {
        let owner = self.owner.as_ref().unwrap();
        let view = owner.view.lock().unwrap();
        assert!(view.status.pauses.contains(&PauseReason::Manual));
        assert!(!owner.conditions.ready());
        assert!(owner.conditions.paused(view.status.settings.as_ref()));
    }
    pub fn render_view(&self) -> Arc<Mutex<View>> {
        self.owner.as_ref().unwrap().view.clone()
    }
    pub fn attach_render_thread(&self, thread: u32) {
        guard();
        assert_ne!(thread, 0);
        assert_ne!(thread, unsafe {
            windows::Win32::System::Threading::GetCurrentThreadId()
        });
        self.owner
            .as_ref()
            .unwrap()
            .render_thread
            .store(thread, Ordering::Release);
    }
    pub fn slide(&self, setting: Setting) {
        self.panel.as_ref().unwrap().probe_slide(setting);
    }
    pub fn change(&self, setting: Setting) {
        match setting {
            Setting::SnowOnWindows(_) | Setting::SnowOnGround(_) => {
                self.panel.as_ref().unwrap().probe_toggle(setting);
            }
            _ => self.slide(setting),
        }
    }
    pub fn settings(&self) -> Settings {
        self.check_gates();
        self.owner
            .as_ref()
            .unwrap()
            .view
            .lock()
            .unwrap()
            .status
            .settings
            .clone()
            .unwrap()
    }
    pub fn apply_next(&self) -> Option<Applied> {
        self.apply_checked(true)
    }
    pub fn reject_next_locked(&self) -> Applied {
        use std::os::windows::fs::OpenOptionsExt;
        guard();
        let path = UserStore::at(self.dir.path()).config_path();
        let before = std::fs::read(&path).unwrap();
        // Hold only this owned temporary config without delete/write sharing.
        // The actual atomic replacement must fail; reads remain allowed.
        let locked = std::fs::OpenOptions::new()
            .read(true)
            .share_mode(1)
            .open(&path)
            .unwrap();
        let result = self
            .apply_checked(false)
            .expect("negative panel command missing");
        assert_eq!(std::fs::read(path).unwrap(), before);
        assert_eq!(self.panel.as_ref().unwrap().probe_value(result.key), 100);
        drop(locked);
        result
    }
    fn apply_checked(&self, save_succeeds: bool) -> Option<Applied> {
        guard();
        let panel = self.panel.as_ref().unwrap();
        let queued = panel.take_request()?;
        let key = crate::ui::command_key(&queued.command);
        let trace = queued.trace.expect("real panel input trace missing");
        let mut message = MSG::default();
        unsafe {
            while PeekMessageW(
                &mut message,
                Some(panel.hwnd()),
                WM_NULL,
                WM_NULL,
                PM_REMOVE,
            )
            .as_bool()
            {}
        }
        let owner = self.owner.as_ref().unwrap();
        let previous_trace = owner.view.lock().unwrap().trace.map(|t| t.id);
        let response = owner
            .submit_traced(queued.command, Some(trace))
            .unwrap()
            .recv_timeout(Duration::from_secs(2))
            .unwrap();
        let view = owner.view.lock().unwrap().clone();
        if save_succeeds {
            assert_eq!(response.code, control::OK);
            assert_eq!(view.trace.unwrap().id, trace.id);
        } else {
            assert_ne!(response.code, control::OK);
            assert_eq!(response.message, "preferences_not_saved");
            assert_eq!(view.trace.map(|t| t.id), previous_trace);
        }
        assert_eq!(
            UserStore::at(self.dir.path())
                .load_config()
                .unwrap()
                .config
                .settings,
            view.status.settings.clone().unwrap()
        );
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            if unsafe {
                PeekMessageW(
                    &mut message,
                    Some(panel.hwnd()),
                    WM_NULL,
                    WM_NULL,
                    PM_REMOVE,
                )
            }
            .as_bool()
            {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "GUI window was not woken after saved publication"
            );
            std::thread::sleep(Duration::from_millis(1));
        }
        panel.complete(&response);
        panel.sync(&view);
        self.check_gates();
        assert!(!unsafe { IsWindowVisible(panel.hwnd()) }.as_bool());
        Some(Applied {
            trace_id: view.trace.unwrap().id,
            key,
        })
    }
    pub fn finish(mut self) -> Summary {
        guard();
        self.check_gates();
        let mut owner = self.owner.take().unwrap();
        owner.stop.store(true, Ordering::Release);
        owner
            .worker
            .take()
            .unwrap()
            .join()
            .expect("real control thread failed");
        drop(owner);
        let panel = self.panel.take().unwrap();
        let hwnd = panel.hwnd();
        drop(panel);
        assert!(!unsafe { IsWindow(Some(hwnd)) }.as_bool());
        drop(self.recorder.take());
        let events: Vec<serde_json::Value> =
            std::fs::read_to_string(self.dir.path().join("control.ndjson"))
                .unwrap()
                .lines()
                .map(|line| serde_json::from_str(line).unwrap())
                .collect();
        let count = |name: &str| events.iter().filter(|e| e["stage"] == name).count();
        let summary = Summary {
            input: count("input"),
            dispatch: count("dispatch"),
            received: count("received"),
            applied: count("applied"),
            superseded: count("superseded"),
            observed: count("observed"),
            submitted: count("submitted"),
            rejected: count("rejected"),
            dropped: events.last().unwrap()["dropped"].as_u64().unwrap(),
            unfinished: events.last().unwrap()["unfinished"].as_u64().unwrap(),
            panel_destroyed: true,
            control_joined: true,
        };
        assert_eq!(summary.observed, 0);
        assert_eq!(summary.submitted, 0);
        summary
    }
}
