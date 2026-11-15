//! Owned hidden Clear notification -> Owner/View -> real Weather.frame -> lower GPU.
use super::*;
use crate::control::Status;
use crate::platform::system_events::Conditions;

const CHILD: &str =
    "platform::weather::worker_probe::snow_pixels::drift_controls::clear::private_clear_child";

#[derive(Default, serde::Serialize, serde::Deserialize)]
pub(super) struct Consumption {
    clear_epoch: u64,
    reset_epoch: u64,
    new_clear: bool,
    full_reset: bool,
    system_ready: bool,
    rendering: bool,
    submitted: bool,
    present_calls: usize,
    ordinary_before: usize,
    material_before: usize,
    ordinary_preserved: bool,
    cached_profiles_before: usize,
    cached_profiles_after: usize,
}
fn air(world: &World, ordinary: bool) -> Vec<String> {
    world
        .flakes
        .iter()
        .filter(|b| !ordinary || (b.slab.is_none() && b.skip_support.is_none()))
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
        .collect()
}
pub(super) fn consume(weather: &mut Weather, status: &Status) -> Consumption {
    guard_control_probe();
    let conditions = Conditions::default();
    assert!(!conditions.ready());
    assert!(status.pauses.contains(&PauseReason::Manual));
    let settings = status.settings.as_ref().unwrap();
    assert!(!settings.interaction.cursor_enabled && !settings.snow.melting);
    let full_reset = weather.reset_epoch != status.reset_epoch;
    let new_clear = !full_reset && weather.epoch != status.clear_epoch;
    let ordinary = air(&weather.world, true);
    let all = air(&weather.world, false);
    let introduced = weather.world.introduced_mass;
    let cache = |weather: &Weather| {
        weather
            .layers
            .values()
            .filter_map(|layer| layer.gpu.as_ref())
            .map(|gpu| gpu.profiles.stats().3.0)
            .sum::<usize>()
    };
    let cached_profiles_before = cache(weather);
    PRESENT_TRACE.with_borrow_mut(|trace| assert!(trace.replace(Vec::new()).is_none()));
    let report = weather.frame(status, &conditions);
    let codes = PRESENT_TRACE.with_borrow_mut(Option::take).unwrap();
    assert!(codes.is_empty() && !report.rendering && !report.submitted);
    assert_eq!(weather.epoch, status.clear_epoch);
    assert_eq!(weather.reset_epoch, status.reset_epoch);
    assert_eq!(weather.world.introduced_mass, introduced);
    let cached_profiles_after = cache(weather);
    let ordinary_preserved = if new_clear {
        assert_eq!(air(&weather.world, false), ordinary);
        assert!(weather.world.drifts.values().all(|d| d.total_mass() == 0.0));
        assert_eq!(weather.world.pending_collapse_count(), 0);
        assert_eq!(weather.world.budget_snapshot().spill_bodies, 0);
        assert_eq!(cached_profiles_after, 0);
        true
    } else if !full_reset {
        assert_eq!(air(&weather.world, false), all);
        true
    } else {
        // Startup priming is permitted only before natural precipitation starts.
        assert_eq!(introduced, 0.0);
        false
    };
    Consumption {
        clear_epoch: status.clear_epoch,
        reset_epoch: status.reset_epoch,
        new_clear,
        full_reset,
        system_ready: conditions.ready(),
        rendering: report.rendering,
        submitted: report.submitted,
        present_calls: codes.len(),
        ordinary_before: ordinary.len(),
        material_before: all.len() - ordinary.len(),
        ordinary_preserved,
        cached_profiles_before,
        cached_profiles_after,
    }
}

