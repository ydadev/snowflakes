use snowflakes_client::{
    geometry::{Point, Rect, Scene, Screen, Window},
    snow::{BASE_COLLAPSE_BUDGET, Flake, MAX_FLAKES, ORDINARY_FLAKES_PER_SCREEN, World},
    timing::StepClock,
};
use snowflakes_protocol::Settings;
use std::time::{Duration, Instant};
fn scene() -> Scene {
    Scene {
        screens: vec![Screen {
            key: 1,
            persistent_id: true,
            rect: Rect {
                left: 0.0,
                top: 0.0,
                right: 800.0,
                bottom: 600.0,
            },
            floor: 600.0,
            scale: 1.0,
        }],
        windows: vec![],
    }
}
fn quiet() -> Settings {
    let mut s = Settings::default();
    s.snow.intensity_percent = 0;
    s.snow.melting = false;
    s
}

#[test]
fn ordinary_air_leaves_reserved_slots_on_each_monitor() {
    let mut world = World::new(two_screens(), 101);
    let ordinary = MAX_FLAKES - BASE_COLLAPSE_BUDGET;
    for x in [50.0, 850.0] {
        for _ in 0..ordinary {
            assert!(world.release(Flake::new(Point::new(x, 100.0), 3.0)));
        }
        assert!(
            !world.release(Flake::new(Point::new(x, 100.0), 3.0)),
            "ordinary air consumed the collapse reserve"
        );
    }
    assert_eq!(world.flakes.len(), ordinary * 2);
    conserved(&world);
}

fn two_stocked_roofs(scale: f32) -> (World, Scene, Settings) {
    let mut desktop = two_screens();
    for screen in &mut desktop.screens {
        screen.rect.left *= scale;
        screen.rect.right *= scale;
        screen.rect.bottom *= scale;
        screen.floor *= scale;
        screen.scale = scale;
    }
    for (key, left) in [(7, 150.0), (8, 950.0)] {
        desktop.windows.push(Window {
            key,
            rect: Rect {
                left: left * scale,
                top: 200.0 * scale,
                right: (left + 500.0) * scale,
                bottom: 450.0 * scale,
            },
            visible: true,
            collect: true,
        });
    }
    let mut world = World::new(desktop.clone(), 101);
    for drift in world.drifts.values_mut().filter(|d| !d.support.ground) {
        drift.mass.fill(5.0 * drift.cell_width / scale);
    }
    world.introduced_mass = world.total_mass();
    let mut settings = quiet();
    settings.snow.drift_size_percent = 100;
    (world, desktop, settings)
}

fn two_stocked_screens(scale: f32) -> (World, Scene, Settings) {
    let (mut world, desktop, settings) = two_stocked_roofs(scale);
    for x in [50.0, 850.0] {
        for _ in 0..ORDINARY_FLAKES_PER_SCREEN {
            assert!(world.release(Flake::new(Point::new(x * scale, 100.0 * scale), 3.0)));
        }
    }
    (world, desktop, settings)
}

#[test]
fn full_air_on_both_screens_releases_slabs_on_the_first_active_step() {
    let (mut world, mut desktop, settings) = two_stocked_screens(1.0);
    let before = world.total_mass();
    world.set_paused_monitors([1].into());
    desktop.windows.clear();
    world.set_scene(desktop);
    world.step(&settings, None);
    assert!(world.flakes.len() > MAX_FLAKES && world.flakes.len() <= MAX_FLAKES * 2);
    let right: Vec<_> = world
        .flakes
        .iter()
        .filter(|body| body.slab.is_some())
        .collect();
    assert!(!right.is_empty());
    assert!(
        right
            .iter()
            .all(|body| body.position.x >= 800.0 && body.age > 1.0)
    );
    assert!(world.pending_collapse_count() > 0);
    assert!(
        world
            .flakes
            .iter()
            .filter(|body| body.position.x < 800.0)
            .all(|body| body.position == Point::new(50.0, 100.0) && body.age == 0.0)
    );
    world.set_paused_monitors(Default::default());
    world.step(&settings, None);
    assert_eq!(world.pending_collapse_count(), 0);
    assert!(
        world
            .flakes
            .iter()
            .any(|body| body.slab.is_some() && body.position.x < 800.0 && body.age > 1.0)
    );
    assert!((world.total_mass() - before).abs() < before * 0.00001);
    assert_eq!(world.removed_mass, 0.0);
    conserved(&world);
}

#[test]
fn a_monitor_crossing_does_not_consume_the_material_reserve_or_delete_bodies() {
    let mut desktop = two_screens();
    desktop.windows.push(Window {
        key: 7,
        rect: Rect {
            left: 950.0,
            top: 200.0,
            right: 1450.0,
            bottom: 450.0,
        },
        visible: true,
        collect: true,
    });
    let mut world = World::new(desktop.clone(), 101);
    let drift = world.drifts.get_mut(&7).unwrap();
    drift.mass.fill(5.0 * drift.cell_width);
    world.introduced_mass = world.total_mass();
    for x in [799.0, 850.0] {
        for _ in 0..ORDINARY_FLAKES_PER_SCREEN {
            let mut body = Flake::new(Point::new(x, 100.0), 3.0);
            body.velocity.x = 100.0;
            assert!(world.release(body));
        }
    }
    let mut settings = quiet();
    settings.snow.drift_size_percent = 100;
    world.step(&settings, None);
    assert!(world.flakes.iter().all(|body| body.position.x >= 800.0));
    let air = world.flakes.len();
    desktop.windows.clear();
    world.set_scene(desktop);
    world.step(&settings, None);
    assert_eq!(
        world
            .flakes
            .iter()
            .filter(|body| body.slab.is_none())
            .count(),
        air
    );
    assert!(
        world
            .flakes
            .iter()
            .any(|body| body.slab.is_some() && body.age > 1.0)
    );
    assert_eq!(world.pending_collapse_count(), 0);
    assert!(world.flakes.len() <= MAX_FLAKES * 2);
    assert!(
        !world.release(Flake::new(Point::new(50.0, 100.0), 3.0)),
        "crossings must not permit unbounded refilling"
    );
    assert_eq!(world.removed_mass, 0.0);
    let mut priority = world
        .flakes
        .iter()
        .find(|body| body.slab.is_some())
        .unwrap()
        .clone();
    priority.position = Point::new(50.0, 100.0);
    for _ in 0..BASE_COLLAPSE_BUDGET {
        assert!(world.release(priority.clone()));
    }
    assert!(
        !world.release(priority.clone()),
        "one screen borrowed another screen's material reserve"
    );
    priority.position = Point::new(850.0, 100.0);
    let mut admitted = 0;
    while world.release(priority.clone()) {
        admitted += 1;
    }
    assert!(admitted > 0);
    assert_eq!(world.flakes.len(), MAX_FLAKES * 2);
    conserved(&world);
}

#[test]
fn material_saturation_keeps_later_collapses_until_an_earlier_slab_lands() {
    let (mut world, mut desktop, settings) = wide_collapse(1.0);
    let first_mass = world.total_mass();
    for _ in 0..ORDINARY_FLAKES_PER_SCREEN {
        assert!(world.release(Flake::new(Point::new(50.0, 100.0), 3.0)));
    }
    desktop.windows.clear();
    world.set_scene(desktop.clone());
    world.step(&settings, None);
    assert_eq!(world.flakes.len(), MAX_FLAKES);
    desktop.windows.push(Window {
        key: 8,
        rect: Rect {
            left: 900.0,
            top: 200.0,
            right: 1500.0,
            bottom: 450.0,
        },
        visible: true,
        collect: true,
    });
    world.set_scene(desktop.clone());
    let drift = world.drifts.get_mut(&8).unwrap();
    drift.mass.fill(5.0 * drift.cell_width);
    let second_mass = drift.total_mass();
    world.introduced_mass += second_mass;
    desktop.windows.clear();
    world.set_scene(desktop);
    let before = world.total_mass();
    world.step(&settings, None);
    assert!(world.pending_collapse_count() > 0);
    assert!((world.total_mass() - before).abs() < before * 0.00001);
    assert_eq!(world.removed_mass, 0.0);
    for _ in 0..360 {
        world.step(&settings, None);
    }
    assert_eq!(world.pending_collapse_count(), 0);
    assert!(world.flakes.iter().all(|body| body.slab.is_none()));
    assert!(world.drifts[&(1 | (1 << 63))].total_mass() > (first_mass + second_mass) * 0.999);
    conserved(&world);
}

fn full_material_with_pending_roofs(scale: f32, paired: bool) -> (World, Scene, Settings, f64) {
    let mut desktop = scene();
    desktop.screens[0].rect.right = 12000.0 * scale;
    desktop.screens[0].rect.bottom = 600.0 * scale;
    desktop.screens[0].floor = 600.0 * scale;
    desktop.screens[0].scale = scale;
    for (key, left, right, top) in [
        (6, 11700.0, 11900.0, 100.0),
        (7, 100.0, 142.0, 200.0),
        (8, 500.0, 11500.0, 200.0),
    ] {
        desktop.windows.push(Window {
            key,
            rect: Rect {
                left: left * scale,
                right: right * scale,
                top: top * scale,
                bottom: 450.0 * scale,
            },
            visible: true,
            collect: true,
        });
    }
    if paired {
        desktop.windows.iter_mut().find(|w| w.key == 8).unwrap().key = 9;
        desktop.windows.push(Window {
            key: 8,
            rect: Rect {
                left: 100.0 * scale,
                right: 142.0 * scale,
                top: 201.0 * scale,
                bottom: 451.0 * scale,
            },
            visible: true,
            collect: true,
        });
    }
    let mut world = World::new(desktop.clone(), 101);
    let roof = world.drifts.get_mut(&6).unwrap();
    roof.mass.fill(5.0 * roof.cell_width / scale);
    world.introduced_mass = world.total_mass();
    desktop.windows.retain(|window| window.key != 6);
    world.set_scene(desktop.clone());
    let mut settings = quiet();
    settings.snow.drift_size_percent = 100;
    world.step(&settings, None);
    let filler = world.flakes[0].clone();
    while world.release(filler.clone()) {}
    assert_eq!(world.flakes.len(), MAX_FLAKES);
    for key in if paired { vec![7, 8, 9] } else { vec![7, 8] } {
        let roof = world.drifts.get_mut(&key).unwrap();
        roof.mass.fill(2.0 * roof.cell_width / scale);
        world.introduced_mass += roof.total_mass();
    }
    let near_mass = world.drifts[&7].total_mass()
        + if paired {
            world.drifts[&8].total_mass()
        } else {
            0.0
        };
    desktop.windows.clear();
    world.set_scene(desktop.clone());
    assert_eq!(world.pending_collapse_count(), BASE_COLLAPSE_BUDGET);
    conserved(&world);
    (world, desktop, settings, near_mass)
}

#[test]
fn merging_new_snow_does_not_send_the_oldest_waiting_roof_to_the_back() {
    for scale in [1.0, 2.0] {
        let (mut world, mut desktop, settings, _) = full_material_with_pending_roofs(scale, false);
        desktop.windows.push(Window {
            key: 9,
            rect: Rect {
                left: 100.0 * scale,
                right: 142.0 * scale,
                top: 200.0 * scale,
                bottom: 450.0 * scale,
            },
            visible: true,
            collect: true,
        });
        world.set_scene(desktop.clone());
        let roof = world.drifts.get_mut(&9).unwrap();
        roof.mass.fill(2.0 * roof.cell_width / scale);
        world.introduced_mass += roof.total_mass();
        desktop.windows.clear();
        world.set_scene(desktop);
        // A real exit frees one slot after this step's admission pass.
        world.flakes[0].position.x = world.scene.bounds().right + 79.0;
        world.flakes[0].velocity.x = 220.0;
        world.step(&settings, None);
        assert_eq!(world.flakes.len(), MAX_FLAKES - 1);
        assert_eq!(world.pending_collapse_count(), BASE_COLLAPSE_BUDGET);
        world.step(&settings, None);
        assert_eq!(world.flakes.len(), MAX_FLAKES);
        assert!(
            world
                .flakes
                .iter()
                .any(|body| body.slab.is_some() && body.position.x < 300.0 * scale),
            "the oldest pending roof was displaced by its new merged snow"
        );
        assert_eq!(world.pending_collapse_count(), BASE_COLLAPSE_BUDGET - 1);
        conserved(&world);
    }
}

#[test]
fn merging_two_waiting_roofs_preserves_their_oldest_admission_position() {
    for scale in [1.0, 2.0] {
        let (mut world, _, settings, _) = full_material_with_pending_roofs(scale, true);
        world.flakes[0].position.x = world.scene.bounds().right + 79.0;
        world.flakes[0].velocity.x = 220.0;
        world.step(&settings, None);
        assert_eq!(world.flakes.len(), MAX_FLAKES - 1);
        world.step(&settings, None);
        assert!(
            world
                .flakes
                .iter()
                .any(|body| body.slab.is_some() && body.position.x < 300.0 * scale),
            "compaction of two old roofs put their mass behind later roofs"
        );
        conserved(&world);
    }
}

#[test]
fn repeated_arrivals_and_paused_scene_refreshes_keep_old_snow_first() {
    for scale in [1.0, 2.0] {
        let (mut world, mut desktop, settings, near_mass) =
            full_material_with_pending_roofs(scale, false);
        let before: Vec<_> = world
            .flakes
            .iter()
            .map(|b| (b.position, b.velocity, b.mass, b.age, b.angle, b.spin))
            .collect();
        world.set_paused_monitors([1].into());
        let mut added = 0.0;
        for key in 10..34 {
            desktop.windows.push(Window {
                key,
                rect: Rect {
                    left: 100.0 * scale,
                    right: 142.0 * scale,
                    top: 200.0 * scale,
                    bottom: 450.0 * scale,
                },
                visible: true,
                collect: true,
            });
            world.set_scene(desktop.clone());
            let roof = world.drifts.get_mut(&key).unwrap();
            roof.mass.fill(2.0 * roof.cell_width / scale);
            let mass = roof.total_mass();
            added += mass;
            world.introduced_mass += mass;
            desktop.windows.clear();
            world.set_scene(desktop.clone());
            world.step(&settings, None);
            assert_eq!(world.pending_collapse_count(), BASE_COLLAPSE_BUDGET);
            conserved(&world);
        }
        assert_eq!(
            before,
            world
                .flakes
                .iter()
                .map(|b| (b.position, b.velocity, b.mass, b.age, b.angle, b.spin))
                .collect::<Vec<_>>()
        );
        world.set_paused_monitors(Default::default());
        world.flakes[0].position.x = world.scene.bounds().right + 79.0;
        world.flakes[0].velocity.x = 220.0;
        world.step(&settings, None);
        world.set_scene(desktop);
        world.step(&settings, None);
        let oldest = world
            .flakes
            .iter()
            .find(|body| body.slab.is_some() && body.position.x < 300.0 * scale)
            .expect("old material was starved by repeated arrivals");
        assert!(
            (f64::from(oldest.mass) - added - near_mass).abs() < (added + near_mass) * 0.001,
            "the emitted aggregate lost its repeatedly merged mass"
        );
        assert_eq!(world.pending_collapse_count(), BASE_COLLAPSE_BUDGET - 1);
        conserved(&world);
    }
}

#[test]
fn compacted_waiting_order_matches_at_thirty_and_sixty_fps() {
    fn run(fps: u32) -> World {
        let (mut world, _, settings, _) = full_material_with_pending_roofs(1.0, true);
        world.flakes[0].position.x = world.scene.bounds().right + 79.0;
        world.flakes[0].velocity.x = 220.0;
        let start = Instant::now();
        let mut clock = StepClock::default();
        clock.advance(start, true);
        for frame in 1..=fps / 5 {
            let now =
                start + Duration::from_nanos(u64::from(frame) * 1_000_000_000 / u64::from(fps));
            for _ in 0..clock.advance(now, true).count {
                world.step(&settings, None);
            }
        }
        conserved(&world);
        world
    }
    let a = run(30);
    let b = run(60);
    assert_eq!(a.pending_collapse_count(), b.pending_collapse_count());
    assert_eq!(a.flakes.len(), b.flakes.len());
    for (a, b) in a.flakes.iter().zip(&b.flakes) {
        assert_eq!(
            (a.position, a.velocity, a.mass, a.age, a.angle),
            (b.position, b.velocity, b.mass, b.age, b.angle)
        );
    }
    assert_eq!(a.total_mass(), b.total_mass());
}

