use snowflakes_client::{
    geometry::{Point, Rect, Scene, Screen, Window},
    snow::{DT, Flake, SnowSlab, World},
};
use snowflakes_protocol::Settings;

fn scene(scale: f32, offset: f32) -> Scene {
    Scene {
        screens: vec![Screen {
            key: 1,
            persistent_id: true,
            rect: Rect {
                left: offset,
                top: 0.0,
                right: offset + 800.0 * scale,
                bottom: 1000.0 * scale,
            },
            floor: 1000.0 * scale,
            scale,
        }],
        windows: Vec::new(),
    }
}

fn quiet() -> Settings {
    let mut settings = Settings::default();
    settings.snow.intensity_percent = 0;
    settings.snow.melting = false;
    settings.snow.drift_size_percent = 100;
    settings
}

fn point(body: &Flake, local: Point) -> Point {
    let (sine, cosine) = body.angle.sin_cos();
    let scale = body.display_scale(1.0);
    Point::new(
        body.position.x + (local.x * cosine - local.y * sine) * scale,
        body.position.y + (local.x * sine + local.y * cosine) * scale,
    )
}

fn arc_fixture(scale: f32, offset: f32, height: f32, spin: f32) -> (World, Settings, Flake) {
    let slab = SnowSlab {
        heights: vec![height; 14],
        cell_width: 80.0 / 14.0,
        center: Point::new(40.0, -height * 0.5),
    };
    let outline: Vec<_> = slab.contours().flatten().collect();
    let vertex = outline
        .iter()
        .filter(|p| p.x == 40.0)
        .min_by(|a, b| (a.y * spin).total_cmp(&(b.y * spin)))
        .copied()
        .unwrap();
    let mass = 80.0 * height;
    let mut body = Flake::new(
        Point::new(offset + 400.0 * scale, 300.0 * scale),
        (mass / 1.2).sqrt(),
    );
    body.mass = mass;
    body.spin = spin;
    // With negative spin the far edge rotates upward; choose an ordinary
    // downward release velocity that still gives a top-side impact there.
    body.velocity.y = if spin < 0.0 { 32.0 } else { 12.0 };
    body.angle =
        ((-vertex.y).atan2(vertex.x) - body.spin * DT * 0.5).rem_euclid(std::f32::consts::TAU);
    body.slab = Some(slab);
    // Predict the same seeded, force-driven step without a receiving window.
    let settings = quiet();
    let mut probe = World::new(scene(scale, offset), 41);
    assert!(probe.release(body.clone()));
    body = probe.flakes[0].clone();
    probe.step(&settings, None);
    let after = &probe.flakes[0];
    assert!(after.spin.abs() <= 0.55 && after.position.y > body.position.y);
    if height < 0.2 {
        assert_ne!(
            body.angle > std::f32::consts::PI,
            after.angle > std::f32::consts::PI
        );
    }
    let endpoint_max = outline
        .iter()
        .flat_map(|&p| [point(&body, p).x, point(after, p).x])
        .fold(f32::NEG_INFINITY, f32::max);
    let mut delta =
        (f64::from(after.angle) - f64::from(body.angle)).rem_euclid(std::f64::consts::TAU);
    if delta > std::f64::consts::PI {
        delta -= std::f64::consts::TAU;
    }
    let angle = f64::from(body.angle) + delta * 0.5;
    let (sine, cosine) = angle.sin_cos();
    let x = (f64::from(body.position.x) + f64::from(after.position.x)) * 0.5
        + f64::from(scale) * (f64::from(vertex.x) * cosine - f64::from(vertex.y) * sine);
    let y = (f64::from(body.position.y) + f64::from(after.position.y)) * 0.5
        + f64::from(scale) * (f64::from(vertex.x) * sine + f64::from(vertex.y) * cosine);
    let ledge = ((f64::from(endpoint_max) + x) * 0.5) as f32;
    assert!(ledge > endpoint_max && f64::from(ledge) < x);
    let mut desktop = scene(scale, offset);
    desktop.windows.push(Window {
        key: 7,
        rect: Rect {
            left: ledge - 6.0 * scale,
            right: ledge + 30.0 * scale,
            top: y as f32,
            bottom: y as f32 + 100.0 * scale,
        },
        visible: true,
        collect: true,
    });
    let world = World::new(desktop, 41);
    assert!(world.drifts[&7].support.right - world.drifts[&7].support.left > 20.0 * scale);
    assert!(world.drifts[&7].support.left > endpoint_max);
    assert!(f64::from(world.drifts[&7].support.left) < x);
    (world, settings, body)
}