#[derive(Default, serde::Serialize, serde::Deserialize)]
struct Evidence {
    stage: String,
    completed: bool,
    samples: Vec<Sample>,
    applied_keys: Vec<u8>,
    diagnostics: Summary,
    stock_removed: Vec<f64>,
    ordinary_pixels_after_clear: Vec<usize>,
    pixels_outside_current_bodies: Vec<usize>,
    new_stock_survived_same_token: bool,
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
    failure_location: Option<(String, u32)>,
}
fn record(e: &mut Evidence, mut sample: Sample) {
    sample.image = Vec::new();
    sample.profiles = Vec::new();
    sample.air_state = Vec::new();
    sample.body_bounds = Vec::new();
    sample.cohesive_bounds = Vec::new();
    sample.stock_bounds = Vec::new();
    e.samples.push(sample);
}
fn apply(g: &Graphics, p: &Probe, setting: Setting, action: Action, e: &mut Evidence) -> Sample {
    g.apply(p, setting, action, &mut e.applied_keys)
}
fn clear(g: &Graphics, p: &Probe, before: &Sample, e: &mut Evidence) -> Sample {
    let applied = p.clear();
    e.applied_keys.push(13);
    let after = g.sample(Action::ConsumeCommands, true);
    assert_eq!(after.trace_id, applied.view_trace_id);
    let consumed = after.command_frame.as_ref().unwrap();
    assert_eq!(
        (consumed.clear_epoch, consumed.reset_epoch),
        (applied.clear_epoch, applied.reset_epoch)
    );
    assert!(consumed.new_clear && !consumed.full_reset && consumed.ordinary_preserved);
    assert!(consumed.ordinary_before > 0 && consumed.cached_profiles_before > 0);
    assert_eq!(after.particles, consumed.ordinary_before);
    assert_eq!(after.intensity, before.intensity);
    assert_eq!(after.introduced, before.introduced);
    assert_eq!(
        (
            after.drift_mass,
            after.pending,
            after.spill_bodies,
            after.active_slabs
        ),
        (0.0, 0, 0, 0)
    );
    let lost = before.total - after.total;
    assert!(lost > 1.0 && (after.removed - before.removed - lost).abs() < 1e-6);
    assert!(after.roof_stock_alpha == 0 && after.ground_stock_alpha == 0);
    let pixels = after
        .image
        .as_chunks::<4>()
        .0
        .iter()
        .filter(|p| p[3] != 0)
        .count();
    assert!(pixels > 0);
    let outside = options::pixels_outside(&after, &after.body_bounds);
    assert_eq!(outside, 0);
    e.stock_removed.push(lost);
    e.ordinary_pixels_after_clear.push(pixels);
    e.pixels_outside_current_bodies.push(outside);
    after
}
fn run(e: &mut Evidence, desktop: &str) {
    guard_if_requested();
    let _dpi = Dpi::pmv2();
    guard_control_probe();
    let probe = Probe::new();
    let graphics = Graphics::start(&probe, desktop);
    e.stage = "prime_real_command_frame_before_precipitation".into();
    let primed = graphics.sample(Action::ConsumeCommands, false);
    assert_eq!(primed.introduced, 0.0);
    assert!(primed.command_frame.as_ref().unwrap().full_reset);
    record(e, primed);
    e.stage = "natural_stock_and_ordinary_air".into();
    let sample = apply(
        &graphics,
        &probe,
        Setting::DriftSize(100),
        Action::Steps(0),
        e,
    );
    record(e, sample);
    let sample = apply(
        &graphics,
        &probe,
        Setting::SnowIntensity(100),
        Action::Fill,
        e,
    );
    record(e, sample);
    let sample = apply(
        &graphics,
        &probe,
        Setting::SnowIntensity(0),
        Action::Rest,
        e,
    );
    assert!(sample.window_mass > 1.0 && sample.ground_mass > 1.0);
    assert!(sample.roof_stock_alpha > 0 && sample.ground_stock_alpha > 0);
    record(e, sample);
    let before = apply(
        &graphics,
        &probe,
        Setting::SnowIntensity(100),
        Action::Steps(60),
        e,
    );
    assert!(before.particles > 0 && before.drift_mass > 1.0);
    e.stage = "first_native_clear".into();
    let cleared = clear(&graphics, &probe, &before, e);
    record(e, before);
    record(e, cleared);
    e.stage = "new_precipitation_and_repeated_token".into();
    let grown = graphics.sample(Action::Fill, false);
    assert!(grown.roof_mass >= 120.0 && grown.intensity == 100);
    let held = graphics.sample(Action::ConsumeCommands, false);
    assert!(!held.command_frame.as_ref().unwrap().new_clear);
    same_material(&grown, &held);
    e.new_stock_survived_same_token = true;
    record(e, grown);
    record(e, held);
    e.stage = "native_motion_detached_material".into();
    let moving = graphics.sample(Action::MoveNativeWindow, false);
    assert!(moving.window_material > moving.window_mass);
    record(e, moving);
    e.stage = "native_close_cohesive_material".into();
    let moving = graphics.sample(Action::CloseNativeWindow, false);
    assert!(moving.active_slabs > 0);
    assert!(moving.cohesive_bounds.iter().any(|bounds| {
        moving
            .image
            .as_chunks::<4>()
            .0
            .iter()
            .enumerate()
            .any(|(i, p)| {
                let point = Point::new(
                    moving.image_origin.x + (i % moving.image_width) as f32 + 0.5,
                    moving.image_origin.y + (i / moving.image_width) as f32 + 0.5,
                );
                p[3] != 0 && bounds.contains(point)
            })
    }));
    e.stage = "second_native_clear".into();
    let cleared = clear(&graphics, &probe, &moving, e);
    assert!(cleared.command_frame.as_ref().unwrap().material_before > 0);
    record(e, moving);
    record(e, cleared);
    assert_eq!(e.applied_keys, [3, 1, 1, 1, 13, 13]);
    e.diagnostics = probe.finish();
    assert_eq!(
        (
            e.diagnostics.input,
            e.diagnostics.dispatch,
            e.diagnostics.received,
            e.diagnostics.applied
        ),
        (4, 4, 4, 4)
    );
    assert_eq!((e.diagnostics.superseded, e.diagnostics.unfinished), (3, 1));
    assert_eq!((e.diagnostics.rejected, e.diagnostics.dropped), (0, 0));
    let end = graphics.finish();
    assert_eq!(end.wakes, 6);
    assert_eq!(end.synthetic_steps, 0);
    e.renderer_wakes = end.wakes;
    e.dpi = end.dpi;
    e.error = end.trace.error;
    e.max_mass_relative_error = end.trace.max_mass_relative_error;
    e.present_calls = end.trace.present_calls;
    e.accepted_presents = end.trace.accepted_presents;
    e.occluded_presents = end.trace.occluded_presents;
    e.native_child_exited = true;
    e.gpu_thread_joined = true;
    e.layers_destroyed = true;
    e.stage = "complete".into();
    e.completed = true;
}

