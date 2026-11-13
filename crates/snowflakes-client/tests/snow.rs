use snowflakes_client::{
    geometry::{Point, Rect, Scene, Screen, Window},
    snow::{Flake, MAX_FLAKES, World},
    timing::StepClock,
};
use snowflakes_protocol::Settings;
use std::time::{Duration, Instant};
fn scene() -> Scene {
    Scene {
        screens: vec![Screen {
            key: 1,
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
fn populated_world_stays_bounded_and_finite() {
    let mut world = World::new(scene(), 77);
    for index in 0..MAX_FLAKES {
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
