use snowflakes_client::{
    geometry::{Rect, Scene, Screen, Window},
    quality::{FrameWork, Governor, RenderQuality, active_output_pixels},
    snow::World,
    timing::StepClock,
};
use snowflakes_protocol::Settings;
use std::{
    collections::BTreeSet,
    time::{Duration, Instant},
};

#[test]
fn adaptive_quality_transitions_preserve_the_actual_snow_world_and_fixed_step_budget() {
    fn run(adaptive: bool) -> (World, Settings, u32, u32) {
        let scene = Scene {
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
            windows: vec![Window {
                key: 7,
                rect: Rect {
                    left: 100.0,
                    top: 300.0,
                    right: 700.0,
                    bottom: 500.0,
                },
                visible: true,
                collect: true,
            }],
        };
        let pixels = active_output_pixels(&scene, &BTreeSet::new(), true);
        let mut world = World::new(scene, 44);
        let mut settings = Settings::default();
        settings.snow.intensity_percent = 60;
        settings.snow.wind_percent = 20;
        world.seed_air(&settings);
        let start = Instant::now();
        let mut now = start;
        let end = start + Duration::from_secs(20);
        let mut clock = StepClock::default();
        clock.advance(now, true);
        let mut governor = Governor::default();
        let mut reductions = 0;
        let mut recoveries = 0;
        let mut previous = 60;
        loop {
            let quality = if adaptive {
                governor.select(&settings, false, pixels, true)
            } else {
                RenderQuality::for_settings(&settings, false)
            };
            if quality.target_fps != previous {
                if quality.target_fps == 30 {
                    reductions += 1;
                } else {
                    recoveries += 1;
                }
                previous = quality.target_fps;
            }
            let steps = clock.advance(now, true);
            assert!(steps.discarded.is_zero() && steps.count <= 5);
            for _ in 0..steps.count {
                world.step(&settings, None);
            }
            if now == end {
                break;
            }
            let elapsed = now.duration_since(start);
            // Managed render costs, not a wall-performance benchmark. Slow work
            // affects cadence but must not alter a single physical operation.
            let work = Duration::from_millis(
                if (Duration::from_secs(4)..Duration::from_secs(10)).contains(&elapsed) {
                    24
                } else {
                    5
                },
            );
            let completed = now + work;
            let next_quality = if adaptive {
                governor.complete(
                    &settings,
                    false,
                    pixels,
                    FrameWork {
                        completed,
                        work,
                        accepted: true,
                        reconfigured: false,
                        active: true,
                    },
                )
            } else {
                quality
            };
            now = (now + next_quality.interval())
                .max(completed + Duration::from_millis(1))
                .min(end);
        }
        assert_eq!(previous, 60);
        (world, settings, reductions, recoveries)
    }
    let (control, control_settings, _, _) = run(false);
    let (changed, changed_settings, reductions, recoveries) = run(true);
    assert_eq!((reductions, recoveries), (1, 1));
    assert_eq!(control_settings, changed_settings);
    assert!(!control.flakes.is_empty());
    assert!(
        control
            .drifts
            .values()
            .any(|drift| drift.total_mass() > 0.0)
    );
    assert_eq!(control.flakes.len(), changed.flakes.len());
    for (a, b) in control.flakes.iter().zip(&changed.flakes) {
        assert_eq!(
            (a.position, a.velocity, a.angle, a.radius, a.mass),
            (b.position, b.velocity, b.angle, b.radius, b.mass)
        );
    }
    for (key, a) in &control.drifts {
        let b = &changed.drifts[key];
        assert_eq!(a.mass, b.mass);
        assert_eq!(a.pending, b.pending);
    }
    assert_eq!(control.total_mass(), changed.total_mass());
    assert_eq!(control.introduced_mass, changed.introduced_mass);
    assert_eq!(control.removed_mass, changed.removed_mass);
}
