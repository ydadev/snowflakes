use snowflakes_client::{
    geometry::{Point, Rect, Scene, Screen, Window},
    snow::{Flake, SnowSlab, World},
};
use snowflakes_protocol::Settings;

fn desktop(count: u64) -> Scene {
    Scene {
        screens: (0..count)
            .map(|i| Screen {
                key: i + 1,
                persistent_id: true,
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
        windows: vec![],
    }
}
fn conserved(world: &World) {
    assert!(
        (world.introduced_mass - world.removed_mass - world.total_mass()).abs()
            < 0.001 * world.introduced_mass.max(1.0)
    );
}
fn quiet() -> Settings {
    let mut settings = Settings::default();
    settings.snow.intensity_percent = 0;
    settings.snow.melting = false;
    settings.snow.drift_size_percent = 100;
    settings
}
fn fill_ordinary(world: &mut World, count: u64) {
    for i in 0..16_001 {
        let x = (i % count) as f32 * 800.0 + 100.0;
        world.release(Flake::new(Point::new(x, 100.0), 1.0));
    }
}
fn stocked(count: u64) -> (World, Scene) {
    let mut scene = desktop(count);
    for screen in 0..count {
        let left = screen as f32 * 800.0;
        scene.windows.push(Window {
            key: 1000 + screen,
            rect: Rect {
                left: left + 150.0,
                top: 200.0,
                right: left + 650.0,
                bottom: 450.0,
            },
            visible: true,
            collect: true,
        });
    }
    let mut world = World::new(scene.clone(), 602);
    for drift in world.drifts.values_mut().filter(|d| !d.support.ground) {
        drift.mass[0] = 5.0 * drift.cell_width;
    }
    world.introduced_mass = world.total_mass();
    (world, scene)
}
fn priority(screen: u64) -> Flake {
    let mut body = Flake::new(Point::new(screen as f32 * 800.0 + 200.0, 100.0), 1.0);
    body.skip_support = Some(1000 + screen);
    body.slab = Some(SnowSlab {
        heights: vec![1.2],
        cell_width: 1.0,
        center: Point::new(0.5, -0.6),
    });
    body.age = 1.0;
    body
}
fn full_eight_display_pool() -> (World, Scene) {
    let (mut world, mut scene) = stocked(8);
    scene.windows.clear();
    world.set_scene(scene.clone());
    fill_ordinary(&mut world, 8);
    assert_eq!(world.flakes.len(), 14_976); // 16,000 minus 128 protected places on each display
    for screen in 0..8 {
        for _ in 0..128 {
            assert!(world.release(priority(screen)));
        }
    }
    assert_eq!(world.flakes.len(), 16_000);
    (world, scene)
}

#[test]
fn eight_displays_cannot_admit_more_than_sixteen_thousand_live_bodies() {
    let mut world = World::new(desktop(8), 601);
    for i in 0..16_001 {
        let x = (i % 8) as f32 * 800.0 + 100.0;
        world.release(Flake::new(Point::new(x, 100.0), 1.0));
    }
    // Literal EN-03 bound, independent of the implementation's budget constants.
    assert!(
        world.flakes.len() <= 16_000,
        "admitted {} live bodies",
        world.flakes.len()
    );
    assert!(world.budget_snapshot().max_active_bodies <= 16_000);
    assert_eq!(world.removed_mass, 0.0);
    assert_eq!(world.introduced_mass, world.total_mass());
}

#[test]
fn all_eight_material_reserves_survive_ordinary_saturation_and_the_absolute_cap() {
    let (mut world, _) = full_eight_display_pool();
    let mass = world.total_mass();
    let introduced = world.introduced_mass;
    for screen in 0..8 {
        assert!(!world.release(priority(screen)));
        assert!(!world.release(Flake::new(
            Point::new(screen as f32 * 800.0 + 100.0, 100.0),
            1.0
        )));
        let count = world
            .flakes
            .iter()
            .filter(|b| b.skip_support == Some(1000 + screen))
            .count();
        assert_eq!(count, 128);
    }
    let budget = world.budget_snapshot();
    assert_eq!(
        (
            budget.active_bodies,
            budget.max_active_bodies,
            budget.global_max_active_bodies
        ),
        (16_000, 16_000, 16_000)
    );
    assert_eq!(budget.pending_bodies, 8);
    assert_eq!(world.total_mass(), mass);
    assert_eq!(world.introduced_mass, introduced);
    assert_eq!(world.removed_mass, 0.0);
    world.set_paused_monitors((1..=8).collect());
    world.step(&quiet(), None);
    assert_eq!(world.total_mass(), mass);
    assert_eq!(world.flakes.len(), 16_000);
    world.set_paused_monitors((1..=7).collect());
    world.step(&quiet(), None);
    assert_eq!(world.flakes.len(), 16_000);
    assert_eq!(world.budget_snapshot().pending_bodies, 8);
    // A natural exit frees one live slot; queued material gets it before new snow.
    let exiting = world
        .flakes
        .iter_mut()
        .find(|b| b.skip_support.is_none() && b.position.x >= 5600.0)
        .unwrap();
    exiting.position.y = 1000.0;
    world.step(&quiet(), None);
    assert_eq!(world.flakes.len(), 15_999);
    world.step(&quiet(), None);
    assert_eq!(world.flakes.len(), 16_000);
    assert_eq!(world.budget_snapshot().pending_bodies, 7);
    assert_eq!(
        world
            .flakes
            .iter()
            .filter(|b| b.skip_support == Some(1007))
            .count(),
        129
    );
    conserved(&world);
}

#[test]
fn screen_shrink_empty_return_and_repeat_do_not_delete_retained_bodies_or_admit_extra_snow() {
    let (mut world, full) = full_eight_display_pool();
    world.set_paused_monitors((1..=8).collect());
    let physics = |world: &World| {
        world
            .flakes
            .iter()
            .map(|b| (b.position, b.velocity, b.mass, b.age, b.angle, b.spin))
            .collect::<Vec<_>>()
    };
    let before = physics(&world);
    let mass = world.total_mass();
    for scene in [desktop(1), Scene::default(), full.clone(), desktop(1), full] {
        world.set_scene(scene);
        world.step(&quiet(), None);
        assert_eq!(world.flakes.len(), 16_000);
        assert_eq!(physics(&world), before);
        assert_eq!(world.total_mass(), mass);
        assert_eq!(world.removed_mass, 0.0);
        let budget = world.budget_snapshot();
        assert_eq!(budget.global_max_active_bodies, 16_000);
        assert!(!world.release(priority(0)));
    }
    world.clear();
    assert!(world.flakes.is_empty());
    assert_eq!(world.budget_snapshot().pending_bodies, 0);
    conserved(&world);
}

#[test]
fn many_displays_keep_ordinary_snow_and_fair_material_reserves_inside_the_cap() {
    let (mut world, mut scene) = stocked(65);
    scene.windows.clear();
    world.set_scene(scene);
    fill_ordinary(&mut world, 65);
    assert_eq!(world.flakes.len(), 8005);
    for screen in 0..65 {
        assert!(world.flakes.iter().any(|b| b.skip_support.is_none()
            && b.position.x >= screen as f32 * 800.0
            && b.position.x < (screen + 1) as f32 * 800.0));
        for _ in 0..123 {
            assert!(world.release(priority(screen)));
        }
    }
    assert_eq!(world.flakes.len(), 16_000);
    assert!(!world.release(priority(64)));
    assert_eq!(world.removed_mass, 0.0);
    conserved(&world);
}

#[test]
fn edge_shedding_obeys_the_global_ordinary_budget_and_resumes_when_room_returns() {
    let (mut world, _) = stocked(8);
    for drift in world.drifts.values_mut().filter(|d| !d.support.ground) {
        drift.mass.fill(20.0 * drift.cell_width);
    }
    world.introduced_mass = world.total_mass();
    fill_ordinary(&mut world, 8);
    assert_eq!(world.flakes.len(), 14_976);
    let roof_mass = |world: &World| {
        world
            .drifts
            .values()
            .filter(|d| !d.support.ground)
            .map(|d| d.total_mass())
            .sum::<f64>()
    };
    let before = roof_mass(&world);
    for _ in 0..60 {
        world.step(&quiet(), None);
    }
    assert_eq!(roof_mass(&world), before);
    assert_eq!(world.budget_snapshot().spill_bodies, 0);
    assert!(world.flakes.iter().all(|b| b.slab.is_none()));
    for body in &mut world.flakes {
        body.position.y = 1000.0;
    }
    world.step(&quiet(), None);
    world.step(&quiet(), None);
    assert!(world.flakes.iter().any(|b| b.slab.is_some()));
    assert!(world.flakes.len() <= 16_000);
    assert!(roof_mass(&world) < before);
    conserved(&world);
}
