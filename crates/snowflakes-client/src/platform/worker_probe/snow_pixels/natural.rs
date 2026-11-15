//! Natural production emission and roof accumulation, without planted snow.
use super::*;

const CHILD: &str =
    "platform::weather::worker_probe::snow_pixels::natural::private_natural_snow_child";
const ON_LIMIT: usize = 1800;
const SETTLE_LIMIT: usize = 7200;

#[derive(Default, serde::Serialize, serde::Deserialize)]
struct Evidence {
    stage: String,
    completed: bool,
    dpi: u32,
    native_create_ms: u64,
    empty_mass: f64,
    empty_pixels: usize,
    first_birth_frame: usize,
    first_roof_frame: usize,
    on_frames: usize,
    introduced_on: f64,
    roof_mass_on: f64,
    airborne_on: usize,
    min_radius_on: f32,
    max_radius_on: f32,
    pixels_on: usize,
    introduced_at_zero: f64,
    roof_mass_at_zero: f64,
    roof_mass_first_zero_frame: f64,
    zero_frames: usize,
    introduced_after_zero: f64,
    roof_mass_after_zero: f64,
    settle_frames: usize,
    introduced_final: f64,
    roof_mass_final: f64,
    ground_mass_final: f64,
    removed_mass_final: f64,
    roof_peak_final: f32,
    pixels_final: usize,
    same_support_and_layer: bool,
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
impl Evidence {
    fn summarize(&mut self) {
        self.error = self.trace.error;
        self.max_mass_relative_error = self.trace.max_mass_relative_error;
        self.present_calls = self.trace.present_calls;
        self.accepted_presents = self.trace.accepted_presents;
        self.occluded_presents = self.trace.occluded_presents;
    }
}
fn roof_mass(weather: &Weather, key: u64) -> f64 {
    weather.world.drifts[&key].total_mass()
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
    assert!(weather.seeded);
    assert_eq!(weather.layers.len(), 1);
    assert!(weather.world.flakes.is_empty());
    assert_eq!(weather.world.introduced_mass, 0.0);
    assert_eq!(weather.world.total_mass(), 0.0);
    let screen_key = *weather.layers.keys().next().unwrap();
    let hwnd = weather.layers[&screen_key].hwnd;
    let epoch = weather.layers[&screen_key].metrics_epoch;
    evidence.dpi = unsafe { GetDpiForWindow(hwnd) };

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
    let rect = bounds(HWND(handle as *mut _), host.id());
    let mut now = base + Duration::from_millis(30);
    evidence.native_create_ms = now
        .duration_since(weather.tracker.origin_for_test())
        .as_millis() as u64;
    assert!(evidence.native_create_ms < 1000);
    frame(&mut weather, &settings, now, &mut evidence.trace);
    let key = weather
        .world
        .scene
        .windows
        .iter()
        .find(|w| w.rect == rect && w.collect && w.visible)
        .unwrap()
        .key;
    assert_eq!(roof_mass(&weather, key), 0.0);
    now += Duration::from_millis(30);
    snapshot_frame(&mut weather, &settings, now, &mut evidence.trace);
    let band = roof_band(&weather.world, key);
    evidence.empty_mass = weather.world.total_mass();
    evidence.empty_pixels = Image::read(&weather.layers[&screen_key]).count(band);
    assert_eq!(evidence.empty_mass, 0.0);
    assert_eq!(evidence.empty_pixels, 0);

    // All following material must enter through the production emitter. There
    // is no World::release, seed_air call, drift write or ledger assignment here.
    evidence.stage = "natural_snowfall".into();
    settings.snow.intensity_percent = 100;
    for index in 0..ON_LIMIT {
        now += crate::timing::STEP;
        frame(&mut weather, &settings, now, &mut evidence.trace);
        evidence.on_frames = index + 1;
        if weather.world.introduced_mass > 0.0 && evidence.first_birth_frame == 0 {
            evidence.first_birth_frame = index + 1;
        }
        if roof_mass(&weather, key) > 0.0 && evidence.first_roof_frame == 0 {
            evidence.first_roof_frame = index + 1;
        }
        if roof_mass(&weather, key) >= 120.0 {
            break;
        }
    }
    evidence.introduced_on = weather.world.introduced_mass;
    evidence.roof_mass_on = roof_mass(&weather, key);
    evidence.airborne_on = weather.world.flakes.len();
    evidence.min_radius_on = weather
        .world
        .flakes
        .iter()
        .map(|f| f.radius)
        .fold(f32::MAX, f32::min);
    evidence.max_radius_on = weather
        .world
        .flakes
        .iter()
        .map(|f| f.radius)
        .fold(0.0, f32::max);
    assert!(
        evidence.first_birth_frame > 0 && evidence.first_roof_frame > evidence.first_birth_frame
    );
    assert!(evidence.roof_mass_on >= 120.0 && evidence.airborne_on > 0);
    assert!(evidence.min_radius_on > 0.0 && evidence.max_radius_on > evidence.min_radius_on * 1.2);
    now += crate::timing::STEP;
    snapshot_frame(&mut weather, &settings, now, &mut evidence.trace);
    evidence.pixels_on = Image::read(&weather.layers[&screen_key]).count(band);
    assert!(evidence.pixels_on > 0);

    evidence.stage = "zero_intensity".into();
    settings.snow.intensity_percent = 0;
    let introduced = weather.world.introduced_mass;
    evidence.introduced_at_zero = introduced;
    evidence.roof_mass_at_zero = roof_mass(&weather, key);
    for _ in 0..120 {
        now += crate::timing::STEP;
        frame(&mut weather, &settings, now, &mut evidence.trace);
        assert_eq!(weather.world.introduced_mass, introduced);
        if evidence.zero_frames == 0 {
            evidence.roof_mass_first_zero_frame = roof_mass(&weather, key);
            assert!(evidence.roof_mass_first_zero_frame + 0.01 >= evidence.roof_mass_at_zero);
        }
        evidence.zero_frames += 1;
    }
    evidence.introduced_after_zero = weather.world.introduced_mass;
    evidence.roof_mass_after_zero = roof_mass(&weather, key);
    assert!(evidence.roof_mass_after_zero > 0.0);

    evidence.stage = "remaining_material_lands".into();
    for index in 0..SETTLE_LIMIT {
        now += crate::timing::STEP;
        frame(&mut weather, &settings, now, &mut evidence.trace);
        assert_eq!(weather.world.introduced_mass, introduced);
        evidence.settle_frames = index + 1;
        if weather.world.flakes.is_empty() && weather.world.pending_collapse_count() == 0 {
            break;
        }
    }
    assert!(weather.world.flakes.is_empty() && weather.world.pending_collapse_count() == 0);
    evidence.introduced_final = weather.world.introduced_mass;
    evidence.roof_mass_final = roof_mass(&weather, key);
    evidence.ground_mass_final = weather.world.drifts[&(screen_key | (1 << 63))].total_mass();
    evidence.removed_mass_final = weather.world.removed_mass;
    let roof = &weather.world.drifts[&key];
    evidence.roof_peak_final = (0..roof.mass.len())
        .map(|i| roof.height(i, &weather.world.scene))
        .fold(0.0, f32::max);
    assert!(
        evidence.roof_mass_final > 0.0
            && evidence.ground_mass_final > 0.0
            && evidence.roof_peak_final > 0.5
    );
    now += crate::timing::STEP;
    snapshot_frame(&mut weather, &settings, now, &mut evidence.trace);
    assert!(weather.world.flakes.is_empty() && weather.world.pending_collapse_count() == 0);
    assert_eq!(weather.world.introduced_mass, introduced);
    evidence.pixels_final = Image::read(&weather.layers[&screen_key]).count(band);
    assert!(evidence.pixels_final > 20);
    assert_eq!(
        weather
            .world
            .scene
            .windows
            .iter()
            .find(|w| w.rect == rect)
            .unwrap()
            .key,
        key
    );
    assert_eq!(weather.layers[&screen_key].hwnd, hwnd);
    assert_eq!(weather.layers[&screen_key].metrics_epoch, epoch);
    evidence.same_support_and_layer = true;
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
#[ignore = "explicit natural snowfall Native/Weather/GPU test on a private desktop"]
fn natural_snowfall_fills_native_roof_and_zero_intensity_stops_new_material() {
    let evidence: Evidence = isolated(CHILD);
    assert!(evidence.completed && evidence.error.is_none());
    assert!(
        evidence.same_support_and_layer
            && evidence.native_child_exited
            && evidence.layers_destroyed
    );
    println!(
        "native_natural_snow {}",
        serde_json::to_string(&evidence).unwrap()
    );
}

#[test]
#[ignore = "helper launched only on the assigned private startup desktop"]
fn private_natural_snow_child() {
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
