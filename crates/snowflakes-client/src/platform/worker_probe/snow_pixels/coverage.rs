//! Front-window clipping and APP cloak through actual Native/Weather/GPU frames.
use super::*;

const CHILD: &str =
    "platform::weather::worker_probe::snow_pixels::coverage::private_coverage_pixels_child";

#[derive(Default, serde::Serialize, serde::Deserialize)]
struct Evidence {
    stage: String,
    completed: bool,
    dpi: u32,
    initial_mass: f64,
    initial_pixels: usize,
    initial_right_pixels: usize,
    covered_pixels: usize,
    uncovered_pixels: usize,
    covered_mass: f64,
    mass_after_covered_flake: f64,
    covered_flake_below_roof: f32,
    restored_pixels: usize,
    cloaked_pixels: usize,
    cloaked_mass: f64,
    mass_after_cloaked_flake: f64,
    cloaked_flake_below_roof: f32,
    passing_flake_pixels: usize,
    uncloaked_pixels: usize,
    final_mass: f64,
    native_checkpoints_ms: Vec<u64>,
    same_generation: bool,
    no_collapse: bool,
    error: Option<i32>,
    max_mass_relative_error: f64,
    present_calls: usize,
    accepted_presents: usize,
    occluded_presents: usize,
    other_present_codes: BTreeMap<i32, usize>,
    native_child_exited: bool,
    layers_destroyed: bool,
    #[serde(skip)]
    trace: super::Evidence,
}
impl Evidence {
    fn summarize(&mut self) {
        self.error = self.trace.error;
        self.max_mass_relative_error = self.trace.max_mass_relative_error;
        self.present_calls = self.trace.present_calls;
        self.accepted_presents = self.trace.accepted_presents;
        self.occluded_presents = self.trace.occluded_presents;
        self.other_present_codes
            .clone_from(&self.trace.other_present_codes);
    }
}

fn snapshot(
    weather: &mut Weather,
    settings: &Settings,
    now: Instant,
    evidence: &mut Evidence,
) -> Image {
    snapshot_frame(weather, settings, now, &mut evidence.trace);
    Image::read(weather.layers.values().next().unwrap())
}
fn checkpoint(weather: &Weather, now: Instant, evidence: &mut Evidence) {
    let millis = now
        .duration_since(weather.tracker.origin_for_test())
        .as_millis() as u64;
    assert!(
        millis < 1000,
        "periodic reconciliation must not mask native visibility delivery"
    );
    evidence.native_checkpoints_ms.push(millis);
}
fn transition(host: &mut NativeTestHost, command: &str, reply: &str) {
    guard_if_requested();
    assert_eq!(host.command(command), reply);
    pump();
}
fn cloak(hwnd: HWND) -> u32 {
    let mut value = 0;
    unsafe { DwmGetWindowAttribute(hwnd, DWMWA_CLOAKED, (&mut value as *mut u32).cast(), 4) }
        .unwrap();
    value
}
fn stock_preserved(weather: &Weather, key: u64, mass: f64) {
    assert!((weather.world.drifts[&key].total_mass() - mass).abs() < 0.01);
    assert_eq!(weather.world.pending_collapse_count(), 0);
    assert!(weather.world.flakes.iter().all(|body| body.slab.is_none()));
}
fn passing_flake(
    weather: &mut Weather,
    position: Point,
    settings: &Settings,
    now: &mut Instant,
    evidence: &mut Evidence,
) -> Point {
    let mut flake = Flake::new(position, 3.0);
    flake.age = 1.0; // past the renderer's birth fade, with an explicit initial velocity
    flake.velocity.y = 150.0;
    let before_count = weather.world.flakes.len();
    let before_removed = weather.world.removed_mass;
    assert!(weather.world.release(flake));
    for step in 0..24 {
        *now += crate::timing::STEP;
        if step == 23 {
            snapshot_frame(weather, settings, *now, &mut evidence.trace);
        } else {
            frame(weather, settings, *now, &mut evidence.trace);
        }
    }
    assert_eq!(weather.world.flakes.len(), before_count + 1);
    assert_eq!(weather.world.removed_mass, before_removed);
    let passed = weather
        .world
        .flakes
        .iter()
        .min_by(|a, b| {
            (a.position.x - position.x)
                .abs()
                .total_cmp(&(b.position.x - position.x).abs())
        })
        .unwrap()
        .position;
    assert!((passed.x - position.x).abs() < 5.0);
    passed
}

