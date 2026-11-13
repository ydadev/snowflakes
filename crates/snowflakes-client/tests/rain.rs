use snowflakes_client::{
    geometry::{Point, Rect, Scene, Screen, Window},
    quality::RenderQuality,
    rain::{Drop, MAX_DROPS, MAX_SPLASHES, Splash, World},
    timing::StepClock,
};
use snowflakes_protocol::{Settings, WeatherMode};
use std::time::{Duration, Instant};

fn scene() -> Scene {
    Scene {
        screens: vec![Screen {
            key: 1,
            persistent_id: true,
            rect: Rect {
                left: 0.0,
                top: 0.0,
                right: 1920.0,
                bottom: 1080.0,
            },
            floor: 1080.0,
            scale: 1.0,
        }],
        windows: vec![],
    }
}
fn settings(intensity: u8) -> Settings {
    let mut s = Settings::default();
    s.weather.mode = WeatherMode::Rain;
    s.rain.intensity_percent = intensity;
    s
}
fn quality(s: &Settings) -> RenderQuality {
    RenderQuality::for_settings(s, false)
}
fn balanced(world: &World) {
    let accounted = world.airborne_volume() + world.impacted_volume + world.removed_volume;
    assert!(
        (world.introduced_volume - accounted).abs() < 0.00001 * world.introduced_volume.max(1.0)
    );
    assert!(world.drops.len() <= MAX_DROPS);
    assert!(world.splashes.len() <= MAX_SPLASHES);
}

#[test]
fn intensity_is_proportional_to_logical_width_and_keeps_size_and_speed_distribution() {
    let mut counts = Vec::new();
    let mut shapes = Vec::new();
    for intensity in [0, 25, 50, 100] {
        for scale in [1.0, 2.0] {
            let mut desktop = scene();
            let screen = &mut desktop.screens[0];
            screen.rect.right *= scale;
            screen.rect.bottom *= scale;
            screen.floor *= scale;
            screen.scale = scale;
            let mut world = World::new(desktop, 83);
            let s = settings(intensity);
            for _ in 0..30 {
                world.step(&s, quality(&s));
            }
            assert_eq!(world.suppressed_drops, 0);
            assert_eq!(world.impacted_volume, 0.0);
            counts.push(world.drops.len());
            if intensity > 0 {
                shapes.push(
                    world
                        .drops
                        .iter()
                        .take(20)
                        .map(|d| (d.width, d.length, d.terminal_speed, d.volume))
                        .collect::<Vec<_>>(),
                );
            }
            balanced(&world);
        }
    }
    assert_eq!(counts, [0, 0, 60, 60, 120, 120, 240, 240]);
    assert!(shapes.iter().all(|s| s == &shapes[0]));
    assert_ne!(shapes[0][0], shapes[0][1]);
}

#[test]
fn swept_contacts_choose_the_first_exposed_ledge_and_emit_each_volume_once() {
    let mut desktop = scene();
    desktop.windows = vec![
        Window {
            key: 11,
            visible: true,
            collect: true,
            rect: Rect {
                left: 350.0,
                top: 150.0,
                right: 500.0,
                bottom: 450.0,
            },
        },
        Window {
            key: 10,
            visible: true,
            collect: true,
            rect: Rect {
                left: 100.0,
                top: 200.0,
                right: 700.0,
                bottom: 500.0,
            },
        },
    ];
    let mut world = World::new(desktop, 1);
    for (x, y, volume) in [(200.0, 190.0, 2.0), (400.0, 100.0, 3.0)] {
        let mut drop = Drop::new(Point::new(x, y));
        drop.velocity.y = 20_000.0;
        drop.volume = volume;
        assert!(world.release(drop));
    }
    let s = settings(0);
    world.step(&s, quality(&s));
    assert_eq!(
        world.impacts.iter().map(|i| i.support).collect::<Vec<_>>(),
        [10, 11]
    );
    assert_eq!(world.impacted_volume, 5.0);
    assert_eq!(world.splashes.len(), 6);
    assert!(world.drops.is_empty());
    balanced(&world);
    world.step(&s, quality(&s));
    assert!(world.impacts.is_empty());
    assert_eq!(world.impacted_volume, 5.0);
    balanced(&world);
}

