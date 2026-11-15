//! Independent birth counting through actual Native/Weather/World/GPU updates.
use super::*;

const CHILD_60: &str =
    "platform::weather::worker_probe::snow_pixels::emission::private_emission_60_child";
const CHILD_30: &str =
    "platform::weather::worker_probe::snow_pixels::emission::private_emission_30_child";
const PHASE_STEPS: usize = 180;

#[derive(Default, serde::Serialize, serde::Deserialize)]
struct Sample {
    intensity: u8,
    model_steps: usize,
    update_frames: usize,
    births: usize,
    expected_births: f64,
    ordinary_before: usize,
    ordinary_after: usize,
    ordinary_departures: usize,
    pixels: usize,
}
#[derive(Default, serde::Serialize, serde::Deserialize)]
struct Evidence {
    stage: String,
    completed: bool,
    cadence: u32,
    dpi: u32,
    logical_width: f32,
    samples: Vec<Sample>,
    peak_active: usize,
    active_limit: usize,
    max_birth_mass_relative_error: f64,
    roof_mass_before_zero: f64,
    zero_births: usize,
    zero_steps: usize,
    ordinary_after_zero: usize,
    introduced_at_zero: f64,
    introduced_after_zero: f64,
    zero_frame_changed: bool,
    same_layer: bool,
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
fn ordinary(weather: &Weather) -> usize {
    weather
        .world
        .flakes
        .iter()
        .filter(|b| b.slab.is_none() && b.skip_support.is_none())
        .count()
}
fn births(weather: &Weather, steps: u32, introduced_before: f64, evidence: &mut Evidence) -> usize {
    let screen = weather.layers.values().next().unwrap().screen.clone();
    let max_age = crate::snow::DT * steps as f32 + 1e-7;
    let mut count = 0;
    let mut mass = 0.0;
    for b in
        weather.world.flakes.iter().filter(|b| {
            b.slab.is_none() && b.skip_support.is_none() && b.age > 0.0 && b.age <= max_age
        })
    {
        // These are new ordinary bodies near the actual emission plane, not
        // detached material or old particles disappearing from the live pool.
        assert!(b.position.y > screen.rect.top - 32.0 * screen.scale);
        assert!(b.position.y < screen.rect.top + 8.0 * screen.scale);
        count += 1;
        mass += f64::from(b.mass);
    }
    let added = weather.world.introduced_mass - introduced_before;
    let relative = (added - mass).abs() / added.max(1.0);
    evidence.max_birth_mass_relative_error = evidence.max_birth_mass_relative_error.max(relative);
    assert!(
        relative < 1e-7,
        "birth counter missed or duplicated introduced material"
    );
    let budget = weather.world.budget_snapshot();
    evidence.peak_active = evidence.peak_active.max(budget.active_bodies);
    evidence.active_limit = budget.max_active_bodies;
    assert!(budget.active_bodies < budget.max_active_bodies - crate::snow::BASE_COLLAPSE_BUDGET);
    assert!(
        budget.pending_bodies <= budget.max_pending_bodies
            && budget.pending_packets <= budget.max_pending_packets
    );
    count
}

fn run(evidence: &mut Evidence, desktop: &str, cadence: u32) {
    guard_if_requested();
    let _dpi = Dpi::pmv2();
    evidence.cadence = cadence;
    assert!(cadence == 30 || cadence == 60);
    let steps = 60 / cadence;
    let mut settings = Settings::default();
    settings.display.selection = MonitorSelection::Primary;
    settings.performance.pause_fullscreen = false;
    settings.performance.target_fps = cadence as u8;
    settings.interaction.cursor_enabled = false;
    settings.snow.intensity_percent = 0;
    settings.snow.wind_percent = 0;
    settings.snow.melting = false;
    let mut weather = Weather::new(true).unwrap();
    let base = Instant::now();
    evidence.stage = "empty_scene".into();
    frame(&mut weather, &settings, base, &mut evidence.trace);
    assert!(weather.seeded && weather.world.flakes.is_empty());
    assert_eq!(weather.world.total_mass(), 0.0);
    assert_eq!(weather.world.introduced_mass, 0.0);
    assert_eq!(weather.layers.len(), 1);
    let screen_key = *weather.layers.keys().next().unwrap();
    let hwnd = weather.layers[&screen_key].hwnd;
    let epoch = weather.layers[&screen_key].metrics_epoch;
    evidence.dpi = unsafe { GetDpiForWindow(hwnd) };
    let screen = weather.layers[&screen_key].screen.clone();
    assert_eq!(screen.scale, evidence.dpi as f32 / 96.0);
    assert!(screen.rect.height() > 480.0 * screen.scale);
    evidence.logical_width = screen.rect.width() / screen.scale;
    assert_eq!(weather.world.scene.screens.len(), 1);
    assert_eq!(weather.world.scene.screens[0].key, screen.key);
    assert_eq!(weather.world.scene.screens[0].rect, screen.rect);
    assert_eq!(weather.world.scene.screens[0].scale, screen.scale);
    let mut host = NativeTestHost::new(desktop);
    let created = host.command("native-create");
    let handles: Vec<usize> = created
        .strip_prefix("native-created ")
        .unwrap()
        .split_whitespace()
        .map(|v| v.parse().unwrap())
        .collect();
    assert_eq!(handles.len(), 2);
    pump();
    let rects: Vec<_> = handles
        .iter()
        .map(|h| bounds(HWND(*h as *mut _), host.id()))
        .collect();
    let mut now = base + Duration::from_millis(30);
    frame(&mut weather, &settings, now, &mut evidence.trace);
    for rect in &rects {
        assert!(
            weather
                .world
                .scene
                .windows
                .iter()
                .any(|w| w.rect == *rect && w.collect && w.visible)
        );
    }
    now += crate::timing::DISCONTINUITY;
    frame(&mut weather, &settings, now, &mut evidence.trace);
    assert_eq!(ordinary(&weather), 0);
    assert_eq!(weather.world.introduced_mass, 0.0);
    let mut last_image = None;
    evidence.stage = "intensity_phases".into();
    for intensity in [25, 50, 100] {
        settings.snow.intensity_percent = intensity;
        let frames = PHASE_STEPS / steps as usize;
        let mut sample = Sample {
            intensity,
            model_steps: PHASE_STEPS,
            update_frames: frames,
            ordinary_before: ordinary(&weather),
            // Regression calibration: 65 births/s at 100% per 1920 DIP.
            // The independent observable is summed new-body ages and mass,
            // never net live-pool growth or a read of World's emission phase.
            expected_births: 65.0 * f64::from(evidence.logical_width) / 1920.0
                * f64::from(intensity)
                / 100.0
                * PHASE_STEPS as f64
                / 60.0,
            ..Default::default()
        };
        for index in 0..frames {
            let introduced = weather.world.introduced_mass;
            now += crate::timing::STEP * steps;
            if index + 1 == frames {
                snapshot_frame(&mut weather, &settings, now, &mut evidence.trace);
                let image = Image::read(&weather.layers[&screen_key]);
                sample.pixels = image.count(screen.rect);
                assert!(sample.pixels > 20);
                last_image = Some(image);
            } else {
                frame(&mut weather, &settings, now, &mut evidence.trace);
            }
            sample.births += births(&weather, steps, introduced, evidence);
        }
        sample.ordinary_after = ordinary(&weather);
        assert!(sample.ordinary_after <= sample.ordinary_before + sample.births);
        sample.ordinary_departures = sample.ordinary_before + sample.births - sample.ordinary_after;
        assert!((sample.births as f64 - sample.expected_births).abs() <= 1.01);
        evidence.samples.push(sample);
    }
    assert!(evidence.samples[1].births > evidence.samples[0].births);
    assert!(evidence.samples[2].births > evidence.samples[1].births);
    assert!(
        evidence
            .samples
            .iter()
            .map(|s| s.ordinary_departures)
            .sum::<usize>()
            > 0
    );
    evidence.roof_mass_before_zero = weather
        .world
        .drifts
        .values()
        .filter(|d| !d.support.ground)
        .map(|d| d.total_mass())
        .sum();
    assert!(evidence.roof_mass_before_zero > 0.0);
    let before_zero = last_image.unwrap();

    evidence.stage = "zero_intensity".into();
    settings.snow.intensity_percent = 0;
    evidence.introduced_at_zero = weather.world.introduced_mass;
    for index in 0..120 / steps {
        let introduced = weather.world.introduced_mass;
        now += crate::timing::STEP * steps;
        if index + 1 == 120 / steps {
            snapshot_frame(&mut weather, &settings, now, &mut evidence.trace);
        } else {
            frame(&mut weather, &settings, now, &mut evidence.trace);
        }
        evidence.zero_births += births(&weather, steps, introduced, evidence);
        evidence.zero_steps += steps as usize;
        assert_eq!(weather.world.introduced_mass, evidence.introduced_at_zero);
    }
    let after_zero = Image::read(&weather.layers[&screen_key]);
    evidence.zero_frame_changed = before_zero.bytes != after_zero.bytes;
    evidence.introduced_after_zero = weather.world.introduced_mass;
    evidence.ordinary_after_zero = ordinary(&weather);
    assert_eq!(evidence.zero_births, 0);
    assert!(evidence.zero_frame_changed && evidence.ordinary_after_zero > 0);
    assert_eq!(weather.layers[&screen_key].hwnd, hwnd);
    assert_eq!(weather.layers[&screen_key].metrics_epoch, epoch);
    evidence.same_layer = true;
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
#[ignore = "explicit native snowfall birth-frequency and GPU test on a private desktop"]
fn native_intensity_births_match_width_percentages_and_model_cadence() {
    let mut results: Vec<Evidence> = Vec::new();
    for child in [CHILD_60, CHILD_30] {
        let evidence: Evidence = isolated(child);
        assert!(evidence.completed && evidence.error.is_none());
        assert!(evidence.same_layer && evidence.native_child_exited && evidence.layers_destroyed);
        println!(
            "native_emission_pixels {}",
            serde_json::to_string(&evidence).unwrap()
        );
        results.push(evidence);
    }
    assert_eq!((results[0].cadence, results[1].cadence), (60, 30));
    assert_eq!(results[0].logical_width, results[1].logical_width);
    for (a, b) in results[0].samples.iter().zip(&results[1].samples) {
        assert_eq!((a.intensity, a.births), (b.intensity, b.births));
    }
}
fn child(cadence: u32) {
    let Ok(expected) = std::env::var(DESKTOP_ENV) else {
        return;
    };
    let path = std::env::var_os(REPORT_ENV).expect("probe report path missing");
    let mut evidence = Evidence::default();
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        run(&mut evidence, &expected, cadence)
    }));
    evidence.summarize();
    std::fs::write(path, serde_json::to_vec(&evidence).unwrap()).unwrap();
    if let Err(error) = result {
        std::panic::resume_unwind(error);
    }
}
#[test]
#[ignore = "helper launched only on the assigned private startup desktop"]
fn private_emission_60_child() {
    child(60);
}
#[test]
#[ignore = "helper launched only on the assigned private startup desktop"]
fn private_emission_30_child() {
    child(30);
}