fn run(evidence: &mut Evidence, desktop: &str) {
    guard_if_requested();
    let _dpi = Dpi::pmv2();
    let mut settings = Settings::default();
    settings.display.selection = MonitorSelection::Primary;
    settings.performance.pause_fullscreen = false;
    settings.interaction.cursor_enabled = false;
    settings.snow.intensity_percent = 0;
    settings.snow.wind_percent = 0;
    settings.snow.melting = false;
    settings.snow.drift_size_percent = 100;
    let mut weather = Weather::new(true).unwrap();
    let base = Instant::now();
    evidence.stage = "empty_native_scene".into();
    frame(&mut weather, &settings, base, &mut evidence.trace);
    assert_eq!(weather.layers.len(), 1);
    let hwnd = weather.layers.values().next().unwrap().hwnd;
    evidence.dpi = unsafe { GetDpiForWindow(hwnd) };
    let mut host = NativeTestHost::new(desktop);
    let created = host.command("native-create");
    let handles: Vec<usize> = created
        .strip_prefix("native-created ")
        .unwrap()
        .split_whitespace()
        .map(|s| s.parse().unwrap())
        .collect();
    assert_eq!(handles.len(), 2);
    pump();
    let target = HWND(handles[0] as *mut _);
    let cover = HWND(handles[1] as *mut _);
    let original = bounds(target, host.id());
    let original_cover = bounds(cover, host.id());
    let mut now = base + Duration::from_millis(30);
    checkpoint(&weather, now, evidence);
    frame(&mut weather, &settings, now, &mut evidence.trace);
    let key = weather
        .world
        .scene
        .windows
        .iter()
        .find(|w| w.rect == original)
        .unwrap()
        .key;
    let cover_key = weather
        .world
        .scene
        .windows
        .iter()
        .find(|w| w.rect == original_cover)
        .unwrap()
        .key;
    assert_ne!(key, cover_key);
    let drift = weather.world.drifts.get_mut(&key).unwrap();
    drift.mass.fill(5.0 * drift.cell_width);
    weather.world.introduced_mass = weather.world.total_mass();
    let mass = weather.world.drifts[&key].total_mass();
    evidence.initial_mass = mass;
    let band = roof_band(&weather.world, key);
    now += Duration::from_millis(30);
    let initial = snapshot(&mut weather, &settings, now, evidence);
    evidence.initial_pixels = initial.count(band);
    assert!(evidence.initial_pixels > 50);

    evidence.stage = "covered".into();
    transition(&mut host, "native-cover", "native-cover-changed");
    now += Duration::from_millis(30);
    checkpoint(&weather, now, evidence);
    let covered = snapshot(&mut weather, &settings, now, evidence);
    let cover_rect = bounds(cover, host.id());
    assert!(cover_rect.top < band.top && cover_rect.bottom > band.bottom);
    let hidden_band = Rect {
        left: cover_rect.left + 4.0,
        ..band
    };
    let exposed_band = Rect {
        right: cover_rect.left - 4.0,
        ..band
    };
    assert!(hidden_band.right > hidden_band.left && exposed_band.right > exposed_band.left);
    assert!(
        weather
            .world
            .scene
            .windows
            .iter()
            .position(|w| w.key == cover_key)
            .unwrap()
            < weather
                .world
                .scene
                .windows
                .iter()
                .position(|w| w.key == key)
                .unwrap()
    );
    assert_eq!(
        weather
            .world
            .scene
            .windows
            .iter()
            .find(|w| w.key == key)
            .unwrap()
            .rect,
        original
    );
    evidence.initial_right_pixels = initial.count(hidden_band);
    evidence.covered_pixels = covered.count(hidden_band);
    evidence.uncovered_pixels = covered.count(exposed_band);
    assert!(evidence.initial_right_pixels > 20 && evidence.uncovered_pixels > 20);
    assert_eq!(evidence.covered_pixels, 0);
    stock_preserved(&weather, key, mass);
    evidence.covered_mass = weather.world.drifts[&key].total_mass();
    let passed = passing_flake(
        &mut weather,
        Point::new(
            (hidden_band.left + hidden_band.right) * 0.5,
            original.top - 20.0,
        ),
        &settings,
        &mut now,
        evidence,
    );
    evidence.covered_flake_below_roof = passed.y - original.top;
    evidence.mass_after_covered_flake = weather.world.drifts[&key].total_mass();
    assert!(passed.y > original.top + 15.0);
    let covered = Image::read(weather.layers.values().next().unwrap());
    assert_eq!(covered.count(hidden_band), 0);
    stock_preserved(&weather, key, mass);
    evidence.mass_after_covered_flake = weather.world.drifts[&key].total_mass();

    evidence.stage = "revealed".into();
    transition(&mut host, "native-uncover", "native-cover-changed");
    now += Duration::from_millis(30);
    checkpoint(&weather, now, evidence);
    let revealed = snapshot(&mut weather, &settings, now, evidence);
    assert_eq!(bounds(cover, host.id()), original_cover);
    evidence.restored_pixels = revealed.count(hidden_band);
    assert!(evidence.restored_pixels > 20);
    stock_preserved(&weather, key, mass);

    evidence.stage = "cloaked".into();
    transition(&mut host, "native-cloak", "native-state-changed");
    now += Duration::from_millis(30);
    checkpoint(&weather, now, evidence);
    let cloaked = snapshot(&mut weather, &settings, now, evidence);
    assert!(cloak(target) & 1 != 0);
    assert!(
        weather
            .world
            .scene
            .windows
            .iter()
            .any(|w| w.key == key && !w.visible && w.collect)
    );
    assert!(weather.world.drifts[&key].support.visible.is_empty());
    evidence.cloaked_pixels = cloaked.count(band);
    assert_eq!(evidence.cloaked_pixels, 0);
    stock_preserved(&weather, key, mass);
    evidence.cloaked_mass = weather.world.drifts[&key].total_mass();
    let passed = passing_flake(
        &mut weather,
        Point::new(
            (exposed_band.left + exposed_band.right) * 0.5,
            original.top - 20.0,
        ),
        &settings,
        &mut now,
        evidence,
    );
    evidence.cloaked_flake_below_roof = passed.y - original.top;
    evidence.mass_after_cloaked_flake = weather.world.drifts[&key].total_mass();
    assert!(passed.y > original.top + 15.0);
    let cloaked = Image::read(weather.layers.values().next().unwrap());
    assert_eq!(cloaked.count(band), 0);
    evidence.passing_flake_pixels = cloaked.count(Rect {
        left: passed.x - 10.0,
        right: passed.x + 10.0,
        top: passed.y - 10.0,
        bottom: passed.y + 10.0,
    });
    assert!(evidence.passing_flake_pixels > 0);
    stock_preserved(&weather, key, mass);
    evidence.mass_after_cloaked_flake = weather.world.drifts[&key].total_mass();

    evidence.stage = "uncloaked".into();
    transition(&mut host, "native-uncloak", "native-state-changed");
    now += Duration::from_millis(30);
    checkpoint(&weather, now, evidence);
    let shown = snapshot(&mut weather, &settings, now, evidence);
    assert_eq!(cloak(target), 0);
    assert!(
        weather
            .world
            .scene
            .windows
            .iter()
            .any(|w| w.key == key && w.visible && w.rect == original)
    );
    evidence.uncloaked_pixels = shown.count(band);
    assert!(evidence.uncloaked_pixels > 50);
    stock_preserved(&weather, key, mass);
    evidence.final_mass = weather.world.drifts[&key].total_mass();
    evidence.same_generation = true;
    evidence.no_collapse = true;
    conserved(&weather.world, &mut evidence.trace);
    evidence.stage = "shutdown".into();
    host.finish();
    evidence.native_child_exited = true;
    drop(weather);
    assert!(!unsafe { IsWindow(Some(hwnd)) }.as_bool());
    evidence.layers_destroyed = true;
    desktop_guard(
        unsafe { GetThreadDesktop(GetCurrentThreadId()) }.unwrap(),
        desktop,
    );
    evidence.stage = "complete".into();
    evidence.completed = true;
}