#[test]
fn occluded_and_cloaked_ledges_do_not_catch_rain() {
    let mut desktop = scene();
    desktop.windows = vec![
        Window {
            key: 20,
            visible: true,
            collect: false,
            rect: Rect {
                left: 100.0,
                top: 100.0,
                right: 500.0,
                bottom: 400.0,
            },
        },
        Window {
            key: 10,
            visible: true,
            collect: true,
            rect: Rect {
                left: 120.0,
                top: 200.0,
                right: 480.0,
                bottom: 500.0,
            },
        },
        Window {
            key: 11,
            visible: false,
            collect: true,
            rect: Rect {
                left: 550.0,
                top: 200.0,
                right: 800.0,
                bottom: 500.0,
            },
        },
    ];
    let mut world = World::new(desktop, 1);
    for x in [250.0, 650.0] {
        world.release(Drop::new(Point::new(x, 195.0)));
    }
    let s = settings(0);
    world.step(&s, quality(&s));
    assert_eq!(world.drops.len(), 2);
    assert!(world.impacts.is_empty());
    for _ in 0..120 {
        world.step(&s, quality(&s));
    }
    assert!(world.drops.is_empty());
    assert_eq!(world.impacted_volume, 2.0);
    balanced(&world);
}

#[test]
fn wind_and_dpi_change_motion_in_physical_pixels_without_changing_volume() {
    let mut states = Vec::new();
    for scale in [1.0, 2.0] {
        let mut desktop = scene();
        desktop.screens[0].scale = scale;
        desktop.screens[0].rect.right *= scale;
        desktop.screens[0].rect.bottom *= scale;
        desktop.screens[0].floor *= scale;
        let mut world = World::new(desktop, 4);
        world.release(Drop::new(Point::new(500.0 * scale, 100.0 * scale)));
        let mut s = settings(0);
        s.rain.wind_percent = 100;
        for _ in 0..30 {
            world.step(&s, quality(&s));
        }
        let drop = &world.drops[0];
        assert!(drop.velocity.x > 200.0 && drop.velocity.x < 270.0);
        assert!(drop.velocity.y > 620.0);
        states.push((
            drop.position.x / scale,
            drop.position.y / scale,
            drop.velocity,
        ));
        balanced(&world);
    }
    assert_eq!(states[0], states[1]);
}

#[test]
fn render_frequency_and_spray_quality_do_not_change_physical_rain() {
    fn run(fps: u64, economy: bool) -> World {
        let mut world = World::new(scene(), 91);
        let mut s = settings(65);
        s.rain.wind_percent = -40;
        let q = RenderQuality::for_settings(&s, economy);
        let start = Instant::now();
        let mut clock = StepClock::default();
        clock.advance(start, true);
        for frame in 1..=fps * 12 {
            for _ in 0..clock
                .advance(
                    start + Duration::from_nanos(frame * 1_000_000_000 / fps),
                    true,
                )
                .count
            {
                world.step(&s, q);
            }
        }
        world
    }
    let full = run(60, false);
    let economy = run(30, true);
    assert_eq!(full.drops, economy.drops);
    assert_eq!(full.impacts, economy.impacts);
    assert_eq!(full.introduced_volume, economy.introduced_volume);
    assert_eq!(full.impacted_volume, economy.impacted_volume);
    assert_eq!(full.removed_volume, economy.removed_volume);
    assert!(!economy.splashes.is_empty());
    assert!(economy.splashes.len() < full.splashes.len());
    balanced(&full);
    balanced(&economy);
}

fn two_screens() -> Scene {
    let mut desktop = scene();
    desktop.screens[0].rect.right = 800.0;
    let mut other = desktop.screens[0].clone();
    other.key = 2;
    other.rect.left = 800.0;
    other.rect.right = 1600.0;
    desktop.screens.push(other);
    desktop
}

