//! Actual melting checkbox, ordinary-emitter stock and lower GPU rendering.
use super::*;

const CHILD: &str =
    "platform::weather::worker_probe::snow_pixels::drift_controls::melting::private_melting_child";

#[derive(Default, serde::Serialize, serde::Deserialize)]
struct Evidence {
    stage: String,
    completed: bool,
    samples: Vec<Sample>,
    applied_keys: Vec<u8>,
    diagnostics: Summary,
    melt_periods: Vec<MeltPeriod>,
    same_time_checks: Vec<SameTime>,
    unchanged_setting_checks: Vec<SameTime>,
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
#[derive(Default, serde::Serialize, serde::Deserialize)]
struct SameTime {
    profiles: bool,
    air: bool,
    ledger: bool,
    pixels: bool,
    changed_pixels: usize,
    max_channel_difference: u8,
}
fn compare(before: &Sample, after: &Sample) -> SameTime {
    SameTime {
        profiles: before.profiles == after.profiles,
        air: before.air_state == after.air_state
            && before.air_mass == after.air_mass
            && (before.particles, before.pending) == (after.particles, after.pending),
        ledger: before.introduced == after.introduced && before.removed == after.removed,
        pixels: before.image == after.image,
        changed_pixels: before
            .image
            .as_chunks::<4>()
            .0
            .iter()
            .zip(after.image.as_chunks::<4>().0)
            .filter(|(a, b)| a != b)
            .count(),
        max_channel_difference: before
            .image
            .iter()
            .zip(&after.image)
            .map(|(a, b)| a.abs_diff(*b))
            .max()
            .unwrap_or(0),
    }
}
fn check_unchanged(check: &SameTime) {
    // Bound native raster/AA variation separately from exact physical inputs.
    // Also measure repeated identical settings, without a checkbox command.
    assert!(
        check.profiles
            && check.air
            && check.ledger
            && check.changed_pixels <= 8
            && check.max_channel_difference <= 16,
        "same-time material/air/ledger/bitmap changed"
    );
}
fn same_time(before: &Sample, after: &Sample, evidence: &mut Evidence) {
    let check = compare(before, after);
    evidence.same_time_checks.push(check);
    check_unchanged(evidence.same_time_checks.last().unwrap());
}
#[derive(Default, serde::Serialize, serde::Deserialize)]
struct MeltPeriod {
    window_loss: f64,
    ground_loss: f64,
    total_loss: f64,
    removed_gain: f64,
    roof_alpha_loss: i64,
    ground_alpha_loss: i64,
    profiles_changed: bool,
    pixels_changed: bool,
}
fn record(evidence: &mut Evidence, mut sample: Sample) {
    sample.image = Vec::new();
    sample.profiles = Vec::new();
    sample.air_state = Vec::new();
    sample.body_bounds = Vec::new();
    sample.cohesive_bounds = Vec::new();
    evidence.samples.push(sample);
}
fn apply(
    graphics: &Graphics,
    probe: &Probe,
    setting: Setting,
    action: Action,
    evidence: &mut Evidence,
) -> Sample {
    let expected = if let Setting::SnowMelting(value) = &setting {
        let mut settings = probe.settings();
        settings.snow.melting = *value;
        Some(settings)
    } else {
        None
    };
    let sample = graphics.apply(probe, setting, action, &mut evidence.applied_keys);
    if let Some(expected) = expected {
        assert_eq!(probe.settings(), expected);
    }
    sample
}
fn quiet(sample: &Sample) {
    assert_eq!(sample.intensity, 0);
    assert_eq!((sample.particles, sample.pending), (0, 0));
    assert_eq!(sample.air_mass, 0.0);
    assert!(sample.on_windows && sample.on_ground);
    assert_eq!(sample.limit, 100);
    assert!(sample.all_excess < 0.001);
    assert!(sample.roof_stock_alpha > 0 && sample.ground_stock_alpha > 0);
}
fn stopped(before: &Sample, after: &Sample) {
    quiet(before);
    quiet(after);
    assert!(!before.melting && !after.melting);
    assert_eq!(after.introduced, before.introduced);
    assert_eq!(after.removed, before.removed);
    assert!((after.total - before.total).abs() < 0.001);
    assert!((after.window_mass - before.window_mass).abs() < 0.001);
    assert!((after.ground_mass - before.ground_mass).abs() < 0.001);
}
fn melted(before: &Sample, after: &Sample, evidence: &mut Evidence) {
    evidence.melt_periods.push(MeltPeriod {
        window_loss: before.window_mass - after.window_mass,
        ground_loss: before.ground_mass - after.ground_mass,
        total_loss: before.total - after.total,
        removed_gain: after.removed - before.removed,
        roof_alpha_loss: before.roof_stock_alpha as i64 - after.roof_stock_alpha as i64,
        ground_alpha_loss: before.ground_stock_alpha as i64 - after.ground_stock_alpha as i64,
        profiles_changed: before.profiles != after.profiles,
        pixels_changed: before.image != after.image,
    });
    evidence.stage = "melting_quiet_and_mass".into();
    quiet(before);
    quiet(after);
    assert!(before.melting && after.melting);
    assert_eq!(after.introduced, before.introduced);
    assert!(after.window_mass < before.window_mass - 1.0);
    assert!(after.ground_mass < before.ground_mass - 1.0);
    let loss = before.total - after.total;
    assert!(loss > 2.0);
    evidence.stage = "melting_ledger".into();
    let removed = after.removed - before.removed;
    assert!(removed > loss * 0.99 && removed < loss * 1.01);
    // Difference of two independently bounded f32 ledgers, each below 1e-5.
    assert!((loss - removed).abs() < 2e-5 * before.introduced.max(1.0));
    evidence.stage = "melting_pixels".into();
    assert_ne!(after.profiles, before.profiles);
    assert_ne!(after.image, before.image);
    assert!(after.roof_stock_alpha < before.roof_stock_alpha);
    assert!(after.ground_stock_alpha < before.ground_stock_alpha);
}
fn run(evidence: &mut Evidence, desktop: &str) {
    guard_if_requested();
    let _dpi = Dpi::pmv2();
    guard_control_probe();
    let probe = Probe::new();
    let graphics = Graphics::start(&probe, desktop);
    evidence.stage = "natural_stock_and_live_air".into();
    let start = apply(
        &graphics,
        &probe,
        Setting::DriftSize(100),
        Action::Steps(0),
        evidence,
    );
    record(evidence, start);
    let mut falling = apply(
        &graphics,
        &probe,
        Setting::SnowIntensity(100),
        Action::Fill,
        evidence,
    );
    evidence.stage = "repeat_identical_settings".into();
    for _ in 0..8 {
        let next = graphics.sample(Action::Steps(0), false);
        evidence
            .unchanged_setting_checks
            .push(compare(&falling, &next));
        check_unchanged(evidence.unchanged_setting_checks.last().unwrap());
        falling = next;
    }
    assert!(falling.particles > 0 && falling.air_mass > 0.0);
    evidence.stage = "enable_with_air_command".into();
    let on = apply(
        &graphics,
        &probe,
        Setting::SnowMelting(true),
        Action::Steps(0),
        evidence,
    );
    assert!(on.melting);
    evidence.stage = "enable_with_air_material".into();
    same_time(&falling, &on, evidence);
    let off = apply(
        &graphics,
        &probe,
        Setting::SnowMelting(false),
        Action::Steps(0),
        evidence,
    );
    assert!(!off.melting);
    same_time(&on, &off, evidence);
    record(evidence, falling);
    record(evidence, on);
    record(evidence, off);
    let stock = apply(
        &graphics,
        &probe,
        Setting::SnowIntensity(0),
        Action::Settle,
        evidence,
    );
    quiet(&stock);
    evidence.stage = "disabled_before_melting".into();
    let mut held = graphics.sample(Action::Steps(120), false);
    stopped(&stock, &held);
    record(evidence, stock);
    // Two independent on/off periods also detect a stale published setting.
    for period in 0..2 {
        evidence.stage = "enabled_melting".into();
        let on = apply(
            &graphics,
            &probe,
            Setting::SnowMelting(true),
            Action::Steps(0),
            evidence,
        );
        same_time(&held, &on, evidence);
        record(evidence, held);
        // 20 model seconds at the existing small rate; no force/rate override.
        let thawed = graphics.sample(Action::Steps(1200), false);
        melted(&on, &thawed, evidence);
        let off = apply(
            &graphics,
            &probe,
            Setting::SnowMelting(false),
            Action::Steps(0),
            evidence,
        );
        same_time(&thawed, &off, evidence);
        record(evidence, on);
        record(evidence, thawed);
        evidence.stage = "disabled_after_melting".into();
        let retained = graphics.sample(Action::Steps(120), false);
        stopped(&off, &retained);
        record(evidence, off);
        // Keep the current raw bitmap only until the next same-time comparison.
        if period == 1 {
            record(evidence, retained);
            break;
        }
        held = retained;
    }
    assert!(probe.apply_next().is_none());
    assert_eq!(evidence.applied_keys, [3, 1, 25, 25, 1, 25, 25, 25, 25]);
    evidence.diagnostics = probe.finish();
    let d = &evidence.diagnostics;
    assert_eq!((d.input, d.dispatch, d.received, d.applied), (9, 9, 9, 9));
    assert_eq!(
        (
            d.superseded,
            d.observed,
            d.submitted,
            d.rejected,
            d.dropped,
            d.unfinished
        ),
        (8, 0, 0, 0, 0, 1)
    );
    let end = graphics.finish();
    assert_eq!(end.wakes, 9);
    evidence.renderer_wakes = end.wakes;
    evidence.dpi = end.dpi;
    evidence.error = end.trace.error;
    evidence.max_mass_relative_error = end.trace.max_mass_relative_error;
    evidence.present_calls = end.trace.present_calls;
    evidence.accepted_presents = end.trace.accepted_presents;
    evidence.occluded_presents = end.trace.occluded_presents;
    evidence.native_child_exited = true;
    evidence.gpu_thread_joined = true;
    evidence.layers_destroyed = true;
    evidence.stage = "complete".into();
    evidence.completed = true;
}
#[test]
#[ignore = "explicit native melting checkbox/natural snow/GPU test on a private desktop"]
fn native_melting_checkbox_removes_natural_stock_and_stops_without_changing_air() {
    let evidence: Evidence = isolated(CHILD);
    assert!(evidence.completed && evidence.error.is_none());
    assert!(
        evidence.native_child_exited && evidence.gpu_thread_joined && evidence.layers_destroyed
    );
    println!(
        "native_melting_pixels {}",
        serde_json::to_string(&evidence).unwrap()
    );
}
#[test]
#[ignore = "helper launched only on the assigned private startup desktop"]
fn private_melting_child() {
    let Ok(desktop) = std::env::var(DESKTOP_ENV) else {
        return;
    };
    let path = std::env::var_os(REPORT_ENV).expect("probe report path missing");
    let mut evidence = Evidence::default();
    // Preserve the first failure from a renderer/control thread. The private
    // child has no inherited stdout; store only a source basename/line, no data.
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
