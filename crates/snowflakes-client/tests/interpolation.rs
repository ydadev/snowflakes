use snowflakes_client::{
    geometry::{Point, Rect, Scene, Screen, Window},
    snow::{DT, Flake, SnowSlab, World},
    timing::{STEP, StepClock},
};
use snowflakes_protocol::Settings;
use std::{
    f32::consts::TAU,
    time::{Duration, Instant},
};

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
    let mut settings = Settings::default();
    settings.snow.intensity_percent = 0;
    settings.snow.melting = false;
    settings.snow.drift_size_percent = 100;
    settings
}
fn conserved(world: &World) {
    assert!(
        (world.introduced_mass - world.removed_mass - world.total_mass()).abs()
            < 0.001 * world.introduced_mass.max(1.0)
    );
}
fn overlap_area(a: Rect, b: Rect) -> f32 {
    (a.right.min(b.right) - a.left.max(b.left)).max(0.0)
        * (a.bottom.min(b.bottom) - a.top.max(b.top)).max(0.0)
}

#[test]
fn flake_and_slab_poses_follow_two_physical_states_across_angle_wrap_and_dpi() {
    for slab in [false, true] {
        for direction in [-1.0, 1.0] {
            let mut body = Flake::new(Point::new(-200.0, 100.0), 4.0);
            body.velocity = Point::new(60.0, 80.0);
            body.angle = if direction > 0.0 { TAU - 0.002 } else { 0.002 };
            body.spin = 2.5 * direction;
            if slab {
                body.slab = Some(SnowSlab {
                    heights: vec![3.0, 5.0, 0.0, 7.0, 6.0],
                    cell_width: 6.0,
                    center: Point::new(15.0, -2.0),
                });
            }
            let mut source = World::new(scene(), 500);
            assert!(source.release(body)); // establishes the source display's physical units
            let mut body = source.flakes.pop().unwrap();
            let previous = body.render_pose(1.0, 0.0);
            body.integrate(Point::default(), 2.5);
            let current = body.render_pose(2.5, 1.0);
            assert_eq!(body.render_pose(2.5, 0.0), previous);
            assert_eq!(current.position, body.position);
            assert_eq!(current.angle, body.angle);
            assert!((current.scale - 1.04).abs() < 0.00001);
            let middle = body.render_pose(2.5, 0.5);
            assert!(
                (middle.position.x - (previous.position.x + current.position.x) / 2.0).abs()
                    < 0.00002
            );
            assert!(
                (middle.position.y - (previous.position.y + current.position.y) / 2.0).abs()
                    < 0.00002
            );
            assert!((middle.scale - 1.02).abs() < 0.00001);
            assert!((middle.age - DT / 2.0).abs() < 0.000001);
            assert!(
                middle.angle.min(TAU - middle.angle) < 0.05,
                "rotation took the long arc"
            );
            assert_eq!(body.render_pose(2.5, -10.0), previous);
            assert_eq!(body.render_pose(2.5, f32::NAN), current);
            assert_eq!(body.render_pose(2.5, 10.0), current);
            let before = format!("{body:?}");
            for phase in 0..=20 {
                let pose = body.render_pose(2.5, phase as f32 / 20.0);
                let bounds = pose.bounds(&body);
                if let Some(profile) = &body.slab {
                    for local in profile.contours().flatten() {
                        let (sine, cosine) = pose.angle.sin_cos();
                        let p = Point::new(
                            pose.position.x + (local.x * cosine - local.y * sine) * pose.scale,
                            pose.position.y + (local.x * sine + local.y * cosine) * pose.scale,
                        );
                        assert!(
                            p.x >= bounds.left
                                && p.x <= bounds.right
                                && p.y >= bounds.top
                                && p.y <= bounds.bottom
                        );
                    }
                } else {
                    assert!((bounds.width() - body.radius * pose.scale * 3.0).abs() < 0.00005);
                }
            }
            assert_eq!(
                format!("{body:?}"),
                before,
                "visual sampling changed physics"
            );
        }
    }
}

