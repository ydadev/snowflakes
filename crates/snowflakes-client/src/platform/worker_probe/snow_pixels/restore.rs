//! Restore a real minimized window before its naturally accumulated snow lands.
use super::*;

const CHILD: &str = "platform::weather::worker_probe::snow_pixels::restore::private_restore_child";
const SETTLE_LIMIT: usize = 7200;

#[derive(Default, serde::Serialize, serde::Deserialize)]
struct Evidence {
    stage: String,
    completed: bool,
    dpi: u32,
    fill_frames: usize,
    initial_settle_frames: usize,
    introduced_mass: f64,
    initial_roof_mass: f64,
    initial_roof_pixels: usize,
    minimized_roof_pixels: usize,
    pending_after_minimize: usize,
    early_fall_steps: usize,
    bodies_before_restore: usize,
    cohesive_before_restore: usize,
    material_mass_before_restore: f64,
    cohesive_mass_before_restore: f64,
    falling_pixels_before_restore: usize,
    bodies_after_restore: usize,
    falling_pixels_after_restore: usize,
    physical_material_exact: bool,
    new_generation_empty: bool,
    restored_roof_pixels: usize,
    final_roof_pixels: usize,
    final_settle_frames: usize,
    ground_gain: f64,
    removed_gain: f64,
    max_ground_height_gain: f32,
    ground_alpha_changes: usize,
    native_checkpoints_ms: Vec<u64>,
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
    panic!("natural material did not finish in the bounded model interval");
}
fn material_state(weather: &Weather, key: u64) -> Vec<String> {
    weather
        .world
        .flakes
        .iter()
        .filter(|b| b.skip_support == Some(key))
        .map(|b| {
            format!(
                "{:?}",
                (
                    b.position,
                    b.velocity,
                    b.radius,
                    b.mass,
                    b.angle,
                    b.spin,
                    b.age,
                    b.opacity,
                    b.shape,
                    b.skip_support,
                    &b.slab,
                    b.display_scale(weather.world.scene.scale_at(b.position)),
                )
            )
        })
        .collect()
}
fn alpha_band(image: &Image, band: Rect) -> Vec<u8> {
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
    let mut bytes = Vec::new();
    for row in y(band.top)..y(band.bottom) {
        for column in x(band.left)..x(band.right) {
            bytes.push(image.bytes[(row * image.width + column) * 4 + 3]);
        }
    }
    bytes
}
fn transition(host: &mut NativeTestHost, command: &str) {
    guard_if_requested();
    assert_eq!(host.command(command), "native-state-changed");
    pump();
}
fn checkpoint(weather: &Weather, now: Instant, evidence: &mut Evidence) {
    evidence.native_checkpoints_ms.push(
        now.duration_since(weather.tracker.origin_for_test())
            .as_millis() as u64,
    );
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
    assert_eq!(weather.world.introduced_mass, 0.0);
    assert_eq!(weather.world.total_mass(), 0.0);
    assert_eq!(weather.layers.len(), 1);
    let screen_key = *weather.layers.keys().next().unwrap();
    let ground_key = screen_key | (1 << 63);
    let layer_hwnd = weather.layers[&screen_key].hwnd;
    let epoch = weather.layers[&screen_key].metrics_epoch;
    evidence.dpi = unsafe { GetDpiForWindow(layer_hwnd) };
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
    let target = HWND(handles[0] as *mut _);
    let rects: Vec<_> = handles
        .iter()
        .map(|h| bounds(HWND(*h as *mut _), host.id()))
        .collect();
    let mut now = base + Duration::from_millis(30);
    frame(&mut weather, &settings, now, &mut evidence.trace);
    let keys: Vec<_> = rects
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
    assert!(weather.layers[&screen_key].screen.floor > rects[0].top + 400.0);

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
    assert!(weather.world.drifts[&key].total_mass() >= 120.0);
    settings.snow.intensity_percent = 0;
    evidence.introduced_mass = weather.world.introduced_mass;
    evidence.initial_settle_frames = settle(&mut weather, &settings, &mut now, evidence);
    let initial_image = snapshot(&mut weather, &settings, now, evidence);
    evidence.initial_roof_mass = weather.world.drifts[&key].total_mass();
    let roof = roof_band(&weather.world, key);
    evidence.initial_roof_pixels = initial_image.count(roof);
    assert!(evidence.initial_roof_mass > 0.0 && evidence.initial_roof_pixels > 20);
    let control_mass = weather.world.drifts[&control].total_mass();
    let ground_before = weather.world.drifts[&ground_key].total_mass();
    let removed_before = weather.world.removed_mass;
    let ground = &weather.world.drifts[&ground_key];
    let heights_before: Vec<_> = (0..ground.mass.len())
        .map(|i| ground.height(i, &weather.world.scene))
        .collect();
    let ground_band = Rect {
        left: ground.support.left,
        right: ground.support.right,
        top: ground.support.y - 40.0,
        bottom: ground.support.y,
    };
    let ground_pixels_before = alpha_band(&initial_image, ground_band);

    evidence.stage = "minimize".into();
    transition(&mut host, "native-minimize");
    assert!(unsafe { IsIconic(target) }.as_bool());
    now += crate::timing::DISCONTINUITY;
    let image = snapshot(&mut weather, &settings, now, evidence);
    checkpoint(&weather, now, evidence);
    assert!(weather.world.scene.windows.iter().all(|w| w.key != key));
    assert!(!weather.world.drifts.contains_key(&key));
    evidence.minimized_roof_pixels = image.count(roof);
    assert_eq!(evidence.minimized_roof_pixels, 0);
    evidence.pending_after_minimize = weather.world.pending_collapse_count();
    assert!(evidence.pending_after_minimize > 0 && weather.world.flakes.is_empty());

    evidence.stage = "early_fall".into();
    for index in 0..120 {
        now += crate::timing::STEP;
        frame(&mut weather, &settings, now, &mut evidence.trace);
        evidence.early_fall_steps = index + 1;
        let bodies: Vec<_> = weather
            .world
            .flakes
            .iter()
            .filter(|b| b.skip_support == Some(key))
            .collect();
        if !bodies.is_empty()
            && weather.world.pending_collapse_count() == 0
            && bodies.iter().all(|b| {
                weather
                    .world
                    .render_pose(b, weather.clock.fraction())
                    .bounds(b)
                    .top
                    > rects[0].top + 12.0
            })
        {
            break;
        }
    }
    let before_restore = material_state(&weather, key);
    evidence.bodies_before_restore = before_restore.len();
    let material: Vec<_> = weather
        .world
        .flakes
        .iter()
        .filter(|b| b.skip_support == Some(key))
        .collect();
    evidence.material_mass_before_restore = material.iter().map(|b| f64::from(b.mass)).sum();
    evidence.cohesive_before_restore = material.iter().filter(|b| b.slab.is_some()).count();
    evidence.cohesive_mass_before_restore = material
        .iter()
        .filter(|b| b.slab.is_some())
        .map(|b| f64::from(b.mass))
        .sum();
    assert!(
        (evidence.material_mass_before_restore - evidence.initial_roof_mass).abs()
            < evidence.initial_roof_mass * 1e-4
    );
    assert!(
        evidence.cohesive_before_restore > 0
            && evidence.cohesive_mass_before_restore > evidence.initial_roof_mass * 0.9
    );
    assert!(evidence.bodies_before_restore > 0 && weather.world.pending_collapse_count() == 0);
    assert!(
        weather
            .world
            .flakes
            .iter()
            .filter(|b| b.skip_support == Some(key))
            .all(|b| weather
                .world
                .render_pose(b, weather.clock.fraction())
                .bounds(b)
                .top
                > rects[0].top + 12.0)
    );
    let falling = Rect {
        left: rects[0].left - 40.0,
        right: rects[0].right + 40.0,
        top: rects[0].top + 12.0,
        bottom: weather.layers[&screen_key].screen.floor - 40.0,
    };
    let image = snapshot(&mut weather, &settings, now, evidence);
    evidence.falling_pixels_before_restore = image.count(falling);
    assert!(evidence.falling_pixels_before_restore > 10);

    evidence.stage = "quick_restore".into();
    transition(&mut host, "native-restore");
    assert!(!unsafe { IsIconic(target) }.as_bool());
    assert_eq!(bounds(target, host.id()), rects[0]);
    now += crate::timing::DISCONTINUITY;
    let image = snapshot(&mut weather, &settings, now, evidence);
    checkpoint(&weather, now, evidence);
    let restored_key = weather
        .world
        .scene
        .windows
        .iter()
        .find(|w| w.rect == rects[0] && w.collect && w.visible)
        .unwrap()
        .key;
    assert_ne!(restored_key, key);
    assert!(!weather.world.drifts.contains_key(&key));
    assert_eq!(weather.world.drifts[&restored_key].total_mass(), 0.0);
    evidence.new_generation_empty = true;
    let after_restore = material_state(&weather, key);
    evidence.bodies_after_restore = after_restore.len();
    assert_eq!(before_restore, after_restore);
    evidence.physical_material_exact = true;
    evidence.restored_roof_pixels = image.count(roof);
    evidence.falling_pixels_after_restore = image.count(falling);
    assert_eq!(evidence.restored_roof_pixels, 0);
    assert!(evidence.falling_pixels_after_restore > 10);
    assert!((weather.world.drifts[&control].total_mass() - control_mass).abs() < 0.01);

    evidence.stage = "landing".into();
    evidence.final_settle_frames = settle(&mut weather, &settings, &mut now, evidence);
    assert_eq!(weather.world.drifts[&restored_key].total_mass(), 0.0);
    assert!((weather.world.drifts[&control].total_mass() - control_mass).abs() < 0.01);
    evidence.ground_gain = weather.world.drifts[&ground_key].total_mass() - ground_before;
    evidence.removed_gain = weather.world.removed_mass - removed_before;
    assert!(evidence.ground_gain > 0.0);
    assert!(
        (evidence.ground_gain + evidence.removed_gain - evidence.initial_roof_mass).abs()
            < evidence.introduced_mass * 1e-5
    );
    let ground = &weather.world.drifts[&ground_key];
    assert_eq!(ground.mass.len(), heights_before.len());
    evidence.max_ground_height_gain = heights_before
        .iter()
        .enumerate()
        .map(|(i, h)| ground.height(i, &weather.world.scene) - h)
        .fold(0.0, f32::max);
    assert!(evidence.max_ground_height_gain > 0.25);
    let image = snapshot(&mut weather, &settings, now, evidence);
    evidence.final_roof_pixels = image.count(roof);
    assert_eq!(evidence.final_roof_pixels, 0);
    let ground_pixels_after = alpha_band(&image, ground_band);
    assert_eq!(ground_pixels_before.len(), ground_pixels_after.len());
    evidence.ground_alpha_changes = ground_pixels_before
        .iter()
        .zip(ground_pixels_after)
        .filter(|(a, b)| **a != *b)
        .count();
    assert!(evidence.ground_alpha_changes > 0);
    assert_eq!(weather.world.introduced_mass, evidence.introduced_mass);
    assert_eq!(weather.layers[&screen_key].hwnd, layer_hwnd);
    assert_eq!(weather.layers[&screen_key].metrics_epoch, epoch);
    evidence.same_layer = true;
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
#[ignore = "explicit natural snow minimize/quick-restore GPU test on a private desktop"]
fn native_quick_restore_keeps_falling_snow_and_returns_an_empty_generation() {
    let evidence: Evidence = isolated(CHILD);
    assert!(evidence.completed && evidence.error.is_none());
    assert!(evidence.same_layer && evidence.native_child_exited && evidence.layers_destroyed);
    println!(
        "native_restore_pixels {}",
        serde_json::to_string(&evidence).unwrap()
    );
}
#[test]
#[ignore = "helper launched only on the assigned private startup desktop"]
fn private_restore_child() {
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