#[test]
fn continuous_roof_inflow_survives_monitor_pauses_and_disconnects() {
    for scale in [1.0, 2.0] {
        let (mut world, mut desktop, settings) = two_stocked_roofs(scale);
        let screens = desktop.screens.clone();
        desktop.windows.clear();
        world.set_scene(desktop.clone());
        world.step(&settings, None);
        // Begin at the material limit, rather than only filling ordinary air.
        for key in [1, 2] {
            let body = world
                .flakes
                .iter()
                .find(|b| {
                    b.slab.is_some() && world.scene.nearest_screen(b.position).unwrap().key == key
                })
                .unwrap()
                .clone();
            while world.release(body.clone()) {}
        }
        assert_eq!(world.flakes.len(), MAX_FLAKES * 2);
        assert!(world.flakes.iter().all(|body| body.slab.is_some()));
        let mut closures = 0;
        let mut max_pending = 0;
        let mut saturated_steps = 0;
        let mut mask_probes = std::collections::BTreeMap::new();
        for tick in 0..4200 {
            let wanted_screens = match tick {
                1200..1500 => &screens[..1],
                1500..1800 => &screens[..0],
                _ => &screens[..],
            };
            if desktop.screens.len() != wanted_screens.len() {
                desktop.screens = wanted_screens.to_vec();
                world.set_scene(desktop.clone());
            }
            let paused: std::collections::BTreeSet<u64> = match tick {
                0..600 | 2400..3000 => [1].into(),
                600..1200 => [2].into(),
                1800..1920 => [1, 2].into(),
                _ => Default::default(),
            };
            world.set_paused_monitors(paused.clone());
            // Eight roofs per simulated second, including the paused display.
            // Loading a known profile isolates release from snowfall admission.
            if tick < 3000 && tick % 15 == 0 {
                for screen in desktop.screens.clone() {
                    let key = 1000 + closures;
                    let left = screen.rect.left + 150.0 * scale;
                    desktop.windows.push(Window {
                        key,
                        rect: Rect {
                            left,
                            right: left + 300.0 * scale,
                            top: 200.0 * scale,
                            bottom: 450.0 * scale,
                        },
                        visible: true,
                        collect: true,
                    });
                    world.set_scene(desktop.clone());
                    let roof = world.drifts.get_mut(&key).unwrap();
                    roof.mass.fill(2.0 * roof.cell_width / scale);
                    world.introduced_mass += roof.total_mass();
                    desktop.windows.clear();
                    world.set_scene(desktop.clone());
                    closures += 1;
                }
            }
            let before_count = world.flakes.len();
            let before_mass = world.total_mass();
            let frozen = desktop.screens.iter().all(|s| paused.contains(&s.key));
            let before_bodies = frozen.then(|| world.flakes.clone());
            let target = MAX_FLAKES * desktop.screens.len().max(1);
            saturated_steps += usize::from(before_count >= target);
            world.step(&settings, None);
            assert!(world.flakes.len() <= before_count.max(target));
            assert!(world.flakes.len() <= MAX_FLAKES * 2);
            assert!(world.pending_collapse_count() <= BASE_COLLAPSE_BUDGET);
            max_pending = max_pending.max(world.pending_collapse_count());
            if let Some(before) = before_bodies {
                assert_eq!(world.total_mass(), before_mass);
                for (a, b) in before.iter().zip(&world.flakes) {
                    assert_eq!(
                        (a.position, a.velocity, a.mass, a.age, a.angle, a.spin),
                        (b.position, b.velocity, b.mass, b.age, b.angle, b.spin)
                    );
                }
                assert_eq!(before.len(), world.flakes.len());
            }
            if tick % 15 == 0 {
                conserved(&world);
                assert!(world.drifts.len() <= 256);
                assert!(world.drifts.values().map(|d| d.mass.len()).sum::<usize>() <= 65536);
                for body in &world.flakes {
                    assert!(body.position.x.is_finite() && body.position.y.is_finite());
                    assert!(body.mass.is_finite() && body.mass > 0.0);
                    if let Some(slab) = &body.slab {
                        assert!(slab.heights.len() <= 16);
                        assert!(slab.heights.iter().all(|h| h.is_finite() && *h >= 0.0));
                    }
                    if let Some(key) = body.skip_support {
                        mask_probes.entry(key).or_insert_with(|| body.clone());
                    }
                }
            }
        }
        assert_eq!(closures, 340);
        assert_eq!(max_pending, BASE_COLLAPSE_BUDGET);
        assert!(saturated_steps > 1);
        assert_eq!(
            world.pending_collapse_count(),
            0,
            "material never resumed after reconnect"
        );
        assert!(
            world.flakes.is_empty(),
            "released material still airborne after drain"
        );
        assert!(
            world
                .drifts
                .values()
                .any(|d| d.support.ground && d.total_mass() > 0.0)
        );
        assert!(!mask_probes.is_empty());
        for body in mask_probes.values() {
            assert!(world.falling_clips(body, body.draw_bounds(scale)).is_none());
        }
        conserved(&world);
        println!(
            "scale={scale}, closures={closures}, max_pending={max_pending}, saturated_steps={saturated_steps}"
        );
    }
}

#[test]
fn stocked_profiles_freeze_with_no_displays_and_resume_unchanged() {
    let profiles = |world: &World| {
        world
            .drifts
            .iter()
            .map(|(&key, d)| {
                (
                    key,
                    d.support.left,
                    d.support.right,
                    d.support.y,
                    d.cell_width,
                    d.mass.clone(),
                    d.pending.clone(),
                )
            })
            .collect::<Vec<_>>()
    };
    for scale in [1.0, 2.0, 2.5] {
        let (mut world, desktop, settings) = two_stocked_roofs(scale);
        for drift in world.drifts.values_mut().filter(|d| d.support.ground) {
            drift.mass.fill(3.0 * drift.cell_width / scale);
        }
        world.introduced_mass = world.total_mass();
        let before = profiles(&world);
        let mass = world.total_mass();
        let mut missing = desktop.clone();
        missing.screens.clear();
        for _ in 0..3 {
            world.set_scene(missing.clone());
            assert!(world.scene.screens.is_empty());
            for _ in 0..60 {
                world.step(&settings, None);
            }
            assert!(
                profiles(&world) == before,
                "resting snow rebased without displays at {scale}"
            );
            assert!(world.flakes.is_empty());
            assert_eq!(world.pending_collapse_count(), 0);
            assert_eq!(world.total_mass(), mass);
            assert_eq!(world.removed_mass, 0.0);
        }
        world.set_scene(desktop);
        assert!(profiles(&world) == before);
        conserved(&world);
    }
}

#[test]
fn closing_a_roof_with_no_displays_uses_its_last_dpi() {
    for scales in [[1.0, 1.0], [1.5, 2.5], [2.5, 1.5], [2.0, 2.0]] {
        let mut desktop = two_screens();
        for (screen, scale) in desktop.screens.iter_mut().zip(scales) {
            screen.scale = scale;
        }
        for (key, left) in [(7, 150.0), (8, 950.0)] {
            desktop.windows.push(Window {
                key,
                rect: Rect {
                    left,
                    right: left + 500.0,
                    top: 200.0,
                    bottom: 450.0,
                },
                visible: true,
                collect: true,
            });
        }
        let mut world = World::new(desktop.clone(), 101);
        let mut cells = std::collections::BTreeMap::new();
        for (&key, roof) in world.drifts.iter_mut().filter(|(_, d)| !d.support.ground) {
            let scale = world
                .scene
                .scale_at(Point::new(roof.x(0), roof.support.y - 1.0));
            roof.mass.fill(5.0 * roof.cell_width / scale);
            cells.insert(key, (roof.cell_width, scale));
        }
        world.introduced_mass = world.total_mass();
        let mut settings = quiet();
        settings.snow.drift_size_percent = 100;
        let screens = desktop.screens.clone();
        let mass = world.total_mass();
        desktop.screens.clear();
        world.set_scene(desktop.clone());
        desktop.windows.clear();
        world.set_scene(desktop.clone());
        for _ in 0..120 {
            world.step(&settings, None);
        }
        assert!(world.flakes.is_empty());
        assert!(world.pending_collapse_count() > 0);
        desktop.screens = screens;
        world.set_scene(desktop);
        world.step(&settings, None);
        assert_eq!(world.pending_collapse_count(), 0);
        let bodies: Vec<_> = world
            .flakes
            .iter()
            .filter(|b| b.skip_support.is_some_and(|key| cells.contains_key(&key)))
            .collect();
        assert!(!bodies.is_empty());
        assert_eq!(
            bodies
                .iter()
                .filter_map(|b| b.skip_support)
                .collect::<std::collections::BTreeSet<_>>(),
            [7, 8].into()
        );
        for body in bodies {
            let (physical_cell, scale) = cells[&body.skip_support.unwrap()];
            let slab = body.slab.as_ref().unwrap();
            assert!(
                (slab.cell_width * scale - physical_cell).abs() < 0.001,
                "last display DPI lost while closing a roof at {scale}"
            );
            assert!(slab.heights.iter().all(|h| (*h - 5.0).abs() < 0.001));
            assert!(world.falling_clips(body, body.draw_bounds(scale)).is_some());
        }
        assert!((world.total_mass() - mass).abs() < mass * 0.0001);
        assert_eq!(world.removed_mass, 0.0);
        conserved(&world);
    }
}

#[test]
fn compaction_with_no_displays_keeps_geometry_and_visibility_on_return() {
    for scale in [1.0, 2.0] {
        let mut desktop = Scene::default();
        for screen in 0..65 {
            let left = screen as f32 * 256.0 * scale;
            desktop.screens.push(Screen {
                key: screen + 1,
                persistent_id: true,
                rect: Rect {
                    left,
                    top: 0.0,
                    right: left + 256.0 * scale,
                    bottom: 600.0 * scale,
                },
                floor: 600.0 * scale,
                scale,
            });
            for hidden in [false, true] {
                desktop.windows.push(Window {
                    key: 1000 + screen * 2 + u64::from(hidden),
                    rect: Rect {
                        left: left + 50.0 * scale,
                        right: left + 104.0 * scale,
                        top: 200.0 * scale,
                        bottom: 450.0 * scale,
                    },
                    visible: !hidden,
                    collect: true,
                });
            }
        }
        let mut world = World::new(desktop.clone(), 101);
        let mut expected = [0.0; 2];
        for (&key, drift) in world.drifts.iter_mut().filter(|(_, d)| !d.support.ground) {
            drift.mass.fill(2.0 * drift.cell_width / scale);
            expected[(key % 2) as usize] += drift.total_mass();
        }
        world.introduced_mass = world.total_mass();
        desktop.windows.clear();
        world.set_scene(desktop.clone());
        assert_eq!(world.pending_collapse_count(), 130);
        let screens = std::mem::take(&mut desktop.screens);
        world.set_scene(desktop.clone());
        assert_eq!(world.pending_collapse_count(), BASE_COLLAPSE_BUDGET);
        for _ in 0..120 {
            world.step(&quiet(), None);
        }
        assert!(world.flakes.is_empty());
        desktop.screens = screens;
        world.set_scene(desktop);
        world.step(&quiet(), None);
        assert_eq!(world.flakes.len(), BASE_COLLAPSE_BUDGET);
        assert_eq!(world.pending_collapse_count(), 0);
        let mut emitted = [0.0; 2];
        for body in &world.flakes {
            let slab = body.slab.as_ref().unwrap();
            let span = slab.cell_width * slab.heights.len() as f32;
            assert!(
                span <= 298.001,
                "profile stretched through absent-display compaction: {span} DIP"
            );
            let hidden = world
                .falling_clips(body, body.draw_bounds(scale))
                .unwrap()
                .is_empty();
            emitted[usize::from(hidden)] += f64::from(body.mass);
        }
        for (actual, wanted) in emitted.into_iter().zip(expected) {
            assert!((actual - wanted).abs() < wanted * 0.001);
        }
        assert_eq!(world.removed_mass, 0.0);
        conserved(&world);
    }
}

fn mixed_particle_dpi() -> Scene {
    let mut desktop = two_screens();
    desktop.screens[1].scale = 2.5;
    desktop
}

#[test]
fn dpi_transition_keeps_flake_size_and_physical_velocity_continuous() {
    for (x, vx, target) in [(799.5, 100.0, 2.5), (800.5, -40.0, 1.0)] {
        let mut world = World::new(mixed_particle_dpi(), 41);
        let mut body = Flake::new(Point::new(x, 80.0), 3.0);
        body.velocity.x = vx;
        assert!(world.release(body));
        let initial_mass = world.total_mass();
        let mut scale = world.particle_scale(&world.flakes[0]);
        let mut physical_velocity = vx * scale;
        for _ in 0..60 {
            world.step(&quiet(), None);
            assert_eq!(world.flakes.len(), 1);
            let body = &world.flakes[0];
            let next = world.particle_scale(body);
            assert!(
                (0.95999..=1.04001).contains(&(next / scale)),
                "DPI size jumped from {scale} to {next}"
            );
            let next_velocity = body.velocity.x * next;
            assert!(
                next_velocity.abs() <= physical_velocity.abs() + 0.02,
                "a change of DIP units added horizontal speed without driving wind"
            );
            assert!(
                (next_velocity - physical_velocity).abs() < 10.0,
                "physical velocity jumped from {physical_velocity} to {next_velocity}"
            );
            assert_eq!(body.radius, 3.0);
            scale = next;
            physical_velocity = next_velocity;
        }
        assert!((scale - target).abs() < 0.00001);
        assert_eq!(world.total_mass(), initial_mass);
        assert_eq!(world.removed_mass, 0.0);
        conserved(&world);
    }
}

#[test]
fn dpi_transition_keeps_a_slab_cohesive_until_one_landing() {
    let mut world = World::new(mixed_particle_dpi(), 41);
    let mut body = Flake::new(Point::new(799.5, 80.0), (240.0_f32 / 1.2).sqrt());
    body.mass = 240.0;
    body.velocity.x = 100.0;
    body.slab = Some(snowflakes_client::snow::SnowSlab {
        heights: vec![5.0; 8],
        cell_width: 6.0,
        center: Point::new(24.0, -2.5),
    });
    assert!(world.release(body));
    let mut settings = quiet();
    settings.snow.drift_size_percent = 100;
    let bounds = world.flakes[0].draw_bounds(world.particle_scale(&world.flakes[0]));
    let mut width = bounds.width();
    let mut landed = false;
    for _ in 0..360 {
        world.step(&settings, None);
        if world.flakes.is_empty() {
            landed = true;
            break;
        }
        assert_eq!(world.flakes.len(), 1, "a DPI change fragmented the slab");
        let body = &world.flakes[0];
        let next = body.draw_bounds(world.particle_scale(body)).width();
        assert!(
            (0.92..=1.07).contains(&(next / width)),
            "rotating slab width jumped from {width} to {next}"
        );
        assert!(body.slab.is_some());
        width = next;
        conserved(&world);
    }
    assert!(landed);
    assert!((world.drifts[&(2 | (1 << 63))].total_mass() - 240.0).abs() < 0.01);
    assert_eq!(world.removed_mass, 0.0);
    conserved(&world);
}

#[test]
fn dpi_transition_waits_for_an_unpaused_step() {
    let mut desktop = scene();
    let mut world = World::new(desktop.clone(), 41);
    assert!(world.release(Flake::new(Point::new(400.0, 80.0), 3.0)));
    let before = world.flakes[0].clone();
    world.set_paused_monitors([1].into());
    desktop.screens[0].scale = 2.5;
    world.set_scene(desktop);
    for _ in 0..180 {
        assert_eq!(
            world.particle_scale(&world.flakes[0]),
            1.0,
            "DPI adaptation advanced during monitor pause"
        );
        world.step(&quiet(), None);
    }
    let body = &world.flakes[0];
    assert_eq!(
        (body.position, body.velocity, body.age, body.mass),
        (before.position, before.velocity, before.age, before.mass)
    );
    world.set_paused_monitors(Default::default());
    let mut scale = 1.0;
    for _ in 0..30 {
        world.step(&quiet(), None);
        let next = world.particle_scale(&world.flakes[0]);
        assert!((1.0..=1.04001).contains(&(next / scale)));
        scale = next;
    }
    assert!((scale - 2.5).abs() < 0.00001);
    conserved(&world);
}

#[test]
fn dpi_transition_is_identical_at_thirty_and_sixty_fps() {
    let run = |fps: u32| {
        let mut world = World::new(mixed_particle_dpi(), 41);
        let mut body = Flake::new(Point::new(799.5, 80.0), 3.0);
        body.velocity.x = 100.0;
        assert!(world.release(body));
        let start = Instant::now();
        let mut clock = StepClock::default();
        clock.advance(start, true);
        for frame in 1..=fps {
            let now =
                start + Duration::from_nanos(u64::from(frame) * 1_000_000_000 / u64::from(fps));
            for _ in 0..clock.advance(now, true).count {
                world.step(&quiet(), None);
            }
        }
        world
    };
    let a = run(30);
    let b = run(60);
    assert_eq!(a.flakes.len(), 1);
    assert_eq!(b.flakes.len(), 1);
    assert_eq!(
        a.particle_scale(&a.flakes[0]),
        b.particle_scale(&b.flakes[0])
    );
    let (a_body, b_body) = (&a.flakes[0], &b.flakes[0]);
    assert_eq!(
        (
            a_body.position,
            a_body.velocity,
            a_body.mass,
            a_body.angle,
            a_body.age
        ),
        (
            b_body.position,
            b_body.velocity,
            b_body.mass,
            b_body.angle,
            b_body.age
        )
    );
    assert_eq!(a.total_mass(), b.total_mass());
    conserved(&a);
    conserved(&b);
}

#[test]
fn seeding_uses_each_displays_capacity_instead_of_filling_the_first_display() {
    let mut desktop = two_screens();
    for (i, screen) in desktop.screens.iter_mut().enumerate() {
        screen.rect.left = i as f32 * 3840.0;
        screen.rect.right = (i + 1) as f32 * 3840.0;
        screen.rect.bottom = 2160.0;
        screen.floor = 2160.0;
    }
    let mut world = World::new(desktop, 101);
    let mut settings = quiet();
    settings.snow.intensity_percent = 100;
    world.seed_air(&settings);
    for key in [1, 2] {
        assert_eq!(
            world
                .flakes
                .iter()
                .filter(|body| world.scene.nearest_screen(body.position).unwrap().key == key)
                .count(),
            1600
        );
    }
    assert!(world.flakes.len() <= MAX_FLAKES * 2);
    conserved(&world);
}

#[test]
fn shrinking_display_capacity_preserves_existing_mass_and_rebuilds_slots_after_exit() {
    let (mut world, mut desktop, settings) = two_stocked_screens(1.0);
    for body in &mut world.flakes {
        if body.position.x >= 800.0 {
            body.position.x = 1000.0;
        }
    }
    let before = world.total_mass();
    desktop.screens.truncate(1);
    desktop.windows.clear();
    world.set_scene(desktop);
    assert!(world.flakes.len() > MAX_FLAKES);
    assert!((world.total_mass() - before).abs() < before * 0.00001);
    assert_eq!(world.removed_mass, 0.0);
    assert!(!world.release(Flake::new(Point::new(50.0, 100.0), 3.0)));
    world.step(&settings, None);
    assert!(world.pending_collapse_count() > 0);
    assert!(world.flakes.len() < MAX_FLAKES);
    world.step(&settings, None);
    assert!(
        world
            .flakes
            .iter()
            .any(|body| body.slab.is_some() && body.position.x < 800.0)
    );
    assert!(world.flakes.len() <= MAX_FLAKES);
    world.clear();
    assert_eq!(world.total_mass(), 0.0);
    assert_eq!(world.pending_collapse_count(), 0);
    assert!(world.release(Flake::new(Point::new(50.0, 100.0), 3.0)));
    conserved(&world);
}

#[test]
fn material_reserves_match_at_thirty_and_sixty_fps_through_pause_and_scene_refresh() {
    fn run(scale: f32, fps: u64) -> World {
        let (mut world, mut desktop, settings) = two_stocked_screens(scale);
        world.set_paused_monitors([1].into());
        desktop.windows.clear();
        world.set_scene(desktop.clone());
        let start = Instant::now();
        let mut clock = StepClock::default();
        clock.advance(start, true);
        let mut ticks = 0;
        for frame in 1..=fps {
            for _ in 0..clock
                .advance(
                    start + Duration::from_nanos(frame * 1_000_000_000 / fps),
                    true,
                )
                .count
            {
                if ticks == 30 {
                    world.set_paused_monitors(Default::default());
                }
                if ticks % 10 == 0 {
                    world.set_scene(desktop.clone());
                }
                world.step(&settings, None);
                ticks += 1;
            }
        }
        world
    }
    for scale in [1.0, 2.0] {
        let a = run(scale, 30);
        let b = run(scale, 60);
        assert!(a.flakes.iter().any(|body| body.slab.is_some()));
        assert_eq!(a.flakes.len(), b.flakes.len());
        assert_eq!(a.pending_collapse_count(), b.pending_collapse_count());
        for (a, b) in a.flakes.iter().zip(&b.flakes) {
            assert_eq!(a.position, b.position);
            assert_eq!(a.velocity, b.velocity);
            assert_eq!(a.mass, b.mass);
        }
        assert_eq!(a.total_mass(), b.total_mass());
        conserved(&a);
        conserved(&b);
    }
}

