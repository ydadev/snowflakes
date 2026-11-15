//! Native drift slider -> saved profile/View -> natural stock and real GPU pixels.
use super::*;
use crate::platform::runtime::control_probe::{Probe, Summary};
use crate::snow::Cursor;
use crate::{control::Setting, session::PauseReason, ui::View};
use std::sync::{Arc, Mutex, mpsc};

const CHILD: &str =
    "platform::weather::worker_probe::snow_pixels::drift_controls::private_drift_control_child";

#[path = "drift_controls/cursor.rs"]
mod cursor;
#[path = "drift_controls/melting.rs"]
mod melting;
#[path = "drift_controls/options.rs"]
mod options;
#[path = "drift_controls/zero.rs"]
mod zero;

#[derive(Default, serde::Serialize, serde::Deserialize)]
struct Sample {
    trace_id: Option<u64>,
    intensity: u8,
    limit: u8,
    visibility: u8,
    on_windows: bool,
    on_ground: bool,
    melting: bool,
    cursor_enabled: bool,
    spill_bodies: usize,
    active_slabs: usize,
    min_body_age: f32,
    max_body_age: f32,
    roof_left: f32,
    roof_y: f32,
    roof_cell_width: f32,
    #[serde(skip)]
    roof_key: u64,
    steps: usize,
    particles: usize,
    pending: usize,
    introduced: f64,
    removed: f64,
    total: f64,
    roof_mass: f64,
    ground_mass: f64,
    window_mass: f64,
    window_material: f64,
    drift_mass: f64,
    air_mass: f64,
    roof_excess: f64,
    ground_excess: f64,
    all_excess: f64,
    roof_peak: f32,
    roof_alpha: u64,
    roof_stock_alpha: u64,
    ground_stock_alpha: u64,
    #[serde(skip)]
    profiles: Vec<(u64, Vec<f32>, Vec<f32>)>,
    #[serde(skip)]
    image: Vec<u8>,
    #[serde(skip)]
    air_state: Vec<String>,
    #[serde(skip)]
    body_bounds: Vec<Rect>,
    #[serde(skip)]
    cohesive_bounds: Vec<Rect>,
    #[serde(skip)]
    stock_bounds: Vec<Rect>,
    #[serde(skip)]
    image_origin: Point,
    #[serde(skip)]
    image_width: usize,
    #[serde(skip)]
    image_height: usize,
    #[serde(skip)]
    image_scale: f32,
}
#[derive(Default, serde::Serialize, serde::Deserialize)]
struct Evidence {
    stage: String,
    completed: bool,
    samples: Vec<Sample>,
    applied_keys: Vec<u8>,
    diagnostics: Summary,
    same_time_reduction_preserved: bool,
    same_time_increase_preserved: bool,
    no_precipitation_no_new_mass: bool,
    new_precipitation_grew_roof: bool,
    renderer_wakes: usize,
    dpi: u32,
    native_child_exited: bool,
    gpu_thread_joined: bool,
    layers_destroyed: bool,
    error: Option<i32>,
    max_mass_relative_error: f64,
    present_calls: usize,
    accepted_presents: usize,
    occluded_presents: usize,
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
#[derive(Clone, Copy)]
enum Action {
    Fill,
    Settle,
    Rest,
    Steps(usize),
    Cursor { steps: usize, input: Cursor },
}
enum Request {
    Sample {
        action: Action,
        publication: bool,
        reply: mpsc::SyncSender<Sample>,
    },
    Stop,
}
struct End {
    trace: super::Evidence,
    dpi: u32,
    wakes: usize,
    synthetic_steps: usize,
}
struct Graphics {
    send: mpsc::Sender<Request>,
    join: Option<std::thread::JoinHandle<End>>,
}
fn wait_publication() {
    guard_control_probe();
    let deadline = Instant::now() + Duration::from_secs(2);
    let mut message = MSG::default();
    loop {
        if unsafe {
            PeekMessageW(
                &mut message,
                Some(HWND(-1_isize as *mut _)),
                WM_NULL,
                WM_NULL,
                PM_REMOVE,
            )
        }
        .as_bool()
        {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "saved View did not wake GPU queue"
        );
        std::thread::sleep(Duration::from_millis(1));
    }
}
fn inspect(weather: &Weather, settings: &Settings, key: u64, image: Image) -> Sample {
    let world = &weather.world;
    let roof = &world.drifts[&key];
    let budget = world.budget_snapshot();
    let mut sample = Sample {
        intensity: settings.snow.intensity_percent,
        limit: settings.snow.drift_size_percent,
        visibility: settings.snow.flake_visibility_percent,
        on_windows: settings.snow.on_windows,
        on_ground: settings.snow.on_ground,
        melting: settings.snow.melting,
        cursor_enabled: settings.interaction.cursor_enabled,
        spill_bodies: budget.spill_bodies,
        active_slabs: budget.active_slabs,
        min_body_age: world
            .flakes
            .iter()
            .map(|b| b.age)
            .reduce(f32::min)
            .unwrap_or(0.0),
        max_body_age: world
            .flakes
            .iter()
            .map(|b| b.age)
            .reduce(f32::max)
            .unwrap_or(0.0),
        roof_key: key,
        roof_left: roof.support.left,
        roof_y: roof.support.y,
        roof_cell_width: roof.cell_width,
        particles: world.flakes.len(),
        pending: world.pending_collapse_count(),
        introduced: world.introduced_mass,
        removed: world.removed_mass,
        total: world.total_mass(),
        roof_mass: roof.total_mass(),
        ground_mass: world
            .drifts
            .values()
            .filter(|d| d.support.ground)
            .map(|d| d.total_mass())
            .sum(),
        window_mass: world
            .drifts
            .values()
            .filter(|d| !d.support.ground)
            .map(|d| d.total_mass())
            .sum(),
        drift_mass: world.drifts.values().map(|d| d.total_mass()).sum(),
        air_mass: world.flakes.iter().map(|b| f64::from(b.mass)).sum(),
        air_state: world
            .flakes
            .iter()
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
                        b.display_scale(world.scene.scale_at(b.position)),
                    )
                )
            })
            .collect(),
        roof_peak: (0..roof.mass.len())
            .map(|i| roof.height(i, &world.scene))
            .fold(0.0, f32::max),
        ..Default::default()
    };
    // Static, cursor-free fixture: only windows can release material. Include
    // their queued spill through total_mass, not just already admitted bodies.
    sample.window_material = sample.total - sample.ground_mass - sample.air_mass
        + world
            .flakes
            .iter()
            .filter(|b| {
                b.skip_support
                    .is_some_and(|id| world.drifts.get(&id).is_some_and(|d| !d.support.ground))
            })
            .map(|b| f64::from(b.mass))
            .sum::<f64>();
    sample.body_bounds = world
        .flakes
        .iter()
        .map(|b| world.render_pose(b, weather.clock.fraction()).bounds(b))
        .collect();
    sample.cohesive_bounds = world
        .flakes
        .iter()
        .zip(&sample.body_bounds)
        .filter(|(b, _)| b.slab.is_some())
        .map(|(_, bounds)| *bounds)
        .collect();
    sample.image_origin = Point::new(image.screen.rect.left, image.screen.rect.top);
    sample.image_width = image.width;
    sample.image_height = image.height;
    sample.image_scale = image.screen.scale;
    // Independent f64 capacity oracle on this unchanged, uniform-DPI scene.
    // No product capacity/remove helper, private scale mutation or planted stock.
    for (&id, drift) in &world.drifts {
        let mut excess = 0.0;
        for i in 0..drift.mass.len() {
            let scale = f64::from(
                world
                    .scene
                    .scale_at(Point::new(drift.x(i), drift.support.y - 1.0)),
            );
            let max_depth = if drift.support.ground { 120.0 } else { 32.0 };
            let active = if drift.support.ground {
                sample.on_ground
            } else {
                sample.on_windows
            };
            let depth = if active {
                (max_depth * f64::from(sample.limit) / 100.0 * scale)
                    .min(f64::from(world.scene.headroom(drift.x(i), drift.support.y)))
            } else {
                0.0
            };
            let cap = depth * f64::from(drift.cell_width) / (scale * scale);
            excess += (f64::from(drift.mass[i]) + f64::from(drift.pending[i]) - cap).max(0.0);
        }
        sample.all_excess += excess;
        if id == key {
            sample.roof_excess = excess;
        }
        if drift.support.ground {
            sample.ground_excess += excess;
        }
        sample
            .profiles
            .push((id, drift.mass.clone(), drift.pending.clone()));
        if drift.mass.iter().any(|&m| m >= 0.001) {
            let peak = (0..drift.mass.len())
                .map(|i| drift.height(i, &world.scene))
                .fold(0.0, f32::max);
            sample.stock_bounds.push(Rect {
                left: drift.support.left,
                right: drift.support.right,
                top: drift.support.y - peak,
                bottom: drift.support.y,
            });
        }
    }
    let band = Rect {
        left: roof.support.left,
        right: roof.support.right,
        top: roof.support.y - 40.0,
        bottom: roof.support.y - 1.0,
    };
    (sample.roof_alpha, sample.roof_stock_alpha) = band_alpha(&image, band, &sample.body_bounds);
    let ground = world.drifts.values().find(|d| d.support.ground).unwrap();
    let ground_band = Rect {
        left: ground.support.left,
        right: ground.support.right,
        top: ground.support.y - 40.0,
        bottom: ground.support.y,
    };
    sample.ground_stock_alpha = band_alpha(&image, ground_band, &sample.body_bounds).1;
    sample.image = image.bytes;
    sample
}
fn band_alpha(image: &Image, band: Rect, bodies: &[Rect]) -> (u64, u64) {
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
    let (left, right, top, bottom) = (x(band.left), x(band.right), y(band.top), y(band.bottom));
    let (width, height) = (right.saturating_sub(left), bottom.saturating_sub(top));
    if width == 0 || height == 0 {
        return (0, 0);
    }
    let mut covered = vec![false; width * height];
    let pad = image.screen.scale * 2.0;
    for body in bodies {
        let (a, b) = (x(body.left - pad).max(left), x(body.right + pad).min(right));
        let (c, d) = (y(body.top - pad).max(top), y(body.bottom + pad).min(bottom));
        if a >= b || c >= d {
            continue;
        }
        for row in c..d {
            covered[(row - top) * width + a - left..(row - top) * width + b - left].fill(true);
        }
    }
    let (mut raw, mut stock) = (0, 0);
    for row in top..bottom {
        for column in left..right {
            let alpha = u64::from(image.bytes[(row * image.width + column) * 4 + 3]);
            raw += alpha;
            if !covered[(row - top) * width + column - left] {
                stock += alpha;
            }
        }
    }
    (raw, stock)
}
fn render(
    view: Arc<Mutex<View>>,
    desktop: String,
    receive: mpsc::Receiver<Request>,
    ready: mpsc::SyncSender<u32>,
) -> End {
    guard_if_requested();
    let _dpi = Dpi::pmv2();
    guard_control_probe();
    let mut weather = Weather::new(true).unwrap();
    let mut trace = super::Evidence::default();
    let initial = view.lock().unwrap().status.settings.clone().unwrap();
    assert_eq!(initial.snow.intensity_percent, 0);
    assert!(!initial.snow.melting && !initial.interaction.cursor_enabled);
    assert!(initial.snow.on_windows && initial.snow.on_ground);
    assert_eq!(initial.snow.wind_percent, 0);
    let mut now = Instant::now();
    frame(&mut weather, &initial, now, &mut trace);
    assert_eq!(weather.world.total_mass(), 0.0);
    assert_eq!(weather.world.introduced_mass, 0.0);
    assert_eq!(weather.layers.len(), 1);
    let hwnd = weather.layers.values().next().unwrap().hwnd;
    let epoch = weather.layers.values().next().unwrap().metrics_epoch;
    let dpi = unsafe { GetDpiForWindow(hwnd) };
    let mut host = NativeTestHost::new(&desktop);
    let created = host.command("native-create");
    let handle: usize = created
        .strip_prefix("native-created ")
        .unwrap()
        .split_whitespace()
        .next()
        .unwrap()
        .parse()
        .unwrap();
    assert_eq!(host.command("native-wide"), "native-resized");
    pump();
    let rect = bounds(HWND(handle as *mut _), host.id());
    now += Duration::from_millis(30);
    frame(&mut weather, &initial, now, &mut trace);
    let key = weather
        .world
        .scene
        .windows
        .iter()
        .find(|w| w.rect == rect && w.collect && w.visible)
        .unwrap()
        .key;
    assert_eq!(weather.world.drifts[&key].total_mass(), 0.0);
    let mut message = MSG::default();
    unsafe {
        let _ = PeekMessageW(&mut message, None, 0, 0, PM_NOREMOVE);
    }
    ready.send(unsafe { GetCurrentThreadId() }).unwrap();
    let mut wakes = 0;
    let mut synthetic_steps = 0;
    let mut last_synthetic: Option<(Point, Instant)> = None;
    while let Ok(request) = receive.recv() {
        let Request::Sample {
            action,
            publication,
            reply,
        } = request
        else {
            break;
        };
        if publication {
            wait_publication();
            wakes += 1;
        }
        let published = view.lock().unwrap().clone();
        assert!(published.status.pauses.contains(&PauseReason::Manual));
        let settings = published.status.settings.unwrap();
        let introduced = weather.world.introduced_mass;
        let limit = match action {
            Action::Fill => 1800,
            Action::Settle | Action::Rest => 7200,
            Action::Steps(n) => n,
            Action::Cursor { steps, .. } => steps,
        };
        let mut steps = 0;
        let mut quiet_steps = 0;
        for _ in 0..limit {
            now += crate::timing::STEP;
            if let Action::Cursor { mut input, .. } = action {
                // Explicit synthetic model input on the guarded private desktop.
                // The following real update uses the same timestamp and cannot
                // advance the clock again. Never move/inject a system cursor.
                guard_control_probe();
                let scale = weather.world.scene.scale_at(input.position);
                let elapsed = steps as f32 * crate::timing::STEP.as_secs_f32();
                input.position.x += input.velocity.x * elapsed * scale;
                input.position.y += input.velocity.y * elapsed * scale;
                // Keep synthetic positions/velocities kinematically consistent,
                // including separate requests around same-time saved settings.
                if let Some((previous, time)) = last_synthetic {
                    let dt = now.duration_since(time).as_secs_f32();
                    for (current, previous, velocity) in [
                        (input.position.x, previous.x, input.velocity.x),
                        (input.position.y, previous.y, input.velocity.y),
                    ] {
                        let tolerance =
                            4.0 * f32::EPSILON * current.abs().max(previous.abs()).max(1.0);
                        assert!(((current - previous) - velocity * dt * scale).abs() <= tolerance);
                    }
                } else {
                    assert_eq!(input.velocity, Point::default());
                }
                last_synthetic = Some((input.position, now));
                weather.advance_models(
                    now,
                    true,
                    &settings,
                    RenderQuality::for_settings(&settings, false),
                    Some(input),
                );
                synthetic_steps += 1;
            }
            frame(&mut weather, &settings, now, &mut trace);
            steps += 1;
            if settings.snow.intensity_percent == 0 {
                assert_eq!(weather.world.introduced_mass, introduced);
            }
            let empty = weather.world.flakes.is_empty()
                && weather.world.pending_collapse_count() == 0
                && weather.world.budget_snapshot().spill_bodies == 0;
            quiet_steps = if empty { quiet_steps + 1 } else { 0 };
            let done = match action {
                Action::Fill => weather.world.drifts[&key].total_mass() >= 120.0,
                Action::Settle => empty,
                Action::Rest => quiet_steps >= 120,
                Action::Steps(_) | Action::Cursor { .. } => false,
            };
            if done {
                break;
            }
        }
        snapshot_frame(&mut weather, &settings, now, &mut trace);
        let layer = weather.layers.values().next().unwrap();
        assert_eq!((layer.hwnd, layer.metrics_epoch), (hwnd, epoch));
        let mut sample = inspect(&weather, &settings, key, Image::read(layer));
        sample.trace_id = published.trace.map(|t| t.id);
        sample.steps = steps;
        match action {
            Action::Fill => assert!(sample.roof_mass >= 120.0),
            Action::Settle | Action::Rest => assert_eq!(
                (sample.particles, sample.pending, sample.spill_bodies),
                (0, 0, 0)
            ),
            Action::Steps(_) | Action::Cursor { .. } => {}
        }
        reply.send(sample).unwrap();
    }
    host.finish();
    drop(weather);
    assert!(!unsafe { IsWindow(Some(hwnd)) }.as_bool());
    guard_control_probe();
    End {
        trace,
        dpi,
        wakes,
        synthetic_steps,
    }
}
impl Graphics {
    fn start(probe: &Probe, desktop: &str) -> Self {
        let (send, receive) = mpsc::channel();
        let (ready, initialized) = mpsc::sync_channel(1);
        let view = probe.render_view();
        let desktop = desktop.to_owned();
        let graphics = Self {
            send,
            join: Some(std::thread::spawn(move || {
                render(view, desktop, receive, ready)
            })),
        };
        probe.attach_render_thread(initialized.recv_timeout(Duration::from_secs(5)).unwrap());
        graphics
    }
    fn sample(&self, action: Action, publication: bool) -> Sample {
        let (reply, response) = mpsc::sync_channel(1);
        self.send
            .send(Request::Sample {
                action,
                publication,
                reply,
            })
            .unwrap();
        response.recv_timeout(Duration::from_secs(20)).unwrap()
    }
    fn apply(
        &self,
        probe: &Probe,
        setting: Setting,
        action: Action,
        applied_keys: &mut Vec<u8>,
    ) -> Sample {
        probe.change(setting);
        let applied = probe.apply_next().expect("native drift command missing");
        let sample = self.sample(action, true);
        assert_eq!(sample.trace_id, Some(applied.trace_id));
        let settings = probe.settings();
        assert_eq!(sample.limit, settings.snow.drift_size_percent);
        assert_eq!(sample.intensity, settings.snow.intensity_percent);
        assert_eq!(sample.visibility, settings.snow.flake_visibility_percent);
        assert_eq!(sample.on_windows, settings.snow.on_windows);
        assert_eq!(sample.on_ground, settings.snow.on_ground);
        assert_eq!(sample.melting, settings.snow.melting);
        assert_eq!(sample.cursor_enabled, settings.interaction.cursor_enabled);
        applied_keys.push(applied.key);
        sample
    }
    fn finish(mut self) -> End {
        self.send.send(Request::Stop).unwrap();
        self.join
            .take()
            .unwrap()
            .join()
            .expect("drift GPU thread failed")
    }
}
impl Drop for Graphics {
    fn drop(&mut self) {
        let _ = self.send.send(Request::Stop);
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}
fn record(evidence: &mut Evidence, mut sample: Sample) {
    // Keep only safe numeric evidence; bound retained readback memory.
    sample.image = Vec::new();
    sample.profiles = Vec::new();
    sample.air_state = Vec::new();
    sample.body_bounds = Vec::new();
    sample.cohesive_bounds = Vec::new();
    evidence.samples.push(sample);
}
fn same_material(before: &Sample, after: &Sample) {
    assert_eq!(after.introduced, before.introduced);
    assert_eq!(after.removed, before.removed);
    assert_eq!(after.profiles, before.profiles);
    assert_eq!(after.air_state, before.air_state);
    assert_eq!(after.air_mass, before.air_mass);
    assert_eq!(
        (after.particles, after.pending),
        (before.particles, before.pending)
    );
}
fn unchanged(before: &Sample, after: &Sample) {
    same_material(before, after);
    assert_eq!(after.image, before.image);
}
fn apply_record(
    graphics: &Graphics,
    probe: &Probe,
    setting: Setting,
    action: Action,
    evidence: &mut Evidence,
) {
    let sample = graphics.apply(probe, setting, action, &mut evidence.applied_keys);
    record(evidence, sample);
}
fn run(evidence: &mut Evidence, desktop: &str) {
    guard_if_requested();
    let _dpi = Dpi::pmv2();
    guard_control_probe();
    evidence.stage = "natural_fill_and_settle".into();
    let probe = Probe::new();
    let graphics = Graphics::start(&probe, desktop);
    apply_record(
        &graphics,
        &probe,
        Setting::DriftSize(100),
        Action::Steps(0),
        evidence,
    );
    apply_record(
        &graphics,
        &probe,
        Setting::SnowIntensity(100),
        Action::Fill,
        evidence,
    );
    let baseline = graphics.apply(
        &probe,
        Setting::SnowIntensity(0),
        Action::Settle,
        &mut evidence.applied_keys,
    );
    assert!(baseline.roof_peak > 0.5 && baseline.roof_alpha > 0);
    let introduced = baseline.introduced;
    evidence.stage = "lower_limit".into();
    let reduced = graphics.apply(
        &probe,
        Setting::DriftSize(1),
        Action::Steps(0),
        &mut evidence.applied_keys,
    );
    unchanged(&baseline, &reduced);
    evidence.same_time_reduction_preserved = true;
    assert!(reduced.roof_excess > 1.0 && reduced.ground_excess > 1.0);
    let excess = reduced.all_excess;
    let removed_before = reduced.removed;
    record(evidence, baseline);
    record(evidence, reduced);
    let first = graphics.sample(Action::Steps(1), false);
    assert!(first.all_excess > excess * 0.8);
    assert!(first.removed > removed_before && first.removed - removed_before < excess * 0.1);
    record(evidence, first);
    let middle = graphics.sample(Action::Steps(59), false);
    assert!(middle.all_excess > excess * 0.2 && middle.all_excess < excess * 0.8);
    record(evidence, middle);
    let low = graphics.sample(Action::Steps(90), false);
    assert!(low.all_excess < 0.001 && low.roof_mass > 0.0 && low.roof_peak <= 0.321);
    assert_eq!(low.introduced, introduced);
    assert_eq!((low.particles, low.pending), (0, 0));
    assert!(low.roof_alpha < evidence.samples[2].roof_alpha);
    evidence.stage = "increase_without_new_snow".into();
    let increased = graphics.apply(
        &probe,
        Setting::DriftSize(100),
        Action::Steps(0),
        &mut evidence.applied_keys,
    );
    unchanged(&low, &increased);
    evidence.same_time_increase_preserved = true;
    let roof_before = increased.roof_mass;
    let alpha_before = increased.roof_alpha;
    let mass_before = increased.total;
    record(evidence, low);
    record(evidence, increased);
    let dry = graphics.sample(Action::Steps(120), false);
    assert_eq!(dry.introduced, introduced);
    assert!((dry.total - mass_before).abs() < 0.001);
    assert_eq!((dry.particles, dry.pending), (0, 0));
    evidence.no_precipitation_no_new_mass = true;
    record(evidence, dry);
    evidence.stage = "growth_from_new_precipitation".into();
    apply_record(
        &graphics,
        &probe,
        Setting::SnowIntensity(100),
        Action::Fill,
        evidence,
    );
    let grown = graphics.apply(
        &probe,
        Setting::SnowIntensity(0),
        Action::Settle,
        &mut evidence.applied_keys,
    );
    assert!(grown.roof_mass > roof_before + 1.0 && grown.roof_alpha > alpha_before);
    assert!(grown.introduced > introduced);
    assert!(grown.roof_mass - roof_before < grown.introduced - introduced);
    evidence.new_precipitation_grew_roof = true;
    record(evidence, grown);
    assert!(probe.apply_next().is_none());
    assert_eq!(evidence.applied_keys, [3, 1, 1, 3, 3, 1, 1]);
    evidence.diagnostics = probe.finish();
    assert_eq!(evidence.diagnostics.applied, 7);
    assert_eq!(evidence.diagnostics.input, 7);
    assert_eq!(evidence.diagnostics.dispatch, 7);
    assert_eq!(evidence.diagnostics.received, 7);
    assert_eq!(evidence.diagnostics.superseded, 6);
    assert_eq!(evidence.diagnostics.rejected, 0);
    assert_eq!(evidence.diagnostics.dropped, 0);
    assert_eq!(evidence.diagnostics.unfinished, 1);
    let end = graphics.finish();
    assert_eq!(end.wakes, 7);
    evidence.renderer_wakes = end.wakes;
    evidence.dpi = end.dpi;
    evidence.trace = end.trace;
    evidence.native_child_exited = true;
    evidence.gpu_thread_joined = true;
    evidence.layers_destroyed = true;
    evidence.stage = "complete".into();
    evidence.completed = true;
}
#[test]
#[ignore = "explicit native drift control/natural snow/GPU bridge on a private desktop"]
fn saved_drift_limit_changes_natural_snow_gradually_and_growth_needs_new_precipitation() {
    let evidence: Evidence = isolated(CHILD);
    assert!(evidence.completed && evidence.gpu_thread_joined && evidence.layers_destroyed);
    assert!(evidence.error.is_none() && evidence.native_child_exited);
    println!(
        "native_drift_control_pixels {}",
        serde_json::to_string(&evidence).unwrap()
    );
}
#[test]
#[ignore = "helper launched only on the assigned private startup desktop"]
fn private_drift_control_child() {
    let Ok(desktop) = std::env::var(DESKTOP_ENV) else {
        return;
    };
    let path = std::env::var_os(REPORT_ENV).expect("probe report path missing");
    let mut evidence = Evidence::default();
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        run(&mut evidence, &desktop)
    }));
    evidence.summarize();
    std::fs::write(path, serde_json::to_vec(&evidence).unwrap()).unwrap();
    if let Err(error) = result {
        std::panic::resume_unwind(error);
    }
}
