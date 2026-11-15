//! Real monitor inventory and a gated natural-stock transfer. No invented screen.
use super::*;
use windows::Win32::Graphics::Gdi::*;

const CHILD: &str =
    "platform::weather::worker_probe::snow_pixels::transfer::private_monitor_transfer_child";
#[derive(Default)]
struct Inventory {
    monitors: Vec<(Rect, bool)>,
    failed: bool,
}
unsafe extern "system" fn monitor(handle: HMONITOR, _: HDC, _: *mut RECT, data: LPARAM) -> BOOL {
    let inventory = unsafe { &mut *(data.0 as *mut Inventory) };
    let mut info = MONITORINFO {
        cbSize: std::mem::size_of::<MONITORINFO>() as u32,
        ..Default::default()
    };
    if !unsafe { GetMonitorInfoW(handle, &mut info) }.as_bool() {
        inventory.failed = true;
        return BOOL(0);
    }
    inventory.monitors.push((
        Rect {
            left: info.rcMonitor.left as f32,
            top: info.rcMonitor.top as f32,
            right: info.rcMonitor.right as f32,
            bottom: info.rcMonitor.bottom as f32,
        },
        info.dwFlags & MONITORINFOF_PRIMARY != 0,
    ));
    BOOL(1)
}
#[derive(Default, serde::Serialize, serde::Deserialize)]
struct Checkpoint {
    name: String,
    stock: f64,
    window_stock: f64,
    introduced: f64,
    removed: f64,
    total: f64,
    active_bodies: usize,
    queued_packets: usize,
    layer_roof_pixels: Vec<usize>,
    layer_ground_pixels: Vec<usize>,
    layer_ground_mass: Vec<f64>,
    pixels_outside_current_material: Vec<usize>,
}
#[derive(Default, serde::Serialize, serde::Deserialize)]
struct Evidence {
    stage: String,
    probe_completed: bool,
    monitors: usize,
    distinct_secondary: bool,
    primary_capture_verified: bool,
    wrong_target_did_not_consume_request: bool,
    transfer_status: String,
    bridge_status: String,
    transferred_dpi: Vec<u32>,
    checkpoints: Vec<Checkpoint>,
    source_identity_retained: bool,
    layers_retained: bool,
    native_child_exited: bool,
    layers_destroyed: bool,
    error: Option<i32>,
    max_mass_relative_error: f64,
    present_calls: usize,
    accepted_presents: usize,
    occluded_presents: usize,
    failure_location: Option<(String, u32)>,
    #[serde(skip)]
    trace: super::Evidence,
}
fn image(
    weather: &mut Weather,
    key: u64,
    settings: &Settings,
    now: Instant,
    e: &mut Evidence,
) -> Image {
    guard_control_probe();
    let gpu = weather.layers[&key].gpu.as_ref().unwrap();
    assert!(
        PIXEL_TARGET
            .replace(Some(gpu._bitmap.as_raw() as usize))
            .is_none()
    );
    snapshot_frame(weather, settings, now, &mut e.trace);
    assert!(PIXEL_TARGET.replace(None).is_some());
    let layer = &weather.layers[&key];
    let image = Image::read(layer);
    assert_eq!(
        (image.width, image.height),
        (
            layer.screen.rect.width() as usize,
            layer.screen.rect.height() as usize
        )
    );
    image
}
fn identities(weather: &Weather) -> BTreeMap<u64, (isize, usize, u64)> {
    weather
        .layers
        .iter()
        .map(|(&id, layer)| {
            (
                id,
                (
                    layer.hwnd.0 as isize,
                    layer.gpu.as_ref().unwrap().swap.as_raw() as usize,
                    layer.metrics_epoch,
                ),
            )
        })
        .collect()
}
fn stock(world: &World) -> f64 {
    world.drifts.values().map(|d| d.total_mass()).sum()
}
fn material_boxes(weather: &Weather) -> Vec<Rect> {
    let world = &weather.world;
    let fraction = weather.clock.fraction();
    let mut boxes: Vec<_> = world
        .flakes
        .iter()
        .map(|b| world.render_pose(b, fraction).bounds(b))
        .collect();
    for drift in world
        .drifts
        .values()
        .filter(|d| d.mass.iter().any(|&m| m > 0.0))
    {
        let peak = (0..drift.mass.len())
            .map(|i| drift.height(i, &world.scene))
            .fold(0.0, f32::max);
        let y = world.render_base(drift, fraction);
        boxes.push(Rect {
            left: drift.support.left,
            right: drift.support.right,
            top: y - peak,
            bottom: y,
        });
    }
    boxes
}
fn outside(image: &Image, boxes: &[Rect]) -> usize {
    let mut covered = vec![false; image.width * image.height];
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
    let pad = 2.0 * image.screen.scale;
    for b in boxes {
        let (left, right, top, bottom) = (
            x(b.left - pad),
            x(b.right + pad),
            y(b.top - pad),
            y(b.bottom + pad),
        );
        if left >= right || top >= bottom {
            continue;
        }
        for row in top..bottom {
            covered[row * image.width + left..row * image.width + right].fill(true);
        }
    }
    image
        .bytes
        .as_chunks::<4>()
        .0
        .iter()
        .zip(covered)
        .filter(|(p, mask)| p[3] != 0 && !mask)
        .count()
}
fn checkpoint(
    weather: &mut Weather,
    key: u64,
    settings: &Settings,
    now: Instant,
    name: &str,
    e: &mut Evidence,
) {
    let drift = &weather.world.drifts[&key];
    let band = Rect {
        left: drift.support.left,
        right: drift.support.right,
        top: drift.support.y
            - 40.0
                * weather
                    .world
                    .scene
                    .scale_at(Point::new(drift.support.left, drift.support.y)),
        bottom: drift.support.y + 1.0,
    };
    let keys: Vec<_> = weather.layers.keys().copied().collect();
    let boxes = material_boxes(weather);
    let mut pixels = Vec::new();
    let mut ground_pixels = Vec::new();
    let mut ground_mass = Vec::new();
    let mut outside_pixels = Vec::new();
    for screen in keys {
        let ground = &weather.world.drifts[&(screen | (1 << 63))];
        let ground_band = Rect {
            left: ground.support.left,
            right: ground.support.right,
            top: ground.support.y - 120.0 * weather.layers[&screen].screen.scale,
            bottom: ground.support.y + 1.0,
        };
        ground_mass.push(ground.total_mass());
        let captured = image(weather, screen, settings, now, e);
        pixels.push(captured.count(band));
        ground_pixels.push(captured.count(ground_band));
        outside_pixels.push(outside(&captured, &boxes));
    }
    assert!(outside_pixels.iter().all(|&n| n == 0));
    let world = &weather.world;
    e.checkpoints.push(Checkpoint {
        name: name.into(),
        stock: stock(world),
        window_stock: world.drifts[&key].total_mass(),
        introduced: world.introduced_mass,
        removed: world.removed_mass,
        total: world.total_mass(),
        active_bodies: world.flakes.len(),
        queued_packets: world.pending_collapse_count(),
        layer_roof_pixels: pixels,
        layer_ground_pixels: ground_pixels,
        layer_ground_mass: ground_mass,
        pixels_outside_current_material: outside_pixels,
    });
}
fn transition(
    weather: &mut Weather,
    host: &mut NativeTestHost,
    settings: &Settings,
    now: &mut Instant,
    command: &str,
    e: &mut Evidence,
) -> bool {
    guard_control_probe();
    let reply = host.command(command);
    if reply == "native-shared-edge-unavailable" {
        return false;
    }
    assert_eq!(reply, "native-placed");
    pump();
    // Ordinary fixed steps cross Registry's 30 ms native-event coalescing.
    for _ in 0..2 {
        *now += crate::timing::STEP;
        frame(weather, settings, *now, &mut e.trace);
    }
    true
}
fn run(e: &mut Evidence, desktop: &str) {
    guard_if_requested();
    let _dpi = Dpi::pmv2();
    guard_control_probe();
    let mut inventory = Inventory::default();
    assert!(
        unsafe {
            EnumDisplayMonitors(
                None,
                None,
                Some(monitor),
                LPARAM((&mut inventory as *mut Inventory) as isize),
            )
        }
        .as_bool()
    );
    assert!(!inventory.failed && !inventory.monitors.is_empty());
    assert!(inventory.monitors.len() <= frame_metrics::MAX_DISPLAYS);
    assert_eq!(inventory.monitors.iter().filter(|m| m.1).count(), 1);
    e.monitors = inventory.monitors.len();
    inventory.monitors.sort_by(|a, b| {
        a.0.left
            .total_cmp(&b.0.left)
            .then(a.0.top.total_cmp(&b.0.top))
    });
    let primary = inventory.monitors.iter().find(|m| m.1).unwrap().0;
    let secondary = inventory
        .monitors
        .iter()
        .find(|m| !m.1 && m.0 != primary)
        .map(|m| m.0);
    e.distinct_secondary = secondary.is_some();
    let mut settings = Settings::default();
    settings.display.selection = MonitorSelection::All;
    settings.performance.pause_fullscreen = false;
    settings.interaction.cursor_enabled = false;
    settings.snow.melting = false;
    settings.snow.intensity_percent = 0;
    settings.snow.drift_size_percent = 100;
    let mut weather = Weather::new(true).unwrap();
    let mut now = Instant::now();
    e.stage = "actual_monitor_inventory_and_selected_capture".into();
    frame(&mut weather, &settings, now, &mut e.trace);
    assert_eq!(weather.world.introduced_mass, 0.0);
    assert_eq!(weather.layers.len(), inventory.monitors.len());
    for layer in weather.layers.values() {
        assert_eq!(
            inventory
                .monitors
                .iter()
                .filter(|m| m.0 == layer.screen.rect)
                .count(),
            1
        );
    }
    let primary_key = *weather
        .layers
        .iter()
        .find(|(_, layer)| layer.screen.rect == primary)
        .unwrap()
        .0;
    let baseline_layers = identities(&weather);
    let target = weather.layers[&primary_key]
        .gpu
        .as_ref()
        .unwrap()
        ._bitmap
        .as_raw() as usize;
    let wrong = !target;
    assert!(
        weather
            .layers
            .values()
            .all(|layer| layer.gpu.as_ref().unwrap()._bitmap.as_raw() as usize != wrong)
    );
    assert!(PIXEL_TARGET.replace(Some(wrong)).is_none());
    assert!(!NEXT_PIXELS.replace(true));
    frame(&mut weather, &settings, now, &mut e.trace);
    assert!(NEXT_PIXELS.get() && FRAME_PIXELS.with_borrow(|slot| slot.is_none()));
    e.wrong_target_did_not_consume_request = true;
    assert_eq!(PIXEL_TARGET.replace(Some(target)), Some(wrong));
    frame(&mut weather, &settings, now, &mut e.trace);
    assert!(!NEXT_PIXELS.get());
    assert_eq!(PIXEL_TARGET.replace(None), Some(target));
    let empty = Image::read(&weather.layers[&primary_key]);
    assert!(empty.bytes.iter().all(|&b| b == 0));
    assert_eq!(
        (empty.width, empty.height),
        (primary.width() as usize, primary.height() as usize)
    );
    e.primary_capture_verified = true;
    drop(empty);
    if let Some(secondary) = secondary {
        e.stage = "natural_stock_on_primary".into();
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
        assert_eq!(host.command("native-place-primary"), "native-placed");
        pump();
        now += Duration::from_millis(30);
        frame(&mut weather, &settings, now, &mut e.trace);
        let root = bounds(HWND(handle as *mut _), host.id());
        assert!(
            primary.contains(Point::new(root.left, root.top))
                && root.right <= primary.right
                && root.bottom <= primary.bottom
        );
        let key = weather
            .world
            .scene
            .windows
            .iter()
            .find(|w| w.rect == root && w.collect)
            .unwrap()
            .key;
        settings.snow.intensity_percent = 100;
        for _ in 0..1800 {
            now += crate::timing::STEP;
            frame(&mut weather, &settings, now, &mut e.trace);
            if weather.world.drifts[&key].total_mass() >= 120.0 {
                break;
            }
        }
        assert!(weather.world.drifts[&key].total_mass() >= 120.0);
        settings.snow.intensity_percent = 0;
        let introduced = weather.world.introduced_mass;
        let mut quiet = 0;
        for _ in 0..7800 {
            now += crate::timing::STEP * 2;
            frame(&mut weather, &settings, now, &mut e.trace);
            assert_eq!(weather.world.introduced_mass, introduced);
            let empty = weather.world.flakes.is_empty()
                && weather.world.pending_collapse_count() == 0
                && weather.world.budget_snapshot().spill_bodies == 0;
            quiet = if empty { quiet + 1 } else { 0 };
            if quiet >= 60 {
                break;
            }
        }
        assert_eq!(quiet, 60);
        checkpoint(&mut weather, key, &settings, now, "primary", e);
        assert!(
            e.checkpoints
                .last()
                .unwrap()
                .layer_ground_mass
                .iter()
                .all(|&mass| mass > 1.0)
        );
        assert!(
            e.checkpoints
                .last()
                .unwrap()
                .layer_ground_pixels
                .iter()
                .all(|&pixels| pixels > 0)
        );
        assert!(
            e.checkpoints
                .last()
                .unwrap()
                .layer_roof_pixels
                .iter()
                .any(|&n| n > 0)
        );
        for (command, name) in [
            ("native-place-secondary", "secondary"),
            ("native-place-bridge", "shared_edge"),
            ("native-place-primary", "primary_return"),
        ] {
            e.stage = name.into();
            let roof_before = weather.world.drifts[&key].total_mass();
            if !transition(&mut weather, &mut host, &settings, &mut now, command, e) {
                assert_eq!(command, "native-place-bridge");
                e.bridge_status = "not_run_no_shared_vertical_edge".into();
                continue;
            }
            let actual = bounds(HWND(handle as *mut _), host.id());
            assert_eq!(
                weather
                    .world
                    .scene
                    .windows
                    .iter()
                    .filter(|w| w.key == key)
                    .count(),
                1
            );
            assert!(
                weather
                    .world
                    .scene
                    .windows
                    .iter()
                    .any(|w| w.key == key && w.rect == actual)
            );
            assert_eq!(weather.world.introduced_mass, introduced);
            let roof_after = weather.world.drifts[&key].total_mass();
            assert!(roof_after > 0.0 && roof_after <= roof_before + roof_before.max(1.0) * 1e-3);
            assert_eq!(identities(&weather), baseline_layers);
            if command == "native-place-secondary" {
                assert!(
                    secondary.contains(Point::new(actual.left, actual.top))
                        && actual.right <= secondary.right
                        && actual.bottom <= secondary.bottom
                );
            }
            if command == "native-place-primary" {
                assert!(
                    primary.contains(Point::new(actual.left, actual.top))
                        && actual.right <= primary.right
                        && actual.bottom <= primary.bottom
                );
            }
            e.transferred_dpi
                .push(unsafe { GetDpiForWindow(HWND(handle as *mut _)) });
            checkpoint(&mut weather, key, &settings, now, name, e);
            for (&before, &after) in e.checkpoints[0]
                .layer_ground_mass
                .iter()
                .zip(&e.checkpoints.last().unwrap().layer_ground_mass)
            {
                assert!((after - before).abs() < before.max(1.0) * 1e-5);
            }
            let pixels = &e.checkpoints.last().unwrap().layer_roof_pixels;
            assert!(pixels.iter().any(|&n| n > 0));
            if command == "native-place-bridge" {
                assert!(
                    actual.left < primary.right.min(secondary.right)
                        && actual.right > primary.left.max(secondary.left)
                );
                assert!(pixels.iter().filter(|&&n| n > 0).count() >= 2);
                e.bridge_status = "passed".into();
            }
        }
        e.source_identity_retained = true;
        e.layers_retained = true;
        host.finish();
        e.native_child_exited = true;
        e.transfer_status = "passed".into();
    } else {
        e.transfer_status = "not_run_distinct_secondary_unavailable".into();
        e.bridge_status = "not_run_distinct_secondary_unavailable".into();
        assert!(e.checkpoints.is_empty() && e.transferred_dpi.is_empty());
    }
    let handles: Vec<_> = weather.layers.values().map(|layer| layer.hwnd).collect();
    drop(weather);
    assert!(
        handles
            .iter()
            .all(|&hwnd| !unsafe { IsWindow(Some(hwnd)) }.as_bool())
    );
    e.layers_destroyed = true;
    e.stage = "complete".into();
    e.probe_completed = true;
}

