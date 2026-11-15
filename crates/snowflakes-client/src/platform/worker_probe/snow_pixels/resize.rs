//! Real resize of naturally accumulated snow, with spatial and GPU evidence.
use super::*;
use crate::snow::Drift;

const RIGHT_CHILD: &str =
    "platform::weather::worker_probe::snow_pixels::resize::private_right_resize_child";
const LEFT_CHILD: &str =
    "platform::weather::worker_probe::snow_pixels::resize::private_left_resize_child";
const SETTLE_LIMIT: usize = 7200;

#[derive(Default, serde::Serialize, serde::Deserialize)]
struct Evidence {
    stage: String,
    completed: bool,
    left_cut: bool,
    dpi: u32,
    fill_frames: usize,
    initial_settle_frames: usize,
    introduced_mass: f64,
    mass_before_growth: f64,
    mass_after_growth: f64,
    max_cell_relative_error: f64,
    mass_before_cut: f64,
    retained_expected: f64,
    retained_actual: f64,
    cut_mass: f64,
    cut_pixels_before: usize,
    cut_pixels_after: usize,
    retained_pixels: usize,
    pending_after_cut: usize,
    cohesive_bodies: usize,
    cohesive_mass: f64,
    falling_pixels: usize,
    final_settle_frames: usize,
    final_roof_mass: f64,
    ground_gain: f64,
    removed_gain: f64,
    ground_pixel_changes: usize,
    max_ground_height_gain: f32,
    final_cut_pixels: usize,
    native_checkpoints_ms: Vec<u64>,
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

fn intervals(drift: &Drift) -> Vec<(f64, f64, f64)> {
    let width = f64::from(drift.support.right - drift.support.left) / drift.mass.len() as f64;
    (0..drift.mass.len())
        .map(|i| {
            let left = f64::from(drift.support.left) + i as f64 * width;
            (
                left,
                left + width,
                f64::from(drift.mass[i]) + f64::from(drift.pending[i]),
            )
        })
        .collect()
}
fn in_range(old: &[(f64, f64, f64)], left: f64, right: f64) -> f64 {
    old.iter()
        .map(|&(start, end, mass)| {
            mass * (end.min(right) - start.max(left)).max(0.0) / (end - start)
        })
        .sum()
}
fn remapped(drift: &Drift, old: &[(f64, f64, f64)], evidence: &mut Evidence) {
    for (left, right, mass) in intervals(drift) {
        let expected = in_range(old, left, right);
        let relative = (mass - expected).abs() / expected.max(1.0);
        evidence.max_cell_relative_error = evidence.max_cell_relative_error.max(relative);
        assert!(
            relative < 1e-4,
            "resize moved snow outside physical overlap: {mass} vs {expected}"
        );
    }
}
fn scene_rect(weather: &Weather, key: u64) -> Rect {
    weather
        .world
        .scene
        .windows
        .iter()
        .find(|w| w.key == key)
        .unwrap()
        .rect
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
fn settle(
    weather: &mut Weather,
    settings: &Settings,
    now: &mut Instant,
    evidence: &mut Evidence,
) -> usize {
    let introduced = weather.world.introduced_mass;
    for index in 0..SETTLE_LIMIT {
        *now += crate::timing::STEP;
        frame(weather, settings, *now, &mut evidence.trace);
        assert_eq!(weather.world.introduced_mass, introduced);
        if weather.world.flakes.is_empty() && weather.world.pending_collapse_count() == 0 {
            return index + 1;
        }
    }
    panic!("natural material did not finish within the bounded model interval");
}
fn alpha_changes(before: &Image, after: &Image, band: Rect) -> usize {
    assert_eq!((before.width, before.height), (after.width, after.height));
    assert_eq!(before.screen.rect, after.screen.rect);
    let x = |value: f32| {
        (value - before.screen.rect.left)
            .ceil()
            .clamp(0.0, before.width as f32) as usize
    };
    let y = |value: f32| {
        (value - before.screen.rect.top)
            .ceil()
            .clamp(0.0, before.height as f32) as usize
    };
    let mut count = 0;
    for row in y(band.top)..y(band.bottom) {
        for column in x(band.left)..x(band.right) {
            let alpha = (row * before.width + column) * 4 + 3;
            count += usize::from(before.bytes[alpha] != after.bytes[alpha]);
        }
    }
    count
}
fn transition(host: &mut NativeTestHost, command: &str) {
    guard_if_requested();
    assert_eq!(host.command(command), "native-resized");
    pump();
}

fn run(evidence: &mut Evidence, desktop: &str, left: bool) {
    guard_if_requested();
    let _dpi = Dpi::pmv2();
    evidence.left_cut = left;
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
    let layer_hwnd = weather.layers[&screen_key].hwnd;
    let epoch = weather.layers[&screen_key].metrics_epoch;
    evidence.dpi = unsafe { GetDpiForWindow(layer_hwnd) };
    let ground_key = screen_key | (1 << 63);
    let mut host = NativeTestHost::new(desktop);
    let created = host.command("native-create");
    let handles: Vec<usize> = created
        .strip_prefix("native-created ")
        .unwrap()
        .split_whitespace()
        .map(|value| value.parse().unwrap())
        .collect();
    assert_eq!(handles.len(), 2);
    transition(&mut host, "native-wide");
    let wide_rect = bounds(HWND(handles[0] as *mut _), host.id());
    let control_rect = bounds(HWND(handles[1] as *mut _), host.id());
    let mut now = base + Duration::from_millis(30);
    frame(&mut weather, &settings, now, &mut evidence.trace);
    let keys: Vec<_> = [wide_rect, control_rect]
        .iter()
        .map(|rect| {
            weather
                .world
                .scene
                .windows
                .iter()
                .find(|w| w.rect == *rect && w.collect && w.visible)
                .unwrap()
                .key
        })
        .collect();
    let key = keys[0];
    let control = keys[1];
    assert_ne!(key, control);
    let wide = weather.world.drifts[&key].support.clone();
    let cut_probe = Rect {
        left: if left {
            wide.left + 8.0
        } else {
            (wide.right - 160.0 + 8.0).max(control_rect.right + 10.0)
        },
        right: if left {
            (wide.left + 280.0 - 8.0).min(control_rect.left - 10.0)
        } else {
            wide.right - 8.0
        },
        top: wide.y - 40.0,
        bottom: wide.y - 1.0,
    };
    assert!(cut_probe.right - cut_probe.left > 20.0);

    // No planted profiles, ledger writes, manual release or seeded particles.
    evidence.stage = "natural_fill".into();
    settings.snow.intensity_percent = 100;
    for index in 0..1800 {
        now += crate::timing::STEP;
        frame(&mut weather, &settings, now, &mut evidence.trace);
        evidence.fill_frames = index + 1;
        let drift = &weather.world.drifts[&key];
        if drift.total_mass() >= 600.0
            && in_range(
                &intervals(drift),
                f64::from(cut_probe.left),
                f64::from(cut_probe.right),
            ) >= 120.0
        {
            break;
        }
    }
    assert!(weather.world.drifts[&key].total_mass() >= 600.0);
    assert!(
        in_range(
            &intervals(&weather.world.drifts[&key]),
            f64::from(cut_probe.left),
            f64::from(cut_probe.right)
        ) >= 120.0
    );
    settings.snow.intensity_percent = 0;
    evidence.introduced_mass = weather.world.introduced_mass;
    evidence.initial_settle_frames = settle(&mut weather, &settings, &mut now, evidence);
    let control_mass = weather.world.drifts[&control].total_mass();
    let old = intervals(&weather.world.drifts[&key]);
    evidence.mass_before_growth = weather.world.drifts[&key].total_mass();
    let initial_ground = weather.world.drifts[&ground_key].total_mass();
    let initial_ground_heights: Vec<_> = (0..weather.world.drifts[&ground_key].mass.len())
        .map(|i| weather.world.drifts[&ground_key].height(i, &weather.world.scene))
        .collect();
    let initial_removed = weather.world.removed_mass;
    assert!(evidence.mass_before_growth > 0.0);

    evidence.stage = "grow".into();
    transition(&mut host, "native-grow-right");
    // Use the production discontinuity rule to observe set_scene before any
    // physical step. No test clock reset or model pause is injected.
    now += crate::timing::DISCONTINUITY;
    let grown_image = snapshot(&mut weather, &settings, now, evidence);
    evidence.native_checkpoints_ms.push(
        now.duration_since(weather.tracker.origin_for_test())
            .as_millis() as u64,
    );
    let grown_rect = bounds(HWND(handles[0] as *mut _), host.id());
    assert_eq!(scene_rect(&weather, key), grown_rect);
    assert_eq!(grown_rect.left, wide_rect.left);
    assert_eq!(grown_rect.right, wide_rect.right + 120.0);
    assert!(weather.world.flakes.is_empty() && weather.world.pending_collapse_count() == 0);
    remapped(&weather.world.drifts[&key], &old, evidence);
    evidence.mass_after_growth = weather.world.drifts[&key].total_mass();
    assert!(
        (evidence.mass_after_growth - evidence.mass_before_growth).abs()
            < evidence.mass_before_growth * 1e-5
    );
    let grown = weather.world.drifts[&key].support.clone();
    let extension = Rect {
        left: wide.right + 8.0,
        right: grown.right - 8.0,
        top: wide.y - 40.0,
        bottom: wide.y - 1.0,
    };
    assert_eq!(grown_image.count(extension), 0);
    evidence.cut_pixels_before = grown_image.count(cut_probe);
    assert!(evidence.cut_pixels_before > 20);
    let old = intervals(&weather.world.drifts[&key]);
    evidence.mass_before_cut = weather.world.drifts[&key].total_mass();

    evidence.stage = "cut".into();
    transition(
        &mut host,
        if left {
            "native-cut-left"
        } else {
            "native-shrink-right"
        },
    );
    now += crate::timing::DISCONTINUITY;
    let cut_image = snapshot(&mut weather, &settings, now, evidence);
    evidence.native_checkpoints_ms.push(
        now.duration_since(weather.tracker.origin_for_test())
            .as_millis() as u64,
    );
    let cut_rect = bounds(HWND(handles[0] as *mut _), host.id());
    assert_eq!(scene_rect(&weather, key), cut_rect);
    if left {
        assert_eq!(cut_rect.right, grown_rect.right);
        assert_eq!(cut_rect.left, grown_rect.left + 280.0);
    } else {
        assert_eq!(cut_rect.left, grown_rect.left);
        assert_eq!(cut_rect.right, grown_rect.right - 280.0);
    }
    let retained = &weather.world.drifts[&key];
    if left {
        assert!(cut_probe.right < retained.support.left);
    } else {
        assert!(cut_probe.left > retained.support.right);
    }
    evidence.retained_expected = in_range(
        &old,
        f64::from(retained.support.left),
        f64::from(retained.support.right),
    );
    evidence.retained_actual = retained.total_mass();
    evidence.cut_mass = evidence.mass_before_cut - evidence.retained_expected;
    remapped(retained, &old, evidence);
    assert!(
        (evidence.retained_actual - evidence.retained_expected).abs()
            < evidence.mass_before_cut * 1e-5
    );
    assert!(evidence.cut_mass > 100.0 && evidence.retained_actual > 0.0);
    evidence.cut_pixels_after = cut_image.count(cut_probe);
    evidence.retained_pixels = cut_image.count(roof_band(&weather.world, key));
    assert_eq!(evidence.cut_pixels_after, 0);
    assert!(evidence.retained_pixels > 20);
    evidence.pending_after_cut = weather.world.pending_collapse_count();
    assert!(evidence.pending_after_cut > 0 && weather.world.flakes.is_empty());
    assert_eq!(weather.world.introduced_mass, evidence.introduced_mass);

    evidence.stage = "cut_falls".into();
    now += crate::timing::STEP;
    frame(&mut weather, &settings, now, &mut evidence.trace);
    evidence.cohesive_bodies = weather
        .world
        .flakes
        .iter()
        .filter(|b| b.slab.is_some())
        .count();
    evidence.cohesive_mass = weather
        .world
        .flakes
        .iter()
        .filter(|b| b.slab.is_some())
        .map(|b| f64::from(b.mass))
        .sum();
    assert!(evidence.cohesive_bodies > 0 && evidence.cohesive_mass > evidence.cut_mass * 0.9);
    for _ in 0..20 {
        now += crate::timing::STEP;
        frame(&mut weather, &settings, now, &mut evidence.trace);
    }
    let image = snapshot(&mut weather, &settings, now, evidence);
    let falling = Rect {
        left: grown.left - 30.0,
        right: grown.right + 30.0,
        top: grown.y + 12.0,
        bottom: weather.layers[&screen_key].screen.floor - 40.0,
    };
    evidence.falling_pixels = image.count(falling);
    assert!(evidence.falling_pixels > 10);

    evidence.stage = "cut_lands".into();
    evidence.final_settle_frames = settle(&mut weather, &settings, &mut now, evidence);
    evidence.final_roof_mass = weather.world.drifts[&key].total_mass();
    evidence.ground_gain = weather.world.drifts[&ground_key].total_mass() - initial_ground;
    evidence.removed_gain = weather.world.removed_mass - initial_removed;
    assert!((weather.world.drifts[&control].total_mass() - control_mass).abs() < 0.01);
    assert!(
        evidence.final_roof_mass > 0.0
            && evidence.final_roof_mass <= evidence.retained_actual + 0.01
    );
    assert!(
        evidence.ground_gain > 0.0
            && evidence.ground_gain + evidence.removed_gain > evidence.cut_mass * 0.99
    );
    assert!(
        (evidence.ground_gain + evidence.removed_gain
            - (evidence.mass_before_cut - evidence.final_roof_mass))
            .abs()
            < evidence.introduced_mass * 1e-5
    );
    let ground = &weather.world.drifts[&ground_key];
    assert_eq!(ground.mass.len(), initial_ground_heights.len());
    evidence.max_ground_height_gain = initial_ground_heights
        .iter()
        .enumerate()
        .map(|(i, before)| ground.height(i, &weather.world.scene) - before)
        .fold(0.0, f32::max);
    assert!(evidence.max_ground_height_gain > 0.25);
    let ground_band = Rect {
        left: ground.support.left,
        right: ground.support.right,
        top: ground.support.y - 40.0,
        bottom: ground.support.y,
    };
    let image = snapshot(&mut weather, &settings, now, evidence);
    assert!(weather.world.flakes.is_empty() && weather.world.pending_collapse_count() == 0);
    assert_eq!(weather.world.introduced_mass, evidence.introduced_mass);
    evidence.final_cut_pixels = image.count(cut_probe);
    evidence.ground_pixel_changes = alpha_changes(&grown_image, &image, ground_band);
    assert_eq!(evidence.final_cut_pixels, 0);
    assert!(evidence.ground_pixel_changes > 0);
    assert_eq!(scene_rect(&weather, control), control_rect);
    assert_eq!(weather.layers[&screen_key].hwnd, layer_hwnd);
    assert_eq!(weather.layers[&screen_key].metrics_epoch, epoch);
    evidence.same_support_and_layer = true;
    conserved(&weather.world, &mut evidence.trace);

    evidence.stage = "shutdown".into();
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
#[ignore = "explicit real resize of natural snow with GPU readback on a private desktop"]
fn native_resize_preserves_natural_snow_profile_and_releases_both_cuts() {
    for child in [RIGHT_CHILD, LEFT_CHILD] {
        let evidence: Evidence = isolated(child);
        assert!(evidence.completed && evidence.error.is_none());
        assert!(
            evidence.same_support_and_layer
                && evidence.native_child_exited
                && evidence.layers_destroyed
        );
        println!(
            "native_resize_pixels {}",
            serde_json::to_string(&evidence).unwrap()
        );
    }
}
fn child(left: bool) {
    let Ok(expected) = std::env::var(DESKTOP_ENV) else {
        return;
    };
    let path = std::env::var_os(REPORT_ENV).expect("probe report path missing");
    let mut evidence = Evidence::default();
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        run(&mut evidence, &expected, left)
    }));
    evidence.summarize();
    std::fs::write(path, serde_json::to_vec(&evidence).unwrap()).unwrap();
    if let Err(error) = result {
        std::panic::resume_unwind(error);
    }
}
#[test]
#[ignore = "helper launched only on the assigned private startup desktop"]
fn private_right_resize_child() {
    child(false);
}
#[test]
#[ignore = "helper launched only on the assigned private startup desktop"]
fn private_left_resize_child() {
    child(true);
}
