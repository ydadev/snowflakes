//! Saved cursor checkbox, natural stock and explicitly synthetic model input.
use super::*;
const CHILD: &str = "platform::weather::worker_probe::snow_pixels::drift_controls::cursor::private_cursor_sweeping_child";

#[derive(Default, serde::Serialize, serde::Deserialize)]
struct Evidence {
    stage: String,
    completed: bool,
    samples: Vec<Sample>,
    applied_keys: Vec<u8>,
    diagnostics: Summary,
    same_time_pixels: Vec<(usize, u8)>,
    locality: Locality,
    hover_max_height_change: f64,
    disabled_max_height_change: f64,
    fragment_pixels: usize,
    final_pixels_outside_stock: usize,
    synthetic_steps: usize,
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
struct Locality {
    near_cells: usize,
    far_cells: usize,
    near_before: f64,
    near_loss: f64,
    far_absolute_change: f64,
    roof_loss: f64,
    other_window_change: f64,
    ground_change: f64,
}
fn record(evidence: &mut Evidence, mut s: Sample) {
    s.image = Vec::new();
    s.profiles = Vec::new();
    s.air_state = Vec::new();
    s.body_bounds = Vec::new();
    s.cohesive_bounds = Vec::new();
    s.stock_bounds = Vec::new();
    evidence.samples.push(s);
}
fn apply(g: &Graphics, p: &Probe, setting: Setting, action: Action, e: &mut Evidence) -> Sample {
    let expected = if let Setting::CursorEnabled(value) = &setting {
        let mut settings = p.settings();
        settings.interaction.cursor_enabled = *value;
        Some(settings)
    } else {
        None
    };
    let s = g.apply(p, setting, action, &mut e.applied_keys);
    if let Some(expected) = expected {
        assert_eq!(p.settings(), expected);
    }
    s
}
fn same_time(a: &Sample, b: &Sample, e: &mut Evidence) {
    same_material(a, b);
    assert_eq!(
        (a.spill_bodies, a.active_slabs),
        (b.spill_bodies, b.active_slabs)
    );
    assert_eq!(
        (a.image_width, a.image_height, a.image.len()),
        (b.image_width, b.image_height, b.image.len())
    );
    let count = a
        .image
        .as_chunks::<4>()
        .0
        .iter()
        .zip(b.image.as_chunks::<4>().0)
        .filter(|(a, b)| a != b)
        .count();
    let max = a
        .image
        .iter()
        .zip(&b.image)
        .map(|(a, b)| a.abs_diff(*b))
        .max()
        .unwrap_or(0);
    e.same_time_pixels.push((count, max));
    assert!(
        count <= 8 && max <= 16,
        "same-time raster changed beyond native AA bound"
    );
}
fn roof(s: &Sample) -> (&[f32], &[f32]) {
    let p = s.profiles.iter().find(|p| p.0 == s.roof_key).unwrap();
    (&p.1, &p.2)
}
fn quiet(s: &Sample) {
    assert_eq!((s.particles, s.pending, s.spill_bodies), (0, 0, 0));
    assert_eq!(s.intensity, 0);
    assert!(s.roof_mass > 1.0 && s.roof_stock_alpha > 0 && s.ground_mass > 1.0);
    assert!(!s.melting && s.on_windows && s.on_ground);
}
fn held(a: &Sample, b: &Sample) -> f64 {
    quiet(a);
    quiet(b);
    assert_eq!(a.introduced, b.introduced);
    assert_eq!(a.removed, b.removed);
    assert!((a.roof_mass - b.roof_mass).abs() < 0.001);
    let (am, ap) = roof(a);
    let (bm, bp) = roof(b);
    let change = am
        .iter()
        .zip(ap)
        .zip(bm.iter().zip(bp))
        .map(|((am, ap), (bm, bp))| {
            ((f64::from(*am) + f64::from(*ap)) - (f64::from(*bm) + f64::from(*bp))).abs()
                * f64::from(a.image_scale)
                / f64::from(a.roof_cell_width)
        })
        .fold(0.0, f64::max);
    assert!(
        change < 0.02,
        "hover/disabled cursor moved the roof profile"
    );
    change
}
fn locality(a: &Sample, b: &Sample, point: Point) -> Locality {
    assert_eq!(a.introduced, b.introduced);
    let (am, ap) = roof(a);
    let (bm, bp) = roof(b);
    assert_eq!(am.len(), bm.len());
    let mut l = Locality::default();
    for (i, ((am, ap), (bm, bp))) in am.iter().zip(ap).zip(bm.iter().zip(bp)).enumerate() {
        let x = a.roof_left + (i as f32 + 0.5) * a.roof_cell_width;
        let before = f64::from(*am) + f64::from(*ap);
        let loss = before - f64::from(*bm) - f64::from(*bp);
        // One cell beyond the 28 DIP input radius allows ordinary pairwise
        // relaxation during this single physical step, without a whole-layer wave.
        if (x - point.x).abs() <= 28.0 * a.image_scale + a.roof_cell_width {
            l.near_cells += 1;
            l.near_before += before;
            l.near_loss += loss;
        } else {
            l.far_cells += 1;
            l.far_absolute_change += loss.abs();
        }
    }
    l.roof_loss = a.roof_mass - b.roof_mass;
    l.other_window_change = ((a.window_mass - a.roof_mass) - (b.window_mass - b.roof_mass)).abs();
    l.ground_change = (a.ground_mass - b.ground_mass).abs();
    assert!(l.near_cells > 0 && l.far_cells > l.near_cells);
    assert!(l.near_loss > 0.05 && l.near_loss < l.near_before * 0.15);
    assert!(l.roof_loss > 0.05 && l.roof_loss < a.roof_mass * 0.2);
    assert!(
        l.far_absolute_change < 0.001 && l.other_window_change < 0.001 && l.ground_change < 0.001
    );
    l
}
fn new_air_pixels(a: &Sample, hidden: &Sample) -> usize {
    assert_eq!(a.image.len(), hidden.image.len());
    a.image
        .as_chunks::<4>()
        .0
        .iter()
        .zip(hidden.image.as_chunks::<4>().0)
        .enumerate()
        .filter(|(i, (a, b))| {
            let p = Point::new(a_pixel_x(*i, hidden), a_pixel_y(*i, hidden));
            a[3] > 0
                && b[3] == 0
                && hidden.body_bounds.iter().any(|r| {
                    p.x >= r.left - 2.0 * hidden.image_scale
                        && p.x <= r.right + 2.0 * hidden.image_scale
                        && p.y >= r.top - 2.0 * hidden.image_scale
                        && p.y <= r.bottom + 2.0 * hidden.image_scale
                })
        })
        .count()
}
fn a_pixel_x(i: usize, s: &Sample) -> f32 {
    s.image_origin.x + (i % s.image_width) as f32 + 0.5
}
fn a_pixel_y(i: usize, s: &Sample) -> f32 {
    s.image_origin.y + (i / s.image_width) as f32 + 0.5
}
fn run(e: &mut Evidence, desktop: &str) {
    guard_if_requested();
    let _dpi = Dpi::pmv2();
    guard_control_probe();
    let p = Probe::new();
    let g = Graphics::start(&p, desktop);
    e.stage = "natural_stock".into();
    let start = apply(&g, &p, Setting::DriftSize(100), Action::Steps(0), e);
    record(e, start);
    let fill = apply(&g, &p, Setting::SnowIntensity(100), Action::Fill, e);
    record(e, fill);
    let stock = apply(&g, &p, Setting::SnowIntensity(0), Action::Rest, e);
    quiet(&stock);
    let (mass, _) = roof(&stock);
    let index = mass
        .iter()
        .enumerate()
        .max_by(|a, b| a.1.total_cmp(b.1))
        .unwrap()
        .0;
    let point = Point::new(
        stock.roof_left + (index as f32 + 0.5) * stock.roof_cell_width,
        stock.roof_y - stock.roof_peak - 2.0,
    );
    e.stage = "stationary_input".into();
    let on = apply(&g, &p, Setting::CursorEnabled(true), Action::Steps(0), e);
    same_time(&stock, &on, e);
    let hover = g.sample(
        Action::Cursor {
            steps: 60,
            input: Cursor {
                position: point,
                velocity: Point::default(),
            },
        },
        false,
    );
    e.hover_max_height_change = held(&on, &hover);
    record(e, stock);
    record(e, on);
    let off = apply(&g, &p, Setting::CursorEnabled(false), Action::Steps(0), e);
    same_time(&hover, &off, e);
    e.stage = "disabled_moving_input".into();
    let disabled_input = Cursor {
        position: Point::new(point.x + 8.0 * off.image_scale, point.y),
        velocity: Point::new(480.0, 0.0),
    };
    let disabled = g.sample(
        Action::Cursor {
            steps: 4,
            input: disabled_input,
        },
        false,
    );
    e.disabled_max_height_change = held(&off, &disabled);
    record(e, hover);
    record(e, off);
    let on = apply(&g, &p, Setting::CursorEnabled(true), Action::Steps(0), e);
    same_time(&disabled, &on, e);
    e.stage = "single_local_sweep".into();
    // Four disabled samples ended 32 DIP to the right. This one-step return
    // has the corresponding velocity; its final position is the dense stock.
    let moving = Cursor {
        position: point,
        velocity: Point::new(-1920.0, 0.0),
    };
    let swept = g.sample(
        Action::Cursor {
            steps: 1,
            input: moving,
        },
        false,
    );
    e.locality = locality(&on, &swept, point);
    assert_eq!(swept.particles, 0);
    assert!(swept.spill_bodies > 0 && swept.spill_bodies <= e.locality.near_cells);
    assert_eq!(swept.active_slabs, 0);
    record(e, disabled);
    record(e, on);
    record(e, swept);
    let visible = g.sample(
        Action::Cursor {
            steps: 1,
            input: Cursor {
                position: point,
                velocity: Point::default(),
            },
        },
        false,
    );
    assert!(
        visible.particles > 0
            && visible.particles <= e.locality.near_cells
            && visible.air_mass > 0.0
    );
    assert_eq!((visible.spill_bodies, visible.active_slabs), (0, 0));
    let age = 1.0 + crate::timing::STEP.as_secs_f32();
    assert!((visible.min_body_age - age).abs() < 1e-5 && (visible.max_body_age - age).abs() < 1e-5);
    e.stage = "rendered_fragments".into();
    let hidden = apply(&g, &p, Setting::FlakeVisibility(0), Action::Steps(0), e);
    same_material(&visible, &hidden);
    e.fragment_pixels = new_air_pixels(&visible, &hidden);
    assert!(e.fragment_pixels > 0);
    let restored = apply(&g, &p, Setting::FlakeVisibility(100), Action::Steps(0), e);
    same_time(&visible, &restored, e);
    let off = apply(&g, &p, Setting::CursorEnabled(false), Action::Steps(0), e);
    same_time(&restored, &off, e);
    record(e, visible);
    record(e, hidden);
    record(e, restored);
    record(e, off);
    e.stage = "natural_settling_without_old_pixels".into();
    let final_stock = g.sample(Action::Rest, false);
    quiet(&final_stock);
    e.final_pixels_outside_stock = options::pixels_outside(&final_stock, &final_stock.stock_bounds);
    assert_eq!(e.final_pixels_outside_stock, 0);
    record(e, final_stock);
    assert!(p.apply_next().is_none());
    assert_eq!(e.applied_keys, [3, 1, 1, 26, 26, 26, 19, 19, 26]);
    e.diagnostics = p.finish();
    let d = &e.diagnostics;
    assert_eq!(
        (d.input, d.dispatch, d.received, d.applied, d.superseded),
        (9, 9, 9, 9, 8)
    );
    assert_eq!(
        (d.observed, d.submitted, d.rejected, d.dropped, d.unfinished),
        (0, 0, 0, 0, 1)
    );
    let end = g.finish();
    assert_eq!((end.wakes, end.synthetic_steps), (9, 66));
    e.synthetic_steps = end.synthetic_steps;
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
#[ignore = "explicit saved cursor checkbox/natural stock/synthetic model input/GPU test on a private desktop"]
fn saved_cursor_flag_preserves_hover_and_local_sweeping_renders_bounded_fragments() {
    let e: Evidence = isolated(CHILD);
    assert!(
        e.completed
            && e.error.is_none()
            && e.native_child_exited
            && e.gpu_thread_joined
            && e.layers_destroyed
    );
    println!(
        "native_cursor_sweeping {}",
        serde_json::to_string(&e).unwrap()
    );
}
#[test]
#[ignore = "helper launched only on the assigned private startup desktop"]
fn private_cursor_sweeping_child() {
    let Ok(desktop) = std::env::var(DESKTOP_ENV) else {
        return;
    };
    let path = std::env::var_os(REPORT_ENV).expect("probe report path missing");
    let mut e = Evidence::default();
    let failure = Arc::new(Mutex::new(None));
    let recorded = failure.clone();
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        if let Some(l) = info.location() {
            recorded.lock().unwrap().get_or_insert_with(|| {
                (
                    l.file().rsplit(['/', '\\']).next().unwrap().to_owned(),
                    l.line(),
                )
            });
        }
    }));
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| run(&mut e, &desktop)));
    std::panic::set_hook(previous);
    e.failure_location = failure.lock().unwrap().take();
    std::fs::write(path, serde_json::to_vec(&e).unwrap()).unwrap();
    if let Err(error) = result {
        std::panic::resume_unwind(error);
    }
}