#[test]
#[ignore = "explicit hidden native Clear/Owner/View/Weather and natural snow/GPU test on a private desktop"]
fn native_clear_button_preserves_ordinary_snow_and_removes_natural_accumulation() {
    let evidence: Evidence = isolated(CHILD);
    assert!(evidence.completed && evidence.error.is_none());
    assert!(
        evidence.native_child_exited && evidence.gpu_thread_joined && evidence.layers_destroyed
    );
    println!(
        "native_clear_pixels {}",
        serde_json::to_string(&evidence).unwrap()
    );
}
#[test]
#[ignore = "helper launched only on the assigned private startup desktop"]
fn private_clear_child() {
    let Ok(desktop) = std::env::var(DESKTOP_ENV) else {
        return;
    };
    let path = std::env::var_os(REPORT_ENV).expect("probe report path missing");
    let mut evidence = Evidence::default();
    let failure = Arc::new(Mutex::new(None));
    let recorded = failure.clone();
    let previous_hook = std::panic::take_hook();
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
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        run(&mut evidence, &desktop)
    }));
    std::panic::set_hook(previous_hook);
    evidence.failure_location = failure.lock().unwrap().take();
    std::fs::write(path, serde_json::to_vec(&evidence).unwrap()).unwrap();
    if let Err(error) = result {
        std::panic::resume_unwind(error);
    }
}