fn wide_collapse(scale: f32) -> (World, Scene, Settings) {
    let mut desktop = scene();
    desktop.screens[0].rect.right = 12_000.0 * scale;
    desktop.screens[0].rect.bottom = 600.0 * scale;
    desktop.screens[0].floor = 600.0 * scale;
    desktop.screens[0].scale = scale;
    desktop.windows.push(Window {
        key: 7,
        rect: Rect {
            left: 100.0 * scale,
            top: 200.0 * scale,
            right: 11_100.0 * scale,
            bottom: 450.0 * scale,
        },
        visible: true,
        collect: true,
    });
    let mut world = World::new(desktop.clone(), 101);
    let drift = world.drifts.get_mut(&7).unwrap();
    drift.mass.fill(5.0 * drift.cell_width / scale);
    world.introduced_mass = world.total_mass();
    let mut settings = quiet();
    settings.snow.drift_size_percent = 100;
    (world, desktop, settings)
}

#[test]
fn a_rising_mound_captures_overlapping_slabs_without_capturing_snow_below_a_roof() {
    let mut world = World::new(scene(), 101);
    let key = 1 | (1 << 63);
    let floor = world.drifts.get_mut(&key).unwrap();
    floor.mass.fill(30.0 * floor.cell_width);
    let before = world.total_mass();
    world.introduced_mass = before;
    let mut body = Flake::new(Point::new(400.0, 580.0), 9.0);
    body.mass = 96.0;
    body.velocity.y = 30.0;
    body.slab = Some(snowflakes_client::snow::SnowSlab {
        heights: vec![4.0; 4],
        cell_width: 6.0,
        center: Point::new(12.0, -2.0),
    });
    assert!(world.release(body));
    let mut settings = quiet();
    settings.snow.drift_size_percent = 100;
    world.step(&settings, None);
    assert!(world.flakes.is_empty());
    assert!((world.drifts[&key].total_mass() - before - 96.0).abs() < 0.01);
    assert_eq!(world.removed_mass, 0.0);
    conserved(&world);
    let (mut world, _, settings) = stocked_window();
    let roof = world.drifts[&7].total_mass();
    let mut body = Flake::new(Point::new(400.0, 220.0), 3.0);
    body.velocity.y = 40.0;
    assert!(world.release(body));
    world.step(&settings, None);
    assert_eq!(world.flakes.len(), 1);
    assert_eq!(world.drifts[&7].total_mass(), roof);
    conserved(&world);
}

#[test]
fn compacted_collapses_preserve_volume_and_center_then_land_as_mounds() {
    for scale in [1.0, 2.0, 4.0] {
        let (mut world, mut desktop, settings) = wide_collapse(scale);
        let before = world.total_mass();
        desktop.windows.clear();
        world.set_scene(desktop);
        assert_eq!(world.pending_collapse_count(), BASE_COLLAPSE_BUDGET);
        assert_eq!(world.removed_mass, 0.0);
        assert!((world.total_mass() - before).abs() < before * 0.00001);
        world.step(&settings, None);
        assert_eq!(world.flakes.len(), BASE_COLLAPSE_BUDGET);
        let mut center_x = 0.0;
        let mut center_y = 0.0;
        let mut mass = 0.0;
        for body in &world.flakes {
            let slab = body.slab.as_ref().unwrap();
            assert!(slab.heights.len() <= 16);
            let area: f32 = slab.heights.iter().sum::<f32>() * slab.cell_width;
            assert!((area - body.mass).abs() < body.mass * 0.0001);
            let old_x = body.position.x - body.velocity.x * scale / 60.0;
            let old_y = body.position.y - body.velocity.y * scale / 60.0;
            center_x += f64::from(old_x) * f64::from(body.mass);
            center_y += f64::from(old_y) * f64::from(body.mass);
            mass += f64::from(body.mass);
        }
        assert!((center_x / mass - f64::from(5600.0 * scale)).abs() < f64::from(0.05 * scale));
        assert!((center_y / mass - f64::from(197.5 * scale)).abs() < f64::from(0.01 * scale));
        let saved = world
            .flakes
            .iter()
            .find(|body| body.skip_support.is_some_and(|key| key >= 1 << 62))
            .unwrap()
            .clone();
        for _ in 0..300 {
            world.step(&settings, None);
            assert!(world.flakes.len() <= MAX_FLAKES);
            assert!(world.pending_collapse_count() <= BASE_COLLAPSE_BUDGET);
        }
        assert!(world.flakes.is_empty());
        assert_eq!(world.pending_collapse_count(), 0);
        assert!(world.drifts[&(1 | (1 << 63))].total_mass() > before * 0.999);
        assert!(world.removed_mass < before * 0.001);
        assert!(
            world
                .falling_clips(&saved, saved.draw_bounds(scale))
                .is_none()
        );
        conserved(&world);
    }
}

#[test]
fn a_paused_full_air_budget_does_not_delay_another_monitors_collapses() {
    let mut desktop = two_screens();
    for key in 1..=240 {
        desktop.windows.push(Window {
            key,
            rect: Rect {
                left: 900.0,
                top: 200.0 + (key % 4) as f32 * 10.0,
                right: 1500.0,
                bottom: 450.0,
            },
            visible: true,
            collect: true,
        });
    }
    let mut world = World::new(desktop.clone(), 101);
    for drift in world.drifts.values_mut().filter(|d| !d.support.ground) {
        drift.mass.fill(0.1 * drift.cell_width);
    }
    let collapsed = world.total_mass();
    world.introduced_mass = collapsed;
    for _ in 0..ORDINARY_FLAKES_PER_SCREEN {
        assert!(world.release(Flake::new(Point::new(50.0, 100.0), 3.0)));
    }
    world.set_paused_monitors([1].into());
    desktop.windows.clear();
    world.set_scene(desktop);
    let before = world.total_mass();
    let mut settings = quiet();
    settings.snow.drift_size_percent = 100;
    world.step(&settings, None);
    assert_eq!(world.pending_collapse_count(), 0);
    assert_eq!(
        world
            .flakes
            .iter()
            .filter(|body| body.slab.is_some())
            .count(),
        BASE_COLLAPSE_BUDGET
    );
    assert!(
        world
            .flakes
            .iter()
            .filter(|body| body.slab.is_some())
            .all(|body| body.age > 1.0)
    );
    assert!((world.total_mass() - before).abs() < before * 0.00001);
    assert_eq!(world.removed_mass, 0.0);
    assert!(
        world
            .flakes
            .iter()
            .filter(|body| body.slab.is_none())
            .all(|body| body.position == Point::new(50.0, 100.0) && body.age == 0.0)
    );
    world.set_paused_monitors(Default::default());
    for _ in 0..300 {
        world.step(&settings, None);
    }
    assert!(world.flakes.iter().all(|body| body.slab.is_none()));
    assert!(world.drifts[&(2 | (1 << 63))].total_mass() > collapsed * 0.999);
    conserved(&world);
}

#[test]
fn compacted_visible_and_invisible_sources_keep_separate_layers() {
    let mut desktop = scene();
    for key in 1..=240 {
        desktop.windows.push(Window {
            key,
            rect: Rect {
                left: 150.0,
                top: 200.0,
                right: 650.0,
                bottom: 450.0,
            },
            visible: key % 2 == 0,
            collect: true,
        });
    }
    let mut world = World::new(desktop.clone(), 101);
    let mut expected_visible = 0.0;
    let mut expected_hidden = 0.0;
    for (&key, drift) in world.drifts.iter_mut().filter(|(_, d)| !d.support.ground) {
        drift.mass.fill(0.25 * drift.cell_width);
        if key % 2 == 0 {
            expected_visible += drift.total_mass();
        } else {
            expected_hidden += drift.total_mass();
        }
    }
    world.introduced_mass = world.total_mass();
    desktop.windows.clear();
    world.set_scene(desktop);
    let mut settings = quiet();
    settings.snow.drift_size_percent = 100;
    world.step(&settings, None);
    let mut visible = 0.0;
    let mut hidden = 0.0;
    for body in &world.flakes {
        if world
            .falling_clips(body, body.draw_bounds(1.0))
            .unwrap()
            .is_empty()
        {
            hidden += f64::from(body.mass);
        } else {
            visible += f64::from(body.mass);
        }
    }
    assert!((visible - expected_visible).abs() < expected_visible * 0.0001);
    assert!((hidden - expected_hidden).abs() < expected_hidden * 0.0001);
    assert_eq!(world.removed_mass, 0.0);
    for _ in 0..300 {
        world.step(&settings, None);
    }
    assert!(world.flakes.is_empty());
    assert!(
        world.drifts[&(1 | (1 << 63))].total_mass() > expected_visible * 0.999,
        "visible={expected_visible}, floor={}, hidden={expected_hidden}, removed={}",
        world.drifts[&(1 | (1 << 63))].total_mass(),
        world.removed_mass
    );
    assert!((world.removed_mass - expected_hidden).abs() < expected_hidden * 0.001);
    conserved(&world);
}

#[test]
fn pending_collapse_budget_supports_monitor_visibility_classes_and_hot_unplug() {
    let mut desktop = Scene::default();
    for screen in 0..65 {
        let left = screen as f32 * 256.0;
        desktop.screens.push(Screen {
            key: screen + 1,
            persistent_id: true,
            rect: Rect {
                left,
                top: 0.0,
                right: left + 256.0,
                bottom: 600.0,
            },
            floor: 600.0,
            scale: 1.0,
        });
        for hidden in [false, true] {
            desktop.windows.push(Window {
                key: 1000 + screen * 2 + u64::from(hidden),
                rect: Rect {
                    left: left + 60.0,
                    top: 200.0,
                    right: left + 180.0,
                    bottom: 450.0,
                },
                visible: !hidden,
                collect: true,
            });
        }
    }
    let mut world = World::new(desktop.clone(), 101);
    for drift in world.drifts.values_mut().filter(|d| !d.support.ground) {
        drift.mass.fill(drift.cell_width);
    }
    world.introduced_mass = world.total_mass();
    let before = world.total_mass();
    world.set_paused_monitors(desktop.screens.iter().map(|s| s.key).collect());
    desktop.windows.clear();
    world.set_scene(desktop.clone());
    assert_eq!(world.pending_collapse_count(), 130);
    world.step(&quiet(), None);
    assert!(world.flakes.is_empty());
    desktop.screens.truncate(1);
    world.set_scene(desktop);
    assert_eq!(world.pending_collapse_count(), BASE_COLLAPSE_BUDGET);
    assert!((world.total_mass() - before).abs() < before * 0.0001);
    assert_eq!(world.removed_mass, 0.0);
    world.clear();
    assert_eq!(world.total_mass(), 0.0);
    assert_eq!(world.pending_collapse_count(), 0);
    assert!((world.removed_mass - before).abs() < before * 0.0001);
    conserved(&world);
}

#[test]
fn compacted_collapses_match_at_thirty_and_sixty_fps_through_pause_and_refresh() {
    fn run(fps: u64) -> World {
        let (mut world, mut desktop, settings) = wide_collapse(1.0);
        desktop.windows.clear();
        world.set_paused_monitors([1].into());
        world.set_scene(desktop.clone());
        let start = Instant::now();
        let mut clock = StepClock::default();
        clock.advance(start, true);
        let mut ticks = 0;
        for frame in 1..=fps * 3 / 2 {
            for _ in 0..clock
                .advance(
                    start + Duration::from_nanos(frame * 1_000_000_000 / fps),
                    true,
                )
                .count
            {
                if ticks == 60 {
                    world.set_paused_monitors(Default::default());
                }
                if ticks % 10 == 0 {
                    world.set_scene(desktop.clone());
                }
                world.step(&settings, None);
                ticks += 1;
            }
        }
        world
    }
    let a = run(30);
    let b = run(60);
    assert!(!a.flakes.is_empty());
    assert_eq!(a.flakes.len(), b.flakes.len());
    for (a, b) in a.flakes.iter().zip(&b.flakes) {
        assert_eq!(a.position, b.position);
        assert_eq!(a.velocity, b.velocity);
        assert_eq!(
            a.slab.as_ref().unwrap().outline(),
            b.slab.as_ref().unwrap().outline()
        );
        assert_eq!(a.skip_support, b.skip_support);
    }
    assert_eq!(a.total_mass(), b.total_mass());
    assert_eq!(a.removed_mass, 0.0);
    assert_eq!(b.removed_mass, 0.0);
    conserved(&a);
    conserved(&b);
}

fn narrow_drift_gap(scale: f32, window: bool) -> (World, Scene, Settings, u64, usize) {
    let mut desktop = scene();
    desktop.screens[0].rect.right *= scale;
    desktop.screens[0].rect.bottom *= scale;
    desktop.screens[0].floor *= scale;
    desktop.screens[0].scale = scale;
    if window {
        desktop.windows.push(Window {
            key: 7,
            rect: Rect {
                left: 150.0 * scale,
                top: 200.0 * scale,
                right: 650.0 * scale,
                bottom: 450.0 * scale,
            },
            visible: true,
            collect: true,
        });
    }
    let key = if window { 7 } else { 1 | (1 << 63) };
    let mut world = World::new(desktop.clone(), 101);
    let drift = world.drifts.get_mut(&key).unwrap();
    let middle = drift.mass.len() / 2;
    let unit = drift.cell_width / scale;
    drift.mass[middle - 1] = 20.0 * unit;
    drift.mass[middle + 1] = 20.0 * unit;
    drift.mass[middle] = 0.1 * unit;
    drift.pending[middle] = 0.1 * unit;
    desktop.windows.insert(
        0,
        overlay(
            90,
            Rect {
                left: drift.x(middle) - drift.cell_width * 0.49,
                right: drift.x(middle) + drift.cell_width * 0.49,
                top: drift.support.y - 25.0 * scale,
                bottom: drift.support.y - 0.5 * scale,
            },
        ),
    );
    world.set_scene(desktop.clone());
    world.introduced_mass = world.total_mass();
    let mut settings = quiet();
    settings.snow.drift_size_percent = 100;
    (world, desktop, settings, key, middle)
}

#[test]
fn relaxation_shares_a_receivers_free_height_between_both_neighbours() {
    for scale in [1.0, 2.0, 4.0] {
        for window in [false, true] {
            let (mut world, _, settings, key, middle) = narrow_drift_gap(scale, window);
            let before = world.total_mass();
            world.step(&settings, None);
            let drift = &world.drifts[&key];
            let height =
                (drift.mass[middle] + drift.pending[middle]) * scale * scale / drift.cell_width;
            assert!(
                height <= 0.5001 * scale,
                "two inflows overfilled the gap: {height}"
            );
            assert!(height >= 0.499 * scale, "a free gap must still accept snow");
            assert!((drift.mass[middle - 1] - drift.mass[middle + 1]).abs() < 0.001);
            assert!((world.total_mass() - before).abs() < 0.01);
            assert_eq!(world.removed_mass, 0.0);
            assert!(drift.mass.iter().all(|m| m.is_finite() && *m >= 0.0));
            conserved(&world);
        }
    }
}

#[test]
fn relaxation_reserves_pending_mass_and_user_limits_before_accepting_flux() {
    for scale in [1.0, 2.0, 4.0] {
        for window in [false, true] {
            for percent in [25, 50, 100] {
                let (mut world, mut desktop, mut settings, key, middle) =
                    narrow_drift_gap(scale, window);
                desktop.windows.retain(|w| w.key != 90);
                world.set_scene(desktop);
                settings.snow.drift_size_percent = percent;
                let limit = (if window { 32.0 } else { 120.0 }) * percent as f32 / 100.0;
                let drift = world.drifts.get_mut(&key).unwrap();
                drift.mass.fill(0.0);
                drift.pending.fill(0.0);
                let unit = drift.cell_width / scale;
                drift.mass[middle - 1] = limit * unit;
                drift.mass[middle + 1] = limit * unit;
                drift.mass[middle] = limit * unit * 0.04;
                drift.pending[middle] = limit * unit * 0.95;
                world.introduced_mass = world.total_mass();
                world.step(&settings, None);
                let drift = &world.drifts[&key];
                let depth = (drift.mass[middle] + drift.pending[middle]) / unit;
                assert!(depth <= limit * 1.00001);
                assert!(depth >= limit * 0.999);
                assert!((drift.mass[middle - 1] - drift.mass[middle + 1]).abs() < 0.001);
                assert_eq!(world.removed_mass, 0.0);
                conserved(&world);
            }
        }
    }
}

#[test]
fn relaxation_keeps_blocked_mass_and_uses_new_room_only_after_uncovering() {
    for window in [false, true] {
        let (mut world, mut desktop, settings, key, middle) = narrow_drift_gap(1.0, window);
        let before = world.total_mass();
        for _ in 0..60 {
            world.step(&settings, None);
            let drift = &world.drifts[&key];
            assert!((drift.mass[middle] + drift.pending[middle]) / drift.cell_width <= 0.5001);
            assert_eq!(world.removed_mass, 0.0);
        }
        let drift = &world.drifts[&key];
        let retained = drift.mass[middle] + drift.pending[middle];
        desktop.windows[0].rect.left += 1000.0;
        desktop.windows[0].rect.right += 1000.0;
        world.set_scene(desktop);
        let drift = &world.drifts[&key];
        assert_eq!(drift.mass[middle] + drift.pending[middle], retained);
        world.step(&settings, None);
        let drift = &world.drifts[&key];
        assert!(drift.mass[middle] + drift.pending[middle] > retained);
        assert!((world.total_mass() - before).abs() < 0.01);
        assert_eq!(world.removed_mass, 0.0);
        conserved(&world);
    }
}

#[test]
fn relaxation_at_a_mixed_dpi_headroom_boundary_respects_both_monitor_pauses() {
    let mut desktop = two_screens();
    desktop.screens[1].rect.top = 198.5;
    desktop.screens[1].scale = 2.0;
    desktop.windows.push(Window {
        key: 7,
        rect: Rect {
            left: 100.0,
            top: 200.0,
            right: 1500.0,
            bottom: 450.0,
        },
        visible: true,
        collect: true,
    });
    let mut world = World::new(desktop, 101);
    let drift = world.drifts.get_mut(&7).unwrap();
    let receiver = (0..drift.mass.len())
        .find(|&i| drift.x(i) >= 800.0)
        .unwrap();
    drift.mass[receiver - 1] = 20.0 * drift.cell_width;
    world.introduced_mass = world.total_mass();
    let mut settings = quiet();
    settings.snow.drift_size_percent = 100;
    world.set_paused_monitors([2].into());
    for _ in 0..30 {
        world.step(&settings, None);
    }
    assert_eq!(world.drifts[&7].mass[receiver], 0.0);
    let frozen = world.drifts[&7].mass[..receiver].to_vec();
    world.set_paused_monitors([1].into());
    for _ in 0..30 {
        world.step(&settings, None);
    }
    assert_eq!(&world.drifts[&7].mass[..receiver], frozen);
    assert_eq!(world.drifts[&7].mass[receiver], 0.0);
    world.set_paused_monitors(Default::default());
    let mut received = false;
    for _ in 0..120 {
        world.step(&settings, None);
        let drift = &world.drifts[&7];
        let height = (drift.mass[receiver] + drift.pending[receiver]) * 4.0 / drift.cell_width;
        assert!(height <= 1.5001);
        received |= height > 0.0;
    }
    assert!(received);
    assert_eq!(world.removed_mass, 0.0);
    conserved(&world);
}