#[test]
#[ignore = "explicit Native/Weather/GPU overlap and cloak test on a private desktop"]
fn native_overlap_and_cloak_mask_pixels_preserve_stock_and_reject_new_snow() {
    let evidence: Evidence = isolated(CHILD);
    assert!(evidence.completed && evidence.error.is_none());
    assert!(
        evidence.same_generation
            && evidence.no_collapse
            && evidence.native_child_exited
            && evidence.layers_destroyed
    );
    println!(
        "native_coverage_pixels {}",
        serde_json::to_string(&evidence).unwrap()
    );
}

#[test]
#[ignore = "helper launched only on the assigned private startup desktop"]
fn private_coverage_pixels_child() {
    let Ok(expected) = std::env::var(DESKTOP_ENV) else {
        return;
    };
    let path = std::env::var_os(REPORT_ENV).expect("probe report path missing");
    let mut evidence = Evidence::default();
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        run(&mut evidence, &expected)
    }));
    evidence.summarize();
    std::fs::write(path, serde_json::to_vec(&evidence).unwrap()).unwrap();
    if let Err(error) = result {
        std::panic::resume_unwind(error);
    }
}

#[derive(Default, serde::Serialize, serde::Deserialize)]
struct ClosedEvidence {
    stage: String,
    completed: bool,
    mass: f64,
    dpi: u32,
    native_close_ms: u64,
    hidden_bodies: usize,
    cloaked_pixels: usize,
    close_pixels: usize,
    final_pixels: usize,
    ground_mass: f64,
    removed_mass: f64,
    settle_frames: usize,
    error: Option<i32>,
    max_mass_relative_error: f64,
    present_calls: usize,
    accepted_presents: usize,
    occluded_presents: usize,
    native_child_exited: bool,
    layers_destroyed: bool,
    #[serde(skip)]
    trace: super::Evidence,
}
fn closed_run(evidence: &mut ClosedEvidence, desktop: &str) {
    guard_if_requested();
    let _dpi = Dpi::pmv2();
    let mut settings = Settings::default();
    settings.display.selection = MonitorSelection::Primary;
    settings.performance.pause_fullscreen = false;
    settings.interaction.cursor_enabled = false;
    settings.snow.intensity_percent = 0;
    settings.snow.wind_percent = 0;
    settings.snow.melting = false;
    settings.snow.drift_size_percent = 100;
    let mut weather = Weather::new(true).unwrap();
    let base = Instant::now();
    frame(&mut weather, &settings, base, &mut evidence.trace);
    assert_eq!(weather.layers.len(), 1);
    let layer_hwnd = weather.layers.values().next().unwrap().hwnd;
    let screen = weather.layers.values().next().unwrap().screen.clone();
    evidence.dpi = unsafe { GetDpiForWindow(layer_hwnd) };
    let mut host = NativeTestHost::new(desktop);
    let created = host.command("native-create");
    let handle: usize = created
        .strip_prefix("native-created ")
        .unwrap()
        .split_whitespace()
        .next()
        .unwrap()
        .parse()
        .unwrap();
    pump();
    let roof = bounds(HWND(handle as *mut _), host.id());
    let mut now = base + Duration::from_millis(30);
    frame(&mut weather, &settings, now, &mut evidence.trace);
    let key = weather
        .world
        .scene
        .windows
        .iter()
        .find(|w| w.rect == roof)
        .unwrap()
        .key;
    let drift = weather.world.drifts.get_mut(&key).unwrap();
    drift.mass.fill(5.0 * drift.cell_width);
    weather.world.introduced_mass = weather.world.total_mass();
    evidence.mass = weather.world.drifts[&key].total_mass();
    now += Duration::from_millis(30);
    frame(&mut weather, &settings, now, &mut evidence.trace);
    evidence.stage = "cloaked_stock".into();
    transition(&mut host, "native-cloak", "native-state-changed");
    now += Duration::from_millis(30);
    snapshot_frame(&mut weather, &settings, now, &mut evidence.trace);
    assert!(cloak(HWND(handle as *mut _)) & 1 != 0);
    assert!(weather.world.drifts[&key].support.draw_clips.is_empty());
    stock_preserved(&weather, key, evidence.mass);
    evidence.cloaked_pixels =
        Image::read(weather.layers.values().next().unwrap()).count(screen.rect);
    assert_eq!(evidence.cloaked_pixels, 0);

    evidence.stage = "hidden_close".into();
    transition(&mut host, "native-close", "native-closed");
    now += Duration::from_millis(30);
    evidence.native_close_ms = now
        .duration_since(weather.tracker.origin_for_test())
        .as_millis() as u64;
    assert!(evidence.native_close_ms < 1000);
    snapshot_frame(&mut weather, &settings, now, &mut evidence.trace);
    assert!(!weather.world.drifts.contains_key(&key));
    assert!(!unsafe { IsWindow(Some(HWND(handle as *mut _))) }.as_bool());
    assert!(!weather.world.flakes.is_empty());
    for body in &weather.world.flakes {
        assert!(body.slab.is_some());
        assert!(
            weather
                .world
                .falling_clips(body, body.draw_bounds(weather.world.particle_scale(body)))
                .is_some_and(|clips| clips.is_empty())
        );
    }
    evidence.hidden_bodies = weather.world.flakes.len();
    evidence.close_pixels = Image::read(weather.layers.values().next().unwrap()).count(screen.rect);
    assert_eq!(evidence.close_pixels, 0);
    evidence.stage = "hidden_landing".into();
    for step in 0..1800 {
        now += crate::timing::STEP;
        frame(&mut weather, &settings, now, &mut evidence.trace);
        evidence.settle_frames = step + 1;
        if weather.world.flakes.is_empty() && weather.world.pending_collapse_count() == 0 {
            break;
        }
    }
    assert!(weather.world.flakes.is_empty() && weather.world.pending_collapse_count() == 0);
    evidence.ground_mass = weather.world.drifts[&(screen.key | (1 << 63))].total_mass();
    evidence.removed_mass = weather.world.removed_mass;
    assert_eq!(evidence.ground_mass, 0.0);
    assert!((evidence.removed_mass - evidence.mass).abs() < evidence.mass * 1e-5);
    now += crate::timing::STEP;
    snapshot_frame(&mut weather, &settings, now, &mut evidence.trace);
    evidence.final_pixels = Image::read(weather.layers.values().next().unwrap()).count(screen.rect);
    assert_eq!(evidence.final_pixels, 0);
    host.finish();
    evidence.native_child_exited = true;
    drop(weather);
    assert!(!unsafe { IsWindow(Some(layer_hwnd)) }.as_bool());
    evidence.layers_destroyed = true;
    desktop_guard(
        unsafe { GetThreadDesktop(GetCurrentThreadId()) }.unwrap(),
        desktop,
    );
    evidence.stage = "complete".into();
    evidence.completed = true;
}