fn conserved(world: &World) {
    assert!(
        (world.introduced_mass - world.removed_mass - world.total_mass()).abs()
            < 0.001 * world.introduced_mass.max(1.0)
    );
}

fn lands(world: &World) {
    assert!(
        world.drifts[&7].total_mass() > 0.0,
        "the rotating contour crossed the real ledge between poses but did not land"
    );
    assert!(world.flakes.is_empty());
    conserved(world);
}

#[test]
fn a_rotating_slab_lands_on_a_finite_ledge_outside_both_endpoint_bounds() {
    for scale in [1.0, 1.25, 1.5, 2.5] {
        for offset in [0.0, -800.0 * scale] {
            for spin in [-0.55, 0.55] {
                let (mut world, settings, body) = arc_fixture(scale, offset, 4.0, spin);
                assert!(world.release(body));
                world.step(&settings, None);
                lands(&world);
            }
        }
    }
}

#[test]
fn angular_contact_follows_the_short_arc_when_a_thin_slab_wraps_through_zero() {
    for scale in [1.0, 2.5] {
        for spin in [-0.55, 0.55] {
            let (mut world, settings, body) = arc_fixture(scale, 0.0, 0.1, spin);
            assert!(world.release(body));
            world.step(&settings, None);
            lands(&world);
        }
    }
}

#[test]
fn angular_contact_does_not_land_beyond_the_true_radial_sweep() {
    let (mut world, settings, body) = arc_fixture(1.0, 0.0, 4.0, 0.55);
    let radius = body
        .slab
        .as_ref()
        .unwrap()
        .contours()
        .flatten()
        .map(|p| f64::from(p.x).hypot(f64::from(p.y)))
        .fold(0.0_f64, f64::max);
    let mut probe = World::new(scene(1.0, 0.0), 41);
    assert!(probe.release(body.clone()));
    probe.step(&settings, None);
    let maximum = f64::from(body.position.x.max(probe.flakes[0].position.x)) + radius;
    let mut desktop = world.scene.clone();
    let left = (maximum as f32).next_up();
    desktop.windows[0].rect.left = left - 6.0;
    desktop.windows[0].rect.right = left + 30.0;
    world.set_scene(desktop);
    assert!(f64::from(world.drifts[&7].support.left) > maximum);
    assert!(world.release(body));
    world.step(&settings, None);
    assert_eq!(world.drifts[&7].total_mass(), 0.0);
    assert_eq!(world.flakes.len(), 1);
    conserved(&world);
}

#[test]
fn angular_contact_respects_the_source_exposed_mask_and_accumulation_switch() {
    for case in 0..3 {
        let (mut world, mut settings, mut body) = arc_fixture(1.0, 0.0, 4.0, 0.55);
        match case {
            0 => body.skip_support = Some(7),
            1 => {
                let mut desktop = world.scene.clone();
                let roof = desktop.windows[0].rect;
                desktop.windows.insert(
                    0,
                    Window {
                        key: 8,
                        rect: Rect {
                            left: roof.left - 20.0,
                            right: roof.right + 20.0,
                            top: roof.top - 10.0,
                            bottom: roof.bottom,
                        },
                        visible: true,
                        collect: false,
                    },
                );
                world.set_scene(desktop);
            }
            _ => settings.snow.on_windows = false,
        }
        assert!(world.release(body));
        world.step(&settings, None);
        assert_eq!(world.drifts[&7].total_mass(), 0.0);
        assert_eq!(world.flakes.len(), 1);
        conserved(&world);
    }
}

#[test]
fn angular_contact_respects_receiver_pause_and_freezes_a_paused_body() {
    for paused in [1, 2] {
        let (mut world, settings, body) = arc_fixture(1.0, 0.0, 4.0, 0.55);
        let mut desktop = world.scene.clone();
        desktop.screens[0].rect.right = 430.0;
        let mut right = desktop.screens[0].clone();
        right.key = 2;
        right.rect.left = 430.0;
        right.rect.right = 800.0;
        desktop.screens.push(right);
        world.set_scene(desktop);
        world.set_paused_monitors([paused].into());
        let before = body.clone();
        assert!(world.release(body));
        world.step(&settings, None);
        assert_eq!(world.drifts[&7].total_mass(), 0.0);
        assert_eq!(world.flakes.len(), 1);
        if paused == 1 {
            assert_eq!(world.flakes[0].position, before.position);
            assert_eq!(world.flakes[0].angle, before.angle);
        } else {
            assert!(world.flakes[0].position.y > before.position.y);
        }
        conserved(&world);
    }
}