#[test]
fn mixed_dpi_relaxation_is_unchanged_when_the_desktop_is_reflected() {
    fn run(reflected: bool) -> World {
        let mut desktop = two_screens();
        desktop.screens[usize::from(!reflected)].scale = 2.0;
        desktop.windows.push(Window {
            key: 7,
            rect: Rect {
                left: 200.0,
                top: 200.0,
                right: 1400.0,
                bottom: 450.0,
            },
            visible: true,
            collect: true,
        });
        let mut world = World::new(desktop, 101);
        let drift = world.drifts.get_mut(&7).unwrap();
        let donor_x = if reflected { 803.0 } else { 797.0 };
        let donor = (0..drift.mass.len())
            .find(|&i| (drift.x(i) - donor_x).abs() < 0.01)
            .unwrap();
        drift.mass[donor] = 20.0 * drift.cell_width;
        world.introduced_mass = world.total_mass();
        let mut settings = quiet();
        settings.snow.drift_size_percent = 100;
        for _ in 0..10 {
            world.step(&settings, None);
        }
        world
    }
    let a = run(false);
    let b = run(true);
    let a_drift = &a.drifts[&7];
    let b_drift = &b.drifts[&7];
    for (i, &mass) in a_drift
        .mass
        .iter()
        .enumerate()
        .filter(|(_, mass)| **mass > 0.0)
    {
        let reflected_x = 1600.0 - a_drift.x(i);
        let j = (0..b_drift.mass.len())
            .find(|&j| (b_drift.x(j) - reflected_x).abs() < 0.01)
            .unwrap();
        assert!((mass - b_drift.mass[j]).abs() < 0.001);
    }
    assert!((a.total_mass() - b.total_mass()).abs() < 0.001);
    assert_eq!(a.removed_mass, 0.0);
    assert_eq!(b.removed_mass, 0.0);
    conserved(&a);
    conserved(&b);
}

#[test]
fn limited_relaxation_matches_at_thirty_and_sixty_fps_with_scene_refreshes() {
    fn run(scale: f32, window: bool, fps: u64) -> World {
        let (mut world, mut desktop, settings, key, middle) = narrow_drift_gap(scale, window);
        let start = Instant::now();
        let mut clock = StepClock::default();
        clock.advance(start, true);
        let mut ticks = 0;
        for frame in 1..=fps * 4 {
            for _ in 0..clock
                .advance(
                    start + Duration::from_nanos(frame * 1_000_000_000 / fps),
                    true,
                )
                .count
            {
                if ticks == 180 {
                    desktop.windows.retain(|w| w.key != 90);
                }
                if ticks % 10 == 0 {
                    world.set_scene(desktop.clone());
                }
                world.step(&settings, None);
                let drift = &world.drifts[&key];
                if ticks < 180 {
                    let height = (drift.mass[middle] + drift.pending[middle]) * scale * scale
                        / drift.cell_width;
                    assert!(height <= 0.5001 * scale);
                }
                assert_eq!(world.removed_mass, 0.0);
                ticks += 1;
            }
        }
        world
    }
    for scale in [1.0, 2.0, 4.0] {
        for window in [false, true] {
            let a = run(scale, window, 30);
            let b = run(scale, window, 60);
            for (key, drift) in &a.drifts {
                assert_eq!(drift.mass, b.drifts[key].mass);
                assert_eq!(drift.pending, b.drifts[key].pending);
            }
            assert_eq!(a.total_mass(), b.total_mass());
            conserved(&a);
            conserved(&b);
        }
    }
}

fn mask_area(rect: Rect) -> f32 {
    rect.width() * rect.height()
}
fn overlap_area(a: Rect, b: Rect) -> f32 {
    (a.right.min(b.right) - a.left.max(b.left)).max(0.0)
        * (a.bottom.min(b.bottom) - a.top.max(b.top)).max(0.0)
}
fn assert_falling_mask(world: &World, body: &Flake, query: Rect, cover: Rect) {
    let clips = world.falling_clips(body, query).unwrap_or(vec![query]);
    for (i, &clip) in clips.iter().enumerate() {
        assert_eq!(overlap_area(clip, cover), 0.0);
        assert!((overlap_area(clip, query) - mask_area(clip)).abs() < 0.01);
        for &other in &clips[..i] {
            assert_eq!(overlap_area(clip, other), 0.0);
        }
    }
    let area: f32 = clips.iter().copied().map(mask_area).sum();
    let expected = mask_area(query) - overlap_area(query, cover);
    assert!((area - expected).abs() < expected.max(1.0) * 0.001);
}

#[test]
fn closing_covered_snow_falls_behind_the_cover_and_still_forms_ground_mounds() {
    let (mut world, mut desktop, settings) = stocked_window();
    let cover = Rect {
        left: 100.0,
        top: 100.0,
        right: 700.0,
        bottom: 400.0,
    };
    desktop.windows.insert(
        0,
        Window {
            key: 90,
            rect: cover,
            visible: true,
            collect: true,
        },
    );
    world.set_scene(desktop.clone());
    let before = world.total_mass();
    desktop.windows.retain(|w| w.key != 7);
    world.set_scene(desktop);
    world.step(&settings, None);
    assert!(!world.flakes.is_empty());
    assert!((world.total_mass() - before).abs() < 0.01);
    assert!(world.flakes.iter().all(|body| {
        body.slab.is_some()
            && world
                .falling_clips(body, body.draw_bounds(1.0))
                .unwrap()
                .is_empty()
    }));
    let mut emerged = false;
    for _ in 0..480 {
        world.step(&settings, None);
        for body in &world.flakes {
            let query = body.draw_bounds(1.0);
            assert_falling_mask(&world, body, query, cover);
            emerged |= !world.falling_clips(body, query).unwrap().is_empty();
        }
    }
    assert!(emerged, "falling material must emerge below the cover");
    assert!(world.flakes.is_empty());
    assert!(world.drifts[&(1 | (1 << 63))].total_mass() > before * 0.99);
    assert_eq!(world.drifts[&90].total_mass(), 0.0);
    conserved(&world);
}

#[test]
fn falling_masks_clip_narrow_covers_and_include_the_rotating_contour() {
    let (mut world, mut desktop, settings) = stocked_window();
    let cover = Rect {
        left: 180.0,
        top: 195.0,
        right: 180.5,
        bottom: 197.0,
    };
    desktop.windows.insert(0, overlay(90, cover));
    world.set_scene(desktop.clone());
    desktop.windows.retain(|w| w.key != 7);
    world.set_scene(desktop);
    world.step(&settings, None);
    let mut body = world
        .flakes
        .iter()
        .find(|body| overlap_area(body.draw_bounds(1.0), cover) > 0.0)
        .unwrap()
        .clone();
    let query = body.draw_bounds(1.0);
    assert!(overlap_area(query, cover) < mask_area(query));
    assert_falling_mask(&world, &body, query, cover);
    for scale in [1.0, 2.0, 4.0] {
        body.angle = 1.1;
        let bounds = body.draw_bounds(scale);
        for p in body.slab.as_ref().unwrap().outline() {
            let p = body.transformed_point(p, scale);
            assert!(p.x >= bounds.left + 1.9 && p.x <= bounds.right - 1.9);
            assert!(p.y >= bounds.top + 1.9 && p.y <= bounds.bottom - 1.9);
        }
        assert_falling_mask(&world, &body, bounds, cover);
    }
    conserved(&world);
}

#[test]
fn falling_masks_follow_live_cover_geometry_without_inheriting_replacement_ids() {
    let (mut world, mut desktop, settings) = stocked_window();
    let full = Rect {
        left: 100.0,
        top: 100.0,
        right: 700.0,
        bottom: 400.0,
    };
    desktop.windows.insert(
        0,
        Window {
            key: 90,
            rect: full,
            visible: true,
            collect: true,
        },
    );
    desktop.windows.push(Window {
        key: 80,
        rect: full,
        visible: true,
        collect: true,
    });
    world.set_scene(desktop.clone());
    desktop.windows.retain(|w| w.key != 7);
    world.set_scene(desktop.clone());
    world.step(&settings, None);
    let body = world.flakes[0].clone();
    let query = body.draw_bounds(1.0);
    assert!(world.falling_clips(&body, query).unwrap().is_empty());
    desktop.windows[0].rect.left += 800.0;
    desktop.windows[0].rect.right += 800.0;
    world.set_scene(desktop.clone());
    assert_eq!(world.falling_clips(&body, query).unwrap(), vec![query]);
    let half = Rect {
        right: (query.left + query.right) * 0.5,
        ..query
    };
    desktop.windows[0].rect = half;
    world.set_scene(desktop.clone());
    assert_falling_mask(&world, &body, query, half);
    desktop.windows[0].visible = false;
    world.set_scene(desktop.clone());
    assert_eq!(world.falling_clips(&body, query).unwrap(), vec![query]);
    desktop.windows[0].visible = true;
    desktop.windows[0].key = 91;
    world.set_scene(desktop.clone());
    assert_eq!(world.falling_clips(&body, query).unwrap(), vec![query]);
    desktop.windows.insert(0, overlay(99, half));
    world.set_scene(desktop.clone());
    assert_falling_mask(&world, &body, query, half);
    desktop.windows.clear();
    world.set_scene(desktop);
    assert_eq!(world.falling_clips(&body, query).unwrap(), vec![query]);
    conserved(&world);
}

#[test]
fn hidden_falling_snow_ignores_cursor_and_passes_behind_front_window_roofs() {
    fn run(cursor: bool) -> World {
        let (mut world, mut desktop, settings) = stocked_window();
        desktop.windows.insert(
            0,
            Window {
                key: 90,
                rect: Rect {
                    left: 100.0,
                    top: 300.0,
                    right: 700.0,
                    bottom: 550.0,
                },
                visible: true,
                collect: true,
            },
        );
        world.set_scene(desktop.clone());
        desktop.windows.retain(|w| w.key != 7);
        world.set_scene(desktop);
        world.step(&settings, None);
        // Move every body fully behind the front window, then sweep there.
        for body in &mut world.flakes {
            body.position.y = 340.0;
        }
        for _ in 0..30 {
            let cursor = cursor.then_some(snowflakes_client::snow::Cursor {
                position: Point::new(world.flakes[0].position.x + 5.0, 340.0),
                velocity: Point::new(120.0, -100.0),
            });
            world.step(&settings, cursor);
        }
        world
    }
    let mut a = run(false);
    let b = run(true);
    assert_eq!(a.flakes.len(), b.flakes.len());
    for (a, b) in a.flakes.iter().zip(&b.flakes) {
        assert_eq!(a.position, b.position);
        assert_eq!(a.velocity, b.velocity);
    }
    // Crossing the roof from above must not put a behind-window slab on it.
    for body in &mut a.flakes {
        body.position.y = 280.0;
        body.angle = 0.0;
        body.spin = 0.0;
        body.velocity = Point::new(0.0, 100.0);
    }
    for _ in 0..300 {
        a.step(&quiet(), None);
    }
    assert_eq!(a.drifts[&90].total_mass(), 0.0);
    assert!(a.drifts[&(1 | (1 << 63))].total_mass() > 0.0);
    conserved(&a);
    conserved(&b);
}

#[test]
fn queued_slabs_use_current_masks_after_monitor_pause() {
    let (mut world, mut desktop, settings) = stocked_window();
    desktop.screens = two_screens().screens;
    let cover = Rect {
        left: 100.0,
        top: 100.0,
        right: 700.0,
        bottom: 400.0,
    };
    desktop.windows.insert(0, overlay(90, cover));
    world.set_scene(desktop.clone());
    world.set_paused_monitors([1].into());
    desktop.windows.retain(|w| w.key != 7);
    world.set_scene(desktop.clone());
    let before = world.total_mass();
    for _ in 0..120 {
        world.step(&settings, None);
    }
    assert!(world.flakes.is_empty());
    assert!((world.total_mass() - before).abs() < 0.01);
    desktop.windows[0].rect.right = 400.0;
    world.set_scene(desktop.clone());
    world.set_paused_monitors(Default::default());
    world.step(&settings, None);
    assert!(!world.flakes.is_empty());
    for body in &world.flakes {
        assert_falling_mask(&world, body, body.draw_bounds(1.0), desktop.windows[0].rect);
    }
    conserved(&world);
}

#[test]
fn falling_masks_and_physics_match_at_thirty_and_sixty_fps_and_scaled_desktops() {
    fn run(scale: f32, fps: u64) -> World {
        let (mut world, mut desktop, settings) = edge_window(scale, 5.0);
        desktop.windows.insert(
            0,
            overlay(
                90,
                Rect {
                    left: 100.0 * scale,
                    top: 150.0 * scale,
                    right: 700.0 * scale,
                    bottom: 350.0 * scale,
                },
            ),
        );
        world.set_scene(desktop.clone());
        desktop.windows.retain(|w| w.key != 7);
        world.set_scene(desktop.clone());
        let start = Instant::now();
        let mut clock = StepClock::default();
        clock.advance(start, true);
        let mut ticks = 0;
        for frame in 1..=fps * 3 / 2 {
            for _ in 0..clock
                .advance(
                    start + Duration::from_nanos(frame * 1_000_000_000 / fps),
                    true,
                )
                .count
            {
                if ticks % 10 == 0 {
                    world.set_scene(desktop.clone());
                }
                world.step(&settings, None);
                ticks += 1;
            }
        }
        world
    }
    for scale in [1.0, 2.0, 4.0] {
        let a = run(scale, 30);
        let b = run(scale, 60);
        assert!(!a.flakes.is_empty());
        assert_eq!(a.flakes.len(), b.flakes.len());
        for (a_body, b_body) in a.flakes.iter().zip(&b.flakes) {
            assert_eq!(a_body.position, b_body.position);
            assert_eq!(a_body.velocity, b_body.velocity);
            assert_eq!(a_body.angle, b_body.angle);
            assert_eq!(
                a.falling_clips(a_body, a_body.draw_bounds(scale)),
                b.falling_clips(b_body, b_body.draw_bounds(scale))
            );
        }
        assert_eq!(a.total_mass(), b.total_mass());
        conserved(&a);
        conserved(&b);
    }
}

#[test]
fn falling_masks_retire_after_landing_and_clear() {
    let (mut world, mut desktop, settings) = stocked_window();
    desktop.windows.insert(
        0,
        overlay(
            90,
            Rect {
                left: 100.0,
                top: 100.0,
                right: 700.0,
                bottom: 400.0,
            },
        ),
    );
    world.set_scene(desktop.clone());
    desktop.windows.retain(|w| w.key != 7);
    world.set_scene(desktop);
    world.step(&settings, None);
    let body = world.flakes[0].clone();
    let query = body.draw_bounds(1.0);
    assert!(world.falling_clips(&body, query).is_some());
    for _ in 0..480 {
        world.step(&settings, None);
    }
    assert!(world.flakes.is_empty());
    assert!(world.falling_clips(&body, query).is_none());
    conserved(&world);
    let (mut world, mut desktop, settings) = stocked_window();
    desktop.windows.clear();
    world.set_scene(desktop);
    world.step(&settings, None);
    let body = world.flakes[0].clone();
    assert!(world.falling_clips(&body, query).is_some());
    world.clear();
    assert!(world.falling_clips(&body, query).is_none());
    assert_eq!(world.total_mass(), 0.0);
    conserved(&world);
}

#[test]
fn closing_an_invisible_source_does_not_move_its_snow_to_the_active_desktop() {
    let (mut world, mut desktop, settings) = stocked_window();
    desktop.screens = two_screens().screens;
    desktop.windows[0].visible = false;
    world.set_scene(desktop.clone());
    world.set_paused_monitors([1].into());
    let before = world.total_mass();
    desktop.windows.clear();
    world.set_scene(desktop);
    for _ in 0..120 {
        world.step(&settings, None);
    }
    assert!(world.flakes.is_empty());
    assert!((world.total_mass() - before).abs() < 0.01);
    world.set_paused_monitors(Default::default());
    world.step(&settings, None);
    assert!(!world.flakes.is_empty());
    for _ in 0..480 {
        for body in &world.flakes {
            assert!(
                world
                    .falling_clips(body, body.draw_bounds(1.0))
                    .unwrap()
                    .is_empty()
            );
        }
        world.step(&settings, None);
    }
    assert!(world.flakes.is_empty());
    assert_eq!(world.total_mass(), 0.0);
    assert!((world.removed_mass - before).abs() < 0.01);
    conserved(&world);
}

#[test]
fn pathological_falling_masks_hide_material_without_changing_its_mass() {
    let (mut world, mut desktop, settings) = stocked_window();
    for i in 0..128 {
        desktop.windows.insert(
            0,
            Window {
                key: 100 + i,
                rect: Rect {
                    left: 160.0 + i as f32 * 0.01,
                    right: 190.0,
                    top: 195.0 + i as f32 * 0.01,
                    bottom: 195.004 + i as f32 * 0.01,
                },
                visible: true,
                collect: true,
            },
        );
    }
    world.set_scene(desktop.clone());
    let before = world.total_mass();
    desktop.windows.retain(|w| w.key != 7);
    world.set_scene(desktop);
    world.step(&settings, None);
    let body = &world.flakes[0];
    assert!(
        world
            .falling_clips(body, body.draw_bounds(1.0))
            .unwrap()
            .is_empty()
    );
    assert!((world.total_mass() - before).abs() < 0.01);
    conserved(&world);
}

#[test]
fn saturated_thin_collapses_preserve_cohesion_and_window_depths() {
    let mut desktop = scene();
    desktop.screens[0].rect.right = 24_000.0;
    desktop.screens[0].rect.bottom = 2000.0;
    desktop.screens[0].floor = 2000.0;
    let cover = Rect {
        left: 0.0,
        top: 100.0,
        right: 12_000.0,
        bottom: 300.0,
    };
    desktop.windows.push(Window {
        key: 90,
        rect: cover,
        visible: true,
        collect: true,
    });
    for (key, left) in [(7, 100.0), (8, 12_100.0)] {
        desktop.windows.push(Window {
            key,
            rect: Rect {
                left,
                top: 200.0,
                right: left + 11_000.0,
                bottom: 450.0,
            },
            visible: true,
            collect: true,
        });
    }
    let mut world = World::new(desktop.clone(), 101);
    for key in [7, 8] {
        let drift = world.drifts.get_mut(&key).unwrap();
        drift.mass.fill(0.1 * drift.cell_width);
    }
    world.introduced_mass = world.total_mass();
    let before = world.total_mass();
    desktop.windows.retain(|w| w.key == 90);
    world.set_scene(desktop);
    world.step(&quiet(), None);
    assert_eq!(world.flakes.len(), 128);
    assert!(world.flakes.iter().all(|body| body.slab.is_some()));
    for body in &world.flakes {
        assert_falling_mask(&world, body, body.draw_bounds(1.0), cover);
    }
    assert_eq!(world.removed_mass, 0.0);
    assert!((world.total_mass() - before).abs() < 0.1);
    assert!(
        world
            .flakes
            .iter()
            .any(|body| body.position.x > cover.right)
    );
    assert!(
        world
            .flakes
            .iter()
            .any(|body| body.position.x < cover.right)
    );
    conserved(&world);
}

