//! Percentage controls on natural particles and real detached material in GPU frames.
use super::*;

const CHILD: &str =
    "platform::weather::worker_probe::snow_pixels::appearance::private_appearance_child";

#[derive(Default, serde::Serialize, serde::Deserialize)]
struct Sample {
    percent: u8,
    pixels: usize,
    alpha_sum: u64,
    contrast_pixels: usize,
    min_radius: f32,
    max_radius: f32,
}
#[derive(Default, serde::Serialize, serde::Deserialize)]
struct Evidence {
    stage: String,
    completed: bool,
    dpi: u32,
    fill_frames: usize,
    ordinary_bodies: usize,
    roof_mass: f64,
    size_samples: Vec<Sample>,
    visibility_samples: Vec<Sample>,
    max_radius_relative_error: f32,
    max_particle_mass_relative_error: f32,
    size_ledger_accounted: bool,
    resting_stock_unchanged: bool,
    visibility_world_bit_equal: bool,
    visibility_restore_pixels_equal: bool,
    cohesive_bodies: usize,
    material_pixels_at_zero_visibility: usize,
    material_size_frames_equal: usize,
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
fn snapshot(
    weather: &mut Weather,
    settings: &Settings,
    now: Instant,
    evidence: &mut Evidence,
) -> Image {
    snapshot_frame(weather, settings, now, &mut evidence.trace);
    Image::read(weather.layers.values().next().unwrap())
}
fn sample(image: &Image, band: Rect, weather: &Weather, percent: u8) -> Sample {
    let x = |value: f32| {
        (value - image.screen.rect.left)
            .ceil()
            .clamp(0.0, image.width as f32) as usize
    };
    let y = |value: f32| {
        (value - image.screen.rect.top)
            .ceil()
            .clamp(0.0, image.height as f32) as usize
    };
    let mut alpha_sum = 0;
    let mut contrast_pixels = 0;
    for row in y(band.top)..y(band.bottom) {
        for column in x(band.left)..x(band.right) {
            let offset = (row * image.width + column) * 4;
            let alpha = image.bytes[offset + 3];
            alpha_sum += u64::from(alpha);
            let darkest = *image.bytes[offset..offset + 3].iter().min().unwrap();
            // Premultiplied channels: exclude quantization at tiny alpha and
            // distinguish the dark contrast contour from white crystal strokes.
            contrast_pixels +=
                usize::from(alpha >= 20 && u16::from(darkest) * 4 < u16::from(alpha) * 3);
        }
    }
    Sample {
        percent,
        pixels: image.count(band),
        alpha_sum,
        contrast_pixels,
        min_radius: weather
            .world
            .flakes
            .iter()
            .filter(|b| b.slab.is_none())
            .map(|b| b.radius)
            .fold(f32::MAX, f32::min),
        max_radius: weather
            .world
            .flakes
            .iter()
            .filter(|b| b.slab.is_none())
            .map(|b| b.radius)
            .fold(0.0, f32::max),
    }
}
fn resized_particles(before: &[Flake], weather: &Weather, percent: u8, evidence: &mut Evidence) {
    assert_eq!(before.len(), weather.world.flakes.len());
    let factor = f32::from(percent) / 100.0;
    for (old, actual) in before.iter().zip(&weather.world.flakes) {
        assert!(old.slab.is_none() && actual.slab.is_none());
        let expected_radius = old.radius * factor;
        let expected_mass = old.mass * factor * factor;
        let radius_error = (actual.radius - expected_radius).abs() / expected_radius;
        let mass_error = (actual.mass - expected_mass).abs() / expected_mass;
        evidence.max_radius_relative_error = evidence.max_radius_relative_error.max(radius_error);
        evidence.max_particle_mass_relative_error =
            evidence.max_particle_mass_relative_error.max(mass_error);
        assert!(radius_error < 1e-5 && mass_error < 1e-5);
        // Normalize only the two fields that are allowed to change. Debug
        // includes private interpolation/display state as well as public poses.
        let mut unchanged = old.clone();
        unchanged.radius = actual.radius;
        unchanged.mass = actual.mass;
        assert_eq!(format!("{unchanged:?}"), format!("{actual:?}"));
    }
}
fn slab_state(weather: &Weather) -> String {
    format!(
        "{:?}",
        weather
            .world
            .flakes
            .iter()
            .filter(|b| b.slab.is_some())
            .collect::<Vec<_>>()
    )
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
    frame(&mut weather, &settings, now, &mut evidence.trace);
    let key = weather
        .world
        .scene
        .windows
        .iter()
        .find(|w| w.rect == rect && w.collect && w.visible)
        .unwrap()
        .key;
    evidence.stage = "natural_fill".into();
    settings.snow.intensity_percent = 100;
    for index in 0..1800 {
        now += crate::timing::STEP;
        frame(&mut weather, &settings, now, &mut evidence.trace);
        evidence.fill_frames = index + 1;
        if weather.world.drifts[&key].total_mass() >= 120.0 {
            break;
        }
    }
    evidence.roof_mass = weather.world.drifts[&key].total_mass();
    assert!(evidence.roof_mass >= 120.0);
    settings.snow.intensity_percent = 0;
    // Discard the elapsed interval through the real clock, then keep the exact
    // same timestamp for every comparison. No body or profile is planted.
    now += crate::timing::DISCONTINUITY;
    let _ = snapshot(&mut weather, &settings, now, evidence);
    let before = weather.world.flakes.clone();
    evidence.ordinary_bodies = before.len();
    assert!(before.len() > 100 && before.iter().all(|b| b.slab.is_none()));
    let drifts = format!("{:?}", weather.world.drifts);
    let air = Rect {
        left: weather.layers[&screen_key].screen.rect.left,
        right: weather.layers[&screen_key].screen.rect.right,
        top: weather.layers[&screen_key].screen.rect.top,
        bottom: rect.top - 40.0,
    };
    assert!(air.bottom - air.top > 20.0);
    evidence.stage = "size_percentages".into();
    for percent in [25, 50, 100, 150, 200, 100] {
        let mass = weather.world.total_mass();
        let introduced = weather.world.introduced_mass;
        let removed = weather.world.removed_mass;
        settings.snow.flake_size_percent = percent;
        let image = snapshot(&mut weather, &settings, now, evidence);
        resized_particles(&before, &weather, percent, evidence);
        assert_eq!(format!("{:?}", weather.world.drifts), drifts);
        let change = weather.world.total_mass() - mass;
        let accounted =
            weather.world.introduced_mass - introduced - (weather.world.removed_mass - removed);
        assert!((change - accounted).abs() < weather.world.introduced_mass * 1e-5);
        let s = sample(&image, air, &weather, percent);
        assert!(s.pixels > 0 && s.max_radius > s.min_radius * 1.2);
        evidence.size_samples.push(s);
    }
    assert!(evidence.size_samples[4].pixels > evidence.size_samples[0].pixels);
    evidence.size_ledger_accounted = true;
    evidence.resting_stock_unchanged = true;

    evidence.stage = "visibility_percentages".into();
    let reference = snapshot(&mut weather, &settings, now, evidence);
    let physics = format!("{:?}", weather.world);
    for percent in [0, 25, 50, 100, 150, 200, 100] {
        settings.snow.flake_visibility_percent = percent;
        let image = snapshot(&mut weather, &settings, now, evidence);
        assert_eq!(format!("{:?}", weather.world), physics);
        let s = sample(&image, air, &weather, percent);
        if percent == 0 {
            assert_eq!(s.pixels, 0);
            assert_eq!(s.alpha_sum, 0);
        }
        if percent == 100 {
            assert_eq!(image.bytes, reference.bytes);
        }
        evidence.visibility_samples.push(s);
    }
    for pair in evidence.visibility_samples[..6].windows(2) {
        assert!(pair[1].alpha_sum > pair[0].alpha_sum);
    }
    assert_eq!(evidence.visibility_samples[3].contrast_pixels, 0);
    assert!(evidence.visibility_samples[5].contrast_pixels > 0);
    evidence.visibility_world_bit_equal = true;
    evidence.visibility_restore_pixels_equal = true;

    evidence.stage = "natural_material".into();
    assert_eq!(host.command("native-close"), "native-closed");
    pump();
    now += crate::timing::DISCONTINUITY;
    frame(&mut weather, &settings, now, &mut evidence.trace);
    assert!(!weather.world.drifts.contains_key(&key));
    assert!(!unsafe { IsWindow(Some(HWND(handle as *mut _))) }.as_bool());
    assert!(weather.world.pending_collapse_count() > 0);
    for _ in 0..20 {
        now += crate::timing::STEP;
        frame(&mut weather, &settings, now, &mut evidence.trace);
    }
    evidence.cohesive_bodies = weather
        .world
        .flakes
        .iter()
        .filter(|b| b.slab.is_some())
        .count();
    assert!(evidence.cohesive_bodies > 0);
    let slabs = slab_state(&weather);
    let mut area: Option<Rect> = None;
    for b in weather.world.flakes.iter().filter(|b| b.slab.is_some()) {
        let bounds = weather
            .world
            .render_pose(b, weather.clock.fraction())
            .bounds(b);
        area = Some(area.map_or(bounds, |old| Rect {
            left: old.left.min(bounds.left),
            right: old.right.max(bounds.right),
            top: old.top.min(bounds.top),
            bottom: old.bottom.max(bounds.bottom),
        }));
    }
    let area = area.unwrap();
    settings.snow.flake_visibility_percent = 0;
    let material = snapshot(&mut weather, &settings, now, evidence);
    evidence.material_pixels_at_zero_visibility = material.count(area);
    assert!(evidence.material_pixels_at_zero_visibility > 10);
    let drifts = format!("{:?}", weather.world.drifts);
    for percent in [25, 200, 100] {
        settings.snow.flake_size_percent = percent;
        let image = snapshot(&mut weather, &settings, now, evidence);
        assert_eq!(slab_state(&weather), slabs);
        assert_eq!(format!("{:?}", weather.world.drifts), drifts);
        assert_eq!(image.bytes, material.bytes);
        evidence.material_size_frames_equal += 1;
    }
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
#[ignore = "explicit natural percentage-control and GPU test on a private desktop"]
fn natural_percentage_controls_preserve_distribution_and_material_and_change_pixels() {
    let evidence: Evidence = isolated(CHILD);
    assert!(evidence.completed && evidence.error.is_none());
    assert!(evidence.same_layer && evidence.native_child_exited && evidence.layers_destroyed);
    println!(
        "native_appearance_pixels {}",
        serde_json::to_string(&evidence).unwrap()
    );
}
#[test]
#[ignore = "helper launched only on the assigned private startup desktop"]
fn private_appearance_child() {
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
