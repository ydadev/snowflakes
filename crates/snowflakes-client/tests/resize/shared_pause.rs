//! Physical interval checks across a shared roof, partial pause and full landing.
use super::*;

#[derive(Clone)]
struct Bin {
    left: f64,
    right: f64,
    mass: f64,
    pending: f64,
    area: f64,
}

fn bins(world: &World) -> Vec<Bin> {
    let roof = &world.drifts[&7];
    let left = f64::from(roof.support.left);
    let width = (f64::from(roof.support.right) - left) / roof.mass.len() as f64;
    (0..roof.mass.len())
        .map(|i| {
            let mass = f64::from(roof.mass[i]);
            let pending = f64::from(roof.pending[i]);
            // The fixture has resting snow wherever pending snow exists. Read
            // the observable physical height rather than private scale state.
            let area = if mass > 0.0 {
                f64::from(roof.height(i, &world.scene)) * width * (mass + pending) / mass
            } else {
                assert_eq!(pending, 0.0);
                0.0
            };
            Bin {
                left: left + i as f64 * width,
                right: left + (i + 1) as f64 * width,
                mass,
                pending,
                area,
            }
        })
        .collect()
}

fn integral(old: &[Bin], left: f64, right: f64) -> (f64, f64, f64) {
    old.iter()
        .fold((0.0, 0.0, 0.0), |(mass, pending, area), bin| {
            let overlap = (right.min(bin.right) - left.max(bin.left)).max(0.0);
            let fraction = overlap / (bin.right - bin.left);
            (
                mass + bin.mass * fraction,
                pending + bin.pending * fraction,
                area + bin.area * fraction,
            )
        })
}

fn remapped(old: &[Bin], world: &World) {
    for bin in bins(world) {
        let (mass, pending, area) = integral(old, bin.left, bin.right);
        assert!(
            (bin.mass - mass).abs() < 0.0002,
            "resting snow moved: {} vs {mass}",
            bin.mass
        );
        assert!(
            (bin.pending - pending).abs() < 0.0002,
            "pending snow moved: {} vs {pending}",
            bin.pending
        );
        assert!(
            (bin.area - area).abs() < area.max(1.0) * 0.00002,
            "physical area changed during resize: {} vs {area}",
            bin.area
        );
    }
}

fn budget(world: &World, introduced: f64) {
    assert_eq!(
        world.introduced_mass, introduced,
        "resize or resume created snow"
    );
    // Mound weights are f32; their explicitly accounted remainder may be a few
    // ULPs. Bound that remainder separately rather than ignoring removed_mass.
    assert!(
        world.removed_mass <= introduced * f64::from(f32::EPSILON) * 16.0,
        "quiet resize discarded more than rounding dust: {}",
        world.removed_mass
    );
    let error = (introduced - world.removed_mass - world.total_mass()).abs();
    assert!(error < introduced.max(1.0) * 0.00002, "mass error {error}");
}

fn shared(scales: (f32, f32), offset: f32) -> (World, Scene, Settings) {
    let (_, mut scene, mut settings) = boundary_fixture(scales.0, scales.1, 260.0);
    scene.windows[0].rect.right = 660.0;
    for screen in &mut scene.screens {
        screen.rect.left += offset;
        screen.rect.right += offset;
    }
    scene.windows[0].rect.left += offset;
    scene.windows[0].rect.right += offset;
    if offset < 0.0 {
        scene.screens.reverse();
    }
    settings.snow.wind_percent = 0;
    let mut world = World::new(scene.clone(), 107);
    let roof = world.drifts.get_mut(&7).unwrap();
    for i in 0..roof.mass.len() {
        let scale = scene.scale_at(Point::new(roof.x(i), roof.support.y - 1.0));
        let units = roof.cell_width / (scale * scale);
        roof.mass[i] = (1.5 + (i % 9) as f32 * 0.12) * units;
        roof.pending[i] = (0.25 + (i % 5) as f32 * 0.05) * units;
    }
    world.introduced_mass = world.total_mass();
    (world, scene, settings)
}

type Frozen = Vec<(usize, f32, f32, f32)>;
fn frozen(world: &World, paused: u64) -> Frozen {
    let roof = &world.drifts[&7];
    let values: Vec<_> = (0..roof.mass.len())
        .filter(|&i| {
            world
                .scene
                .nearest_screen(Point::new(roof.x(i), roof.support.y - 1.0))
                .unwrap()
                .key
                == paused
        })
        .map(|i| {
            (
                i,
                roof.mass[i],
                roof.pending[i],
                roof.height(i, &world.scene),
            )
        })
        .collect();
    assert!(
        values
            .iter()
            .any(|&(_, mass, pending, _)| mass > 0.0 && pending > 0.0)
    );
    values
}
fn assert_frozen(world: &World, before: &Frozen) {
    let roof = &world.drifts[&7];
    for &(i, mass, pending, height) in before {
        assert_eq!(roof.mass[i], mass);
        assert_eq!(roof.pending[i], pending);
        assert_eq!(roof.height(i, &world.scene), height);
    }
}
fn floor(world: &World, screen: u64) -> f64 {
    world.drifts[&(screen | (1 << 63))].total_mass()
}

