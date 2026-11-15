//! Logical melt rates, actual material conversion, pause and model cadence.
use super::*;
const GROUND: u64 = 1 | (1 << 63);

fn stocked(scale: f32, roof: bool) -> (World, Settings, u64) {
    let mut desktop = scene();
    let screen = &mut desktop.screens[0];
    screen.rect.right *= scale;
    screen.rect.bottom *= scale;
    screen.floor *= scale;
    screen.scale = scale;
    if roof {
        desktop.windows.push(Window {
            key: 7,
            visible: true,
            collect: true,
            rect: Rect {
                left: 0.0,
                top: 300.0 * scale,
                right: 800.0 * scale,
                bottom: 500.0 * scale,
            },
        });
    }
    let mut world = World::new(desktop, 8);
    let mut settings = quiet();
    settings.snow.drift_size_percent = 100;
    settings.interaction.cursor_enabled = false;
    world.step(&settings, None);
    let key = if roof { 7 } else { GROUND };
    let drift = world.drifts.get_mut(&key).unwrap();
    drift.mass.fill(3.0 * drift.cell_width / scale);
    world.introduced_mass = world.total_mass();
    settings.snow.melting = true;
    (world, settings, key)
}
fn logical_height(world: &World, key: u64, scale: f32) -> f64 {
    let d = &world.drifts[&key];
    (0..d.mass.len())
        .map(|i| f64::from(d.height(i, &world.scene)) / f64::from(scale))
        .sum::<f64>()
        / d.mass.len() as f64
}
fn balance(world: &World) -> f64 {
    assert!(world.flakes.is_empty() && world.pending_collapse_count() == 0);
    (world.introduced_mass - world.removed_mass - world.total_mass()).abs()
}
#[derive(serde::Serialize)]
struct Rate {
    scale: f32,
    roof: bool,
    fps: u64,
    steps: usize,
    drop_dip: f64,
    expected_dip: f64,
    max_balance_error: f64,
    profile_and_ledger_hash: String,
}
#[test]
fn equal_logical_melting_rates_match_across_five_scales_and_thirty_sixty_model_hz() {
    let mut reports = Vec::new();
    let mut max_error = 0.0_f64;
    for scale in [1.0, 1.25, 1.5, 2.0, 2.5] {
        for roof in [false, true] {
            for fps in [30, 60] {
                let (mut world, settings, key) = stocked(scale, roof);
                let height = logical_height(&world, key, scale);
                let introduced = world.introduced_mass;
                let start = Instant::now();
                let mut clock = StepClock::default();
                clock.advance(start, true);
                let (mut steps, mut max_balance) = (0, 0.0_f64);
                let mut hash = 0xcbf29ce484222325_u64;
                for frame in 1..=60 * fps {
                    let now = start + Duration::from_nanos(frame * 1_000_000_000 / fps);
                    for _ in 0..clock.advance(now, true).count {
                        world.step(&settings, None);
                        steps += 1;
                        assert_eq!(world.introduced_mass, introduced);
                        max_balance = max_balance.max(balance(&world));
                        if scale == 1.0 {
                            for m in world.drifts[&key]
                                .mass
                                .iter()
                                .chain(&world.drifts[&key].pending)
                            {
                                hash = (hash ^ u64::from(m.to_bits())).wrapping_mul(0x100000001b3);
                            }
                            hash =
                                (hash ^ world.removed_mass.to_bits()).wrapping_mul(0x100000001b3);
                        }
                    }
                }
                let drop = height - logical_height(&world, key, scale);
                // Independent fixed-60-Hz integral, not the product melt helper.
                let expected = 0.002 * steps as f64 / 60.0;
                max_error = max_error.max((drop - expected).abs());
                reports.push(Rate {
                    scale,
                    roof,
                    fps,
                    steps,
                    drop_dip: drop,
                    expected_dip: expected,
                    max_balance_error: max_balance,
                    profile_and_ledger_hash: if scale == 1.0 {
                        format!("{hash:016x}")
                    } else {
                        String::new()
                    },
                });
            }
        }
    }
    println!(
        "melting_units fixed {}",
        serde_json::to_string(&reports).unwrap()
    );
    assert!(
        reports
            .iter()
            .all(|r| r.steps == 3599 && r.max_balance_error < 1e-9)
    );
    for pair in reports.as_chunks::<2>().0 {
        assert_eq!(pair[0].drop_dip, pair[1].drop_dip);
        assert_eq!(
            pair[0].profile_and_ledger_hash,
            pair[1].profile_and_ledger_hash
        );
    }
    assert!(
        max_error < 0.002,
        "logical melting rate depends on display scale"
    );
}
#[test]
fn mixed_scale_melting_preserves_paused_stock_and_resumes_without_a_backlog() {
    let mut desktop = scene();
    desktop.screens[0].rect.right = 1000.0;
    desktop.screens[0].rect.bottom = 750.0;
    desktop.screens[0].floor = 750.0;
    desktop.screens[0].scale = 1.25;
    desktop.screens.push(Screen {
        key: 2,
        persistent_id: true,
        rect: Rect {
            left: 1000.0,
            top: 0.0,
            right: 3000.0,
            bottom: 1500.0,
        },
        floor: 1500.0,
        scale: 2.5,
    });
    let mut world = World::new(desktop, 8);
    let mut settings = quiet();
    settings.snow.drift_size_percent = 100;
    settings.interaction.cursor_enabled = false;
    world.step(&settings, None);
    let right = 2 | (1 << 63);
    for (key, scale) in [(GROUND, 1.25), (right, 2.5)] {
        let d = world.drifts.get_mut(&key).unwrap();
        d.mass.fill(3.0 * d.cell_width / scale);
    }
    world.introduced_mass = world.total_mass();
    settings.snow.melting = true;
    let frozen = world.drifts[&GROUND].mass.clone();
    let height = logical_height(&world, right, 2.5);
    world.set_paused_monitors([1].into());
    for _ in 0..3600 {
        world.step(&settings, None);
        assert!(balance(&world) < 1e-9);
    }
    assert_eq!(world.drifts[&GROUND].mass, frozen);
    let active_drop = height - logical_height(&world, right, 2.5);
    let before = (
        logical_height(&world, GROUND, 1.25),
        logical_height(&world, right, 2.5),
    );
    world.set_paused_monitors(Default::default());
    world.step(&settings, None);
    let resumed = (
        before.0 - logical_height(&world, GROUND, 1.25),
        before.1 - logical_height(&world, right, 2.5),
    );
    let retained: Vec<_> = world
        .drifts
        .values()
        .map(|d| (d.mass.clone(), d.pending.clone()))
        .collect();
    let removed = world.removed_mass;
    settings.snow.melting = false;
    for _ in 0..600 {
        world.step(&settings, None);
    }
    assert_eq!(
        retained,
        world
            .drifts
            .values()
            .map(|d| (d.mass.clone(), d.pending.clone()))
            .collect::<Vec<_>>()
    );
    assert_eq!(world.removed_mass, removed);
    println!(
        "melting_units mixed {{\"active_drop\":{active_drop},\"left_resume_drop\":{},\"right_resume_drop\":{},\"balance\":{}}}",
        resumed.0,
        resumed.1,
        balance(&world)
    );
    assert!((active_drop - 0.12).abs() < 0.002);
    assert!((resumed.0 - 0.002 / 60.0).abs() < 2e-6 && (resumed.1 - 0.002 / 60.0).abs() < 2e-6);
}
#[derive(Default, serde::Serialize)]
struct Transition {
    active_steps: usize,
    paused_steps: usize,
    transitional_cells: usize,
    min_material_scale: f64,
    max_material_scale: f64,
    max_depth_error: f64,
    max_ledger_relative_error: f64,
}
fn transition_step(
    world: &mut World,
    control: &mut World,
    settings: &Settings,
    t: &mut Transition,
) {
    let prior = world.drifts[&GROUND].mass.clone();
    let control_mass = control.drifts[&GROUND].mass.clone();
    let mut off = settings.clone();
    off.snow.melting = false;
    world.step(settings, None);
    control.step(&off, None);
    t.active_steps += 1;
    assert_eq!(control.drifts[&GROUND].mass, control_mass);
    let reference = &control.drifts[&GROUND];
    let actual = &world.drifts[&GROUND];
    for (i, prior) in prior.iter().enumerate() {
        let h = f64::from(reference.height(i, &control.scene));
        let unit = (h * f64::from(reference.cell_width) / f64::from(reference.mass[i])).sqrt();
        t.min_material_scale = t.min_material_scale.min(unit);
        t.max_material_scale = t.max_material_scale.max(unit);
        if (unit - 2.5).abs() > 1e-4 {
            t.transitional_cells += 1;
        }
        // The off reference supplies this step's transformed geometry. Normalize
        // it to the on world's previous mass, then measure removed height in
        // current material units; no product scale/melt helper is used.
        let without = h * f64::from(*prior) / f64::from(reference.mass[i]);
        let drop = (without - f64::from(actual.height(i, &world.scene))) / unit;
        t.max_depth_error = t.max_depth_error.max((drop - 0.002 / 60.0).abs());
    }
    t.max_ledger_relative_error = t
        .max_ledger_relative_error
        .max(balance(world) / world.introduced_mass);
}
#[test]
fn melting_uses_current_material_units_during_dpi_transition_and_freezes_on_pause() {
    let (mut world, settings, _) = stocked(1.0, false);
    let (mut control, _, _) = stocked(1.0, false);
    let mut changed = world.scene.clone();
    changed.screens[0].scale = 2.5;
    world.set_scene(changed.clone());
    control.set_scene(changed);
    let mut t = Transition {
        min_material_scale: f64::INFINITY,
        ..Default::default()
    };
    for _ in 0..7 {
        transition_step(&mut world, &mut control, &settings, &mut t);
    }
    let frozen = world.drifts[&GROUND].mass.clone();
    let heights: Vec<_> = (0..frozen.len())
        .map(|i| world.drifts[&GROUND].height(i, &world.scene))
        .collect();
    let removed = world.removed_mass;
    world.set_paused_monitors([1].into());
    control.set_paused_monitors([1].into());
    let mut off = settings.clone();
    off.snow.melting = false;
    for _ in 0..600 {
        world.step(&settings, None);
        control.step(&off, None);
        t.paused_steps += 1;
    }
    assert_eq!(world.drifts[&GROUND].mass, frozen);
    assert_eq!(world.removed_mass, removed);
    assert_eq!(
        heights,
        (0..frozen.len())
            .map(|i| world.drifts[&GROUND].height(i, &world.scene))
            .collect::<Vec<_>>()
    );
    world.set_paused_monitors(Default::default());
    control.set_paused_monitors(Default::default());
    for _ in 0..60 {
        transition_step(&mut world, &mut control, &settings, &mut t);
    }
    println!(
        "melting_units transition {}",
        serde_json::to_string(&t).unwrap()
    );
    assert!(
        t.transitional_cells > 0
            && t.min_material_scale < 2.0
            && (t.max_material_scale - 2.5).abs() < 1e-4
    );
    assert!(t.max_ledger_relative_error < 1e-5);
    assert!(
        t.max_depth_error < 2e-6,
        "melting uses the wrong material unit during DPI transition"
    );
}
