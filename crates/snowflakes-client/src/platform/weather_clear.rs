//! Actual Clear command -> Controller status -> Weather, without creating a layer.
use super::*;
use crate::{
    control::{Command, Controller, Setting},
    geometry::Window,
    session::PauseReason,
    storage::{CacheClock, UserStore},
};

fn controller(dir: &std::path::Path, now: Instant) -> Controller {
    let mut controller = Controller::open(
        UserStore::at(dir),
        1,
        CacheClock {
            unix_ms: 2_000_000_000_000,
            uptime_ms: 100_000,
        },
        now,
    )
    .unwrap();
    assert_eq!(
        controller
            .handle(Command::FollowServer { enabled: false }, now)
            .code,
        0
    );
    controller
}

fn populated(weather: &mut Weather, settings: &Settings) {
    weather.world = World::new(
        Scene {
            screens: vec![Screen {
                key: 1,
                persistent_id: false,
                rect: Rect {
                    left: 0.0,
                    top: 0.0,
                    right: 800.0,
                    bottom: 600.0,
                },
                floor: 600.0,
                scale: 1.0,
            }],
            windows: vec![Window {
                key: 7,
                visible: true,
                collect: true,
                rect: Rect {
                    left: 100.0,
                    top: 300.0,
                    right: 700.0,
                    bottom: 500.0,
                },
            }],
        },
        103,
    );
    weather.world.seed_air(settings);
    for _ in 0..7 {
        weather.world.step(settings, None);
    }
    assert!(!weather.world.flakes.is_empty());
    for drift in weather.world.drifts.values_mut() {
        drift.mass.fill(3.25);
        drift.pending.fill(0.75);
    }
    weather.world.introduced_mass = weather.world.total_mass() + weather.world.removed_mass;
}

fn air(world: &World) -> Vec<[u32; 13]> {
    world
        .flakes
        .iter()
        .map(|b| {
            [
                b.position.x.to_bits(),
                b.position.y.to_bits(),
                b.velocity.x.to_bits(),
                b.velocity.y.to_bits(),
                b.radius.to_bits(),
                b.mass.to_bits(),
                b.angle.to_bits(),
                b.spin.to_bits(),
                b.age.to_bits(),
                b.opacity.to_bits(),
                b.shape.to_bits(),
                b.display_scale(1.0).to_bits(),
                u32::from(b.skip_support.is_some() || b.slab.is_some()),
            ]
        })
        .collect()
}

#[test]
fn actual_clear_command_removes_stock_and_preserves_precipitation_and_preferences() {
    let _serial = super::tests::WEATHER_TEST.lock().unwrap();
    for manual_pause in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let now = Instant::now();
        let mut controller = controller(dir.path(), now);
        if manual_pause {
            controller.handle(Command::Pause {}, now);
        }
        let status = controller.status(now);
        let settings = status.settings.as_ref().unwrap();
        let mut weather = Weather::new(true).unwrap();
        // No observer is invented: unavailable system events keep output suspended.
        let conditions = Conditions::default();
        weather.frame(&status, &conditions);
        populated(&mut weather, settings);
        let before = air(&weather.world);
        let mass = weather.world.total_mass();
        let saved = std::fs::read(dir.path().join("config.toml")).unwrap();
        assert_eq!(controller.handle(Command::Clear {}, now).code, 0);
        let cleared = controller.status(now);
        let report = weather.frame(&cleared, &conditions);
        assert!(
            air(&weather.world) == before,
            "Clear must preserve all {} ordinary snowfall states",
            before.len()
        );
        assert!(weather.world.drifts.values().all(|d| d.total_mass() == 0.0));
        assert!(weather.world.total_mass() < mass);
        assert!(
            (weather.world.introduced_mass
                - weather.world.removed_mass
                - weather.world.total_mass())
            .abs()
                < 1e-9
        );
        assert_eq!(cleared.settings, status.settings);
        assert_eq!(cleared.pauses, status.pauses);
        assert_eq!(cleared.pauses.contains(&PauseReason::Manual), manual_pause);
        assert_eq!(
            std::fs::read(dir.path().join("config.toml")).unwrap(),
            saved
        );
        assert!(!report.rendering && !report.submitted && weather.layers.is_empty());
        // Re-reading the same command token must not remove newly fallen stock.
        weather.world.drifts.values_mut().next().unwrap().pending[0] = 2.0;
        weather.world.introduced_mass += 2.0;
        weather.frame(&cleared, &conditions);
        assert_eq!(
            weather
                .world
                .drifts
                .values()
                .map(|d| d.total_mass())
                .sum::<f64>(),
            2.0
        );
        // Model time advances independently of the suspended native output.
        let introduced = weather.world.introduced_mass;
        for _ in 0..120 {
            weather.world.step(settings, None);
        }
        assert!(weather.world.introduced_mass > introduced);
        eprintln!(
            "clear-command: manual_pause={manual_pause} retained_air={} stock_removed=true same_token_preserves_new_stock=true emission_continues=true",
            before.len()
        );
    }
}

#[test]
fn full_reset_survives_coalesced_off_on_and_mode_changes_followed_by_clear() {
    let _serial = super::tests::WEATHER_TEST.lock().unwrap();
    for mode_change in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let now = Instant::now();
        let mut controller = controller(dir.path(), now);
        controller.handle(Command::Pause {}, now);
        let mut weather = Weather::new(false).unwrap();
        let conditions = Conditions::default();
        let status = controller.status(now);
        weather.frame(&status, &conditions);
        populated(&mut weather, status.settings.as_ref().unwrap());
        for setting in if mode_change {
            [
                Setting::Mode(WeatherMode::Rain),
                Setting::Mode(WeatherMode::Snow),
            ]
        } else {
            [
                Setting::EffectsEnabled(false),
                Setting::EffectsEnabled(true),
            ]
        } {
            assert_eq!(controller.handle(Command::Set { setting }, now).code, 0);
        }
        assert_eq!(controller.handle(Command::Clear {}, now).code, 0);
        let latest = controller.status(now);
        assert_eq!(latest.settings, status.settings);
        assert_ne!(latest.reset_epoch, status.reset_epoch);
        let report = weather.frame(&latest, &conditions);
        assert!(weather.world.flakes.is_empty());
        assert_eq!(weather.world.total_mass(), 0.0);
        assert_eq!(weather.world.introduced_mass, weather.world.removed_mass);
        assert!(!report.rendering && !report.submitted && weather.layers.is_empty());
        assert_eq!(latest.pauses, [PauseReason::Manual]);
        // A later manual clear uses the narrower action again.
        weather
            .world
            .release(Flake::new(Point::new(20.0, 20.0), 2.0));
        controller.handle(Command::Clear {}, now);
        weather.frame(&controller.status(now), &conditions);
        assert_eq!(weather.world.flakes.len(), 1);
    }
}