#[test]
#[ignore = "explicit selected-GPU capture and real distinct-monitor transfer probe on a private desktop"]
fn native_real_monitor_transfer_reports_missing_secondary_without_substitution() {
    let evidence: Evidence = isolated(CHILD);
    assert!(
        evidence.probe_completed
            && evidence.primary_capture_verified
            && evidence.wrong_target_did_not_consume_request
    );
    assert!(evidence.error.is_none() && evidence.layers_destroyed);
    assert_eq!(
        evidence.transfer_status == "passed",
        evidence.distinct_secondary
    );
    println!(
        "native_monitor_transfer {}",
        serde_json::to_string(&evidence).unwrap()
    );
}
#[test]
#[ignore = "helper launched only on the assigned private startup desktop"]
fn private_monitor_transfer_child() {
    let Ok(desktop) = std::env::var(DESKTOP_ENV) else {
        return;
    };
    let path = std::env::var_os(REPORT_ENV).expect("probe report path missing");
    let mut e = Evidence::default();
    let failure = Arc::new(Mutex::new(None));
    let recorded = failure.clone();
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        if let Some(location) = info.location() {
            recorded.lock().unwrap().get_or_insert_with(|| {
                (
                    location
                        .file()
                        .rsplit(['/', '\\'])
                        .next()
                        .unwrap()
                        .to_owned(),
                    location.line(),
                )
            });
        }
    }));
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| run(&mut e, &desktop)));
    std::panic::set_hook(previous);
    e.failure_location = failure.lock().unwrap().take();
    e.error = e.trace.error;
    e.max_mass_relative_error = e.trace.max_mass_relative_error;
    e.present_calls = e.trace.present_calls;
    e.accepted_presents = e.trace.accepted_presents;
    e.occluded_presents = e.trace.occluded_presents;
    std::fs::write(path, serde_json::to_vec(&e).unwrap()).unwrap();
    if let Err(error) = result {
        std::panic::resume_unwind(error);
    }
}
