//! Genuine child windows -> Native -> Weather/World -> GPU readback.
//! Simulation time is explicit; no input desktop, input injection or visible-frame claim.
use super::*;
use crate::platform::scene::NativeTestHost;
use windows::Win32::Graphics::Dwm::*;

const CHILD: &str = "platform::weather::worker_probe::snow_pixels::private_snow_pixels_child";

#[derive(Default, serde::Serialize, serde::Deserialize)]
struct Evidence {
    stage: String,
    completed: bool,
    error: Option<i32>,
    dpi: u32,
    introduced_mass: f64,
    max_mass_relative_error: f64,
    initial_roof_pixels: usize,
    post_present_roof_pixels: usize,
    old_position_pixels_after_move: usize,
    moved_roof_pixels: usize,
    falling_material_pixels: usize,
    old_roof_pixels_after_landing: usize,
    ground_pixels: usize,
    ground_mass: f64,
    ground_peak: f32,
    control_mass: f64,
    close_checkpoint_ms: u64,
    cohesive_bodies_after_close: usize,
    settle_frames: usize,
    free_bodies_at_30s: usize,
    cohesive_bodies_at_30s: usize,
    pending_at_30s: usize,
    present_calls: usize,
    accepted_presents: usize,
    occluded_presents: usize,
    other_present_codes: BTreeMap<i32, usize>,
    native_child_exited: bool,
    layers_destroyed: bool,
}

fn pump() {
    let deadline = Instant::now() + Duration::from_millis(150);
    loop {
        unsafe {
            let mut message = MSG::default();
            for _ in 0..512 {
                if !PeekMessageW(&mut message, None, 0, 0, PM_REMOVE).as_bool() {
                    break;
                }
                assert_ne!(message.message, WM_QUIT);
                let _ = TranslateMessage(&message);
                DispatchMessageW(&message);
            }
        }
        if Instant::now() >= deadline {
            return;
        }
        std::thread::sleep(Duration::from_millis(2));
    }
}

fn conserved(world: &World, evidence: &mut Evidence) {
    let relative = (world.introduced_mass - world.removed_mass - world.total_mass()).abs()
        / world.introduced_mass.max(1.0);
    evidence.max_mass_relative_error = evidence.max_mass_relative_error.max(relative);
    assert!(relative < 1e-5);
}

fn frame(weather: &mut Weather, settings: &Settings, now: Instant, evidence: &mut Evidence) {
    guard_if_requested();
    PRESENT_TRACE.with_borrow_mut(|trace| assert!(trace.replace(Vec::new()).is_none()));
    let result = weather.update(now, settings, RenderQuality::for_settings(settings, false));
    let codes = PRESENT_TRACE.with_borrow_mut(Option::take).unwrap();
    if let Err(error) = &result {
        evidence.error = Some(error.code().0);
    }
    let accepted = result.unwrap();
    assert!(weather.tracker.ready(), "native checkpoint not ready");
    assert_eq!(codes.len(), weather.layers.len());
    assert_eq!(
        accepted as usize,
        codes.iter().filter(|&&code| code == 0).count()
    );
    for code in codes {
        assert!(code >= 0);
        evidence.present_calls += 1;
        match code {
            0 => evidence.accepted_presents += 1,
            0x087a0001 => evidence.occluded_presents += 1,
            _ => {
                assert!(evidence.other_present_codes.len() < 16);
                *evidence.other_present_codes.entry(code).or_default() += 1;
            }
        }
    }
    assert!(weather.graphics.error.is_none());
    conserved(&weather.world, evidence);
}

fn snapshot_frame(
    weather: &mut Weather,
    settings: &Settings,
    now: Instant,
    evidence: &mut Evidence,
) {
    assert!(!NEXT_PIXELS.replace(true));
    frame(weather, settings, now, evidence);
    assert!(!NEXT_PIXELS.get());
}

fn bounds(hwnd: HWND, owner: u32) -> Rect {
    let mut pid = 0;
    assert_ne!(unsafe { GetWindowThreadProcessId(hwnd, Some(&mut pid)) }, 0);
    assert_eq!(pid, owner);
    assert!(
        unsafe {
            AreDpiAwarenessContextsEqual(
                GetWindowDpiAwarenessContext(hwnd),
                DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
            )
        }
        .as_bool()
    );
    let mut rect = RECT::default();
    unsafe {
        DwmGetWindowAttribute(
            hwnd,
            DWMWA_EXTENDED_FRAME_BOUNDS,
            (&mut rect as *mut RECT).cast(),
            std::mem::size_of_val(&rect) as u32,
        )
    }
    .unwrap();
    Rect {
        left: rect.left as f32,
        top: rect.top as f32,
        right: rect.right as f32,
        bottom: rect.bottom as f32,
    }
}