#[test]
fn collision_columns_cover_negative_wide_desktops_and_moved_supports() {
    let mut desktop = scene();
    desktop.screens[0].rect.left = -40_000.0;
    desktop.screens[0].rect.right = 40_000.0;
    let positions = [-39_900.0, -312.5, 0.0, 312.5, 39_900.0];
    for (i, &x) in positions.iter().enumerate() {
        desktop.windows.push(Window {
            key: 21 + i as u64,
            visible: true,
            collect: true,
            rect: Rect {
                left: x - 80.0,
                top: 200.0,
                right: x + 80.0,
                bottom: 450.0,
            },
        });
    }
    let mut world = World::new(desktop.clone(), 101);
    let mut s = quiet();
    s.snow.drift_size_percent = 100;
    for x in positions {
        let mut body = Flake::new(Point::new(x, 195.0), 3.0);
        body.velocity.y = 1000.0;
        assert!(world.release(body));
    }
    world.step(&s, None);
    for key in 21..26 {
        assert!(world.drifts[&key].total_mass() > 0.0);
    }
    let before = world.drifts[&23].total_mass();
    desktop.windows[2].rect.left += 1000.0;
    desktop.windows[2].rect.right += 1000.0;
    world.set_scene(desktop);
    let mut body = Flake::new(Point::new(1000.0, 195.0), 3.0);
    body.velocity.y = 1000.0;
    world.release(body);
    world.step(&s, None);
    assert!(world.drifts[&23].total_mass() > before);
    conserved(&world);
}
#[test]
fn stationary_window_edges_release_cohesive_snow_that_lands_as_mounds() {
    let (mut world, _, s) = edge_window(1.0, 20.0);
    let before = world.drifts[&7].total_mass();
    for _ in 0..60 {
        world.step(&s, None);
    }
    assert!(world.drifts[&7].total_mass() < before);
    assert!(!world.flakes.is_empty());
    assert!(
        world
            .flakes
            .iter()
            .all(|f| f.slab.is_some() && f.skip_support == Some(7))
    );
    for _ in 0..420 {
        world.step(&s, None);
    }
    assert!(world.drifts[&(1 | (1 << 63))].total_mass() > 0.0);
    assert!(world.flakes.iter().all(|f| f.slab.is_some()));
    conserved(&world);
    let (mut thin, _, s) = edge_window(1.0, 5.0);
    let before = thin.total_mass();
    for _ in 0..180 {
        thin.step(&s, None);
    }
    assert_eq!(thin.total_mass(), before);
    assert!(thin.flakes.is_empty());
}

fn edge_window(scale: f32, height: f32) -> (World, Scene, Settings) {
    let mut desktop = scene();
    let screen = &mut desktop.screens[0];
    screen.rect.right *= scale;
    screen.rect.bottom *= scale;
    screen.floor *= scale;
    screen.scale = scale;
    desktop.windows.push(Window {
        key: 7,
        visible: true,
        collect: true,
        rect: Rect {
            left: 150.0 * scale,
            top: 200.0 * scale,
            right: 650.0 * scale,
            bottom: 450.0 * scale,
        },
    });
    let mut world = World::new(desktop.clone(), 101);
    let drift = world.drifts.get_mut(&7).unwrap();
    drift.mass.fill(height * drift.cell_width / scale);
    world.introduced_mass = world.total_mass();
    let mut s = quiet();
    s.snow.drift_size_percent = 100;
    (world, desktop, s)
}

#[test]
fn narrow_occluders_stop_edge_shedding_even_when_the_corners_are_visible() {
    let (mut world, mut desktop, s) = edge_window(1.0, 20.0);
    let support = &world.drifts[&7].support;
    let left = support.left;
    let right = support.right;
    for x in [left + 7.0, right - 7.5] {
        desktop.windows.insert(
            0,
            overlay(
                20 + x as u64,
                Rect {
                    left: x,
                    top: 170.0,
                    right: x + 0.5,
                    bottom: 190.0,
                },
            ),
        );
    }
    world.set_scene(desktop);
    let support = &world.drifts[&7].support;
    let query = Rect {
        left,
        top: 180.0,
        right: left + 17.0,
        bottom: 200.0,
    };
    assert!(
        support.exposed(Point::new(query.left, query.top))
            && support.exposed(Point::new(query.right, query.bottom))
    );
    assert!(!support.exposes_rect(query));
    let mut strips = support.clone();
    strips.draw_clips = vec![
        Rect {
            left,
            top: 180.0,
            right: left + 8.0,
            bottom: 200.0,
        },
        Rect {
            left: left + 8.0,
            top: 180.0,
            right: left + 17.0,
            bottom: 200.0,
        },
    ];
    assert!(strips.exposes_rect(query));
    let before = world.total_mass();
    for _ in 0..180 {
        world.step(&s, None);
    }
    assert!(world.flakes.is_empty());
    assert!((world.total_mass() - before).abs() < 0.001);
    conserved(&world);
}

#[test]
fn edge_shedding_respects_source_and_destination_monitor_pauses() {
    let mut desktop = two_screens();
    desktop.windows.push(Window {
        key: 7,
        visible: true,
        collect: true,
        rect: Rect {
            left: 100.0,
            top: 200.0,
            right: 1500.0,
            bottom: 450.0,
        },
    });
    let mut world = World::new(desktop, 101);
    let drift = world.drifts.get_mut(&7).unwrap();
    drift.mass.fill(drift.cell_width * 20.0);
    let frozen = drift.mass.clone();
    world.introduced_mass = world.total_mass();
    let mut s = quiet();
    s.snow.drift_size_percent = 100;
    world.set_paused_monitors([1].into());
    for _ in 0..120 {
        world.step(&s, None);
    }
    let drift = &world.drifts[&7];
    for (i, &mass) in frozen.iter().enumerate() {
        if drift.x(i) < 800.0 {
            assert_eq!(drift.mass[i], mass);
        }
    }
    assert!(!world.flakes.is_empty());
    assert!(world.flakes.iter().all(|f| f.position.x > 800.0));
    conserved(&world);
    let mut desktop = two_screens();
    desktop.windows.push(Window {
        key: 7,
        visible: true,
        collect: true,
        rect: Rect {
            left: 300.0,
            top: 200.0,
            right: 800.0,
            bottom: 450.0,
        },
    });
    let mut world = World::new(desktop, 101);
    let drift = world.drifts.get_mut(&7).unwrap();
    drift.mass.fill(drift.cell_width * 20.0);
    let n = drift.mass.len();
    let last = drift.mass[n - 1];
    world.introduced_mass = world.total_mass();
    world.set_paused_monitors([2].into());
    for _ in 0..60 {
        world.step(&s, None);
    }
    assert_eq!(world.drifts[&7].mass[n - 1], last);
    assert!(world.flakes.iter().all(|f| f.velocity.x < 0.0));
    conserved(&world);
}

#[test]
fn edge_timing_and_shapes_match_at_thirty_and_sixty_fps_with_scene_refreshes() {
    fn run(fps: u64) -> World {
        let (mut world, desktop, s) = edge_window(1.0, 20.0);
        let start = Instant::now();
        let mut clock = StepClock::default();
        clock.advance(start, true);
        let mut ticks = 0;
        for frame in 1..=fps * 5 {
            for _ in 0..clock
                .advance(
                    start + Duration::from_nanos(frame * 1_000_000_000 / fps),
                    true,
                )
                .count
            {
                if ticks % 10 == 0 {
                    world.set_scene(desktop.clone());
                }
                world.step(&s, None);
                ticks += 1;
            }
        }
        world
    }
    let a = run(30);
    let b = run(60);
    assert!(!a.flakes.is_empty());
    assert_eq!(a.flakes.len(), b.flakes.len());
    for (a, b) in a.flakes.iter().zip(&b.flakes) {
        assert_eq!(a.position, b.position);
        assert_eq!(a.velocity, b.velocity);
        assert_eq!(
            a.slab.as_ref().unwrap().outline(),
            b.slab.as_ref().unwrap().outline()
        );
    }
    for (key, drift) in &a.drifts {
        assert_eq!(drift.mass, b.drifts[key].mass);
        assert_eq!(drift.pending, b.drifts[key].pending);
    }
    assert_eq!(a.total_mass(), b.total_mass());
    conserved(&a);
    conserved(&b);
}

#[test]
fn full_particle_budget_keeps_edge_material_until_capacity_returns() {
    let (mut world, _, s) = edge_window(1.0, 20.0);
    for _ in 0..ORDINARY_FLAKES_PER_SCREEN {
        assert!(world.release(Flake::new(Point::new(50.0, 100.0), 3.0)));
    }
    let before = world.drifts[&7].total_mass();
    for _ in 0..60 {
        world.step(&s, None);
    }
    assert_eq!(world.drifts[&7].total_mass(), before);
    assert!(world.flakes.iter().all(|f| f.slab.is_none()));
    for f in &mut world.flakes {
        f.position.x = -200.0;
    }
    world.step(&s, None);
    world.step(&s, None);
    assert!(!world.flakes.is_empty());
    assert!(world.flakes.len() <= 2);
    assert!(world.flakes.iter().all(|f| f.slab.is_some()));
    conserved(&world);
}

#[test]
fn many_visible_windows_get_bounded_edge_service_without_starvation() {
    let mut desktop = scene();
    desktop.screens[0].rect.right = 30_000.0;
    desktop.screens[0].rect.bottom = 4000.0;
    desktop.screens[0].floor = 4000.0;
    for key in 1..=256 {
        let x = 100.0 * key as f32;
        desktop.windows.push(Window {
            key,
            visible: true,
            collect: true,
            rect: Rect {
                left: x,
                top: 500.0,
                right: x + 80.0,
                bottom: 650.0,
            },
        });
    }
    let mut world = World::new(desktop, 101);
    for drift in world.drifts.values_mut() {
        if !drift.support.ground {
            drift.mass.fill(drift.cell_width * 20.0);
        }
    }
    world.introduced_mass = world.total_mass();
    let mut s = quiet();
    s.snow.drift_size_percent = 100;
    let mut seen = std::collections::BTreeSet::new();
    let mut previous = 0;
    for _ in 0..240 {
        world.step(&s, None);
        assert!(world.flakes.len().saturating_sub(previous) <= 8);
        previous = world.flakes.len();
        assert!(world.flakes.len() <= MAX_FLAKES);
        for body in &world.flakes {
            assert!(body.slab.is_some());
            if let Some(key) = body.skip_support {
                seen.insert(key);
            }
        }
    }
    assert_eq!(seen.len(), 256);
    conserved(&world);
}

#[test]
fn saturated_spill_does_not_convert_existing_cohesive_bodies_into_flakes() {
    let mut desktop = scene();
    desktop.screens[0].rect.right = 12_000.0;
    desktop.screens[0].rect.bottom = 2000.0;
    desktop.screens[0].floor = 2000.0;
    desktop.windows.push(Window {
        key: 7,
        visible: true,
        collect: true,
        rect: Rect {
            left: 100.0,
            top: 200.0,
            right: 11_100.0,
            bottom: 450.0,
        },
    });
    let mut world = World::new(desktop.clone(), 101);
    let drift = world.drifts.get_mut(&7).unwrap();
    drift.mass.fill(drift.cell_width * 5.0);
    world.introduced_mass = world.total_mass();
    let before = world.total_mass();
    desktop.windows.clear();
    world.set_scene(desktop);
    assert_eq!(
        world.removed_mass, 0.0,
        "a full queue must retain collapsed snow"
    );
    assert!((world.total_mass() - before).abs() < 0.1);
    world.step(&quiet(), None);
    assert_eq!(world.flakes.len(), 128);
    assert!(world.flakes.iter().all(|f| f.slab.is_some()));
    assert_eq!(world.removed_mass, 0.0);
    conserved(&world);
}

#[test]
fn edge_chunks_use_logical_dimensions_at_different_dpi() {
    let mut volumes = Vec::new();
    for scale in [1.0, 2.0, 4.0] {
        let (mut world, _, s) = edge_window(scale, 20.0);
        for _ in 0..60 {
            world.step(&s, None);
        }
        assert_eq!(world.flakes.len(), 2);
        let mut volume = 0.0;
        for body in &world.flakes {
            let slab = body.slab.as_ref().unwrap();
            assert!(
                slab.heights.len() <= 12 && slab.cell_width * slab.heights.len() as f32 <= 18.001
            );
            close_edge(
                body.mass,
                slab.heights.iter().sum::<f32>() * slab.cell_width,
            );
            volume += body.mass;
        }
        volumes.push(volume);
        conserved(&world);
    }
    assert!(
        volumes
            .iter()
            .all(|v| (*v - volumes[0]).abs() < volumes[0] * 0.05)
    );
    fn close_edge(a: f32, b: f32) {
        assert!((a - b).abs() < 0.001 * a.max(1.0));
    }
}

