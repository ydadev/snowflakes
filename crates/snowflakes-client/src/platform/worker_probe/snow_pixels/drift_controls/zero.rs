//! Zero accumulation through the existing actual Panel/Owner/disk/View/GPU bridge.
use super::*;

const CHILD: &str =
    "platform::weather::worker_probe::snow_pixels::drift_controls::zero::private_zero_cap_child";

#[derive(Default, serde::Serialize, serde::Deserialize)]
struct Evidence {
    stage: String,
    completed: bool,
    samples: Vec<Sample>,
    applied_keys: Vec<u8>,
    diagnostics: Summary,
    immediate_zero_preserved: bool,
    positive_cap_preserved_empty_layer: bool,
    clear_pixels: usize,
    visible_zero_cap_pixels: usize,
    held_zero_pixels: usize,
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
}
fn record(evidence: &mut Evidence, mut sample: Sample) {
    sample.profiles = Vec::new();
    sample.image = Vec::new();
    sample.air_state = Vec::new();
    evidence.samples.push(sample);
}
fn apply(
    graphics: &Graphics,
    probe: &Probe,
    setting: Setting,
    action: Action,
    evidence: &mut Evidence,
) -> Sample {
    graphics.apply(probe, setting, action, &mut evidence.applied_keys)
}
fn apply_record(
    graphics: &Graphics,
    probe: &Probe,
    setting: Setting,
    action: Action,
    evidence: &mut Evidence,
) {
    let sample = apply(graphics, probe, setting, action, evidence);
    record(evidence, sample);
}
fn pixels(sample: &Sample) -> usize {
    sample
        .image
        .as_chunks::<4>()
        .0
        .iter()
        .filter(|p| p[3] != 0)
        .count()
}
fn falling_snow_continues(before: &Sample, after: &Sample) {
    assert_eq!(after.intensity, 100);
    assert!(after.introduced > before.introduced);
    assert!(after.particles > 0 && after.air_mass > 0.0);
    // Flake opacity is deliberately hidden, never its physics or emitter.
    assert_eq!(after.visibility, 0);
}
fn run(evidence: &mut Evidence, desktop: &str) {
    guard_if_requested();
    let _dpi = Dpi::pmv2();
    guard_control_probe();
    let probe = Probe::new();
    let graphics = Graphics::start(&probe, desktop);
    evidence.stage = "ordinary_emitter_stock".into();
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
    let stock = apply(
        &graphics,
        &probe,
        Setting::SnowIntensity(0),
        Action::Settle,
        evidence,
    );
    assert!(stock.roof_mass > 1.0 && stock.ground_mass > 1.0 && stock.roof_alpha > 0);
    assert_eq!((stock.particles, stock.pending), (0, 0));
    let introduced_at_stock = stock.introduced;
    let hidden_air = apply(
        &graphics,
        &probe,
        Setting::FlakeVisibility(0),
        Action::Steps(0),
        evidence,
    );
    unchanged(&stock, &hidden_air);
    record(evidence, stock);
    record(evidence, hidden_air);
    let snowing = apply(
        &graphics,
        &probe,
        Setting::SnowIntensity(100),
        Action::Steps(60),
        evidence,
    );
    assert!(snowing.particles > 0 && snowing.air_mass > 0.0);
    assert!(snowing.introduced > introduced_at_stock && snowing.drift_mass > 1.0);
    evidence.stage = "zero_cap_with_ongoing_emission".into();
    let zero = apply(
        &graphics,
        &probe,
        Setting::DriftSize(0),
        Action::Steps(0),
        evidence,
    );
    unchanged(&snowing, &zero);
    assert_eq!((zero.limit, zero.intensity, zero.visibility), (0, 100, 0));
    evidence.immediate_zero_preserved = true;
    let mass_before = zero.drift_mass;
    let roof_before = zero.roof_mass;
    let ground_before = zero.ground_mass;
    let introduced_before_zero = zero.introduced;
    record(evidence, snowing);
    record(evidence, zero);
    let first = graphics.sample(Action::Steps(1), false);
    assert!(first.drift_mass > mass_before * 0.9 && first.drift_mass < mass_before);
    assert!(first.introduced >= introduced_before_zero && first.particles > 0);
    record(evidence, first);
    let middle = graphics.sample(Action::Steps(59), false);
    assert!(middle.drift_mass > mass_before * 0.35 && middle.drift_mass < mass_before * 0.65);
    assert!(middle.roof_mass > roof_before * 0.35 && middle.roof_mass < roof_before * 0.65);
    assert!(middle.ground_mass > ground_before * 0.35 && middle.ground_mass < ground_before * 0.65);
    falling_snow_continues(&evidence.samples[5], &middle);
    record(evidence, middle);
    let clear = graphics.sample(Action::Steps(90), false);
    assert!(clear.drift_mass < 0.001 && clear.roof_mass < 0.001 && clear.ground_mass < 0.001);
    falling_snow_continues(&evidence.samples[7], &clear);
    evidence.clear_pixels = pixels(&clear);
    assert_eq!(evidence.clear_pixels, 0);
    // A zero accumulation ceiling must still render the flying snowfall.
    let visible = apply(
        &graphics,
        &probe,
        Setting::FlakeVisibility(100),
        Action::Steps(0),
        evidence,
    );
    same_material(&clear, &visible);
    assert_eq!(
        (visible.limit, visible.intensity, visible.visibility),
        (0, 100, 100)
    );
    evidence.visible_zero_cap_pixels = pixels(&visible);
    assert!(evidence.visible_zero_cap_pixels > 20);
    let hidden_again = apply(
        &graphics,
        &probe,
        Setting::FlakeVisibility(0),
        Action::Steps(0),
        evidence,
    );
    unchanged(&clear, &hidden_again);
    record(evidence, clear);
    record(evidence, visible);
    record(evidence, hidden_again);
    let held = graphics.sample(Action::Steps(120), false);
    assert!(held.drift_mass < 0.001);
    falling_snow_continues(&evidence.samples[8], &held);
    evidence.held_zero_pixels = pixels(&held);
    assert_eq!(evidence.held_zero_pixels, 0);
    evidence.stage = "positive_cap_needs_precipitation".into();
    let restored = apply(
        &graphics,
        &probe,
        Setting::DriftSize(100),
        Action::Steps(0),
        evidence,
    );
    unchanged(&held, &restored);
    evidence.positive_cap_preserved_empty_layer = true;
    record(evidence, held);
    record(evidence, restored);
    let growing = graphics.sample(Action::Steps(180), false);
    falling_snow_continues(&evidence.samples[12], &growing);
    assert!(growing.roof_mass > 1.0 && growing.roof_alpha > 0);
    record(evidence, growing);
    let final_stock = apply(
        &graphics,
        &probe,
        Setting::SnowIntensity(0),
        Action::Settle,
        evidence,
    );
    assert!(final_stock.roof_mass > 1.0 && final_stock.ground_mass > 1.0);
    assert!(final_stock.drift_mass <= final_stock.introduced - introduced_at_stock + 0.001);
    assert_eq!((final_stock.particles, final_stock.pending), (0, 0));
    record(evidence, final_stock);
    assert!(probe.apply_next().is_none());
    assert_eq!(evidence.applied_keys, [3, 1, 1, 19, 1, 3, 19, 19, 3, 1]);
    evidence.diagnostics = probe.finish();
    assert_eq!(evidence.diagnostics.input, 10);
    assert_eq!(evidence.diagnostics.dispatch, 10);
    assert_eq!(evidence.diagnostics.received, 10);
    assert_eq!(evidence.diagnostics.applied, 10);
    assert_eq!(evidence.diagnostics.superseded, 9);
    assert_eq!(evidence.diagnostics.rejected, 0);
    assert_eq!(evidence.diagnostics.dropped, 0);
    assert_eq!(evidence.diagnostics.unfinished, 1);
    let end = graphics.finish();
    assert_eq!(end.wakes, 10);
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
#[ignore = "explicit zero cap/native controls/natural snow/GPU bridge on a private desktop"]
fn native_zero_cap_clears_stock_without_stopping_emission_and_positive_cap_needs_precipitation() {
    let evidence: Evidence = isolated(CHILD);
    assert!(evidence.completed && evidence.error.is_none());
    assert!(
        evidence.native_child_exited && evidence.gpu_thread_joined && evidence.layers_destroyed
    );
    println!(
        "native_zero_cap_pixels {}",
        serde_json::to_string(&evidence).unwrap()
    );
}
#[test]
#[ignore = "helper launched only on the assigned private startup desktop"]
fn private_zero_cap_child() {
    let Ok(desktop) = std::env::var(DESKTOP_ENV) else {
        return;
    };
    let path = std::env::var_os(REPORT_ENV).expect("probe report path missing");
    let mut evidence = Evidence::default();
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        run(&mut evidence, &desktop)
    }));
    std::fs::write(path, serde_json::to_vec(&evidence).unwrap()).unwrap();
    if let Err(error) = result {
        std::panic::resume_unwind(error);
    }
}
