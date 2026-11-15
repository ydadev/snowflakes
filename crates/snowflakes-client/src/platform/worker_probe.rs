//! Explicit hardware integration probe. Every worker HWND belongs to a new
//! process whose startup desktop is private and never made the input desktop.
use super::*;
use crate::{
    control::{Command, Controller, Setting},
    storage::{CacheClock, UserStore},
};
use std::{
    ffi::OsStr,
    os::windows::ffi::OsStrExt,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU32, Ordering},
    },
};
use windows::{
    Win32::System::{
        JobObjects::*, RemoteDesktop::ProcessIdToSessionId, StationsAndDesktops::*, Threading::*,
    },
    core::{BOOL, PCWSTR, PWSTR},
};

const DESKTOP_ENV: &str = "SNOWFLAKES_WORKER_PROBE_DESKTOP";
const REPORT_ENV: &str = "SNOWFLAKES_WORKER_PROBE_REPORT";
const PREFIX: &str = "SnowflakesNative-GpuProbe-";
const CHILD: &str = "platform::weather::worker_probe::private_desktop_worker_child";
const ACCESS: u32 = DESKTOP_CREATEWINDOW.0
    | DESKTOP_ENUMERATE.0
    | DESKTOP_HOOKCONTROL.0
    | DESKTOP_READOBJECTS.0
    | DESKTOP_WRITEOBJECTS.0;

#[path = "worker_probe/monitors.rs"]
mod monitors;
#[path = "worker_probe/snow_pixels.rs"]
mod snow_pixels;

thread_local! {
    static LAST_PRESENT: std::cell::Cell<Option<i32>> = const { std::cell::Cell::new(None) };
    static PRESENT_TRACE: std::cell::RefCell<Option<Vec<i32>>> = const { std::cell::RefCell::new(None) };
    static NEXT_PIXELS: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    static FRAME_PIXELS: std::cell::RefCell<Option<PixelBuffer>> = const { std::cell::RefCell::new(None) };
}
struct PixelBuffer {
    width: usize,
    height: usize,
    bytes: Vec<u8>,
}
pub(super) fn capture_before_present(gpu: &Gpu) -> Result<()> {
    if NEXT_PIXELS.replace(false) {
        let size = unsafe { gpu._bitmap.GetPixelSize() };
        let pixels = PixelBuffer {
            width: size.width as usize,
            height: size.height as usize,
            bytes: unsafe { tests::profile_pixels(gpu) }?,
        };
        FRAME_PIXELS.with_borrow_mut(|slot| assert!(slot.replace(pixels).is_none()));
    }
    Ok(())
}
pub(super) fn observe_present(result: windows::core::HRESULT) {
    LAST_PRESENT.set(Some(result.0));
    PRESENT_TRACE.with_borrow_mut(|trace| {
        if let Some(codes) = trace {
            assert!(codes.len() < 64, "test Present capture exceeded its bound");
            codes.push(result.0);
        }
    });
}

fn wide(value: &OsStr) -> Vec<u16> {
    value.encode_wide().chain(Some(0)).collect()
}

fn desktop_guard(desktop: HDESK, expected: &str) {
    assert!(expected.starts_with(PREFIX));
    unsafe {
        let mut name = [0_u16; 256];
        GetUserObjectInformationW(
            HANDLE(desktop.0),
            UOI_NAME,
            Some(name.as_mut_ptr().cast()),
            std::mem::size_of_val(&name) as u32,
            None,
        )
        .unwrap();
        let end = name.iter().position(|&c| c == 0).unwrap();
        assert!(String::from_utf16(&name[..end]).unwrap() == expected);
        let mut input = BOOL(1);
        GetUserObjectInformationW(
            HANDLE(desktop.0),
            UOI_IO,
            Some((&mut input as *mut BOOL).cast()),
            std::mem::size_of_val(&input) as u32,
            None,
        )
        .unwrap();
        assert!(!input.as_bool(), "probe desktop must never receive input");
    }
}