#[test]
fn emission_tracks_logical_width_and_intensity_and_zero_intensity_adds_no_mass() {
    let mut counts = Vec::new();
    for intensity in [25, 50, 100] {
        for scale in [1.0, 2.0] {
            let mut desktop = scene();
            let screen = &mut desktop.screens[0];
            screen.rect.right = 1920.0 * scale;
            screen.rect.bottom = 3000.0 * scale;
            screen.floor = screen.rect.bottom;
            screen.scale = scale;
            let mut world = World::new(desktop, 24);
            let mut s = quiet();
            s.snow.intensity_percent = intensity;
            for _ in 0..240 {
                world.step(&s, None);
            }
            let expected = intensity as f32 * 0.65 * 4.0;
            assert!((world.flakes.len() as f32 - expected).abs() <= 1.01);
            counts.push(world.flakes.len());
            assert!(world.drifts.values().all(|drift| drift.total_mass() == 0.0));
            let introduced = world.introduced_mass;
            s.snow.intensity_percent = 0;
            for _ in 0..120 {
                world.step(&s, None);
            }
            assert_eq!(world.introduced_mass, introduced);
            conserved(&world);
        }
    }
    assert_eq!(counts[0], counts[1]);
    assert_eq!(counts[2], counts[3]);
    assert_eq!(counts[4], counts[5]);
}
#[test]
fn nonzero_limit_reduction_keeps_its_deadline_through_resize_and_scene_refresh() {
    let mut desktop = scene();
    desktop.windows.push(Window {
        key: 7,
        visible: true,
        collect: true,
        rect: Rect {
            left: 100.0,
            top: 300.0,
            right: 500.0,
            bottom: 500.0,
        },
    });
    let mut world = World::new(desktop.clone(), 21);
    let mut s = quiet();
    s.snow.drift_size_percent = 100;
    let drift = world.drifts.get_mut(&7).unwrap();
    drift.mass.fill(drift.cell_width * 32.0);
    world.introduced_mass = world.total_mass();
    world.step(&s, None);
    s.snow.drift_size_percent = 99;
    for frame in 0..150 {
        if frame == 30 {
            desktop.windows[0].rect.right = 480.0;
        }
        if frame % 15 == 0 {
            world.set_scene(desktop.clone());
        }
        world.step(&s, None);
    }
    let drift = &world.drifts[&7];
    assert!((0..drift.mass.len()).all(|i| drift.height(i, &world.scene) <= 32.0 * 0.99 + 0.001));
    conserved(&world);
}
#[test]
fn nonzero_shrink_freezes_per_monitor_and_increasing_the_limit_cancels_it() {
    let mut world = World::new(two_screens(), 22);
    let mut s = quiet();
    s.snow.drift_size_percent = 100;
    for drift in world.drifts.values_mut() {
        drift.mass.fill(drift.cell_width * 120.0);
    }
    world.introduced_mass = world.total_mass();
    world.step(&s, None);
    let left = 1 | (1 << 63);
    let right = 2 | (1 << 63);
    let frozen = world.drifts[&left].mass.clone();
    world.set_paused_monitors([1].into());
    s.snow.drift_size_percent = 99;
    for _ in 0..150 {
        world.step(&s, None);
    }
    assert_eq!(world.drifts[&left].mass, frozen);
    assert!(
        (0..world.drifts[&right].mass.len())
            .all(|i| world.drifts[&right].height(i, &world.scene) <= 118.801)
    );
    world.set_paused_monitors(Default::default());
    for _ in 0..30 {
        world.step(&s, None);
    }
    let before = world.total_mass();
    s.snow.drift_size_percent = 100;
    for _ in 0..150 {
        world.step(&s, None);
    }
    assert!((world.total_mass() - before).abs() < 0.01);
    conserved(&world);
}
#[test]
fn nonzero_shrink_respects_pending_mass_and_matches_at_thirty_and_sixty_fps() {
    fn run(fps: u64, scale: f32) -> World {
        let mut desktop = scene();
        desktop.screens[0].rect.right *= scale;
        desktop.screens[0].rect.bottom *= scale;
        desktop.screens[0].floor *= scale;
        desktop.screens[0].scale = scale;
        let mut world = World::new(desktop, 23);
        let mut s = quiet();
        s.snow.drift_size_percent = 100;
        for drift in world.drifts.values_mut() {
            drift.mass.fill(drift.cell_width * 110.0 / scale);
            drift.pending.fill(drift.cell_width * 10.0 / scale);
        }
        world.introduced_mass = world.total_mass();
        world.step(&s, None);
        s.snow.drift_size_percent = 99;
        let start = Instant::now();
        let mut clock = StepClock::default();
        clock.advance(start, true);
        for frame in 1..=fps * 5 / 2 {
            for _ in 0..clock
                .advance(
                    start + Duration::from_nanos(frame * 1_000_000_000 / fps),
                    true,
                )
                .count
            {
                world.step(&s, None);
            }
        }
        world
    }
    for scale in [1.0, 2.0] {
        let a = run(30, scale);
        let b = run(60, scale);
        let ground = 1 | (1 << 63);
        assert_eq!(a.drifts[&ground].mass, b.drifts[&ground].mass);
        assert_eq!(a.drifts[&ground].pending, b.drifts[&ground].pending);
        let drift = &a.drifts[&ground];
        assert!((0..drift.mass.len()).all(|i| drift.height(i, &a.scene) <= 118.801 * scale));
        assert!((a.total_mass() - 800.0 * 118.8).abs() < 0.5);
        conserved(&a);
        conserved(&b);
    }
}
#[test]
fn a_small_nonzero_limit_reduction_removes_excess_over_two_seconds() {
    let mut desktop = scene();
    desktop.windows.push(Window {
        key: 7,
        visible: true,
        collect: true,
        rect: Rect {
            left: 100.0,
            top: 300.0,
            right: 500.0,
            bottom: 500.0,
        },
    });
    let mut world = World::new(desktop, 21);
    let mut s = quiet();
    s.snow.drift_size_percent = 100;
    for drift in world.drifts.values_mut() {
        drift
            .mass
            .fill(drift.cell_width * if drift.support.ground { 120.0 } else { 32.0 });
    }
    world.introduced_mass = world.total_mass();
    world.step(&s, None);
    let before = world.total_mass();
    s.snow.drift_size_percent = 99;
    let target = before * 0.99;
    let excess = before - target;
    for _ in 0..60 {
        world.step(&s, None);
    }
    assert!(world.total_mass() > target + excess * 0.35);
    assert!(world.total_mass() < target + excess * 0.65);
    for _ in 0..90 {
        world.step(&s, None);
    }
    assert!((world.total_mass() - target).abs() < 0.01);
    assert!(world.flakes.is_empty());
    conserved(&world);
}
#[test]
fn global_opacity_changes_no_particle_motion_or_accumulation() {
    fn run(opacity: u8) -> World {
        let mut s = quiet();
        s.snow.intensity_percent = 60;
        s.weather.opacity_percent = opacity;
        let mut world = World::new(scene(), 61);
        world.seed_air(&s);
        for _ in 0..600 {
            world.step(&s, None);
        }
        world
    }
    let a = run(10);
    let b = run(100);
    assert_eq!(a.flakes.len(), b.flakes.len());
    for (a, b) in a.flakes.iter().zip(&b.flakes) {
        assert_eq!(a.position, b.position);
        assert_eq!(a.velocity, b.velocity);
    }
    for (key, drift) in &a.drifts {
        assert_eq!(drift.mass, b.drifts[key].mass);
        assert_eq!(drift.pending, b.drifts[key].pending);
    }
    assert_eq!(a.total_mass(), b.total_mass());
}
#[test]
fn disabling_accumulation_freezes_on_paused_screens_and_survives_scene_refresh() {
    let desktop = two_screens();
    let mut world = World::new(desktop.clone(), 8);
    for drift in world.drifts.values_mut() {
        drift.mass.fill(drift.cell_width * 2.0);
    }
    world.introduced_mass = world.total_mass();
    let left = 1 | (1 << 63);
    let right = 2 | (1 << 63);
    let frozen = world.drifts[&left].mass.clone();
    let initial = world.drifts[&right].total_mass();
    let mut s = quiet();
    s.snow.on_ground = false;
    world.set_paused_monitors([1].into());
    for frame in 0..150 {
        if frame % 15 == 0 {
            world.set_scene(desktop.clone());
        }
        world.step(&s, None);
    }
    assert_eq!(world.drifts[&left].mass, frozen);
    assert!(world.drifts[&right].total_mass() < initial * 0.001);
    world.set_paused_monitors(Default::default());
    for _ in 0..60 {
        world.step(&s, None);
    }
    assert!(world.drifts[&left].total_mass() > initial * 0.4);
    for _ in 0..90 {
        world.step(&s, None);
    }
    assert!(world.total_mass() < 0.001);
    conserved(&world);
}
#[test]
fn restoring_accumulation_stops_the_fade_and_resize_does_not_restart_it() {
    let mut desktop = scene();
    desktop.windows.push(Window {
        key: 7,
        visible: true,
        collect: true,
        rect: Rect {
            left: 100.0,
            top: 300.0,
            right: 500.0,
            bottom: 500.0,
        },
    });
    let mut world = World::new(desktop.clone(), 8);
    let drift = world.drifts.get_mut(&7).unwrap();
    drift.mass.fill(drift.cell_width * 3.0);
    world.introduced_mass = world.total_mass();
    let mut s = quiet();
    s.snow.on_windows = false;
    for _ in 0..30 {
        world.step(&s, None);
    }
    s.snow.on_windows = true;
    let before = world.total_mass();
    for _ in 0..60 {
        world.step(&s, None);
    }
    assert!((before - world.total_mass()).abs() < 0.01);
    s.snow.on_windows = false;
    for _ in 0..30 {
        world.step(&s, None);
    }
    desktop.windows[0].rect.right = 560.0;
    world.set_scene(desktop);
    for _ in 0..120 {
        world.step(&s, None);
    }
    assert!(world.total_mass() < 0.001);
    conserved(&world);
}
#[test]
fn inactive_accumulation_uses_physical_steps_while_cursor_and_melting_are_independent() {
    fn run(fps: u64) -> World {
        let mut world = World::new(scene(), 8);
        for drift in world.drifts.values_mut() {
            drift.mass.fill(drift.cell_width * 2.0);
        }
        world.introduced_mass = world.total_mass();
        let mut s = quiet();
        s.snow.on_ground = false;
        let start = Instant::now();
        let mut clock = StepClock::default();
        clock.advance(start, true);
        for frame in 1..=fps {
            for _ in 0..clock
                .advance(
                    start + Duration::from_nanos(frame * 1_000_000_000 / fps),
                    true,
                )
                .count
            {
                world.step(&s, None);
            }
        }
        world
    }
    let a = run(30);
    let b = run(60);
    let ground = 1 | (1 << 63);
    assert_eq!(a.drifts[&ground].mass, b.drifts[&ground].mass);
    let mut world = World::new(scene(), 9);
    let drift = world.drifts.get_mut(&ground).unwrap();
    drift.mass.fill(drift.cell_width * 3.0);
    world.introduced_mass = world.total_mass();
    let mut s = quiet();
    s.interaction.cursor_enabled = false;
    let before = world.total_mass();
    let cursor = Some(snowflakes_client::snow::Cursor {
        position: Point::new(400.0, 597.0),
        velocity: Point::new(2000.0, 0.0),
    });
    for _ in 0..60 {
        world.step(&s, cursor);
    }
    assert!((world.total_mass() - before).abs() < 0.01);
    assert!(world.flakes.is_empty());
    s.snow.melting = true;
    for _ in 0..60 {
        world.step(&s, cursor);
    }
    assert!(world.total_mass() < before);
    assert!(world.flakes.is_empty());
    s.snow.melting = false;
    s.interaction.cursor_enabled = true;
    world.step(&s, cursor);
    world.step(&s, None); // Swept material enters the particle set on the next step.
    assert!(!world.flakes.is_empty());
    conserved(&world);
}
#[test]
fn accumulation_switches_remove_small_piles_and_pending_mass_gradually() {
    let mut desktop = scene();
    desktop.windows.push(Window {
        key: 7,
        visible: true,
        collect: true,
        rect: Rect {
            left: 100.0,
            top: 300.0,
            right: 500.0,
            bottom: 500.0,
        },
    });
    let mut world = World::new(desktop, 11);
    let ground = 1 | (1 << 63);
    for drift in world.drifts.values_mut() {
        drift.mass.fill(drift.cell_width * 3.0);
        drift.pending.fill(drift.cell_width * 0.6);
    }
    world.introduced_mass = world.total_mass();
    let window_before = world.drifts[&7].total_mass();
    let ground_before = world.drifts[&ground].total_mass();
    let mut s = quiet();
    s.snow.on_windows = false;
    for _ in 0..60 {
        world.step(&s, None);
    }
    assert!(world.drifts[&7].total_mass() > window_before * 0.35);
    assert!(world.drifts[&7].total_mass() < window_before * 0.65);
    assert!((world.drifts[&ground].total_mass() - ground_before).abs() < 0.01);
    for _ in 0..90 {
        world.step(&s, None);
    }
    assert!(world.drifts[&7].total_mass() < 0.001);
    s.snow.on_ground = false;
    world.step(&s, None);
    assert!(world.drifts[&ground].total_mass() > ground_before * 0.9);
    for _ in 0..149 {
        world.step(&s, None);
    }
    assert!(world.total_mass() < 0.001);
    assert!(world.flakes.is_empty());
    let drift = world.drifts.get_mut(&ground).unwrap();
    drift.mass.fill(drift.cell_width * 2.0);
    let added = drift.total_mass();
    world.introduced_mass += added;
    s.snow.on_ground = true;
    s.snow.drift_size_percent = 0;
    world.step(&s, None);
    assert!(world.total_mass() > added * 0.9);
    for _ in 0..149 {
        world.step(&s, None);
    }
    assert!(world.total_mass() < 0.001);
    conserved(&world);
}
#[test]
fn ground_tracks_a_changed_floor_gradually_without_moving_material_between_cells() {
    let mut s = scene();
    let mut world = World::new(s.clone(), 9);
    let key = 1 | (1 << 63);
    let drift = world.drifts.get_mut(&key).unwrap();
    drift.mass.fill(10.0 * drift.cell_width);
    let profile = drift.mass.clone();
    world.introduced_mass = world.total_mass();
    for target in [560.0, 600.0] {
        let start = world.drifts[&key].support.y;
        s.screens[0].floor = target;
        world.set_scene(s.clone());
        assert_eq!(
            world.drifts[&key].support.y, start,
            "scene events must not teleport the floor"
        );
        for _ in 0..120 {
            let before = world.drifts[&key].support.y;
            world.step(&quiet(), None);
            let after = world.drifts[&key].support.y;
            assert!((after - before).abs() <= 6.001);
            assert!((target - after).abs() <= (target - before).abs());
            assert!(after >= start.min(target) && after <= start.max(target));
            assert_eq!(world.drifts[&key].mass, profile);
            assert!(world.flakes.is_empty());
            conserved(&world);
        }
        assert!((world.drifts[&key].support.y - target).abs() < 0.1);
    }
}

#[test]
fn a_paused_monitor_holds_its_floor_while_other_floors_move() {
    let mut s = two_screens();
    let mut world = World::new(s.clone(), 9);
    let left = 1 | (1 << 63);
    let right = 2 | (1 << 63);
    world.set_paused_monitors([1].into());
    for screen in &mut s.screens {
        screen.floor = 560.0;
    }
    world.set_scene(s);
    for _ in 0..120 {
        world.step(&quiet(), None);
    }
    assert_eq!(world.drifts[&left].support.y, 600.0);
    assert!((world.drifts[&right].support.y - 560.0).abs() < 0.1);
    world.set_paused_monitors(Default::default());
    world.step(&quiet(), None);
    assert!(world.drifts[&left].support.y >= 594.0 && world.drifts[&left].support.y < 600.0);
}

#[test]
fn a_moving_floor_uses_current_cover_masks_and_refreshes_contact_buckets() {
    let mut s = scene();
    let mut world = World::new(s.clone(), 9);
    let key = 1 | (1 << 63);
    s.screens[0].floor = 480.0;
    s.windows.push(overlay(
        99,
        Rect {
            left: 0.0,
            top: 480.0,
            right: 800.0,
            bottom: 600.0,
        },
    ));
    world.set_scene(s.clone());
    for _ in 0..120 {
        world.step(&quiet(), None);
        assert!(
            world.drifts[&key]
                .support
                .draw_clips
                .iter()
                .all(|r| r.bottom <= 480.0)
        );
    }
    assert!((world.drifts[&key].support.y - 480.0).abs() < 0.1);
    s.windows.clear();
    s.screens[0].floor = 600.0;
    world.set_scene(s);
    for _ in 0..120 {
        world.step(&quiet(), None);
    }
    assert!((world.drifts[&key].support.y - 600.0).abs() < 0.1);
    let mut flake = Flake::new(Point::new(400.0, 595.0), 3.0);
    flake.velocity.y = 500.0;
    world.release(flake);
    for _ in 0..3 {
        world.step(&quiet(), None);
    }
    assert!(world.drifts[&key].total_mass() > 0.0);
    assert!(world.flakes.is_empty());
    conserved(&world);
}

#[test]
fn display_topology_changes_rebase_the_floor_instead_of_animating_from_old_coordinates() {
    let mut s = scene();
    let mut world = World::new(s.clone(), 9);
    s.screens[0].rect.top = -200.0;
    s.screens[0].rect.bottom = 400.0;
    s.screens[0].floor = 360.0;
    world.set_scene(s);
    assert_eq!(world.drifts[&(1 | (1 << 63))].support.y, 360.0);
}

#[test]
fn floor_motion_uses_fixed_steps_and_dip_speed_at_different_display_rates() {
    fn run(fps: u32, scale: f32) -> f32 {
        let mut s = scene();
        s.screens[0].rect.bottom *= scale;
        s.screens[0].floor *= scale;
        s.screens[0].scale = scale;
        let mut world = World::new(s.clone(), 9);
        s.screens[0].floor = 560.0 * scale;
        world.set_scene(s);
        let mut clock = StepClock::default();
        let start = Instant::now();
        clock.advance(start, true);
        for frame in 1..=fps / 5 {
            let now = start + Duration::from_nanos(frame as u64 * 1_000_000_000 / fps as u64);
            for _ in 0..clock.advance(now, true).count {
                world.step(&quiet(), None);
            }
        }
        world.drifts[&(1 | (1 << 63))].support.y / scale
    }
    let y = run(30, 1.0);
    assert!(y > 560.0 && y < 590.0);
    assert_eq!(y, run(60, 1.0));
    assert!((y - run(60, 2.0)).abs() < 0.001);
}

fn overlay(key: u64, rect: Rect) -> Window {
    Window {
        key,
        rect,
        visible: true,
        collect: false,
    }
}

#[test]
fn a_cover_above_a_ledge_masks_the_pile_without_hiding_the_base() {
    let (_, mut s, _) = stocked_window();
    s.windows.insert(
        0,
        overlay(
            90,
            Rect {
                left: 300.0,
                top: 170.0,
                right: 500.0,
                bottom: 198.0,
            },
        ),
    );
    let supports = s.supports();
    let ledge = supports.iter().find(|s| s.key == 7).unwrap();
    assert!(ledge.visible_at(400.0));
    assert!(!ledge.exposed(Point::new(400.0, 195.0)));
    assert!(ledge.exposed(Point::new(400.0, 200.0)));
    assert_eq!(ledge.exposed_headroom(Point::new(400.0, 200.0)), 2.0);
    // Splitting the mask must not introduce an imaginary ceiling beside it.
    assert_eq!(ledge.exposed_headroom(Point::new(200.0, 200.0)), 140.0);
    s.windows.swap(0, 1);
    assert!(
        s.supports()
            .iter()
            .find(|s| s.key == 7)
            .unwrap()
            .exposed(Point::new(400.0, 195.0))
    );
}

#[test]
fn floor_masks_match_the_union_of_visible_overlays_without_double_drawing() {
    let mut s = scene();
    s.screens[0].rect.left = -200.0;
    s.screens[0].rect.top = -100.0;
    s.screens[0].scale = 2.0;
    s.windows = vec![
        overlay(
            1,
            Rect {
                left: -250.0,
                top: 480.0,
                right: 100.0,
                bottom: 650.0,
            },
        ),
        overlay(
            2,
            Rect {
                left: 50.0,
                top: 450.0,
                right: 500.0,
                bottom: 580.0,
            },
        ),
        overlay(
            3,
            Rect {
                left: 450.0,
                top: 520.0,
                right: 900.0,
                bottom: 700.0,
            },
        ),
    ];
    let mut hidden = overlay(4, s.screens[0].rect);
    hidden.visible = false;
    s.windows.push(hidden);
    let mut ordinary = overlay(5, s.screens[0].rect);
    ordinary.collect = true;
    s.windows.push(ordinary);
    let supports = s.supports();
    let floor = supports.iter().find(|s| s.ground).unwrap();
    for x in (-220..830).step_by(11) {
        for y in (100..620).step_by(7) {
            let point = Point::new(x as f32 + 0.25, y as f32 + 0.25);
            let expected = point.x >= -200.0
                && point.x < 800.0
                && point.y >= 110.0
                && point.y < 600.0
                && !s.windows[..3].iter().any(|w| w.rect.contains(point));
            let painted = floor
                .draw_clips
                .iter()
                .filter(|r| r.contains(point))
                .count();
            assert_eq!(painted, usize::from(expected), "point {point:?}");
        }
    }
}

#[test]
fn a_pathological_mask_hides_accumulation_instead_of_exceeding_its_budget() {
    let mut s = scene();
    for i in 0..50 {
        let x = 5.0 + i as f32 * 15.0;
        let y = 115.0 + i as f32 * 9.0;
        s.windows.push(overlay(
            2 * i,
            Rect {
                left: x,
                top: 0.0,
                right: x + 2.0,
                bottom: 600.0,
            },
        ));
        s.windows.push(overlay(
            2 * i + 1,
            Rect {
                left: 0.0,
                top: y,
                right: 800.0,
                bottom: y + 2.0,
            },
        ));
    }
    assert!(
        s.supports()
            .iter()
            .find(|s| s.ground)
            .unwrap()
            .draw_clips
            .is_empty()
    );
}

#[test]
fn covered_snow_keeps_its_mass_and_ignores_cursor_and_new_deposits() {
    let (mut world, mut s, settings) = stocked_window();
    let mass = world.drifts[&7].total_mass();
    s.windows.insert(
        0,
        overlay(
            90,
            Rect {
                left: 100.0,
                top: 170.0,
                right: 700.0,
                bottom: 198.0,
            },
        ),
    );
    world.set_scene(s.clone());
    let cursor = snowflakes_client::snow::Cursor {
        position: Point::new(400.0, 195.0),
        velocity: Point::new(600.0, 0.0),
    };
    for _ in 0..60 {
        world.step(&settings, Some(cursor));
    }
    assert!(world.flakes.is_empty());
    assert!((world.drifts[&7].total_mass() - mass).abs() < 0.001);
    let mut flake = Flake::new(Point::new(400.0, 194.0), 3.0);
    flake.velocity.y = 500.0;
    world.release(flake);
    world.step(&settings, None);
    assert_eq!(world.flakes.len(), 1);
    assert!((world.drifts[&7].total_mass() - mass).abs() < 0.001);
    s.windows.remove(0);
    world.set_scene(s);
    assert!(world.drifts[&7].support.exposed(Point::new(400.0, 195.0)));
    assert!((world.drifts[&7].total_mass() - mass).abs() < 0.001);
    world.step(&settings, Some(cursor));
    assert!(world.drifts[&7].total_mass() < mass);
    conserved(&world);
}

