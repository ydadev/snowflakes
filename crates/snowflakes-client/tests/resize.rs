use snowflakes_client::{
    geometry::{Point, Rect, Scene, Screen, Window},
    snow::{Flake, MAX_FLAKES, ORDINARY_FLAKES_PER_SCREEN, SnowSlab, World},
};
use snowflakes_protocol::Settings;

fn fixture(scale: f32) -> (World, Scene, Settings) {
    let scene = Scene {
        screens: vec![Screen {
            key: 1,
            persistent_id: true,
            rect: Rect {
                left: 0.0,
                top: 0.0,
                right: 800.0 * scale,
                bottom: 600.0 * scale,
            },
            floor: 600.0 * scale,
            scale,
        }],
        windows: vec![Window {
            key: 7,
            rect: Rect {
                left: 100.0 * scale,
                top: 160.0 * scale,
                right: 500.0 * scale,
                bottom: 400.0 * scale,
            },
            visible: true,
            collect: true,
        }],
    };
    let mut settings = Settings::default();
    settings.snow.intensity_percent = 0;
    settings.snow.melting = false;
    settings.snow.drift_size_percent = 100;
    (World::new(scene.clone(), 704), scene, settings)
}

fn stock(world: &mut World, scale: f32, pending: bool) {
    let roof = world.drifts.get_mut(&7).unwrap();
    roof.mass
        .fill((if pending { 3.0 } else { 4.0 }) * roof.cell_width / scale);
    if pending {
        roof.pending.fill(roof.cell_width / scale);
    }
    world.introduced_mass = world.total_mass();
}
fn conserved(world: &World) {
    let error = (world.introduced_mass - world.removed_mass - world.total_mass()).abs();
    assert!(
        error <= world.introduced_mass.max(1.0) * 0.001,
        "mass error {error}"
    );
}

#[test]
fn either_edge_and_center_shrink_preserve_supported_density_and_release_only_the_cut_volume() {
    for scale in [1.0, 1.25, 1.5, 2.5] {
        for offset in [0.0, -800.0 * scale] {
            for (left, right) in [(50.0, 0.0), (0.0, 50.0), (50.0, 50.0)] {
                let (_, mut scene, settings) = fixture(scale);
                scene.screens[0].rect.left += offset;
                scene.screens[0].rect.right += offset;
                scene.windows[0].rect.left += offset;
                scene.windows[0].rect.right += offset;
                let mut world = World::new(scene.clone(), 705);
                stock(&mut world, scale, true);
                let before = world.total_mass();
                scene.windows[0].rect.left += left * scale;
                scene.windows[0].rect.right -= right * scale;
                world.set_scene(scene);
                let retained = before - f64::from(4.0 * (left + right));
                assert!((world.drifts[&7].total_mass() - retained).abs() < 0.002);
                let roof = &world.drifts[&7];
                for (&mass, &pending) in roof.mass.iter().zip(&roof.pending) {
                    assert!((mass / roof.cell_width - 3.0 / scale).abs() < 0.0001);
                    assert!((pending / roof.cell_width - 1.0 / scale).abs() < 0.0001);
                }
                assert!(world.pending_collapse_count() > 0);
                assert_eq!(world.removed_mass, 0.0);
                conserved(&world);
                world.step(&settings, None);
                assert!(world.flakes.iter().all(|f| f.slab.is_some()));
                for _ in 0..240 {
                    world.step(&settings, None);
                    conserved(&world);
                }
                assert!(
                    (world.drifts[&7].total_mass() - retained).abs() < 0.02,
                    "cut snow returned to its resized source"
                );
            }
        }
    }
}

#[test]
fn expanding_either_edge_keeps_the_existing_peak_in_place_and_adds_no_snow() {
    for scale in [1.0, 1.25, 1.5, 2.5] {
        for (left, right) in [(-100.0, 0.0), (0.0, 100.0), (-50.0, 50.0)] {
            let (mut world, mut scene, _) = fixture(scale);
            let roof = world.drifts.get_mut(&7).unwrap();
            let index = roof.mass.len() / 2;
            roof.mass[index] = 20.0;
            let center = roof.x(index);
            world.introduced_mass = world.total_mass();
            scene.windows[0].rect.left += left * scale;
            scene.windows[0].rect.right += right * scale;
            world.set_scene(scene);
            let roof = &world.drifts[&7];
            let mean = roof
                .mass
                .iter()
                .enumerate()
                .map(|(i, m)| f64::from(*m) * f64::from(roof.x(i)))
                .sum::<f64>()
                / roof.total_mass();
            assert!((mean - f64::from(center)).abs() <= f64::from(roof.cell_width) * 0.51);
            for (i, &m) in roof.mass.iter().enumerate() {
                if (roof.x(i) - center).abs() > roof.cell_width * 2.0 {
                    assert_eq!(m, 0.0);
                }
            }
            assert_eq!(world.pending_collapse_count(), 0);
            assert!(world.flakes.is_empty());
            conserved(&world);
        }
    }
}

