use super::*;
use crate::{frame_metrics::Capture, geometry::Screen};

fn settings() -> Settings {
    Settings::default()
}
fn rect(left: f32, top: f32, width: f32, height: f32) -> Rect {
    Rect {
        left,
        top,
        right: left + width,
        bottom: top + height,
    }
}
fn full() -> RenderQuality {
    RenderQuality {
        target_fps: 60,
        secondary_details: true,
    }
}
fn reduced() -> RenderQuality {
    RenderQuality {
        target_fps: 30,
        secondary_details: false,
    }
}
fn ready() -> (Governor, Instant) {
    let mut governor = Governor::default();
    assert_eq!(
        governor.select(&settings(), false, 1920 * 1080, true),
        full()
    );
    let start = Instant::now();
    governor.observe(start, Duration::ZERO, true);
    (governor, start)
}
fn run(
    governor: &mut Governor,
    mut at: Instant,
    count: u32,
    interval_ms: u64,
    work_ms: u64,
) -> Instant {
    for _ in 0..count {
        at += Duration::from_millis(interval_ms);
        governor.observe(at, Duration::from_millis(work_ms), true);
    }
    at
}

#[test]
fn adaptive_quality_pixel_pressure_counts_active_physical_targets_and_uses_hysteresis() {
    let scene = Scene {
        screens: vec![
            Screen {
                key: 1,
                persistent_id: true,
                rect: rect(-3840.0, 0.0, 3840.0, 2160.0),
                floor: 2100.0,
                scale: 1.0,
            },
            Screen {
                key: 2,
                persistent_id: true,
                rect: rect(200.0, -1000.0, 2160.0, 3840.0),
                floor: 2800.0,
                scale: 2.5,
            },
        ],
        windows: vec![],
    };
    let excluded = BTreeSet::from([2]);
    assert_eq!(active_output_pixels(&scene, &excluded, false), 16_588_800);
    assert_eq!(active_output_pixels(&scene, &excluded, true), 8_294_400);
    assert_eq!(
        active_output_pixels(
            &Scene {
                screens: vec![scene.screens[1].clone()],
                windows: vec![]
            },
            &BTreeSet::new(),
            true
        ),
        8_294_400
    );
    assert_eq!(output_pixels([rect(0.0, 0.0, 10.2, 10.2)]), 105);
    assert_eq!(output_pixels([rect(0.0, 0.0, -5.0, 10.0)]), 0);
    assert_eq!(
        output_pixels([rect(0.0, 0.0, f32::INFINITY, 1.0)]),
        u64::MAX
    );
    assert_eq!(
        output_pixels([rect(0.0, 0.0, f32::MAX, f32::MAX); 3]),
        u64::MAX
    );
    let mut governor = Governor::default();
    let original = settings();
    assert_eq!(governor.select(&original, false, 16_588_799, true), full());
    assert_eq!(
        governor.select(&original, false, 16_588_800, true),
        reduced()
    );
    assert_eq!(
        governor.select(&original, false, 14_000_000, true),
        reduced()
    );
    assert_eq!(governor.select(&original, false, 12_441_600, true), full());
    assert_eq!(original, settings());
}

#[test]
fn adaptive_quality_requires_sustained_work_and_a_longer_recovery_margin() {
    let (mut governor, start) = ready();
    let at = run(&mut governor, start, 39, 25, 20);
    assert_eq!(governor.current(), Some(full()));
    let at = run(&mut governor, at, 1, 25, 20);
    assert_eq!(governor.current(), Some(reduced()));
    let at = run(&mut governor, at, 99, 50, 5);
    assert_eq!(governor.current(), Some(reduced()));
    run(&mut governor, at, 1, 50, 5);
    assert_eq!(governor.current(), Some(full()));
}

#[test]
fn adaptive_quality_interruptions_and_idle_gaps_cannot_promote_or_accumulate_pressure() {
    let (mut governor, start) = ready();
    let at = run(&mut governor, start, 20, 25, 20);
    governor.observe(
        at + Duration::from_millis(25),
        Duration::from_millis(20),
        false,
    );
    let at = run(&mut governor, at + Duration::from_millis(25), 40, 25, 20);
    assert_eq!(governor.current(), Some(full())); // First new observation only anchors time.
    let at = run(&mut governor, at, 1, 25, 20);
    assert_eq!(governor.current(), Some(reduced()));
    let at = run(&mut governor, at, 90, 50, 5);
    governor.observe(at + Duration::from_secs(60), Duration::from_millis(5), true);
    assert_eq!(governor.current(), Some(reduced()));
    let at = run(&mut governor, at + Duration::from_secs(60), 99, 50, 5);
    assert_eq!(governor.current(), Some(reduced()));
    run(&mut governor, at, 1, 50, 5);
    assert_eq!(governor.current(), Some(full()));
    assert_eq!(governor.select(&settings(), false, u64::MAX, false), full());
    assert!(governor.current().is_none());
}

#[test]
fn adaptive_quality_respects_battery_requested_fps_and_the_held_rain_path() {
    let mut governor = Governor::default();
    let mut settings = settings();
    assert_eq!(
        governor.select(&settings, true, 1920 * 1080, true),
        reduced()
    );
    let start = Instant::now();
    run(&mut governor, start, 200, 50, 5);
    assert_eq!(governor.current(), Some(reduced()));
    assert_eq!(governor.select(&settings, false, 1920 * 1080, true), full());
    settings.performance.target_fps = 30;
    let requested = RenderQuality {
        target_fps: 30,
        secondary_details: true,
    };
    assert_eq!(
        governor.select(&settings, false, 1920 * 1080, true),
        requested
    );
    governor.observe(start, Duration::ZERO, true);
    let at = run(&mut governor, start, 10, 100, 70);
    assert_eq!(governor.current(), Some(reduced()));
    run(&mut governor, at, 100, 50, 5);
    assert_eq!(governor.current(), Some(requested));
    settings.weather.mode = WeatherMode::Rain;
    assert_eq!(governor.select(&settings, false, u64::MAX, true), requested);
    assert!(governor.current().is_none());
    assert_eq!(settings.performance.target_fps, 30);
}