#[test]
fn floor_covered_by_a_popup_neither_collects_nor_loses_snow_to_the_cursor() {
    let mut s = scene();
    let mut world = World::new(s.clone(), 11);
    let key = 1 | (1 << 63);
    let drift = world.drifts.get_mut(&key).unwrap();
    drift.mass.fill(5.0 * drift.cell_width);
    world.introduced_mass = world.total_mass();
    let mass = world.total_mass();
    s.windows.push(overlay(
        99,
        Rect {
            left: 100.0,
            top: 550.0,
            right: 700.0,
            bottom: 650.0,
        },
    ));
    world.set_scene(s);
    world.step(
        &quiet(),
        Some(snowflakes_client::snow::Cursor {
            position: Point::new(400.0, 595.0),
            velocity: Point::new(600.0, 0.0),
        }),
    );
    assert!(world.flakes.is_empty());
    assert_eq!(world.total_mass(), mass);
    let mut flake = Flake::new(Point::new(400.0, 594.0), 3.0);
    flake.velocity.y = 500.0;
    world.release(flake);
    world.step(&quiet(), None);
    assert_eq!(world.flakes.len(), 1);
    assert_eq!(world.drifts[&key].total_mass(), mass);
    conserved(&world);
}

#[test]
fn deposition_fills_a_visible_gap_without_growing_into_its_cover() {
    let mut s = scene();
    s.windows.push(overlay(
        99,
        Rect {
            left: 0.0,
            top: 550.0,
            right: 800.0,
            bottom: 595.0,
        },
    ));
    let mut world = World::new(s, 2);
    let key = 1 | (1 << 63);
    for _ in 0..200 {
        let mut flake = Flake::new(Point::new(400.0, 580.0), 12.0);
        flake.velocity.y = 500.0;
        world.release(flake);
        for _ in 0..8 {
            world.step(&quiet(), None);
        }
    }
    let drift = &world.drifts[&key];
    assert!(drift.total_mass() > 10.0);
    for i in 0..drift.mass.len() {
        assert!((drift.mass[i] + drift.pending[i]) / drift.cell_width <= 5.001);
    }
    conserved(&world);
}
fn conserved(world: &World) {
    let difference = (world.introduced_mass - world.removed_mass - world.total_mass()).abs();
    assert!(
        difference < 0.001 * world.introduced_mass.max(1.0),
        "mass difference {difference}"
    );
}
#[test]
fn gravity_drag_and_mass_determine_terminal_speed() {
    let mut light = Flake::new(Point::new(10.0, 0.0), 3.0);
    let mut heavy = light.clone();
    heavy.mass *= 2.0;
    for _ in 0..1200 {
        light.integrate(Point::default(), 1.0);
        heavy.integrate(Point::default(), 1.0);
    }
    assert!(light.velocity.y > 25.0 && light.velocity.y < 70.0);
    assert!(heavy.velocity.y > light.velocity.y * 1.3);
    let last = light.velocity.y;
    light.integrate(Point::default(), 1.0);
    assert!((light.velocity.y - last).abs() < 0.01);
}
#[test]
fn changing_wind_accelerates_a_body_instead_of_replacing_its_path() {
    let mut flake = Flake::new(Point::new(100.0, 0.0), 4.0);
    flake.velocity.x = 60.0;
    flake.integrate(Point::new(-40.0, 0.0), 1.0);
    assert!(flake.velocity.x > 0.0 && flake.velocity.x < 60.0);
    let after = flake.position.x;
    for _ in 0..600 {
        flake.integrate(Point::new(-40.0, 0.0), 1.0);
    }
    assert!(flake.velocity.x < -30.0 && flake.position.x < after);
    assert!(flake.angle.is_finite() && flake.angle > 0.01 && flake.spin.abs() < 2.5);
}
#[test]
fn visible_ledges_respect_front_to_back_order_and_monitor_edges() {
    let mut s = scene();
    s.windows = vec![
        Window {
            key: 10,
            rect: Rect {
                left: 300.0,
                top: 200.0,
                right: 600.0,
                bottom: 550.0,
            },
            visible: true,
            collect: true,
        },
        Window {
            key: 11,
            rect: Rect {
                left: 100.0,
                top: 300.0,
                right: 700.0,
                bottom: 500.0,
            },
            visible: true,
            collect: true,
        },
    ];
    let supports = s.supports();
    let rear = supports.iter().find(|s| s.key == 11).unwrap();
    assert!(rear.visible_at(200.0));
    assert!(!rear.visible_at(450.0));
    assert!(rear.visible_at(650.0));
    s.windows[0].visible = false;
    assert!(
        s.supports()
            .iter()
            .find(|s| s.key == 11)
            .unwrap()
            .visible_at(450.0)
    );
    s.windows[1].rect.top = 0.0;
    assert_eq!(s.headroom(400.0, 0.0), 0.0);
}
#[test]
fn swept_contact_deposits_mass_without_tunneling() {
    let mut s = scene();
    s.windows.push(Window {
        key: 9,
        rect: Rect {
            left: 100.0,
            top: 100.0,
            right: 700.0,
            bottom: 250.0,
        },
        visible: true,
        collect: true,
    });
    let mut world = World::new(s, 1);
    let mut flake = Flake::new(Point::new(200.0, 95.0), 3.0);
    flake.velocity.y = 500.0;
    let mass = flake.mass;
    assert!(world.release(flake));
    world.step(&quiet(), None);
    assert!(world.flakes.is_empty());
    assert!((world.drifts[&9].total_mass() - f64::from(mass)).abs() < 0.001);
    conserved(&world);
}
#[test]
fn window_resize_close_and_handle_replacement_keep_material_accounted_for() {
    let mut s = scene();
    s.windows.push(Window {
        key: 9,
        rect: Rect {
            left: 100.0,
            top: 100.0,
            right: 400.0,
            bottom: 250.0,
        },
        visible: true,
        collect: true,
    });
    let mut world = World::new(s.clone(), 5);
    let mut flake = Flake::new(Point::new(200.0, 95.0), 3.0);
    flake.velocity.y = 500.0;
    world.release(flake);
    world.step(&quiet(), None);
    let mass = world.total_mass();
    s.windows[0].rect = Rect {
        left: 200.0,
        top: 200.0,
        right: 700.0,
        bottom: 400.0,
    };
    world.set_scene(s.clone());
    assert!((world.total_mass() - mass).abs() < 0.001);
    assert!(world.drifts[&9].mass.iter().sum::<f32>() > 0.0);
    s.windows[0].key = 10;
    world.set_scene(s);
    assert!(!world.drifts.contains_key(&9));
    assert_eq!(world.drifts[&10].mass.iter().sum::<f32>(), 0.0);
    assert!((world.total_mass() - mass).abs() < 0.001);
    world.step(&quiet(), None);
    conserved(&world);
}
#[test]
fn emission_and_drift_limit_are_independent_and_clear_is_accounted() {
    let mut a = World::new(scene(), 71);
    let mut b = World::new(scene(), 71);
    let mut sa = quiet();
    sa.snow.intensity_percent = 40;
    let mut sb = sa.clone();
    sb.snow.drift_size_percent = 0;
    for _ in 0..600 {
        a.step(&sa, None);
        b.step(&sb, None);
    }
    assert!((a.introduced_mass - b.introduced_mass).abs() < 0.001);
    conserved(&a);
    conserved(&b);
    a.clear();
    assert_eq!(a.total_mass(), 0.0);
    conserved(&a);
}
#[test]
fn physical_state_matches_at_thirty_and_sixty_display_frames() {
    fn run(fps: u32) -> World {
        let mut world = World::new(scene(), 44);
        let mut settings = quiet();
        settings.snow.intensity_percent = 60;
        let mut clock = StepClock::default();
        let start = Instant::now();
        clock.advance(start, true);
        for index in 1..=fps * 15 {
            let now = start + Duration::from_nanos(index as u64 * 1_000_000_000 / fps as u64);
            for _ in 0..clock.advance(now, true).count {
                world.step(&settings, None);
            }
        }
        world
    }
    let a = run(30);
    let b = run(60);
    assert_eq!(a.flakes.len(), b.flakes.len());
    for (a, b) in a.flakes.iter().zip(&b.flakes) {
        assert_eq!(a.position, b.position);
        assert_eq!(a.velocity, b.velocity);
    }
    assert_eq!(a.total_mass(), b.total_mass());
}
#[test]
fn battery_transitions_preserve_trajectories_and_accumulation() {
    use snowflakes_client::quality::RenderQuality;
    fn run(transitions: bool) -> World {
        let mut world = World::new(scene(), 44);
        let mut settings = quiet();
        settings.snow.intensity_percent = 60;
        settings.snow.wind_percent = 20;
        world.seed_air(&settings);
        let mut clock = StepClock::default();
        let start = Instant::now();
        clock.advance(start, true);
        for segment in 0..6 {
            let quality = RenderQuality::for_settings(&settings, transitions && segment % 2 != 0);
            let fps = u64::from(quality.target_fps);
            for frame in 1..=fps * 3 {
                let now = start
                    + Duration::from_secs(segment * 3)
                    + Duration::from_nanos(frame * 1_000_000_000 / fps);
                for _ in 0..clock.advance(now, true).count {
                    world.step(&settings, None);
                }
            }
        }
        world
    }
    let steady = run(false);
    let changing = run(true);
    assert!(!steady.flakes.is_empty());
    assert!(
        steady
            .drifts
            .values()
            .any(|d| d.mass.iter().sum::<f32>() > 0.0)
    );
    assert_eq!(steady.flakes.len(), changing.flakes.len());
    for (a, b) in steady.flakes.iter().zip(&changing.flakes) {
        assert_eq!(a.position, b.position);
        assert_eq!(a.velocity, b.velocity);
        assert_eq!(a.angle, b.angle);
    }
    for (key, drift) in &steady.drifts {
        assert_eq!(drift.mass, changing.drifts[key].mass);
    }
    assert_eq!(steady.total_mass(), changing.total_mass());
    assert_eq!(steady.introduced_mass, changing.introduced_mass);
    conserved(&changing);
}
#[test]
fn populated_world_stays_bounded_and_finite() {
    let mut world = World::new(scene(), 77);
    for index in 0..ORDINARY_FLAKES_PER_SCREEN {
        assert!(world.release(Flake::new(Point::new((index % 800) as f32, 100.0), 3.0)));
    }
    assert!(!world.release(Flake::new(Point::default(), 3.0)));
    let mut settings = quiet();
    settings.snow.intensity_percent = 100;
    for _ in 0..600 {
        world.step(&settings, None);
        assert!(world.flakes.len() <= MAX_FLAKES);
    }
    assert!(
        world
            .flakes
            .iter()
            .all(|f| f.position.x.is_finite() && f.position.y.is_finite() && f.angle.is_finite())
    );
    conserved(&world);
}

#[test]
fn percentage_scaling_preserves_the_distribution_and_existing_velocity() {
    let mut world = World::new(scene(), 82);
    let settings = Settings::default();
    world.seed_air(&settings);
    let original = world.flakes.clone();
    assert!(original.len() > 2);
    let smallest = original
        .iter()
        .map(|f| f.radius)
        .fold(f32::INFINITY, f32::min);
    let largest = original.iter().map(|f| f.radius).fold(0.0, f32::max);
    assert!(largest > smallest * 2.0);
    world.set_flake_size(150);
    for (old, new) in original.iter().zip(&world.flakes) {
        assert!((new.radius / old.radius - 1.5).abs() < 0.0001);
        assert_eq!(new.position, old.position);
        assert_eq!(new.velocity, old.velocity);
    }
    conserved(&world);
    world.set_flake_size(25);
    conserved(&world);
}

#[test]
fn visibility_is_an_appearance_setting_and_cannot_change_physics() {
    let mut a = World::new(scene(), 91);
    let mut b = World::new(scene(), 91);
    let mut sa = quiet();
    sa.snow.intensity_percent = 30;
    let mut sb = sa.clone();
    sb.snow.flake_visibility_percent = 200;
    a.seed_air(&sa);
    b.seed_air(&sb);
    for _ in 0..240 {
        a.step(&sa, None);
        b.step(&sb, None);
    }
    for (a, b) in a.flakes.iter().zip(&b.flakes) {
        assert_eq!(a.position, b.position);
        assert_eq!(a.radius, b.radius);
    }
    assert_eq!(a.total_mass(), b.total_mass());
}

#[test]
fn a_maximized_window_cannot_trap_new_snow_above_the_monitor() {
    let mut s = scene();
    s.windows.push(Window {
        key: 5,
        rect: Rect {
            left: 0.0,
            top: 0.0,
            right: 800.0,
            bottom: 600.0,
        },
        visible: true,
        collect: true,
    });
    let mut world = World::new(s, 8);
    let mut settings = quiet();
    settings.snow.intensity_percent = 50;
    for _ in 0..1800 {
        world.step(&settings, None);
    }
    assert_eq!(world.drifts[&5].mass.iter().sum::<f32>(), 0.0);
    assert!(world.drifts[&(1_u64 | (1 << 63))].mass.iter().sum::<f32>() > 5.0);
    conserved(&world);
}

#[test]
fn full_drift_does_not_recycle_excess_into_a_future_growth_wave() {
    let mut world = World::new(scene(), 5);
    let key = 1_u64 | (1 << 63);
    let mut settings = quiet();
    settings.snow.drift_size_percent = 10;
    let drift = world.drifts.get_mut(&key).unwrap();
    drift.mass.fill(12.0 * drift.cell_width);
    world.introduced_mass = world.total_mass();
    let mut flake = Flake::new(Point::new(400.0, 583.0), 3.0);
    flake.velocity.y = 160.0;
    world.release(flake);
    for _ in 0..600 {
        world.step(&settings, None);
    }
    assert!(
        world.flakes.is_empty(),
        "excess snow is being recycled above a full drift"
    );
    let mass = world.total_mass();
    settings.snow.drift_size_percent = 100;
    for _ in 0..240 {
        world.step(&settings, None);
    }
    assert!(
        (world.total_mass() - mass).abs() < 0.001,
        "raising a ceiling created material"
    );
    conserved(&world);
}

#[test]
fn raising_a_drift_limit_does_not_inflate_a_resting_profile() {
    let key = 1_u64 | (1 << 63);
    let mut a = World::new(scene(), 11);
    let mut b = World::new(scene(), 11);
    for world in [&mut a, &mut b] {
        let drift = world.drifts.get_mut(&key).unwrap();
        for (index, mass) in drift.mass.iter_mut().enumerate() {
            *mass = (3.0 + (index % 3) as f32) * drift.cell_width;
        }
        world.introduced_mass = world.total_mass();
    }
    let before = a.drifts[&key].mass.clone();
    let mut low = quiet();
    low.snow.drift_size_percent = 10;
    let mut high = low.clone();
    high.snow.drift_size_percent = 100;
    for _ in 0..300 {
        a.step(&high, None);
        b.step(&low, None);
    }
    assert_eq!(a.drifts[&key].mass, before);
    assert_eq!(a.drifts[&key].mass, b.drifts[&key].mass);
    assert!(a.flakes.is_empty());
    conserved(&a);
}

#[test]
fn landed_snow_settles_progressively_across_its_contact_area() {
    let mut world = World::new(scene(), 2);
    let key = 1_u64 | (1 << 63);
    let settings = quiet();
    let mut flake = Flake::new(Point::new(400.0, 595.0), 3.0);
    flake.velocity.y = 160.0;
    let mass = flake.mass;
    world.release(flake);
    world.step(&settings, None);
    let drift = &world.drifts[&key];
    let visible = drift.mass.iter().sum::<f32>();
    assert!(visible > 0.0 && visible < mass * 0.5);
    assert!(drift.pending.iter().filter(|&&m| m > 0.0).count() >= 3);
    assert!((drift.total_mass() - f64::from(mass)).abs() < 0.001);
    for _ in 0..90 {
        world.step(&settings, None);
    }
    assert!(world.drifts[&key].mass.iter().sum::<f32>() > mass * 0.99);
    conserved(&world);
}

#[test]
fn granular_relaxation_has_no_left_to_right_travelling_bias() {
    let mut world = World::new(scene(), 4);
    let key = 1_u64 | (1 << 63);
    let drift = world.drifts.get_mut(&key).unwrap();
    let middle = drift.mass.len() / 2;
    drift.mass[middle - 1] = 300.0;
    drift.mass[middle] = 300.0;
    world.introduced_mass = world.total_mass();
    let mut settings = quiet();
    settings.snow.drift_size_percent = 100;
    for _ in 0..120 {
        world.step(&settings, None);
    }
    let mass = &world.drifts[&key].mass;
    for index in 0..mass.len() {
        assert!((mass[index] - mass[mass.len() - 1 - index]).abs() < 0.001);
    }
    conserved(&world);
}

fn stocked_window() -> (World, Scene, Settings) {
    let mut s = scene();
    s.windows.push(Window {
        key: 7,
        rect: Rect {
            left: 150.0,
            top: 200.0,
            right: 650.0,
            bottom: 450.0,
        },
        visible: true,
        collect: true,
    });
    let mut world = World::new(s.clone(), 101);
    let drift = world.drifts.get_mut(&7).unwrap();
    drift.mass.fill(5.0 * drift.cell_width);
    world.introduced_mass = world.total_mass();
    let mut settings = quiet();
    settings.snow.drift_size_percent = 100;
    (world, s, settings)
}

#[test]
fn gentle_window_motion_retains_the_snow() {
    let (mut world, mut s, settings) = stocked_window();
    let before = world.drifts[&7].total_mass();
    for _ in 0..60 {
        world.step(&settings, None);
        world.step(&settings, None);
        s.windows[0].rect.left += 0.8;
        s.windows[0].rect.right += 0.8;
        world.set_scene(s.clone());
    }
    assert!((world.drifts[&7].total_mass() - before).abs() < 0.01);
    assert!(world.flakes.is_empty());
    conserved(&world);
}

#[test]
fn abrupt_window_acceleration_detaches_material_and_it_falls_past_its_ledge() {
    let (mut world, mut s, settings) = stocked_window();
    let before = world.drifts[&7].total_mass();
    world.step(&settings, None);
    world.step(&settings, None);
    s.windows[0].rect.left += 100.0;
    s.windows[0].rect.right += 100.0;
    world.set_scene(s.clone());
    let retained = world.drifts[&7].total_mass();
    assert!(retained < before * 0.95 && retained > 0.0);
    conserved(&world);
    for _ in 0..180 {
        world.step(&settings, None);
    }
    let fragments = world
        .flakes
        .iter()
        .filter(|f| f.skip_support == Some(7))
        .collect::<Vec<_>>();
    assert!(!fragments.is_empty());
    assert!(fragments.iter().all(|f| f.position.y > 220.0));
    assert!(world.drifts[&7].total_mass() <= retained + 0.001);
    conserved(&world);
}

#[test]
fn closing_a_window_releases_all_its_remaining_material() {
    let (mut world, mut s, settings) = stocked_window();
    let before = world.total_mass();
    s.windows.clear();
    world.set_scene(s);
    assert!(!world.drifts.contains_key(&7));
    assert!((world.total_mass() - before).abs() < 0.001);
    for _ in 0..120 {
        world.step(&settings, None);
    }
    assert!(
        world
            .flakes
            .iter()
            .any(|f| f.skip_support == Some(7) && f.position.y > 210.0)
    );
    conserved(&world);
}