#[test]
fn subcell_cut_preserves_the_fragment_width_height_and_mass() {
    let (mut world, mut scene, settings) = fixture(1.0);
    stock(&mut world, 1.0, false);
    let before = world.total_mass();
    scene.windows[0].rect.right -= 0.25;
    world.set_scene(scene);
    assert!((world.drifts[&7].total_mass() - (before - 1.0)).abs() < 0.001);
    world.step(&settings, None);
    assert_eq!(world.flakes.len(), 1);
    let body = &world.flakes[0];
    let slab = body.slab.as_ref().unwrap();
    assert_eq!(slab.heights.len(), 1);
    assert!((slab.cell_width * world.particle_scale(body) - 0.25).abs() < 0.0001);
    assert!((slab.heights[0] * world.particle_scale(body) - 4.0).abs() < 0.0001);
    assert!((slab.heights[0] * slab.cell_width - body.mass).abs() < 0.001);
    conserved(&world);
}

#[test]
fn shrinking_to_a_frame_without_a_snow_ledge_releases_all_material() {
    for (from, to) in [(2.5, 2.5), (1.0, 2.5)] {
        let (mut world, mut scene, settings) = fixture(from);
        stock(&mut world, from, false);
        scene.screens[0].scale = to;
        scene.windows[0].rect.right = scene.windows[0].rect.left + 24.0;
        world.set_scene(scene);
        assert!(world.drifts[&7].total_mass() < 0.0001);
        world.step(&settings, None);
        assert!(!world.flakes.is_empty());
        assert!(world.flakes.iter().all(|f| f.slab.is_some()));
        conserved(&world);
    }
}

#[test]
fn paused_resize_retains_detached_material_until_the_monitor_resumes() {
    let (mut world, mut scene, settings) = fixture(1.0);
    stock(&mut world, 1.0, false);
    world.set_paused_monitors([1].into());
    scene.windows[0].rect.right -= 100.0;
    world.set_scene(scene);
    let retained = world.drifts[&7].total_mass();
    let count = world.pending_collapse_count();
    assert!(count > 0);
    for _ in 0..120 {
        world.step(&settings, None);
        assert_eq!(world.pending_collapse_count(), count);
        assert!(world.flakes.is_empty());
        assert_eq!(world.drifts[&7].total_mass(), retained);
        conserved(&world);
    }
    world.set_paused_monitors(Default::default());
    world.step(&settings, None);
    assert!(!world.flakes.is_empty());
    conserved(&world);
}

#[test]
fn successive_cuts_keep_independent_front_layers_when_the_source_is_reordered() {
    let (mut world, mut scene, settings) = fixture(1.0);
    scene.windows.insert(
        0,
        Window {
            key: 8,
            rect: Rect {
                left: 320.0,
                top: 0.0,
                right: 550.0,
                bottom: 430.0,
            },
            visible: true,
            collect: true,
        },
    );
    world.set_scene(scene.clone());
    stock(&mut world, 1.0, false);
    scene.windows[1].rect.right = 400.0;
    world.set_scene(scene.clone());
    world.step(&settings, None);
    let old_count = world.flakes.len();
    assert!(old_count > 0);
    let probe = Rect {
        left: 430.0,
        top: 140.0,
        right: 470.0,
        bottom: 180.0,
    };
    assert_eq!(world.falling_clips(&world.flakes[0], probe), Some(vec![]));
    scene.windows.swap(0, 1);
    scene.windows[0].rect.right = 350.0;
    world.set_scene(scene);
    world.step(&settings, None);
    assert!(world.flakes.len() > old_count);
    assert_eq!(world.falling_clips(&world.flakes[0], probe), Some(vec![]));
    let probe = Rect {
        left: 360.0,
        top: 140.0,
        right: 380.0,
        bottom: 180.0,
    };
    assert_eq!(
        world.falling_clips(&world.flakes[old_count], probe),
        Some(vec![probe])
    );
    conserved(&world);
}

