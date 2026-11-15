//! Independent native accumulation checkboxes on ordinary-emitter stock.
use super::*;

const CHILD: &str = "platform::weather::worker_probe::snow_pixels::drift_controls::options::private_accumulation_options_child";

#[derive(Default, serde::Serialize, serde::Deserialize)]
struct Evidence {
    stage: String,
    completed: bool,
    samples: Vec<Sample>,
    applied_keys: Vec<u8>,
    diagnostics: Summary,
    all_stock_gone_pixels_outside_bodies: usize,
    ordinary_snow_pixels_both_off: usize,
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
fn continues(before: &Sample, after: &Sample) {
    assert_eq!(after.intensity, 100);
    assert!(after.introduced > before.introduced);
    assert!(after.particles > 0 && after.air_mass > 0.0);
}
fn pixels_outside(sample: &Sample, bodies: &[Rect]) -> usize {
    let width = sample.image_width;
    let height = sample.image_height;
    assert_eq!(sample.image.len(), width * height * 4);
    let x = |v: f32| (v - sample.image_origin.x).ceil().clamp(0.0, width as f32) as usize;
    let y = |v: f32| (v - sample.image_origin.y).ceil().clamp(0.0, height as f32) as usize;
    // Pad two DIP for the stroke/AA using this actual layer's DPI scale.
    assert!(sample.image_scale > 0.0);
    let pad = sample.image_scale * 2.0;
    let mut covered = vec![false; width * height];
    for body in bodies {
        let (a, b, c, d) = (
            x(body.left - pad),
            x(body.right + pad),
            y(body.top - pad),
            y(body.bottom + pad),
        );
        if a >= b || c >= d {
            continue;
        }
        for row in c..d {
            covered[row * width + a..row * width + b].fill(true);
        }
    }
    sample
        .image
        .as_chunks::<4>()
        .0
        .iter()
        .zip(covered)
        .filter(|(p, mask)| !mask && p[3] != 0)
        .count()
}
fn run(evidence: &mut Evidence, desktop: &str) {
    guard_if_requested();
    let _dpi = Dpi::pmv2();
    guard_control_probe();
    let probe = Probe::new();
    let graphics = Graphics::start(&probe, desktop);
    evidence.stage = "natural_stock".into();
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
    apply_record(
        &graphics,
        &probe,
        Setting::SnowIntensity(0),
        Action::Settle,
        evidence,
    );
    apply_record(
        &graphics,
        &probe,
        Setting::FlakeVisibility(0),
        Action::Steps(0),
        evidence,
    );
    let initial = apply(
        &graphics,
        &probe,
        Setting::SnowIntensity(100),
        Action::Steps(60),
        evidence,
    );
    assert!(initial.window_mass > 1.0 && initial.ground_mass > 1.0 && initial.particles > 0);
    assert!(initial.roof_stock_alpha > 0 && initial.ground_stock_alpha > 0);
    let ground_before = initial.ground_mass;
    evidence.stage = "windows_only_off".into();
    let off = apply(
        &graphics,
        &probe,
        Setting::SnowOnWindows(false),
        Action::Steps(0),
        evidence,
    );
    assert!(!off.on_windows && off.on_ground);
    unchanged(&initial, &off);
    let windows_before = off.window_mass;
    record(evidence, initial);
    record(evidence, off);
    let first = graphics.sample(Action::Steps(1), false);
    assert!(first.window_mass > windows_before * 0.9);
    assert!(first.roof_stock_alpha > 0 && first.ground_stock_alpha > 0);
    assert!(first.ground_mass + 0.001 >= ground_before);
    record(evidence, first);
    let middle = graphics.sample(Action::Steps(59), false);
    assert!(
        middle.window_mass > windows_before * 0.35 && middle.window_mass < windows_before * 0.65
    );
    assert!(middle.ground_mass + 0.001 >= ground_before && middle.ground_stock_alpha > 0);
    continues(&evidence.samples[5], &middle);
    record(evidence, middle);
    let clear = graphics.sample(Action::Steps(90), false);
    assert!(clear.window_mass < 0.001 && clear.roof_stock_alpha == 0);
    assert!(clear.ground_mass + 0.001 >= ground_before && clear.ground_stock_alpha > 0);
    continues(&evidence.samples[7], &clear);
    let restored = apply(
        &graphics,
        &probe,
        Setting::SnowOnWindows(true),
        Action::Steps(0),
        evidence,
    );
    unchanged(&clear, &restored);
    record(evidence, clear);
    record(evidence, restored);
    let growing = graphics.sample(Action::Steps(180), false);
    assert!(growing.window_mass > 1.0 && growing.roof_stock_alpha > 0);
    assert!(growing.ground_mass + 0.001 >= ground_before);
    continues(&evidence.samples[9], &growing);
    evidence.stage = "ground_only_off".into();
    let off = apply(
        &graphics,
        &probe,
        Setting::SnowOnGround(false),
        Action::Steps(0),
        evidence,
    );
    assert!(off.on_windows && !off.on_ground);
    unchanged(&growing, &off);
    let ground_before = off.ground_mass;
    let window_before = off.window_material;
    record(evidence, growing);
    record(evidence, off);
    let first = graphics.sample(Action::Steps(1), false);
    assert!(first.ground_mass > ground_before * 0.9 && first.ground_stock_alpha > 0);
    assert!(first.window_material + 0.001 >= window_before && first.roof_stock_alpha > 0);
    record(evidence, first);
    let middle = graphics.sample(Action::Steps(59), false);
    assert!(middle.ground_mass > ground_before * 0.35 && middle.ground_mass < ground_before * 0.65);
    assert!(middle.window_material + 0.001 >= window_before && middle.roof_stock_alpha > 0);
    continues(&evidence.samples[11], &middle);
    record(evidence, middle);
    let clear = graphics.sample(Action::Steps(90), false);
    assert!(clear.ground_mass < 0.001 && clear.ground_stock_alpha == 0);
    assert!(clear.window_material + 0.001 >= window_before && clear.roof_stock_alpha > 0);
    continues(&evidence.samples[13], &clear);
    evidence.stage = "both_off".into();
    let both = apply(
        &graphics,
        &probe,
        Setting::SnowOnWindows(false),
        Action::Steps(0),
        evidence,
    );
    unchanged(&clear, &both);
    record(evidence, clear);
    record(evidence, both);
    let empty = graphics.sample(Action::Steps(150), false);
    assert!(!empty.on_windows && !empty.on_ground && empty.drift_mass < 0.001);
    assert_eq!((empty.roof_stock_alpha, empty.ground_stock_alpha), (0, 0));
    continues(&evidence.samples[15], &empty);
    assert_eq!(probe.settings().snow.flake_visibility_percent, 0);
    // Only genuine current bodies may remain in the GPU image, including slabs.
    evidence.all_stock_gone_pixels_outside_bodies = pixels_outside(&empty, &empty.body_bounds);
    assert_eq!(evidence.all_stock_gone_pixels_outside_bodies, 0);
    let visible = apply(
        &graphics,
        &probe,
        Setting::FlakeVisibility(100),
        Action::Steps(0),
        evidence,
    );
    same_material(&empty, &visible);
    // With zero drift mass, pixels outside slab bounds belong to ordinary snow.
    evidence.ordinary_snow_pixels_both_off = pixels_outside(&visible, &visible.cohesive_bounds);
    assert!(evidence.ordinary_snow_pixels_both_off > 20);
    let hidden = apply(
        &graphics,
        &probe,
        Setting::FlakeVisibility(0),
        Action::Steps(0),
        evidence,
    );
    unchanged(&empty, &hidden);
    record(evidence, empty);
    record(evidence, visible);
    record(evidence, hidden);
    evidence.stage = "restore_both_without_resurrecting_stock".into();
    let before = graphics.sample(Action::Steps(0), false);
    let restored = apply(
        &graphics,
        &probe,
        Setting::SnowOnWindows(true),
        Action::Steps(0),
        evidence,
    );
    unchanged(&before, &restored);
    let restored_ground = apply(
        &graphics,
        &probe,
        Setting::SnowOnGround(true),
        Action::Steps(0),
        evidence,
    );
    unchanged(&restored, &restored_ground);
    record(evidence, before);
    record(evidence, restored);
    record(evidence, restored_ground);
    let growing = graphics.sample(Action::Steps(180), false);
    continues(&evidence.samples[21], &growing);
    assert!(growing.window_mass > 1.0 && growing.roof_stock_alpha > 0);
    record(evidence, growing);
    let final_stock = apply(
        &graphics,
        &probe,
        Setting::SnowIntensity(0),
        Action::Settle,
        evidence,
    );
    assert!(final_stock.window_mass > 1.0 && final_stock.ground_mass > 1.0);
    assert!(final_stock.roof_stock_alpha > 0 && final_stock.ground_stock_alpha > 0);
    assert_eq!((final_stock.particles, final_stock.pending), (0, 0));
    record(evidence, final_stock);
    assert!(probe.apply_next().is_none());
    assert_eq!(
        evidence.applied_keys,
        [3, 1, 1, 19, 1, 23, 23, 24, 23, 19, 19, 23, 24, 1]
    );
    evidence.diagnostics = probe.finish();
    assert_eq!(evidence.diagnostics.input, 14);
    assert_eq!(evidence.diagnostics.dispatch, 14);
    assert_eq!(evidence.diagnostics.received, 14);
    assert_eq!(evidence.diagnostics.applied, 14);
    assert_eq!(evidence.diagnostics.superseded, 13);
    assert_eq!(evidence.diagnostics.rejected, 0);
    assert_eq!(evidence.diagnostics.dropped, 0);
    assert_eq!(evidence.diagnostics.unfinished, 1);
    let end = graphics.finish();
    assert_eq!(end.wakes, 14);
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
#[ignore = "explicit independent native accumulation flags/natural snow/GPU test on a private desktop"]
fn native_window_and_ground_accumulation_flags_preserve_other_material_and_rendered_snowfall() {
    let evidence: Evidence = isolated(CHILD);
    assert!(evidence.completed && evidence.error.is_none());
    assert!(
        evidence.native_child_exited && evidence.gpu_thread_joined && evidence.layers_destroyed
    );
    println!(
        "native_accumulation_options_pixels {}",
        serde_json::to_string(&evidence).unwrap()
    );
}
#[test]
#[ignore = "helper launched only on the assigned private startup desktop"]
fn private_accumulation_options_child() {
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
