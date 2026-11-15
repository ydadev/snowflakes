//! Managed input and times; no GetCursorPos, layers, GPU or input injection.
use super::*;
use crate::timing::{DISCONTINUITY, STEP};

fn weather() -> Weather {
    // The ordinary constructor registers the class/WinEvent hooks. No windows
    // are created, and native scene readiness is never overridden.
    let mut weather = Weather::new(false).unwrap();
    weather.world = World::new(
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
        },
        902,
    );
    for x in [210.0, 240.0, 280.0] {
        assert!(weather.world.release(Flake::new(Point::new(x, 100.0), 3.0)));
    }
    weather
}

#[test]
fn cursor_cadence_keeps_moving_input_and_world_equal_at_thirty_and_sixty_presentation_fps() {
    let _serial = tests::WEATHER_TEST.lock().unwrap();
    fn run(fps: u8, use_input: bool) -> (String, u32, u32) {
        let mut weather = weather();
        let mut settings = Settings::default();
        settings.snow.intensity_percent = 0;
        settings.performance.target_fps = fps;
        let original = settings.clone();
        let quality = RenderQuality::for_settings(&settings, false);
        let start = Instant::now();
        let end = start + STEP * 120;
        weather.next = start;
        let mut now = start;
        let mut samples = 0;
        let mut presentations = 0;
        loop {
            if now >= weather.next {
                presentations += 1;
                weather.next = now + quality.interval();
            }
            weather.advance_models_sampled(now, true, &settings, quality, |weather| {
                samples += 1;
                let position = Point::new(180.0 + (samples % 60) as f32 * 4.0, 100.0);
                let cursor = weather.cursor_sample(Ok((position, start + STEP * samples)));
                if use_input { cursor } else { None }
            });
            if now == end {
                break;
            }
            now = weather.work_deadline().min(end);
        }
        assert_eq!(settings, original);
        (format!("{:?}", weather.world), samples, presentations)
    }
    let low = run(30, true);
    let high = run(60, true);
    let control = run(60, false);
    assert_eq!(low.1, 120, "input cadence followed presentation FPS");
    assert_eq!(high.1, 120);
    assert!(low.2 <= 62 && high.2 >= 119);
    assert_eq!(
        low.0, high.0,
        "moving input changed physical state with FPS"
    );
    assert_ne!(
        high.0, control.0,
        "the controlled cursor never affected snow"
    );
}

#[test]
fn cursor_cadence_samples_each_bounded_catchup_step_without_replaying_one_velocity() {
    let _serial = tests::WEATHER_TEST.lock().unwrap();
    let mut weather = weather();
    let settings = Settings::default();
    let quality = RenderQuality::for_settings(&settings, false);
    let start = Instant::now();
    weather.clock.advance(start, true);
    let mut samples = 0;
    let mut velocities = Vec::new();
    weather.advance_models_sampled(
        start + STEP * 12 + STEP / 2,
        true,
        &settings,
        quality,
        |weather| {
            samples += 1;
            let cursor = weather
                .cursor_sample(Ok((
                    Point::new(200.0 + samples as f32, 100.0),
                    start + STEP * (20 + samples),
                )))
                .unwrap();
            velocities.push(cursor.velocity.x);
            Some(cursor)
        },
    );
    assert_eq!(samples, 5);
    assert_eq!(velocities[0], 0.0);
    assert!(velocities[1..].iter().all(|v| *v > 0.0));
    assert!((weather.clock.fraction() - 0.5).abs() < 0.000001);
    assert_eq!(weather.clock.deadline(), Some(start + STEP * 13));
}

#[test]
fn cursor_cadence_skips_substeps_and_inactive_time_and_resumes_without_old_velocity() {
    let _serial = tests::WEATHER_TEST.lock().unwrap();
    let mut weather = weather();
    let settings = Settings::default();
    let quality = RenderQuality::for_settings(&settings, false);
    let start = Instant::now();
    let mut samples = 0;
    for (now, running) in [
        (start, true),
        (start + STEP / 2, true),
        (start + STEP, false),
        (start + DISCONTINUITY, true),
    ] {
        weather.advance_models_sampled(now, running, &settings, quality, |_| {
            samples += 1;
            None
        });
    }
    assert_eq!(samples, 0, "inactive/substep updates sampled input");
    weather.cursor = Some((Point::new(-500.0, -500.0), start));
    weather.advance_models_sampled(start + DISCONTINUITY * 2, true, &settings, quality, |_| {
        samples += 1;
        None
    });
    assert_eq!(samples, 0);
    assert!(weather.cursor.is_none());
    weather.advance_models_sampled(
        start + DISCONTINUITY * 2 + STEP,
        true,
        &settings,
        quality,
        |weather| {
            samples += 1;
            let cursor = weather
                .cursor_sample(Ok((
                    Point::new(200.0, 100.0),
                    start + DISCONTINUITY * 2 + STEP,
                )))
                .unwrap();
            assert_eq!(cursor.velocity, Point::default());
            Some(cursor)
        },
    );
    assert_eq!(samples, 1);
}

#[test]
fn cursor_cadence_retains_intermediate_work_and_reconfiguration_until_presentation() {
    let _serial = tests::WEATHER_TEST.lock().unwrap();
    let mut weather = weather();
    weather.output_changed = true;
    assert_eq!(weather.record_work(Duration::from_millis(5), false), None);
    weather.output_changed = false;
    assert_eq!(weather.record_work(Duration::from_millis(7), false), None);
    assert_eq!(
        weather.record_work(Duration::from_millis(11), true),
        Some((Duration::from_millis(23), true))
    );
    assert_eq!(
        weather.record_work(Duration::from_millis(2), true),
        Some((Duration::from_millis(2), false))
    );
    weather.output_changed = true;
    weather.record_work(Duration::from_millis(20), false);
    weather.reset_clock();
    weather.output_changed = false;
    assert_eq!(
        weather.record_work(Duration::from_millis(3), true),
        Some((Duration::from_millis(3), false))
    );
}