#[test]
fn monitor_pause_resume_and_clock_discontinuity_cannot_replay_an_old_pose() {
    let mut desktop = scene();
    desktop.screens.push(Screen {
        key: 2,
        rect: Rect {
            left: 800.0,
            right: 1600.0,
            ..desktop.screens[0].rect
        },
        scale: 2.5,
        ..desktop.screens[0].clone()
    });
    let mut world = World::new(desktop.clone(), 501);
    for x in [200.0, 1100.0] {
        let mut body = Flake::new(Point::new(x, 100.0), 3.0);
        body.velocity = Point::new(0.0, 80.0);
        assert!(world.release(body));
    }
    world.step(&quiet(), None);
    assert_ne!(
        world.render_pose(&world.flakes[0], 0.0).position,
        world.flakes[0].position
    );
    world.set_paused_monitors([2].into());
    let frozen = world.render_pose(&world.flakes[1], 1.0);
    for _ in 0..5 {
        world.step(&quiet(), None);
        for phase in [0.0, 0.25, 0.75, 1.0] {
            assert_eq!(world.render_pose(&world.flakes[1], phase), frozen);
        }
    }
    world.set_paused_monitors(Default::default());
    assert_eq!(world.render_pose(&world.flakes[1], 0.0), frozen);
    world.reset_interpolation();
    for body in &world.flakes {
        assert_eq!(world.render_pose(body, 0.0), world.render_pose(body, 1.0));
    }
    world.step(&quiet(), None);
    world.set_scene(Scene {
        screens: vec![],
        windows: vec![],
    });
    world.step(&quiet(), None);
    world.set_scene(desktop);
    for body in &world.flakes {
        assert_eq!(world.render_pose(body, 0.0), world.render_pose(body, 1.0));
    }
    conserved(&world);
}

#[test]
fn newborn_and_readmitted_bodies_have_no_pose_from_a_different_lifetime() {
    let mut body = Flake::new(Point::new(200.0, 100.0), 3.0);
    assert_eq!(body.render_pose(1.0, 0.0), body.render_pose(1.0, 1.0));
    body.velocity = Point::new(60.0, 60.0);
    body.integrate(Point::default(), 1.0);
    assert_ne!(body.render_pose(1.0, 0.0), body.render_pose(1.0, 1.0));
    body.position = Point::new(500.0, 200.0);
    let mut world = World::new(scene(), 502);
    assert!(world.release(body));
    let admitted = &world.flakes[0];
    assert_eq!(
        world.render_pose(admitted, 0.0),
        world.render_pose(admitted, 1.0)
    );
    assert_eq!(
        world.render_pose(admitted, 0.0).position,
        Point::new(500.0, 200.0)
    );
    world.clear();
    assert!(world.flakes.is_empty());
    conserved(&world);
}

#[test]
fn ground_motion_interpolates_but_native_geometry_and_pause_remain_immediate() {
    let mut desktop = scene();
    let mut world = World::new(desktop.clone(), 503);
    desktop.screens[0].floor = 540.0;
    world.set_scene(desktop.clone());
    let key = 1 | (1 << 63);
    let before = world.drifts[&key].support.y;
    world.step(&quiet(), None);
    let drift = &world.drifts[&key];
    assert!(drift.support.y < before);
    assert_eq!(world.render_base(drift, 0.0), before);
    assert_eq!(world.render_base(drift, 1.0), drift.support.y);
    assert!((world.render_base(drift, 0.5) - (before + drift.support.y) / 2.0).abs() < 0.0001);
    // An unchanged periodic scene snapshot must not restart the visual interval.
    let middle = world.render_base(drift, 0.5);
    world.set_scene(desktop.clone());
    assert_eq!(world.render_base(&world.drifts[&key], 0.5), middle);
    world.set_paused_monitors([1].into());
    let frozen = world.drifts[&key].support.y;
    world.step(&quiet(), None);
    assert_eq!(world.render_base(&world.drifts[&key], 0.0), frozen);
    world.set_paused_monitors(Default::default());
    assert_eq!(world.render_base(&world.drifts[&key], 0.0), frozen);
    desktop.windows.push(Window {
        key: 7,
        rect: Rect {
            left: 150.0,
            top: 200.0,
            right: 650.0,
            bottom: 400.0,
        },
        visible: true,
        collect: true,
    });
    world.set_scene(desktop.clone());
    let before = world.drifts[&7].support.y;
    desktop.windows[0].rect.top += 40.0;
    desktop.windows[0].rect.bottom += 40.0;
    world.set_scene(desktop);
    let roof = &world.drifts[&7];
    assert!(roof.support.y > before);
    for phase in [0.0, 0.25, 0.75, 1.0] {
        assert_eq!(world.render_base(roof, phase), roof.support.y);
    }
    conserved(&world);
}