fn fill_active_budget(world: &mut World, scene: &mut Scene, settings: &Settings) {
    scene.windows.push(Window {
        key: 9,
        rect: Rect {
            left: 20.0,
            top: 80.0,
            right: 80.0,
            bottom: 120.0,
        },
        visible: true,
        collect: true,
    });
    world.set_scene(scene.clone());
    world.drifts.get_mut(&9).unwrap().mass[0] = 1.0;
    world.introduced_mass += 1.0;
    scene.windows.pop();
    world.set_scene(scene.clone());
    world.step(settings, None);
    let mut prototype = world.flakes[0].clone();
    prototype.position = Point::new(20.0, 20.0);
    prototype.mass = 1.0;
    prototype.slab = Some(SnowSlab {
        heights: vec![1.0],
        cell_width: 1.0,
        center: Point::new(0.5, -0.5),
    });
    for _ in 0..ORDINARY_FLAKES_PER_SCREEN {
        assert!(world.release(Flake::new(Point::new(20.0, 20.0), 2.0)));
    }
    while world.flakes.len() < MAX_FLAKES {
        assert!(world.release(prototype.clone()));
    }
}

#[test]
fn full_active_budget_keeps_resize_material_queued_until_a_natural_exit() {
    let (mut world, mut scene, settings) = fixture(1.0);
    stock(&mut world, 1.0, false);
    fill_active_budget(&mut world, &mut scene, &settings);
    scene.windows[0].rect.right -= 100.0;
    world.set_scene(scene);
    let waiting = world.pending_collapse_count();
    assert!(waiting > 0);
    world.step(&settings, None);
    assert_eq!(world.flakes.len(), MAX_FLAKES);
    assert_eq!(world.pending_collapse_count(), waiting);
    world.flakes[0].position.y = 800.0;
    for _ in 0..2 {
        world.step(&settings, None);
        conserved(&world);
    }
    assert!(world.flakes.iter().any(|f| {
        f.slab
            .as_ref()
            .is_some_and(|s| s.heights.iter().any(|&h| h > 1.0))
    }));
    assert!(world.flakes.len() <= MAX_FLAKES);
}

#[test]
fn common_translation_carries_the_peak_while_edge_changes_do_not_act_as_a_shake() {
    let (mut world, mut scene, settings) = fixture(1.0);
    let roof = world.drifts.get_mut(&7).unwrap();
    let middle = roof.mass.len() / 2;
    roof.mass[middle] = 20.0;
    world.introduced_mass = world.total_mass();
    world.step(&settings, None);
    let center = |world: &World| {
        let roof = &world.drifts[&7];
        roof.mass
            .iter()
            .enumerate()
            .map(|(i, m)| f64::from(*m) * f64::from(roof.x(i)))
            .sum::<f64>()
            / roof.total_mass()
    };
    let before = center(&world);
    scene.windows[0].rect.left += 1.0;
    scene.windows[0].rect.right += 51.0;
    world.set_scene(scene);
    assert!((center(&world) - before - 1.0).abs() < 3.0);
    world.step(&settings, None);
    assert!(world.flakes.is_empty());
    assert_eq!(world.pending_collapse_count(), 0);
    conserved(&world);
}

#[test]
fn repeated_subcell_cuts_compact_with_live_origin_exclusion_and_keep_the_source_roof_unchanged() {
    let (mut world, mut scene, settings) = fixture(1.0);
    stock(&mut world, 1.0, false);
    fill_active_budget(&mut world, &mut scene, &settings);
    let before = world.drifts[&7].total_mass();
    for _ in 0..1100 {
        scene.windows[0].rect.right -= 0.125;
        world.set_scene(scene.clone());
        conserved(&world);
        let budget = world.budget_snapshot();
        assert!(budget.pending_packets <= budget.max_pending_packets);
        assert!(budget.pending_bodies <= budget.max_pending_bodies);
    }
    let budget = world.budget_snapshot();
    assert!(
        budget.pending_bodies < 1100,
        "no forced compaction was exercised"
    );
    let retained = before - 550.0;
    assert!((world.drifts[&7].total_mass() - retained).abs() < 0.02);
    // Put the live source under its detached queue. Even merged auxiliary layers
    // must remember the original support, rather than depositing back onto it.
    scene.windows[0].rect.left += 100.0;
    scene.windows[0].rect.right += 100.0;
    world.set_scene(scene.clone());
    for body in &mut world.flakes {
        body.position.y = 10_000.0;
    }
    for _ in 0..300 {
        world.step(&settings, None);
        conserved(&world);
    }
    assert!(
        (world.drifts[&7].total_mass() - retained).abs() < 0.05,
        "compacted snow returned to its live source"
    );
    assert_eq!(world.pending_collapse_count(), 0);
    world.set_scene(scene);
    conserved(&world);
}