#[test]
fn paused_screens_freeze_drops_and_spray_without_emission_debt() {
    let mut world = World::new(two_screens(), 1);
    let left = Drop::new(Point::new(100.0, 100.0));
    world.release(left.clone());
    world.release(Drop::new(Point::new(1000.0, 100.0)));
    let spray = Splash {
        position: Point::new(150.0, 400.0),
        velocity: Point::new(20.0, -50.0),
        age: 0.0,
        lifetime: 0.4,
    };
    world.splashes.push(spray.clone());
    world.set_paused_monitors([1].into());
    let s = settings(50);
    for _ in 0..60 {
        world.step(&s, quality(&s));
    }
    assert_eq!(world.drops[0], left);
    assert_eq!(world.splashes[0], spray);
    assert_eq!(
        world.drops.iter().filter(|d| d.position.x < 800.0).count(),
        1
    );
    let before = world.drops.clone();
    world.set_paused_monitors([1, 2].into());
    for _ in 0..600 {
        world.step(&s, quality(&s));
    }
    assert_eq!(world.drops, before);
    world.set_paused_monitors(Default::default());
    world.step(&s, quality(&s));
    assert!(world.drops.iter().filter(|d| d.position.x < 800.0).count() <= 3);
    assert_ne!(world.drops[0], left);
    balanced(&world);
}

#[test]
fn crossing_into_a_paused_screen_waits_at_the_boundary() {
    let mut world = World::new(two_screens(), 2);
    let mut drop = Drop::new(Point::new(799.0, 100.0));
    drop.velocity.x = 270.0;
    world.release(drop.clone());
    world.set_paused_monitors([2].into());
    let mut s = settings(0);
    s.rain.wind_percent = 100;
    world.step(&s, quality(&s));
    assert_eq!(world.drops, [drop]);
    world.set_paused_monitors(Default::default());
    world.step(&s, quality(&s));
    assert!(world.drops[0].position.x > 800.0);
    balanced(&world);
}

#[test]
fn budgets_are_bounded_and_zero_intensity_never_replays_suppressed_rain() {
    let mut world = World::new(scene(), 3);
    for _ in 0..MAX_DROPS {
        assert!(world.release(Drop::new(Point::new(500.0, 1078.0))));
    }
    assert!(!world.release(Drop::new(Point::new(500.0, 1078.0))));
    let mut s = settings(100);
    world.step(&s, quality(&s));
    assert_eq!(world.suppressed_drops, 8);
    assert_eq!(world.splashes.len(), MAX_SPLASHES);
    assert_eq!(world.impacts.len(), MAX_DROPS);
    assert_eq!(world.impacted_volume, MAX_DROPS as f64);
    s.rain.intensity_percent = 0;
    for _ in 0..120 {
        world.step(&s, quality(&s));
    }
    assert!(world.drops.is_empty() && world.splashes.is_empty() && world.impacts.is_empty());
    assert_eq!(world.introduced_volume, MAX_DROPS as f64);
    balanced(&world);
}

#[test]
fn ground_motion_topology_and_clear_keep_the_volume_accounted_for() {
    let mut desktop = scene();
    desktop.screens[0].floor = 900.0;
    let mut world = World::new(desktop.clone(), 9);
    desktop.screens[0].floor = 800.0;
    world.set_scene(desktop.clone());
    world.release(Drop::new(Point::new(500.0, 890.0)));
    let s = settings(0);
    world.step(&s, quality(&s));
    assert_eq!(world.impacts.len(), 1);
    assert!(world.impacts[0].position.y >= 894.0 && world.impacts[0].position.y < 900.0);
    desktop.screens[0].floor = 1020.0;
    world.set_scene(desktop);
    for _ in 0..120 {
        world.step(&s, quality(&s));
    }
    world.release(Drop::new(Point::new(500.0, 1018.0)));
    world.step(&s, quality(&s));
    assert_eq!(world.impacts.len(), 1);
    assert!((world.impacts[0].position.y - 1020.0).abs() < 0.1);
    world.release(Drop::new(Point::new(500.0, 100.0)));
    world.clear();
    assert!(world.drops.is_empty() && world.splashes.is_empty() && world.impacts.is_empty());
    balanced(&world);
    world.release(Drop::new(Point::new(500.0, 100.0)));
    world.set_scene(Scene::default());
    assert!(world.drops.is_empty());
    balanced(&world);
}