// Called before the real Worker creates its queue, hooks, COM or windows.
// Compiled out of the product and inactive in unrelated library tests.
pub(super) fn guard_if_requested() {
    if let Ok(expected) = std::env::var(DESKTOP_ENV) {
        unsafe {
            let mut startup = STARTUPINFOW::default();
            GetStartupInfoW(&mut startup);
            assert!(!startup.lpDesktop.is_null());
            assert!(startup.lpDesktop.to_string().unwrap() == expected);
            desktop_guard(GetThreadDesktop(GetCurrentThreadId()).unwrap(), &expected);
        }
    }
}

// Required by the hidden-control component bridge, never an optional UI guard.
pub(in crate::platform) fn guard_control_probe() {
    assert!(std::env::var(DESKTOP_ENV).is_ok());
    guard_if_requested();
    assert!(
        unsafe {
            AreDpiAwarenessContextsEqual(
                GetThreadDpiAwarenessContext(),
                DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
            )
        }
        .as_bool()
    );
}

struct Dpi(DPI_AWARENESS_CONTEXT);
impl Dpi {
    fn pmv2() -> Self {
        let previous =
            unsafe { SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) };
        assert!(!previous.0.is_null());
        Self(previous)
    }
}
impl Drop for Dpi {
    fn drop(&mut self) {
        let restored = unsafe { SetThreadDpiAwarenessContext(self.0) };
        if !std::thread::panicking() {
            assert!(!restored.0.is_null());
        }
    }
}

struct Desktop(HDESK);
impl Drop for Desktop {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseDesktop(self.0);
        }
    }
}
struct Child(PROCESS_INFORMATION);
impl Drop for Child {
    fn drop(&mut self) {
        unsafe {
            // Only the process created by this probe; never a PID search.
            if WaitForSingleObject(self.0.hProcess, 0) != WAIT_OBJECT_0 {
                let _ = TerminateProcess(self.0.hProcess, 1);
                let _ = WaitForSingleObject(self.0.hProcess, 5_000);
            }
            let _ = CloseHandle(self.0.hThread);
            let _ = CloseHandle(self.0.hProcess);
        }
    }
}

