//! Genuine gentle/abrupt motion and closure of a naturally stocked native roof.
use super::*;

const CHILD: &str =
    "platform::weather::worker_probe::snow_pixels::motion::private_natural_motion_child";

#[derive(Default, serde::Serialize, serde::Deserialize)]
struct Evidence {
    stage: String,
    completed: bool,
    dpi: u32,
    fill_frames: usize,
    initial_settle_updates: usize,
    introduced_mass: f64,
    initial_roof_mass: f64,
    initial_roof_pixels: usize,
    gentle_moves: usize,
    gentle_max_mass_delta: f64,
    gentle_roof_pixels: usize,
    abrupt_delta_x: f32,
    abrupt_delta_y: f32,
    retained_after_acceleration: f64,
    shed_bodies: usize,
    shed_mass_after_two_seconds: f64,
    old_roof_pixels_after_two_seconds: usize,
    moved_roof_pixels: usize,
    falling_pixels_after_two_seconds: usize,
    close_cohesive_bodies: usize,
    close_cohesive_mass: f64,
    close_ordinary_mass: f64,
    close_early_steps: usize,
    closed_roof_pixels: usize,
    final_old_roof_pixels: usize,
    final_moved_roof_pixels: usize,
    final_settle_updates: usize,
    ground_gain: f64,
    removed_gain: f64,
    max_ground_height_gain: f32,
    ground_alpha_changes: usize,
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
fn advance(
    weather: &mut Weather,
    settings: &Settings,
    now: &mut Instant,
    count: usize,
    evidence: &mut Evidence,
) {
    for _ in 0..count {
        *now += crate::timing::STEP;
        frame(weather, settings, *now, &mut evidence.trace);
        assert_eq!(weather.world.introduced_mass, evidence.introduced_mass);
    }
}
fn settle(
    weather: &mut Weather,
    settings: &Settings,
    now: &mut Instant,
    evidence: &mut Evidence,
) -> usize {
    assert_eq!(settings.snow.intensity_percent, 0);
    // Same fixed-step World, two steps per lower update. Bound the rendering
    // work while allowing even tiny ordinary fragments their real lifetime.
    for index in 0..7800 {
        *now += crate::timing::STEP * 2;
        frame(weather, settings, *now, &mut evidence.trace);
        assert_eq!(weather.world.introduced_mass, evidence.introduced_mass);
        if weather.world.flakes.is_empty() && weather.world.pending_collapse_count() == 0 {
            return index + 1;
        }
    }
    panic!("natural material did not finish within the bounded active interval");
}
fn alpha_band(image: &Image, band: Rect) -> Vec<u8> {
    let x = |v: f32| {
        (v - image.screen.rect.left)
            .ceil()
            .clamp(0.0, image.width as f32) as usize
    };
    let y = |v: f32| {
        (v - image.screen.rect.top)
            .ceil()
            .clamp(0.0, image.height as f32) as usize
    };
    let mut alpha = Vec::new();
    for row in y(band.top)..y(band.bottom) {
        for column in x(band.left)..x(band.right) {
            alpha.push(image.bytes[(row * image.width + column) * 4 + 3]);
        }
    }
    alpha
}
fn change(host: &mut NativeTestHost, command: &str, expected: &str) {
    guard_if_requested();
    assert_eq!(host.command(command), expected);
    pump();
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
    let mut now = Instant::now();
    evidence.stage = "empty_native_scene".into();
    frame(&mut weather, &settings, now, &mut evidence.trace);
    assert!(weather.seeded && weather.world.flakes.is_empty());
    assert_eq!(weather.world.introduced_mass, 0.0);
    assert_eq!(weather.world.total_mass(), 0.0);
    assert_eq!(weather.layers.len(), 1);
    let screen_key = *weather.layers.keys().next().unwrap();
    let ground_key = screen_key | (1 << 63);
    let hwnd = weather.layers[&screen_key].hwnd;
    let epoch = weather.layers[&screen_key].metrics_epoch;
    evidence.dpi = unsafe { GetDpiForWindow(hwnd) };
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
    now += Duration::from_millis(30);
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
    assert_eq!(weather.world.total_mass(), 0.0);
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
    evidence.initial_settle_updates = settle(&mut weather, &settings, &mut now, evidence);
    let image = snapshot(&mut weather, &settings, now, evidence);
    let old_band = roof_band(&weather.world, key);
    let control_band = roof_band(&weather.world, control);
    evidence.initial_roof_mass = weather.world.drifts[&key].total_mass();
    evidence.initial_roof_pixels = image.count(old_band);
    assert!(evidence.initial_roof_mass > 100.0 && evidence.initial_roof_pixels > 20);
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
    let ground_before_alpha = alpha_band(&image, ground_band);
    drop(image);

    evidence.stage = "gentle_native_motion".into();
    for offset in 1..=10 {
        advance(&mut weather, &settings, &mut now, 2, evidence);
        change(&mut host, "native-nudge", "native-nudged");
        now += Duration::from_millis(30);
        frame(&mut weather, &settings, now, &mut evidence.trace);
        let rect = bounds(target, host.id());
        assert_eq!(rect.left, rects[0].left + offset as f32);
        assert_eq!(rect.top, rects[0].top);
        assert_eq!(
            weather
                .world
                .scene
                .windows
                .iter()
                .find(|w| w.key == key)
                .unwrap()
                .rect,
            rect
        );
        let delta = (weather.world.drifts[&key].total_mass() - evidence.initial_roof_mass).abs();
        evidence.gentle_max_mass_delta = evidence.gentle_max_mass_delta.max(delta);
        assert!(delta < 0.01);
        assert!((weather.world.drifts[&control].total_mass() - control_mass).abs() < 0.01);
        assert!(weather.world.flakes.is_empty() && weather.world.pending_collapse_count() == 0);
        evidence.gentle_moves += 1;
    }
    let image = snapshot(&mut weather, &settings, now, evidence);
    evidence.gentle_roof_pixels = image.count(roof_band(&weather.world, key));
    assert!(evidence.gentle_roof_pixels > 20);
    drop(image);
    evidence.stage = "abrupt_native_motion".into();
    advance(&mut weather, &settings, &mut now, 2, evidence);
    change(&mut host, "native-move", "native-moved");
    now += Duration::from_millis(30);
    frame(&mut weather, &settings, now, &mut evidence.trace);
    let moved = bounds(target, host.id());
    evidence.abrupt_delta_x = moved.left - rects[0].left;
    evidence.abrupt_delta_y = moved.top - rects[0].top;
    // SetWindowPos uses the outer frame; Native uses DWM's visible frame.
    // Its unchanged inset cancels when comparing the measured displacement.
    assert_eq!(
        (evidence.abrupt_delta_x, evidence.abrupt_delta_y),
        (80.0, 40.0)
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
        moved
    );
    evidence.retained_after_acceleration = weather.world.drifts[&key].total_mass();
    assert!(
        evidence.retained_after_acceleration >= evidence.initial_roof_mass * 0.5
            && evidence.retained_after_acceleration < evidence.initial_roof_mass * 0.95
    );
    let moved_band = roof_band(&weather.world, key);
    advance(&mut weather, &settings, &mut now, 120, evidence);
    let bodies: Vec<_> = weather
        .world
        .flakes
        .iter()
        .filter(|b| b.skip_support == Some(key))
        .collect();
    evidence.shed_bodies = bodies.len();
    evidence.shed_mass_after_two_seconds = bodies.iter().map(|b| f64::from(b.mass)).sum();
    assert!(
        (evidence.initial_roof_mass
            - weather.world.drifts[&key].total_mass()
            - evidence.shed_mass_after_two_seconds
            - (weather.world.removed_mass - removed_before))
            .abs()
            < evidence.introduced_mass * 1e-5
    );
    assert!(evidence.shed_bodies > 0 && evidence.shed_bodies <= 128);
    assert!(bodies.iter().all(|b| {
        weather
            .world
            .render_pose(b, weather.clock.fraction())
            .bounds(b)
            .top
            > rects[0].top + 12.0
    }));
    let image = snapshot(&mut weather, &settings, now, evidence);
    evidence.old_roof_pixels_after_two_seconds = image.count(old_band);
    evidence.moved_roof_pixels = image.count(moved_band);
    let falling = Rect {
        left: rects[0].left - 80.0,
        right: moved.right + 80.0,
        top: rects[0].top + 12.0,
        bottom: weather.layers[&screen_key].screen.floor - 40.0,
    };
    evidence.falling_pixels_after_two_seconds = image.count(falling);
    assert_eq!(evidence.old_roof_pixels_after_two_seconds, 0);
    assert!(evidence.moved_roof_pixels > 20 && evidence.falling_pixels_after_two_seconds > 10);
    assert!(image.count(control_band) > 20);
    drop(image);

    evidence.stage = "close_remaining_natural_stock".into();
    change(&mut host, "native-close", "native-closed");
    now += Duration::from_millis(30);
    frame(&mut weather, &settings, now, &mut evidence.trace);
    assert!(weather.world.scene.windows.iter().all(|w| w.key != key));
    assert!(!weather.world.drifts.contains_key(&key));
    for index in 0..120 {
        let source: Vec<_> = weather
            .world
            .flakes
            .iter()
            .filter(|b| b.skip_support == Some(key))
            .collect();
        if !source.is_empty()
            && weather.world.pending_collapse_count() == 0
            && source.iter().all(|b| {
                weather
                    .world
                    .render_pose(b, weather.clock.fraction())
                    .bounds(b)
                    .top
                    > moved.top + 12.0
            })
        {
            break;
        }
        advance(&mut weather, &settings, &mut now, 1, evidence);
        evidence.close_early_steps = index + 1;
    }
    for body in weather
        .world
        .flakes
        .iter()
        .filter(|b| b.skip_support == Some(key))
    {
        if body.slab.is_some() {
            evidence.close_cohesive_bodies += 1;
            evidence.close_cohesive_mass += f64::from(body.mass);
        } else {
            evidence.close_ordinary_mass += f64::from(body.mass);
        }
        assert!(
            weather
                .world
                .render_pose(body, weather.clock.fraction())
                .bounds(body)
                .top
                > moved.top + 12.0
        );
    }
    assert!(
        evidence.close_cohesive_bodies > 0
            && evidence.close_cohesive_mass > evidence.retained_after_acceleration * 0.9
    );
    let image = snapshot(&mut weather, &settings, now, evidence);
    evidence.closed_roof_pixels = image.count(moved_band);
    assert_eq!(evidence.closed_roof_pixels, 0);
    drop(image);
    evidence.stage = "landing".into();
    evidence.final_settle_updates = settle(&mut weather, &settings, &mut now, evidence);
    assert!((weather.world.drifts[&control].total_mass() - control_mass).abs() < 0.01);
    evidence.ground_gain = weather.world.drifts[&ground_key].total_mass() - ground_before;
    evidence.removed_gain = weather.world.removed_mass - removed_before;
    assert!(evidence.ground_gain > evidence.initial_roof_mass * 0.5);
    assert!(
        (evidence.ground_gain + evidence.removed_gain - evidence.initial_roof_mass).abs()
            < evidence.introduced_mass * 1e-5
    );
    let ground = &weather.world.drifts[&ground_key];
    evidence.max_ground_height_gain = heights_before
        .iter()
        .enumerate()
        .map(|(i, before)| ground.height(i, &weather.world.scene) - before)
        .fold(0.0, f32::max);
    assert!(evidence.max_ground_height_gain > 0.25);
    let image = snapshot(&mut weather, &settings, now, evidence);
    evidence.final_old_roof_pixels = image.count(old_band);
    evidence.final_moved_roof_pixels = image.count(moved_band);
    assert_eq!(
        (
            evidence.final_old_roof_pixels,
            evidence.final_moved_roof_pixels
        ),
        (0, 0)
    );
    let ground_after_alpha = alpha_band(&image, ground_band);
    assert_eq!(ground_before_alpha.len(), ground_after_alpha.len());
    evidence.ground_alpha_changes = ground_before_alpha
        .iter()
        .zip(ground_after_alpha)
        .filter(|(a, b)| **a != *b)
        .count();
    assert!(evidence.ground_alpha_changes > 0);
    assert_eq!(weather.world.introduced_mass, evidence.introduced_mass);
    assert_eq!(
        (
            weather.layers[&screen_key].hwnd,
            weather.layers[&screen_key].metrics_epoch
        ),
        (hwnd, epoch)
    );
    evidence.same_layer = true;
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
#[ignore = "explicit naturally stocked native window motion/close GPU test on a private desktop"]
fn natural_snow_follows_gentle_motion_sheds_after_acceleration_and_closes_into_a_ground_pile() {
    let evidence: Evidence = isolated(CHILD);
    assert!(evidence.completed && evidence.error.is_none());
    assert!(evidence.same_layer && evidence.native_child_exited && evidence.layers_destroyed);
    println!(
        "native_natural_motion_pixels {}",
        serde_json::to_string(&evidence).unwrap()
    );
}
#[test]
#[ignore = "helper launched only on the assigned private startup desktop"]
fn private_natural_motion_child() {
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
