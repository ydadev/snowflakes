//! Accumulation provenance, queues and emitter continuity at explicit clear.
use super::*;
use crate::geometry::Window;

#[test]
fn clear_accumulation_removes_loose_and_cohesive_outflow_across_paused_monitors() {
    let mut scene = Scene {
        screens: (0..2)
            .map(|i| Screen {
                key: i + 1,
                persistent_id: false,
                rect: Rect {
                    left: i as f32 * 800.0,
                    top: 0.0,
                    right: (i + 1) as f32 * 800.0,
                    bottom: 600.0,
                },
                floor: 600.0,
                scale: 1.0,
            })
            .collect(),
        windows: (0..2)
            .map(|i| Window {
                key: 7 + i,
                rect: Rect {
                    left: i as f32 * 800.0 + 100.0,
                    top: 300.0,
                    right: i as f32 * 800.0 + 700.0,
                    bottom: 500.0,
                },
                visible: true,
                collect: true,
            })
            .collect(),
    };
    let mut world = World::new(scene.clone(), 103);
    let mut settings = Settings::default();
    settings.snow.intensity_percent = 0;
    settings.snow.melting = false;
    settings.interaction.cursor_enabled = false;
    for drift in world.drifts.values_mut() {
        drift.mass.fill(4.0);
        drift.pending.fill(0.5);
    }
    world.set_paused_monitors([2].into());
    scene.windows.clear();
    world.set_scene(scene);
    // Use the production loose-material creator, including the origin=None case
    // used by cursor sweeping; no radius/opacity heuristic identifies that case.
    for x in [100.0, 900.0] {
        world.queue_spill(Point::new(x, 100.0), 3.0, Point::new(1.0, 2.0), None, 1.0);
    }
    world.step(&settings, None);
    assert!(
        world
            .flakes
            .iter()
            .any(|b| b.from_accumulation && b.slab.is_none() && b.skip_support.is_none())
    );
    assert!(world.flakes.iter().any(|b| b.slab.is_some()));
    assert!(
        !world.collapses.is_empty() && !world.spill.is_empty() && !world.falling_masks.is_empty()
    );
    for x in [200.0, 1000.0] {
        assert!(world.release(Flake::new(Point::new(x, 40.0), 2.0)));
        world
            .flakes
            .last_mut()
            .unwrap()
            .integrate(Point::new(3.0, 1.0), 1.0);
    }
    let ordinary = |world: &World| {
        format!(
            "{:?}",
            world
                .flakes
                .iter()
                .filter(|b| !b.from_accumulation && b.slab.is_none() && b.skip_support.is_none())
                .collect::<Vec<_>>()
        )
    };
    let before = ordinary(&world);
    assert!(world.drifts.values().any(|d| d.total_mass() > 0.0));
    world.introduced_mass = world.total_mass() + world.removed_mass;
    world.emission = 0.375;
    let (rng, wind, phase, age) = (
        world.rng.0,
        world.wind,
        world.emission.to_bits(),
        world.scene_age.to_bits(),
    );
    world.clear_accumulation();
    assert_eq!(ordinary(&world), before);
    assert_eq!(world.flakes.len(), 2);
    assert!(world.spill.is_empty() && world.collapses.is_empty() && world.falling_masks.is_empty());
    assert!(world.drifts.values().all(|d| d.total_mass() == 0.0));
    assert_eq!(
        (
            world.rng.0,
            world.wind,
            world.emission.to_bits(),
            world.scene_age.to_bits()
        ),
        (rng, wind, phase, age)
    );
    assert_eq!(world.paused_monitors, [2].into());
    assert_eq!(world.usage.values().map(|u| u.total).sum::<usize>(), 2);
    assert_eq!(world.usage.values().map(|u| u.ordinary).sum::<usize>(), 2);
    assert!((world.introduced_mass - world.removed_mass - world.total_mass()).abs() < 1e-9);
    settings.snow.intensity_percent = 100;
    let introduced = world.introduced_mass;
    for _ in 0..120 {
        world.step(&settings, None);
    }
    assert!(world.introduced_mass > introduced && world.flakes.len() > 2);
    eprintln!(
        "clear-material: retained_air=2 loose_without_origin=true active_slabs=true queued_slabs=true paused_material=true emitter_phase_rng_wind_preserved=true"
    );
}