#[test]
fn adaptive_quality_real_slow_work_is_bounded_and_invalid_clock_samples_are_discarded() {
    let (mut governor, start) = ready();
    let at = run(&mut governor, start, 9, 501, 500);
    assert_eq!(governor.current(), Some(full()));
    let at = run(&mut governor, at, 1, 501, 500);
    assert_eq!(governor.current(), Some(reduced()));
    governor.observe(at, Duration::ZERO, true); // Zero elapsed.
    governor.observe(start, Duration::ZERO, true); // Clock moved backwards.
    governor.observe(
        start + Duration::from_millis(1),
        Duration::from_millis(2),
        true,
    );
    assert_eq!(governor.current(), Some(reduced()));
    let at = run(&mut governor, start + Duration::from_millis(1), 99, 50, 5);
    assert_eq!(governor.current(), Some(reduced()));
    run(&mut governor, at, 1, 50, 5);
    assert_eq!(governor.current(), Some(full()));
}

#[test]
fn adaptive_quality_context_changes_drop_old_pressure_and_never_raise_requested_quality() {
    let (mut governor, start) = ready();
    let at = run(&mut governor, start, 39, 25, 20);
    governor.select(&settings(), false, 1920 * 1080 + 1, true);
    run(&mut governor, at, 39, 25, 20);
    assert_eq!(governor.current(), Some(full()));
    governor.reset();
    assert!(governor.current().is_none());
    assert_eq!(
        governor.select(&settings(), false, 1920 * 1080, true),
        full()
    );
    let mut user = settings();
    user.performance.target_fps = 30;
    user.performance.battery_saver = false;
    let original = user.clone();
    assert_eq!(governor.select(&user, true, 16_588_800, true), reduced());
    assert_eq!(governor.select(&user, false, 0, true).target_fps, 30);
    assert_eq!(user, original);
}

#[test]
fn adaptive_quality_capture_epochs_follow_actual_quality_without_editing_settings() {
    let settings = settings();
    let original = settings.clone();
    let mut governor = Governor::default();
    let mut capture = Capture::default();
    for (pixels, changed, epoch) in [
        (1920 * 1080, true, 1),
        (1920 * 1080, false, 1),
        (16_588_800, true, 2),
        (16_588_800, false, 2),
        (8_294_400, true, 3),
    ] {
        let quality = governor.select(&settings, false, pixels, true);
        assert!(capture.observe(Some(&settings), Some(quality), 0, true, false) == changed);
        assert_eq!(capture.epoch, epoch);
    }
    assert_eq!(settings, original);
}

#[test]
fn adaptive_quality_completed_frames_exclude_initialization_and_keep_the_rendered_snapshot() {
    let settings = settings();
    let (mut governor, start) = ready();
    let rendered = governor.select(&settings, false, 1920 * 1080, true);
    let next = governor.complete(
        &settings,
        false,
        16_588_800,
        FrameWork {
            completed: start + Duration::from_secs(3),
            work: Duration::from_secs(3),
            accepted: true,
            reconfigured: true,
            active: true,
        },
    );
    assert_eq!(rendered, full());
    assert_eq!(next, reduced());
    assert_eq!(governor.select(&settings, false, 16_588_800, true), next);
    let mut capture = Capture::default();
    assert!(capture.observe(Some(&settings), Some(rendered), 0, true, false));
    assert_eq!(capture.epoch, 1);
    assert!(capture.observe(Some(&settings), Some(next), 0, true, false));
    assert_eq!(capture.epoch, 2);
    let next = governor.complete(
        &settings,
        false,
        1920 * 1080,
        FrameWork {
            completed: start + Duration::from_secs(4),
            work: Duration::from_secs(1),
            accepted: true,
            reconfigured: true,
            active: true,
        },
    );
    assert_eq!(next, full());
    // Initialization was excluded; accepted slow evidence starts from scratch.
    let at = run(&mut governor, start + Duration::from_secs(4), 40, 25, 20);
    assert_eq!(governor.current(), Some(full()));
    let at = run(&mut governor, at, 1, 25, 20);
    assert_eq!(governor.current(), Some(reduced()));
    assert_eq!(
        governor.complete(
            &settings,
            false,
            1920 * 1080,
            FrameWork {
                completed: at + Duration::from_secs(60),
                work: Duration::from_millis(1),
                accepted: false,
                reconfigured: false,
                active: false,
            }
        ),
        full()
    );
    assert!(governor.current().is_none());
}

#[test]
fn adaptive_quality_isolated_spikes_and_neutral_cost_do_not_flip_modes() {
    let (mut governor, start) = ready();
    let at = run(&mut governor, start, 1, 25, 20);
    let at = run(&mut governor, at, 100, 20, 15);
    assert_eq!(governor.current(), Some(full()));
    let at = run(&mut governor, at, 20, 25, 20);
    let at = run(&mut governor, at, 1, 20, 15);
    let at = run(&mut governor, at, 20, 25, 20);
    assert_eq!(governor.current(), Some(full()));
    let at = run(&mut governor, at, 20, 25, 20);
    assert_eq!(governor.current(), Some(reduced()));
    run(&mut governor, at, 200, 50, 10);
    assert_eq!(governor.current(), Some(reduced()));
}