#[test]
fn shared_irregular_resize_expansion_preserves_physical_stock_and_freezes_only_paused_cells() {
    for scales in [(1.0, 2.5), (2.5, 1.0)] {
        for paused in [1, 2] {
            for offset in [0.0, -800.0] {
                for (left, right) in [(-60.0, 0.0), (0.0, 60.0), (-40.0, 40.0)] {
                    let (mut world, mut scene, settings) = shared(scales, offset);
                    let old = bins(&world);
                    let introduced = world.introduced_mass;
                    world.set_paused_monitors([paused].into());
                    scene.windows[0].rect.left += left;
                    scene.windows[0].rect.right += right;
                    world.set_scene(scene);
                    remapped(&old, &world);
                    let current = bins(&world);
                    assert!(
                        current
                            .iter()
                            .any(|b| b.right <= old[0].left || b.left >= old.last().unwrap().right)
                    );
                    for b in current {
                        if b.right <= old[0].left || b.left >= old.last().unwrap().right {
                            assert_eq!((b.mass, b.pending), (0.0, 0.0));
                        }
                    }
                    let frozen = frozen(&world, paused);
                    let pending_before: f64 =
                        world.drifts[&7].pending.iter().map(|&p| f64::from(p)).sum();
                    let frozen_pending: f64 = frozen.iter().map(|&(_, _, p, _)| f64::from(p)).sum();
                    assert!(pending_before > frozen_pending);
                    for _ in 0..120 {
                        world.step(&settings, None);
                        assert_frozen(&world, &frozen);
                        assert!(world.flakes.is_empty());
                        assert_eq!(world.pending_collapse_count(), 0);
                        budget(&world, introduced);
                    }
                    let pending: f64 = world.drifts[&7].pending.iter().map(|&p| f64::from(p)).sum();
                    assert!(pending - frozen_pending < (pending_before - frozen_pending) * 0.001);
                    world.set_paused_monitors(Default::default());
                    for _ in 0..120 {
                        world.step(&settings, None);
                        assert!(world.flakes.is_empty());
                        assert_eq!(world.pending_collapse_count(), 0);
                        budget(&world, introduced);
                    }
                    assert!(
                        world.drifts[&7]
                            .pending
                            .iter()
                            .map(|&p| f64::from(p))
                            .sum::<f64>()
                            < pending_before * 0.001
                    );
                    assert_eq!(floor(&world, 1) + floor(&world, 2), 0.0);
                    assert_eq!(world.removed_mass, 0.0);
                    assert!(
                        (world.drifts[&7].total_mass() - introduced).abs() < introduced * 0.00002
                    );
                }
            }
        }
    }
}

#[test]
fn shared_irregular_resize_cuts_wait_per_monitor_and_land_after_resume_without_returning_to_roof() {
    for scales in [(1.0, 2.5), (2.5, 1.0)] {
        for paused in [1, 2] {
            for offset in [0.0, -800.0] {
                let (mut world, mut scene, settings) = shared(scales, offset);
                let old = bins(&world);
                let introduced = world.introduced_mass;
                world.set_paused_monitors([paused].into());
                scene.windows[0].rect.left += 45.0;
                scene.windows[0].rect.right -= 90.0;
                world.set_scene(scene);
                remapped(&old, &world);
                let roof = &world.drifts[&7];
                let left = integral(&old, old[0].left, f64::from(roof.support.left));
                let right = integral(
                    &old,
                    f64::from(roof.support.right),
                    old.last().unwrap().right,
                );
                let cuts = [left.0 + left.1, right.0 + right.1];
                assert!(cuts.iter().all(|mass| *mass > 0.0));
                let retained = roof.total_mass();
                let frozen = frozen(&world, paused);
                world.step(&settings, None);
                assert!(!world.flakes.is_empty());
                assert!(
                    world.flakes.iter().all(|body| world
                        .scene
                        .nearest_screen(body.position)
                        .unwrap()
                        .key
                        != paused)
                );
                let waiting = world.pending_collapse_count();
                assert!(waiting > 0);
                for _ in 0..600 {
                    world.step(&settings, None);
                    assert_frozen(&world, &frozen);
                    assert_eq!(world.pending_collapse_count(), waiting);
                    budget(&world, introduced);
                }
                assert!(world.flakes.is_empty(), "active cut did not finish landing");
                let active = if paused == 1 { 2 } else { 1 };
                assert_eq!(floor(&world, paused), 0.0);
                assert!(
                    (floor(&world, active) - cuts[active as usize - 1]).abs()
                        < introduced * 0.00002
                );
                assert!(
                    (world.total_mass()
                        - world.drifts[&7].total_mass()
                        - floor(&world, active)
                        - cuts[paused as usize - 1])
                        .abs()
                        < introduced * 0.00002
                );
                world.set_paused_monitors(Default::default());
                world.step(&settings, None);
                assert!(!world.flakes.is_empty());
                assert!(
                    world.flakes.iter().all(|body| world
                        .scene
                        .nearest_screen(body.position)
                        .unwrap()
                        .key
                        == paused)
                );
                for _ in 0..1800 {
                    world.step(&settings, None);
                    budget(&world, introduced);
                    if world.flakes.is_empty() && world.pending_collapse_count() == 0 {
                        break;
                    }
                }
                assert!(world.flakes.is_empty() && world.pending_collapse_count() == 0);
                assert!((world.drifts[&7].total_mass() - retained).abs() < introduced * 0.00002);
                for screen in [1, 2] {
                    assert!(
                        (floor(&world, screen) - cuts[screen as usize - 1]).abs()
                            < introduced * 0.00002
                    );
                    let ground = &world.drifts[&(screen | (1 << 63))];
                    assert!((0..ground.mass.len()).any(|i| ground.height(i, &world.scene) > 0.001));
                }
                budget(&world, introduced);
            }
        }
    }
}