#[test]
fn interpolated_slabs_use_current_window_masks_at_the_displayed_position() {
    let mut desktop = scene();
    let cover = Rect {
        left: 250.0,
        top: 250.0,
        right: 550.0,
        bottom: 400.0,
    };
    desktop.windows = vec![
        Window {
            key: 8,
            rect: cover,
            visible: true,
            collect: false,
        },
        Window {
            key: 7,
            rect: Rect {
                left: 150.0,
                top: 200.0,
                right: 650.0,
                bottom: 450.0,
            },
            visible: true,
            collect: true,
        },
    ];
    let mut world = World::new(desktop.clone(), 507);
    world.drifts.get_mut(&7).unwrap().mass.fill(20.0);
    world.introduced_mass = world.total_mass();
    desktop.windows.retain(|w| w.key != 7);
    world.set_scene(desktop.clone());
    let mut tested = false;
    for _ in 0..120 {
        world.step(&quiet(), None);
        let candidate = world.flakes.iter().enumerate().find_map(|(index, body)| {
            let pose = world.render_pose(body, 0.25);
            let bounds = pose.bounds(body);
            let area = overlap_area(bounds, cover);
            (body.slab.is_some() && area > 10.0).then_some((index, pose, bounds))
        });
        let Some((index, pose, bounds)) = candidate else {
            continue;
        };
        let clips = world.falling_clips(&world.flakes[index], bounds).unwrap();
        let area = clips.iter().map(|r| r.width() * r.height()).sum::<f32>();
        assert!(area < bounds.width() * bounds.height());
        for clip in clips {
            assert_eq!(overlap_area(clip, cover), 0.0);
        }
        desktop.windows[0].rect.left += 650.0;
        desktop.windows[0].rect.right += 650.0;
        world.set_scene(desktop);
        assert_eq!(world.render_pose(&world.flakes[index], 0.25), pose);
        assert_eq!(
            world.falling_clips(&world.flakes[index], bounds).unwrap(),
            vec![bounds]
        );
        tested = true;
        break;
    }
    assert!(tested, "no falling contour reached the foreground cover");
    conserved(&world);
}

#[test]
fn closing_and_landing_change_topology_immediately_without_duplicate_visual_snow() {
    let mut desktop = scene();
    desktop.windows.push(Window {
        key: 7,
        rect: Rect {
            left: 150.0,
            top: 200.0,
            right: 650.0,
            bottom: 400.0,
        },
        visible: true,
        collect: true,
    });
    let mut world = World::new(desktop.clone(), 504);
    world.drifts.get_mut(&7).unwrap().mass.fill(20.0);
    world.introduced_mass = world.total_mass();
    desktop.windows.clear();
    world.set_scene(desktop);
    assert!(!world.drifts.contains_key(&7));
    let mut falling = false;
    let mut landed = false;
    for _ in 0..900 {
        world.step(&quiet(), None);
        falling |= world.flakes.iter().any(|b| b.slab.is_some());
        for body in &world.flakes {
            let pose = world.render_pose(body, 0.0);
            assert!(pose.position.x.is_finite() && pose.position.y.is_finite());
        }
        let ground = world.drifts.values().find(|d| d.support.ground).unwrap();
        if ground.mass.iter().sum::<f32>() > 10.0 && world.flakes.is_empty() {
            let peak = (0..ground.mass.len())
                .map(|i| ground.height(i, &world.scene))
                .fold(0.0_f32, f32::max);
            assert!(
                peak > 0.1,
                "deposition was delayed behind a visual particle"
            );
            landed = true;
            break;
        }
    }
    assert!(falling && landed);
    world.clear();
    assert!(world.flakes.is_empty());
    assert!(
        world
            .drifts
            .values()
            .all(|d| d.mass.iter().all(|m| *m == 0.0))
    );
    conserved(&world);
}

#[test]
fn visual_sampling_at_30_60_and_120_fps_keeps_identical_physics() {
    let settings = quiet();
    let base = Instant::now();
    let run = |subframes: u64| {
        let mut world = World::new(scene(), 505);
        for x in [200.0, 350.0, 500.0] {
            let mut body = Flake::new(Point::new(x, 100.0), 3.0);
            body.velocity = Point::new(20.0, 60.0);
            assert!(world.release(body));
        }
        let mut clock = StepClock::default();
        clock.advance(base, true);
        let frames = 120 * subframes / 2;
        let mut total = 0;
        let mut common_frames = Vec::new();
        for frame in 1..=frames {
            let nanos = STEP.as_nanos() as u64 * frame * 2 / subframes;
            let steps = clock.advance(base + Duration::from_nanos(nanos), true);
            assert!(steps.count <= 5);
            assert_eq!(steps.discarded, Duration::ZERO);
            for _ in 0..steps.count {
                world.step(&settings, None);
                total += 1;
            }
            for body in &world.flakes {
                let pose = world.render_pose(body, clock.fraction());
                let previous = world.render_pose(body, 0.0);
                assert!(pose.position.x >= previous.position.x.min(body.position.x) - 0.0001);
                assert!(pose.position.x <= previous.position.x.max(body.position.x) + 0.0001);
                let _ = pose.bounds(body);
            }
            if frame % subframes == 0 {
                common_frames.push(
                    world
                        .flakes
                        .iter()
                        .map(|b| {
                            (
                                b.position,
                                b.velocity,
                                b.angle,
                                b.age,
                                world.render_pose(b, clock.fraction()),
                            )
                        })
                        .collect::<Vec<_>>(),
                );
            }
        }
        assert_eq!(total, 120);
        conserved(&world);
        (world.total_mass(), common_frames)
    };
    let thirty = run(1);
    assert_eq!(thirty, run(2));
    assert_eq!(thirty, run(4));
}