fn roof_band(world: &World, key: u64) -> Rect {
    let drift = &world.drifts[&key];
    Rect {
        left: drift.support.left + 10.0,
        right: drift.support.right - 10.0,
        top: drift.support.y - 10.0,
        bottom: drift.support.y - 1.0,
    }
}
struct Image {
    screen: Screen,
    width: usize,
    height: usize,
    bytes: Vec<u8>,
}
impl Image {
    fn read(layer: &Layer) -> Self {
        let pixels = FRAME_PIXELS
            .with_borrow_mut(Option::take)
            .expect("pre-Present pixels missing");
        Self {
            screen: layer.screen.clone(),
            width: pixels.width,
            height: pixels.height,
            bytes: pixels.bytes,
        }
    }
    fn count(&self, rect: Rect) -> usize {
        let x = |value: f32| {
            (value - self.screen.rect.left)
                .ceil()
                .clamp(0.0, self.width as f32) as usize
        };
        let y = |value: f32| {
            (value - self.screen.rect.top)
                .ceil()
                .clamp(0.0, self.height as f32) as usize
        };
        let mut count = 0;
        for row in y(rect.top)..y(rect.bottom) {
            for column in x(rect.left)..x(rect.right) {
                count += usize::from(self.bytes[(row * self.width + column) * 4 + 3] > 0);
            }
        }
        count
    }
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
    frame(&mut weather, &settings, base, evidence);
    assert!(weather.world.scene.windows.is_empty());
    assert_eq!(weather.layers.len(), 1);
    let screen_key = *weather.layers.keys().next().unwrap();
    let layer_hwnd = weather.layers[&screen_key].hwnd;
    evidence.dpi = unsafe { GetDpiForWindow(layer_hwnd) };
    // This explicit fixture requires the primary to contain its two native windows.
    assert!(
        weather.layers[&screen_key]
            .screen
            .rect
            .contains(Point::new(500.0, 250.0))
    );

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
    evidence.stage = "native_roofs".into();
    let mut now = base + Duration::from_millis(30);
    frame(&mut weather, &settings, now, evidence);
    let rects: Vec<_> = handles
        .iter()
        .map(|&h| bounds(HWND(h as *mut _), host.id()))
        .collect();
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
    assert_ne!(keys[0], keys[1]);
    for &key in &keys {
        let drift = weather.world.drifts.get_mut(&key).unwrap();
        drift.mass.fill(5.0 * drift.cell_width);
    }
    weather.world.introduced_mass = weather.world.total_mass();
    evidence.introduced_mass = weather.world.introduced_mass;
    let initial_roof_mass = weather.world.drifts[&keys[0]].total_mass();
    let control_mass = weather.world.drifts[&keys[1]].total_mass();
    evidence.control_mass = control_mass;
    now += Duration::from_millis(30);
    snapshot_frame(&mut weather, &settings, now, evidence);
    let old_band = roof_band(&weather.world, keys[0]);
    let control_band = roof_band(&weather.world, keys[1]);
    let image = Image::read(&weather.layers[&screen_key]);
    evidence.initial_roof_pixels = image.count(old_band);
    let gpu = weather.layers[&screen_key].gpu.as_ref().unwrap();
    let after = Image {
        screen: image.screen.clone(),
        width: image.width,
        height: image.height,
        bytes: unsafe { tests::profile_pixels(gpu) }.unwrap(),
    };
    evidence.post_present_roof_pixels = after.count(old_band);
    assert!(evidence.initial_roof_pixels > 50 && image.count(control_band) > 50);

    evidence.stage = "native_move".into();
    assert_eq!(host.command("native-slide"), "native-slid");
    pump();
    now += Duration::from_millis(30);
    frame(&mut weather, &settings, now, evidence);
    let moved_rect = bounds(HWND(handles[0] as *mut _), host.id());
    assert_eq!(moved_rect.top, rects[0].top + 20.0);
    assert_eq!(
        weather
            .world
            .scene
            .windows
            .iter()
            .find(|w| w.key == keys[0])
            .unwrap()
            .rect,
        moved_rect
    );
    let moved_band = roof_band(&weather.world, keys[0]);
    for step in 0..40 {
        now += crate::timing::STEP;
        if step == 39 {
            snapshot_frame(&mut weather, &settings, now, evidence);
        } else {
            frame(&mut weather, &settings, now, evidence);
        }
    }
    let image = Image::read(&weather.layers[&screen_key]);
    evidence.old_position_pixels_after_move = image.count(old_band);
    evidence.moved_roof_pixels = image.count(moved_band);
    assert_eq!(evidence.old_position_pixels_after_move, 0);
    assert!(evidence.moved_roof_pixels > 50 && image.count(control_band) > 50);
    assert!((weather.world.drifts[&keys[1]].total_mass() - control_mass).abs() < 0.01);