#[test]
#[ignore = "explicit Native/Weather/GPU hidden-close test on a private desktop"]
fn native_cloaked_window_close_keeps_the_gpu_frame_and_ground_empty() {
    let evidence: ClosedEvidence = isolated(
        "platform::weather::worker_probe::snow_pixels::coverage::private_hidden_close_pixels_child",
    );
    assert!(evidence.completed && evidence.error.is_none());
    assert!(evidence.native_child_exited && evidence.layers_destroyed);
    println!(
        "native_hidden_close_pixels {}",
        serde_json::to_string(&evidence).unwrap()
    );
}

#[test]
#[ignore = "helper launched only on the assigned private startup desktop"]
fn private_hidden_close_pixels_child() {
    let Ok(expected) = std::env::var(DESKTOP_ENV) else {
        return;
    };
    let path = std::env::var_os(REPORT_ENV).expect("probe report path missing");
    let mut evidence = ClosedEvidence::default();
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        closed_run(&mut evidence, &expected)
    }));
    evidence.error = evidence.trace.error;
    evidence.max_mass_relative_error = evidence.trace.max_mass_relative_error;
    evidence.present_calls = evidence.trace.present_calls;
    evidence.accepted_presents = evidence.trace.accepted_presents;
    evidence.occluded_presents = evidence.trace.occluded_presents;
    std::fs::write(path, serde_json::to_vec(&evidence).unwrap()).unwrap();
    if let Err(error) = result {
        std::panic::resume_unwind(error);
    }
}