#[test]
fn teleporting_a_window_does_not_create_a_particle_explosion() {
    let (mut world, mut s, settings) = stocked_window();
    s.screens.push(Screen {
        key: 2,
        persistent_id: true,
        rect: Rect {
            left: 800.0,
            top: 0.0,
            right: 1600.0,
            bottom: 600.0,
        },
        floor: 600.0,
        scale: 1.0,
    });
    world.step(&settings, None);
    world.step(&settings, None);
    let before = world.drifts[&7].total_mass();
    s.windows[0].rect.left += 900.0;
    s.windows[0].rect.right += 900.0;
    world.set_scene(s);
    world.step(&settings, None);
    assert!(world.flakes.is_empty());
    assert!((world.drifts[&7].total_mass() - before).abs() < 0.001);
    conserved(&world);
}

#[test]
fn light_fragments_have_stable_drag_under_strong_air_motion() {
    let mut flake = Flake::new(Point::default(), 1.3);
    flake.mass = 0.001;
    for _ in 0..600 {
        flake.integrate(Point::new(140.0, 0.0), 1.0);
        assert!(flake.position.x.is_finite() && flake.position.y.is_finite());
        assert!((0.0..=140.0).contains(&flake.velocity.x));
    }
}

#[test]
fn repeated_shaking_does_not_turn_trace_mass_into_visible_hovering_dots() {
    let (mut world, mut s, settings) = stocked_window();
    for i in 0..24 {
        world.step(&settings, None);
        world.step(&settings, None);
        let dx = if i % 2 == 0 { 70.0 } else { -70.0 };
        s.windows[0].rect.left += dx;
        s.windows[0].rect.right += dx;
        world.set_scene(s.clone());
    }
    world.step(&settings, None);
    let fragments: Vec<_> = world
        .flakes
        .iter()
        .filter(|f| f.skip_support == Some(7))
        .collect();
    assert!(!fragments.is_empty());
    for fragment in fragments {
        // A visible flake needs material proportional to its projected size.
        // Repeated fractional shedding must not magnify microscopic remnants.
        assert!(
            fragment.mass + 0.0001 >= fragment.radius.powi(2) * 1.2,
            "trace mass {} drawn at radius {}",
            fragment.mass,
            fragment.radius
        );
        if fragment.radius >= 0.5 {
            let mut falling = fragment.clone();
            falling.velocity = Point::default();
            let start = falling.position.y;
            for _ in 0..180 {
                falling.integrate(Point::default(), 1.0);
            }
            assert!(
                falling.position.y > start + 35.0,
                "visible fragment hovers in still air"
            );
        }
    }
    conserved(&world);
}

#[test]
fn closing_releases_connected_slabs_with_the_original_profile_and_mass() {
    let (mut world, mut s, settings) = stocked_window();
    let before = world.total_mass();
    s.windows.clear();
    world.set_scene(s);
    world.step(&settings, None);
    assert!(world.flakes.len() >= 3 && world.flakes.len() <= 12);
    assert!(world.flakes.iter().all(|f| f.slab.is_some()));
    for body in &world.flakes {
        let slab = body.slab.as_ref().unwrap();
        assert!(slab.cell_width * slab.heights.len() as f32 > 15.0);
        assert!(slab.heights.iter().all(|h| (h - 5.0).abs() < 0.001));
        let area = slab.heights.iter().sum::<f32>() * slab.cell_width;
        assert!((area - body.mass).abs() < 0.001);
        assert!(body.velocity.y > 12.0);
    }
    assert!((world.total_mass() - before).abs() < 0.01);
    conserved(&world);
}

#[test]
fn flake_size_does_not_resize_a_detached_slab() {
    let (mut world, mut s, settings) = stocked_window();
    s.windows.clear();
    world.set_scene(s);
    world.step(&settings, None);
    let before: Vec<_> = world
        .flakes
        .iter()
        .map(|f| (f.mass, f.slab.as_ref().unwrap().outline()))
        .collect();
    world.set_flake_size(200);
    for (body, (mass, outline)) in world.flakes.iter().zip(before) {
        assert_eq!(body.mass, mass);
        assert_eq!(body.slab.as_ref().unwrap().outline(), outline);
    }
    conserved(&world);
}

#[test]
fn falling_slabs_hit_the_floor_and_form_mounds_without_spraying_particles() {
    let (mut world, mut s, settings) = stocked_window();
    s.windows.clear();
    world.set_scene(s);
    world.step(&settings, None);
    // A low platform isolates the shape contact: the bottom, not a large
    // circular proxy around the center, should cause the first impact.
    for body in &mut world.flakes {
        body.position.y = 600.0 - body.radius - 0.5;
        body.velocity = Point::new(0.0, 40.0);
        body.spin = 0.0;
        body.angle = 0.0;
    }
    world.step(&settings, None);
    assert!(world.flakes.iter().all(|f| f.slab.is_some()));
    assert_eq!(world.drifts[&(1_u64 | (1 << 63))].total_mass(), 0.0);
    for _ in 0..300 {
        world.step(&settings, None);
    }
    assert!(
        world.flakes.is_empty(),
        "landed slabs must stay in the snow pile"
    );
    let floor = &world.drifts[&(1_u64 | (1 << 63))];
    assert!(floor.total_mass() > world.introduced_mass * 0.99);
    let peak = (0..floor.mass.len())
        .map(|i| floor.height(i, &world.scene))
        .fold(0.0, f32::max);
    assert!(
        peak > 8.0,
        "the fallen layer should gather into local mounds"
    );
    conserved(&world);
}

fn two_screens() -> Scene {
    let mut s = scene();
    s.screens.push(Screen {
        key: 2,
        persistent_id: true,
        rect: Rect {
            left: 800.0,
            top: 0.0,
            right: 1600.0,
            bottom: 600.0,
        },
        floor: 600.0,
        scale: 1.0,
    });
    s
}

#[test]
fn monitor_pause_freezes_its_particles_and_deposits_while_the_other_screen_runs() {
    let mut world = World::new(two_screens(), 17);
    let mut settings = quiet();
    settings.snow.melting = true;
    for x in [200.0, 1000.0] {
        world.release(Flake::new(Point::new(x, 100.0), 3.0));
    }
    for key in [1_u64 | (1 << 63), 2_u64 | (1 << 63)] {
        let drift = world.drifts.get_mut(&key).unwrap();
        drift.mass.fill(2.0 * drift.cell_width);
        drift.pending.fill(0.2 * drift.cell_width);
    }
    world.introduced_mass = world.total_mass();
    let frozen = world.drifts[&(1_u64 | (1 << 63))].mass.clone();
    let pending = world.drifts[&(1_u64 | (1 << 63))].pending.clone();
    world.set_paused_monitors([1].into());
    for _ in 0..120 {
        world.step(&settings, None);
    }
    assert_eq!(world.flakes[0].position, Point::new(200.0, 100.0));
    assert_eq!(world.flakes[0].age, 0.0);
    assert!(world.flakes[1].position.y > 120.0);
    assert_eq!(world.drifts[&(1_u64 | (1 << 63))].mass, frozen);
    assert_eq!(world.drifts[&(1_u64 | (1 << 63))].pending, pending);
    assert_ne!(world.drifts[&(2_u64 | (1 << 63))].pending, pending);
    world.set_paused_monitors(Default::default());
    world.step(&settings, None);
    assert!(world.flakes[0].position.y > 100.0 && world.flakes[0].position.y < 101.0);
    conserved(&world);
}

#[test]
fn paused_screen_does_not_seed_or_emit_precipitation_and_resume_has_no_backlog() {
    let mut world = World::new(two_screens(), 8);
    let mut settings = quiet();
    settings.snow.intensity_percent = 100;
    world.set_paused_monitors([1].into());
    world.seed_air(&settings);
    assert!(world.flakes.iter().all(|f| f.position.x >= 800.0));
    world.clear();
    for _ in 0..120 {
        world.step(&settings, None);
    }
    assert!(!world.flakes.is_empty());
    assert!(world.flakes.iter().all(|f| f.position.x >= 790.0));
    world.set_paused_monitors([1, 2].into());
    let before = world.total_mass();
    let count = world.flakes.len();
    for _ in 0..600 {
        world.step(&settings, None);
    }
    assert_eq!(world.total_mass(), before);
    assert_eq!(world.flakes.len(), count);
    world.set_paused_monitors(Default::default());
    world.step(&settings, None);
    assert!(world.flakes.len() <= count + 2);
    conserved(&world);
}

#[test]
fn a_shared_window_profile_cannot_transfer_or_receive_mass_in_its_paused_segment() {
    let mut s = two_screens();
    s.windows.push(Window {
        key: 7,
        rect: Rect {
            left: 100.0,
            top: 200.0,
            right: 1500.0,
            bottom: 400.0,
        },
        visible: true,
        collect: true,
    });
    let mut world = World::new(s, 4);
    let mut settings = quiet();
    settings.snow.drift_size_percent = 100;
    settings.snow.melting = true;
    let drift = world.drifts.get_mut(&7).unwrap();
    let first_active = (0..drift.mass.len())
        .find(|&i| drift.x(i) >= 800.0)
        .unwrap();
    for i in 0..first_active {
        drift.mass[i] = 25.0 * drift.cell_width;
        drift.pending[i] = drift.cell_width;
    }
    let frozen = drift.mass[..first_active].to_vec();
    let pending = drift.pending[..first_active].to_vec();
    world.introduced_mass = world.total_mass();
    world.set_paused_monitors([1].into());
    let mut flake = Flake::new(Point::new(802.0, 180.0), 5.0);
    flake.velocity.y = 100.0;
    world.release(flake);
    let cursor = snowflakes_client::snow::Cursor {
        position: Point::new(790.0, 176.0),
        velocity: Point::new(100.0, 0.0),
    };
    for _ in 0..120 {
        world.step(&settings, Some(cursor));
    }
    assert_eq!(&world.drifts[&7].mass[..first_active], frozen);
    assert_eq!(&world.drifts[&7].pending[..first_active], pending);
    conserved(&world);
}

#[test]
fn closing_a_window_on_a_paused_monitor_keeps_its_slabs_until_resume() {
    let (mut world, mut s, settings) = stocked_window();
    s.screens = two_screens().screens;
    world.set_scene(s.clone());
    world.set_paused_monitors([1].into());
    let mass = world.total_mass();
    s.windows.clear();
    world.set_scene(s);
    for _ in 0..120 {
        world.step(&settings, None);
    }
    assert!(world.flakes.is_empty());
    assert!((world.total_mass() - mass).abs() < 0.001);
    world.set_paused_monitors(Default::default());
    world.step(&settings, None);
    assert!(!world.flakes.is_empty());
    assert!(
        world
            .flakes
            .iter()
            .all(|f| f.slab.is_some() && f.position.y < 201.0)
    );
    conserved(&world);
}

#[test]
fn particles_in_the_emission_margin_above_a_paused_monitor_stay_frozen() {
    let mut world = World::new(two_screens(), 4);
    world.release(Flake::new(Point::new(200.0, -12.0), 3.0));
    world.set_paused_monitors([1].into());
    for _ in 0..120 {
        world.step(&quiet(), None);
    }
    assert_eq!(world.flakes[0].position, Point::new(200.0, -12.0));
    assert_eq!(world.flakes[0].age, 0.0);
    conserved(&world);
}

#[test]
fn offscreen_scale_follows_the_nearest_display_in_a_staggered_desktop() {
    let desktop = Scene {
        screens: vec![
            Screen {
                key: 1,
                persistent_id: true,
                rect: Rect {
                    left: -1600.0,
                    top: -600.0,
                    right: 0.0,
                    bottom: 600.0,
                },
                floor: 600.0,
                scale: 2.0,
            },
            Screen {
                key: 2,
                persistent_id: true,
                rect: Rect {
                    left: 0.0,
                    top: 0.0,
                    right: 800.0,
                    bottom: 600.0,
                },
                floor: 600.0,
                scale: 1.0,
            },
        ],
        windows: vec![],
    };
    for (point, expected) in [
        (Point::new(-800.0, -624.0), 2.0), // emission margin, negative origin
        (Point::new(-800.0, 624.0), 2.0),  // lower exit margin
        (Point::new(20.0, -300.0), 2.0),   // gap beside the taller display
        (Point::new(300.0, -12.0), 1.0),   // directly above the lower display
        (Point::new(-1.0, 200.0), 2.0),
        (Point::new(1.0, 200.0), 1.0),
    ] {
        assert_eq!(desktop.scale_at(point), expected, "point={point:?}");
    }
    assert_eq!(Scene::default().scale_at(Point::new(-20.0, -20.0)), 1.0);
}

#[test]
fn snow_in_the_upper_margin_advances_in_its_monitors_logical_units() {
    for scale in [1.0, 1.5, 2.0, 2.5] {
        let mut desktop = scene();
        desktop.screens[0].rect = Rect {
            left: -800.0 * scale,
            top: -600.0 * scale,
            right: 0.0,
            bottom: 0.0,
        };
        desktop.screens[0].floor = 0.0;
        desktop.screens[0].scale = scale;
        let mut world = World::new(desktop, 41);
        let point = Point::new(-400.0 * scale, -612.0 * scale);
        let mut body = Flake::new(point, 3.0);
        body.velocity.y = 30.0;
        assert!(world.release(body));
        world.step(&quiet(), None);
        assert_eq!(world.flakes.len(), 1);
        let dip_distance = (world.flakes[0].position.y - point.y) / scale;
        assert!(
            (0.49..0.53).contains(&dip_distance),
            "scale={scale}, logical first-step distance={dip_distance}"
        );
        assert_eq!(world.scene.scale_at(world.flakes[0].position), scale);
        conserved(&world);
    }
}

#[test]
fn moving_a_stocked_roof_to_the_monitor_top_does_not_resize_its_snow() {
    for scale in [1.0, 1.5, 2.0, 2.5] {
        let (mut world, mut desktop, settings) = two_stocked_roofs(scale);
        let mass = world.total_mass();
        let physical_cell_width = world.drifts[&7].cell_width;
        let before: Vec<_> = (0..world.drifts[&7].mass.len())
            .map(|i| world.drifts[&7].height(i, &world.scene))
            .collect();
        desktop
            .windows
            .iter_mut()
            .find(|w| w.key == 7)
            .unwrap()
            .rect
            .top = 0.0;
        world.set_scene(desktop.clone());
        for (i, expected) in before.iter().enumerate() {
            let height = world.drifts[&7].height(i, &world.scene);
            assert!(
                (height - expected).abs() < 0.001,
                "scale={scale}, roof snow changed height at the display edge: {height} vs {expected}"
            );
        }
        desktop.windows.clear();
        world.set_scene(desktop);
        world.step(&settings, None);
        assert_eq!(world.pending_collapse_count(), 0);
        assert!(world.flakes.iter().all(|b| b.slab.is_some()));
        let roof_bodies: Vec<_> = world
            .flakes
            .iter()
            .filter(|b| b.skip_support == Some(7))
            .collect();
        assert!(!roof_bodies.is_empty());
        for body in roof_bodies {
            let slab = body.slab.as_ref().unwrap();
            assert!(slab.heights.iter().all(|h| (*h - 5.0).abs() < 0.001));
            assert!((slab.cell_width * scale - physical_cell_width).abs() < 0.001);
        }
        assert!((world.total_mass() - mass).abs() < mass * 0.0001);
        conserved(&world);
    }
}

#[test]
fn a_landing_slab_immediately_leaves_a_visible_mound_with_all_its_mass() {
    let mut world = World::new(scene(), 42);
    let mut settings = quiet();
    settings.snow.drift_size_percent = 100;
    let mut body = Flake::new(Point::new(400.0, 596.0), (288.0_f32 / 1.2).sqrt());
    body.mass = 288.0;
    body.velocity.y = 160.0;
    body.slab = Some(snowflakes_client::snow::SnowSlab {
        heights: vec![6.0; 8],
        cell_width: 6.0,
        center: Point::new(24.0, -3.0),
    });
    assert!(world.release(body));
    world.step(&settings, None);
    let floor = &world.drifts[&(1_u64 | (1 << 63))];
    assert!(world.flakes.is_empty());
    assert!((floor.total_mass() - 288.0).abs() < 0.001);
    assert!(
        floor.mass.iter().sum::<f32>() > 288.0 * 0.8,
        "impact must not leave a blank frame"
    );
    let heights: Vec<_> = (0..floor.mass.len())
        .map(|i| floor.height(i, &world.scene))
        .collect();
    assert!(heights.iter().copied().fold(0.0, f32::max) > 9.0);
    assert!(
        floor
            .mass
            .iter()
            .enumerate()
            .all(|(i, &mass)| mass == 0.0 || (floor.x(i) - 400.0).abs() < 48.0)
    );
    for _ in 0..180 {
        world.step(&settings, None);
    }
    assert!(world.flakes.is_empty());
    conserved(&world);
}

#[test]
fn slab_contours_round_off_the_corners_of_a_flat_snow_layer() {
    let slab = snowflakes_client::snow::SnowSlab {
        heights: vec![8.0; 12],
        cell_width: 6.0,
        center: Point::new(36.0, -4.0),
    };
    let contour = slab.outline();
    for corner in [
        Point::new(-36.0, -4.0),
        Point::new(-36.0, 4.0),
        Point::new(36.0, -4.0),
        Point::new(36.0, 4.0),
    ] {
        assert!(
            contour
                .iter()
                .all(|p| (p.x - corner.x).hypot(p.y - corner.y) > 0.2),
            "unrounded corner {corner:?}"
        );
    }
    assert!(
        contour
            .iter()
            .all(|p| p.x.abs() <= 36.0 && p.y.abs() <= 4.0)
    );
    // Preserve the bulk of the original silhouette while smoothing the edges.
    let area = contour
        .iter()
        .zip(contour.iter().cycle().skip(1))
        .map(|(a, b)| a.x * b.y - a.y * b.x)
        .sum::<f32>()
        .abs()
        * 0.5;
    assert!(area > 72.0 * 8.0 * 0.95);
}

#[test]
fn compact_slabs_fall_quickly_under_forces_without_changing_snowflake_flight() {
    let mut flake = Flake::new(Point::default(), 3.0);
    let mut slab = Flake::new(Point::default(), (288.0_f32 / 1.2).sqrt());
    slab.mass = 288.0;
    slab.slab = Some(snowflakes_client::snow::SnowSlab {
        heights: vec![6.0; 8],
        cell_width: 6.0,
        center: Point::new(24.0, -3.0),
    });
    for _ in 0..120 {
        flake.integrate(Point::default(), 1.0);
        slab.integrate(Point::default(), 1.0);
    }
    assert!(
        slab.position.y > 280.0,
        "slab falls too slowly: {} DIP in two seconds",
        slab.position.y
    );
    assert!(slab.velocity.y > flake.velocity.y * 4.0);
    assert!(flake.position.y < 80.0);
}