struct Job(HANDLE);
impl Job {
    fn new() -> Self {
        let job = Self(unsafe { CreateJobObjectW(None, PCWSTR::null()) }.unwrap());
        let limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION {
            BasicLimitInformation: JOBOBJECT_BASIC_LIMIT_INFORMATION {
                LimitFlags: JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
                ..Default::default()
            },
            ..Default::default()
        };
        unsafe {
            SetInformationJobObject(
                job.0,
                JobObjectExtendedLimitInformation,
                (&limits as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
                std::mem::size_of_val(&limits) as u32,
            )
            .unwrap();
        }
        job
    }
}
impl Drop for Job {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}

#[derive(Default, serde::Serialize, serde::Deserialize)]
struct Evidence {
    stage: String,
    probe_completed: bool,
    worker_cycle: String,
    system_pause_verified: bool,
    gpu_layer: String,
    gpu_error: Option<i32>,
    layer_presents: u64,
    layer_present_codes: Vec<i32>,
    nontransparent_pixels: usize,
    layer_dpi: u32,
    lower_layer_destroyed: bool,
    last_renderer: String,
    last_error: Option<i32>,
    last_particles: u32,
    last_monitors: u32,
    last_presents: Vec<u64>,
    system_ready: bool,
    system_pauses: Vec<crate::session::PauseReason>,
    monitors: u32,
    initial_presents: Vec<u64>,
    resumed_presents: Vec<u64>,
    dpi: Vec<u32>,
    paused_particles: u32,
    paused_presents_stable: bool,
    clear_while_paused: bool,
    layers_destroyed: bool,
}

fn isolated<T: serde::de::DeserializeOwned + serde::Serialize>(child_name: &str) -> T {
    let name = format!("{PREFIX}{}-{}", std::process::id(), unsafe {
        GetCurrentThreadId()
    });
    let mut desktop_name = wide(OsStr::new(&name));
    let desktop = Desktop(unsafe {
        CreateDesktopW(
            PCWSTR(desktop_name.as_ptr()),
            PCWSTR::null(),
            None,
            Default::default(),
            ACCESS,
            None,
        )
        .unwrap()
    });
    desktop_guard(desktop.0, &name);
    let temporary = tempfile::tempdir().unwrap();
    let report_path = temporary.path().join("worker.json");
    // Pass a private environment block without changing this process's globals.
    let mut environment: Vec<_> = std::env::vars_os()
        .filter(|(key, _)| {
            !key.eq_ignore_ascii_case(DESKTOP_ENV) && !key.eq_ignore_ascii_case(REPORT_ENV)
        })
        .collect();
    environment.push((DESKTOP_ENV.into(), name.clone().into()));
    environment.push((REPORT_ENV.into(), report_path.as_os_str().to_owned()));
    environment.sort_by_key(|(key, _)| key.to_string_lossy().to_uppercase());
    let mut block = Vec::new();
    for (key, value) in environment {
        block.extend(key.encode_wide());
        block.push(b'=' as u16);
        block.extend(value.encode_wide());
        block.push(0);
    }
    block.push(0);
    let executable = std::env::current_exe().unwrap();
    let application = wide(executable.as_os_str());
    let mut command = wide(OsStr::new(&format!(
        "\"{}\" --exact {child_name} --ignored --nocapture --test-threads=1",
        executable.display()
    )));
    let startup = STARTUPINFOW {
        cb: std::mem::size_of::<STARTUPINFOW>() as u32,
        lpDesktop: PWSTR(desktop_name.as_mut_ptr()),
        dwFlags: STARTF_FORCEOFFFEEDBACK,
        ..Default::default()
    };
    let mut info = PROCESS_INFORMATION::default();
    let job = Job::new();
    unsafe {
        CreateProcessW(
            PCWSTR(application.as_ptr()),
            Some(PWSTR(command.as_mut_ptr())),
            None,
            None,
            false,
            CREATE_NO_WINDOW | CREATE_UNICODE_ENVIRONMENT | CREATE_SUSPENDED,
            Some(block.as_ptr().cast()),
            PCWSTR::null(),
            &startup,
            &mut info,
        )
        .unwrap();
    }
    let child = Child(info);
    unsafe {
        AssignProcessToJobObject(job.0, child.0.hProcess).unwrap();
        let mut member = BOOL(0);
        IsProcessInJob(child.0.hProcess, Some(job.0), &mut member).unwrap();
        assert!(member.as_bool());
        assert_eq!(ResumeThread(child.0.hThread), 1);
    }
    assert_eq!(
        unsafe { WaitForSingleObject(child.0.hProcess, 30_000) },
        WAIT_OBJECT_0
    );
    let mut code = u32::MAX;
    unsafe { GetExitCodeProcess(child.0.hProcess, &mut code) }.unwrap();
    let mut accounting = JOBOBJECT_BASIC_ACCOUNTING_INFORMATION::default();
    let cleanup_deadline = Instant::now() + Duration::from_secs(2);
    loop {
        unsafe {
            QueryInformationJobObject(
                Some(job.0),
                JobObjectBasicAccountingInformation,
                (&mut accounting as *mut JOBOBJECT_BASIC_ACCOUNTING_INFORMATION).cast(),
                std::mem::size_of_val(&accounting) as u32,
                None,
            )
            .unwrap();
        }
        if accounting.ActiveProcesses == 0 || Instant::now() >= cleanup_deadline {
            break;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(
        accounting.ActiveProcesses, 0,
        "fixture descendant still active"
    );
    let evidence: T = serde_json::from_slice(&std::fs::read(report_path).unwrap()).unwrap();
    assert_eq!(
        code,
        0,
        "probe failed: {}",
        serde_json::to_string(&evidence).unwrap()
    );
    desktop_guard(desktop.0, &name);
    evidence
}

#[test]
#[ignore = "explicit hardware/DWM probe on a private non-input startup desktop"]
fn native_private_desktop_worker_and_layer_capability_probe() {
    let evidence: Evidence = isolated(CHILD);
    assert!(evidence.probe_completed);
    assert!(evidence.worker_cycle == "passed" || evidence.system_pause_verified);
    assert!(matches!(
        evidence.gpu_layer.as_str(),
        "passed" | "rendered_present_status" | "not_run_resource_creation"
    ));
    println!(
        "private_worker_probe {}",
        serde_json::to_string(&evidence).unwrap()
    );
}

fn counts(report: &Report) -> Vec<u64> {
    report
        .frame_metrics
        .as_ref()
        .map_or_else(Vec::new, |snapshot| {
            assert!(!snapshot.display_latency_measured);
            assert_eq!(snapshot.omitted_displays, 0);
            snapshot
                .displays
                .iter()
                .map(|d| d.counters.presents)
                .collect()
        })
}

fn wait(
    report: &Mutex<Report>,
    evidence: &mut Evidence,
    conditions: &Conditions,
    predicate: impl Fn(&Report) -> bool,
) -> Report {
    let deadline = Instant::now() + Duration::from_secs(8);
    loop {
        let snapshot = report.lock().unwrap().clone();
        evidence.last_renderer = snapshot.renderer.into();
        evidence.last_error = snapshot.graphics_error;
        evidence.last_particles = snapshot.particles;
        evidence.last_monitors = snapshot.monitors;
        evidence.last_presents = counts(&snapshot);
        evidence.system_ready = conditions.ready();
        let mut session = crate::session::Session::new(Settings::default()).unwrap();
        conditions.apply_to(&mut session);
        evidence.system_pauses = session
            .snapshot(Instant::now())
            .pauses
            .into_iter()
            .collect();
        assert_eq!(snapshot.graphics_error, None);
        if predicate(&snapshot) {
            return snapshot;
        }
        assert!(
            Instant::now() < deadline,
            "worker wait expired: {}",
            snapshot.renderer
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

unsafe extern "system" fn collect(hwnd: HWND, data: LPARAM) -> BOOL {
    // EnumThreadWindows is synchronous; the owned Vec outlives the callback.
    let windows = unsafe { &mut *(data.0 as *mut Vec<HWND>) };
    let mut name = [0_u16; 128];
    let len = unsafe { GetClassNameW(hwnd, &mut name) };
    if String::from_utf16_lossy(&name[..len as usize]) == "SnowflakesWeatherLayer" {
        windows.push(hwnd);
    }
    BOOL(1)
}

fn layers(thread: u32, expected: &str) -> Vec<HWND> {
    desktop_guard(unsafe { GetThreadDesktop(thread) }.unwrap(), expected);
    let mut windows = Vec::new();
    assert!(
        unsafe {
            EnumThreadWindows(
                thread,
                Some(collect),
                LPARAM((&mut windows as *mut Vec<HWND>) as isize),
            )
        }
        .as_bool()
    );
    for &hwnd in &windows {
        unsafe {
            let mut pid = 0;
            assert_eq!(GetWindowThreadProcessId(hwnd, Some(&mut pid)), thread);
            assert_eq!(pid, std::process::id());
            assert!(
                AreDpiAwarenessContextsEqual(
                    GetWindowDpiAwarenessContext(hwnd),
                    DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2
                )
                .as_bool()
            );
            assert_eq!(
                GetWindowLongPtrW(hwnd, GWL_EXSTYLE) as u32 & LAYER_STYLE.0,
                LAYER_STYLE.0
            );
            assert!(GetDpiForWindow(hwnd) > 0);
        }
    }
    windows
}

fn lower_layer(evidence: &mut Evidence, expected: &str) -> Result<()> {
    guard_if_requested();
    let _dpi = Dpi::pmv2();
    let _weather = Weather::new(false)?; // register the real class, without driving its loop
    let screen = Screen {
        key: 1,
        persistent_id: false,
        rect: Rect {
            left: 0.0,
            top: 0.0,
            right: 256.0,
            bottom: 256.0,
        },
        floor: 240.0,
        scale: 1.0,
    };
    let mut layer = Layer::new(screen, 1, true)?;
    evidence.gpu_layer = "created".into();
    let hwnd = layer.hwnd;
    assert!(!unsafe { IsWindowVisible(hwnd) }.as_bool());
    assert_eq!(
        layers(unsafe { GetCurrentThreadId() }, expected),
        vec![hwnd]
    );
    let dpi = unsafe { GetDpiForWindow(hwnd) };
    assert_eq!(layer.screen.scale, (dpi as f32 / 96.0).max(0.5));
    evidence.layer_dpi = dpi;
    let scene = Scene {
        screens: vec![layer.screen.clone()],
        windows: vec![],
    };
    let mut world = World::new(scene.clone(), 85);
    let mut flake = Flake::new(Point::new(128.0, 100.0), 10.0);
    flake.age = 1.0; // past the real renderer's 0.7-second birth fade
    assert!(world.release(flake));
    let rain = rain::World::new(scene, 86);
    let settings = Settings::default();
    let quality = RenderQuality::for_settings(&settings, false);
    let gpu = layer.gpu.as_ref().unwrap();
    gpu.render_snow(&layer.screen, &world, &settings, quality, 1.0)?;
    let pixels = unsafe { tests::profile_pixels(gpu) }?;
    let occupied = pixels
        .as_chunks::<4>()
        .0
        .iter()
        .filter(|pixel| pixel[3] > 0)
        .count();
    evidence.nontransparent_pixels = occupied;
    assert!(occupied > 20 && occupied < 256 * 256 / 2);
    let origin = Instant::now();
    for _ in 0..4 {
        let accepted = layer.draw(&world, &rain, &settings, quality, origin, 1.0)?;
        let code = LAST_PRESENT.get().expect("real Present code missing");
        assert_eq!(accepted, code == 0);
        evidence.layer_present_codes.push(code);
        evidence.layer_presents += u64::from(accepted);
        layer.show(true);
        assert!(unsafe { IsWindowVisible(hwnd) }.as_bool());
    }
    assert_eq!(
        layer.meter.as_ref().unwrap().counters.presents,
        evidence.layer_presents
    );
    layer.show(false);
    assert!(!unsafe { IsWindowVisible(hwnd) }.as_bool());
    drop(layer);
    assert!(!unsafe { IsWindow(Some(hwnd)) }.as_bool());
    evidence.lower_layer_destroyed = true;
    evidence.gpu_layer = if evidence.layer_presents == 4 {
        "passed"
    } else {
        "rendered_present_status"
    }
    .into();
    Ok(())
}

fn finish(evidence: &mut Evidence, expected: &str) {
    evidence.stage = "lower_layer_resources_and_present".into();
    if let Err(error) = lower_layer(evidence, expected) {
        evidence.gpu_error = Some(error.code().0);
        // Only initial native resource capability can be unavailable. Once
        // created, an actual render/readback/Present error fails the probe.
        assert!(
            evidence.gpu_layer.is_empty(),
            "GPU failed after resource creation: {:?}",
            error.code()
        );
        evidence.gpu_layer = "not_run_resource_creation".into();
    }
    evidence.stage = "complete".into();
    evidence.probe_completed = true;
}

fn run(evidence: &mut Evidence, expected: &str) {
    guard_if_requested();
    let directory = tempfile::tempdir().unwrap();
    let now = Instant::now();
    let mut session_id = 0;
    unsafe { ProcessIdToSessionId(std::process::id(), &mut session_id) }.unwrap();
    let mut controller = Controller::open(
        UserStore::at(directory.path()),
        session_id,
        CacheClock {
            unix_ms: 2_000_000_000_000,
            uptime_ms: 100_000,
        },
        now,
    )
    .unwrap();
    assert_eq!(
        controller
            .handle(Command::FollowServer { enabled: false }, now)
            .code,
        0
    );
    for setting in [
        Setting::Mode(WeatherMode::Snow),
        Setting::EffectsEnabled(true),
        Setting::PauseFullscreen(false),
        Setting::PauseRemoteSession(false),
        Setting::CursorEnabled(false),
    ] {
        assert_eq!(controller.handle(Command::Set { setting }, now).code, 0);
    }
    let view = Arc::new(Mutex::new(controller.ui_view(now)));
    let report = Arc::new(Mutex::new(Report::default()));
    let conditions = Arc::new(Conditions::default());
    let wake = Arc::new(AtomicU32::new(0));
    evidence.stage = "initial_present".into();
    let worker = Worker::start(
        view.clone(),
        report.clone(),
        conditions.clone(),
        session_id,
        wake.clone(),
        None,
        true,
    );
    assert!(view.lock().unwrap().status.desired_effects_enabled);
    assert!(view.lock().unwrap().status.pauses.is_empty());
    let settings = view.lock().unwrap().status.settings.clone().unwrap();
    let active = wait(&report, evidence, &conditions, |r| {
        (r.rendering
            && r.particles > 0
            && !counts(r).is_empty()
            && counts(r).iter().all(|&n| n >= 4))
            || (conditions.ready()
                && conditions.paused(Some(&settings))
                && r.renderer == "d3d11_direct2d_idle")
    });
    let thread = wake.load(Ordering::Acquire);
    assert_ne!(thread, 0);
    let handles = layers(thread, expected);
    if !active.rendering {
        assert!(conditions.paused(Some(&settings)) && conditions.ready());
        assert!(handles.is_empty() && counts(&active).is_empty() && active.particles == 0);
        std::thread::sleep(Duration::from_millis(250));
        let held = report.lock().unwrap().clone();
        assert!(!held.rendering && !held.submitted && held.graphics_error.is_none());
        assert!(counts(&held).is_empty() && held.particles == 0);
        assert!(layers(thread, expected).is_empty());
        evidence.system_pause_verified = true;
        evidence.worker_cycle = "not_run_system_pause".into();
        drop(worker);
        assert_eq!(wake.load(Ordering::Acquire), 0);
        assert!(!conditions.ready());
        finish(evidence, expected);
        return;
    }
    assert_eq!(handles.len(), active.monitors as usize);
    assert!(
        handles
            .iter()
            .all(|&h| unsafe { IsWindowVisible(h) }.as_bool())
    );
    assert!(conditions.ready());
    evidence.monitors = active.monitors;
    evidence.initial_presents = counts(&active);
    evidence.dpi = handles
        .iter()
        .map(|&h| unsafe { GetDpiForWindow(h) })
        .collect();
    let publish = |controller: &Controller| {
        *view.lock().unwrap() = controller.ui_view(Instant::now());
        unsafe { PostThreadMessageW(thread, WM_NULL, WPARAM(0), LPARAM(0)) }.unwrap();
    };
    evidence.stage = "manual_pause".into();
    assert_eq!(controller.handle(Command::Pause {}, Instant::now()).code, 0);
    publish(&controller);
    let paused = wait(&report, evidence, &conditions, |r| {
        !r.rendering && !r.submitted && r.monitors == 0
    });
    assert!(
        handles
            .iter()
            .all(|&h| !unsafe { IsWindowVisible(h) }.as_bool())
    );
    std::thread::sleep(Duration::from_millis(250));
    let held = report.lock().unwrap().clone();
    assert!(!held.rendering && !held.submitted);
    assert_eq!(counts(&held), counts(&paused));
    assert_eq!(held.particles, paused.particles);
    evidence.paused_particles = held.particles;
    evidence.paused_presents_stable = true;
    evidence.stage = "clear_while_paused".into();
    assert_eq!(controller.handle(Command::Clear {}, Instant::now()).code, 0);
    publish(&controller);
    wait(&report, evidence, &conditions, |r| {
        !r.rendering && r.particles == 0
    });
    evidence.clear_while_paused = true;
    evidence.stage = "resume".into();
    assert_eq!(
        controller.handle(Command::Resume {}, Instant::now()).code,
        0
    );
    publish(&controller);
    let resumed = wait(&report, evidence, &conditions, |r| {
        r.rendering
            && r.particles > 0
            && counts(r).len() == counts(&paused).len()
            && counts(r)
                .iter()
                .zip(counts(&paused))
                .all(|(&n, old)| n >= old + 4)
    });
    assert_eq!(layers(thread, expected), handles);
    evidence.resumed_presents = counts(&resumed);
    evidence.stage = "shutdown".into();
    drop(worker);
    assert_eq!(wake.load(Ordering::Acquire), 0);
    assert!(
        handles
            .iter()
            .all(|&h| !unsafe { IsWindow(Some(h)) }.as_bool())
    );
    assert!(!conditions.ready());
    evidence.layers_destroyed = true;
    evidence.worker_cycle = "passed".into();
    finish(evidence, expected);
}

#[test]
#[ignore = "helper launched only by the private startup-desktop parent"]
fn private_desktop_worker_child() {
    let Ok(expected) = std::env::var(DESKTOP_ENV) else {
        return;
    };
    let path = std::env::var_os(REPORT_ENV).expect("probe report path missing");
    let mut evidence = Evidence::default();
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        run(&mut evidence, &expected)
    }));
    std::fs::write(path, serde_json::to_vec(&evidence).unwrap()).unwrap();
    if let Err(error) = outcome {
        std::panic::resume_unwind(error);
    }
}