    evidence.stage = "native_close".into();
    assert_eq!(host.command("native-close"), "native-closed");
    pump();
    now += Duration::from_millis(30);
    evidence.close_checkpoint_ms = now.duration_since(base).as_millis() as u64;
    assert!(
        evidence.close_checkpoint_ms < 1000,
        "periodic refresh must not mask native close delivery"
    );
    frame(&mut weather, &settings, now, evidence);
    assert!(!weather.world.drifts.contains_key(&keys[0]));
    assert!(weather.world.scene.windows.iter().all(|w| w.key != keys[0]));
    assert!(!unsafe { IsWindow(Some(HWND(handles[0] as *mut _))) }.as_bool());
    evidence.cohesive_bodies_after_close = weather
        .world
        .flakes
        .iter()
        .filter(|b| b.slab.is_some())
        .count();
    assert!(evidence.cohesive_bodies_after_close > 0 || weather.world.pending_collapse_count() > 0);
    for step in 0..20 {
        now += crate::timing::STEP;
        if step == 19 {
            snapshot_frame(&mut weather, &settings, now, evidence);
        } else {
            frame(&mut weather, &settings, now, evidence);
        }
    }
    let falling_area = Rect {
        left: moved_band.left - 20.0,
        right: moved_band.right + 20.0,
        top: moved_band.bottom + 10.0,
        bottom: weather.layers[&screen_key].screen.floor - 30.0,
    };
    evidence.falling_material_pixels =
        Image::read(&weather.layers[&screen_key]).count(falling_area);
    assert!(evidence.falling_material_pixels > 50);

    evidence.stage = "landing".into();
    for step in 0..7200 {
        now += crate::timing::STEP;
        frame(&mut weather, &settings, now, evidence);
        evidence.settle_frames = step + 1;
        if step == 1799 {
            evidence.free_bodies_at_30s = weather
                .world
                .flakes
                .iter()
                .filter(|b| b.slab.is_none())
                .count();
            evidence.cohesive_bodies_at_30s = weather
                .world
                .flakes
                .iter()
                .filter(|b| b.slab.is_some())
                .count();
            evidence.pending_at_30s = weather.world.pending_collapse_count();
        }
        if weather.world.flakes.is_empty() && weather.world.pending_collapse_count() == 0 {
            break;
        }
    }
    assert!(weather.world.flakes.is_empty() && weather.world.pending_collapse_count() == 0);
    let ground = &weather.world.drifts[&(screen_key | (1 << 63))];
    evidence.ground_mass = ground.total_mass();
    evidence.ground_peak = (0..ground.mass.len())
        .map(|i| ground.height(i, &weather.world.scene))
        .fold(0.0, f32::max);
    assert!(
        (evidence.ground_mass + weather.world.removed_mass - initial_roof_mass).abs()
            < evidence.introduced_mass * 1e-5
    );
    assert!((weather.world.drifts[&keys[1]].total_mass() - control_mass).abs() < 0.01);
    assert!(evidence.ground_peak > 1.0);
    let ground_band = Rect {
        left: ground.support.left,
        right: ground.support.right,
        top: ground.support.y - 30.0,
        bottom: ground.support.y,
    };
    now += crate::timing::STEP;
    snapshot_frame(&mut weather, &settings, now, evidence);
    let image = Image::read(&weather.layers[&screen_key]);
    evidence.old_roof_pixels_after_landing = image.count(moved_band);
    evidence.ground_pixels = image.count(ground_band);
    assert_eq!(evidence.old_roof_pixels_after_landing, 0);
    assert!(evidence.ground_pixels > 50 && image.count(control_band) > 50);
    conserved(&weather.world, evidence);

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
#[ignore = "explicit cross-process Native/Weather/GPU pixel test on a private desktop"]
fn native_window_snow_moves_without_old_pixels_and_closes_into_a_rendered_ground_pile() {
    let evidence: Evidence = isolated(CHILD);
    assert!(evidence.completed && evidence.error.is_none());
    assert!(evidence.native_child_exited && evidence.layers_destroyed);
    println!(
        "native_snow_pixels {}",
        serde_json::to_string(&evidence).unwrap()
    );
}

#[test]
#[ignore = "helper launched only on the assigned private startup desktop"]
fn private_snow_pixels_child() {
    let Ok(expected) = std::env::var(DESKTOP_ENV) else {
        return;
    };
    let path = std::env::var_os(REPORT_ENV).expect("probe report path missing");
    let mut evidence = Evidence::default();
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        run(&mut evidence, &expected)
    }));
    std::fs::write(path, serde_json::to_vec(&evidence).unwrap()).unwrap();
    if let Err(error) = result {
        std::panic::resume_unwind(error);
    }
}