#[test]
fn resize_mass_matches_at_thirty_and_sixty_scene_updates_per_second() {
    for scale in [1.0, 2.5] {
        let mut results = Vec::new();
        for fps in [30, 60] {
            let (mut world, mut scene, settings) = fixture(scale);
            stock(&mut world, scale, false);
            let before = world.total_mass();
            for tick in 1..=480 {
                if tick <= 60 && tick % (60 / fps) == 0 {
                    scene.windows[0].rect.right = (500.0 - tick as f32) * scale;
                    world.set_scene(scene.clone());
                }
                world.step(&settings, None);
                conserved(&world);
            }
            assert!((world.drifts[&7].total_mass() - (before - 240.0)).abs() < 0.02);
            let ground = world
                .drifts
                .values()
                .filter(|d| d.support.ground)
                .map(|d| d.total_mass())
                .sum::<f64>();
            assert!((ground - 240.0).abs() < 0.02);
            // Deposits can leave a few f32 ULPs, which are explicitly accounted as outflow.
            assert!(world.removed_mass < 0.001);
            results.push((world.drifts[&7].total_mass(), ground));
        }
        assert!((results[0].0 - results[1].0).abs() < 0.02);
        assert!((results[0].1 - results[1].1).abs() < 0.02);
    }
}

#[test]
fn a_cut_across_mixed_dpi_screens_keeps_physical_height_and_respects_each_monitor_pause() {
    let (mut world, mut scene, settings) = fixture(1.0);
    scene.screens[0].rect.right = 400.0;
    let mut right = scene.screens[0].clone();
    right.key = 2;
    right.rect.left = 400.0;
    right.rect.right = 800.0;
    right.scale = 2.5;
    scene.screens.push(right);
    scene.windows[0].rect.left = 260.0;
    scene.windows[0].rect.right = 660.0;
    world.set_scene(scene.clone());
    let roof = world.drifts.get_mut(&7).unwrap();
    for i in 0..roof.mass.len() {
        let unit = scene.scale_at(Point::new(roof.x(i), roof.support.y - 1.0));
        roof.mass[i] = 5.0 * roof.cell_width / (unit * unit);
    }
    let retained = roof.mass[..13].iter().map(|&m| f64::from(m)).sum::<f64>();
    let cut = roof.support.left + 13.0 * roof.cell_width;
    world.introduced_mass = world.total_mass();
    world.set_paused_monitors([1].into());
    scene.windows[0].rect.right = cut + 6.0;
    world.set_scene(scene);
    assert!((world.drifts[&7].total_mass() - retained).abs() < 0.01);
    for i in 0..world.drifts[&7].mass.len() {
        assert!((world.drifts[&7].height(i, &world.scene) - 5.0).abs() < 0.001);
    }
    world.step(&settings, None);
    assert!(!world.flakes.is_empty());
    assert!(world.flakes.iter().all(|f| f.position.x >= 400.0));
    assert!(world.pending_collapse_count() > 0);
    for body in &world.flakes {
        let slab = body.slab.as_ref().unwrap();
        let area = slab.heights.iter().sum::<f32>() * slab.cell_width;
        assert!((area - body.mass).abs() < body.mass * 0.001);
    }
    conserved(&world);
    world.set_paused_monitors(Default::default());
    world.step(&settings, None);
    assert!(world.flakes.iter().any(|f| f.position.x < 400.0));
    conserved(&world);
}

#[test]
fn repeated_expansion_does_not_create_subpixel_fragments_on_the_unchanged_edge() {
    for scale in [1.0, 1.25, 1.5, 2.5] {
        let (mut world, mut scene, _) = fixture(scale);
        stock(&mut world, scale, false);
        for _ in 0..256 {
            scene.windows[0].rect.left -= 1.0;
            world.set_scene(scene.clone());
            assert_eq!(
                world.pending_collapse_count(),
                0,
                "an unchanged right edge emitted a rounding-error fragment"
            );
            assert!(world.flakes.is_empty());
            conserved(&world);
        }
    }
}

#[test]
fn right_edge_shrink_releases_snow_above_the_removed_strip_instead_of_compressing_it() {
    let (mut world, mut scene, settings) = fixture(1.0);
    let roof = world.drifts.get_mut(&7).unwrap();
    for i in 0..roof.mass.len() {
        if roof.x(i) > 430.0 {
            roof.mass[i] = 5.0 * roof.cell_width;
        }
    }
    world.introduced_mass = world.total_mass();
    let before = world.total_mass();
    assert!(before > 0.0);
    scene.windows[0].rect.right = 400.0;
    world.set_scene(scene);
    assert!(
        world.drifts[&7].total_mass() < 0.0001,
        "snow from the removed right strip was squeezed onto the remaining roof"
    );
    assert!(world.pending_collapse_count() > 0);
    assert!((world.total_mass() - before).abs() < before * 0.0001);
    world.step(&settings, None);
    assert!(!world.flakes.is_empty());
    assert!(
        world
            .flakes
            .iter()
            .all(|body| body.slab.is_some() && body.skip_support.is_some())
    );
    assert!((world.total_mass() - before).abs() < before * 0.001);
    assert_eq!(world.removed_mass, 0.0);
}
