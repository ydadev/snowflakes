use snowflakes_client::{
    session::{
        CACHE_LIFETIME, ExitReason, Limits, PauseReason, ProfileSource, REQUEST_TIMEOUT, Session,
        SessionError,
    },
    timing::{DISCONTINUITY, MAX_STEPS, STEP, StepClock},
};
use snowflakes_protocol::{Configuration, Settings, WeatherMode};
use std::time::{Duration, Instant};

fn profile() -> Configuration {
    Configuration::from_json(include_bytes!(
        "../../../protocol/fixtures/configuration.json"
    ))
    .unwrap()
}

fn accept(session: &mut Session, config: &Configuration, now: Instant) {
    let ticket = session.begin_sync(now).unwrap().unwrap();
    session
        .receive(ticket, &serde_json::to_vec(config).unwrap(), now)
        .unwrap();
}

#[test]
fn new_client_waits_and_local_mode_restores_its_own_values() {
    let now = Instant::now();
    let mut local = Settings::default();
    local.snow.intensity_percent = 17;
    let mut session = Session::new(local.clone()).unwrap();
    assert!(session.follows_server());
    assert_eq!(
        session.snapshot(now).source,
        ProfileSource::WaitingForServer
    );
    assert!(!session.snapshot(now).should_render);
    assert_eq!(
        session.edit_local(Settings::default(), now),
        Err(SessionError::LocalEditingUnavailable)
    );
    accept(&mut session, &profile(), now);
    assert_eq!(
        session
            .snapshot(now)
            .settings
            .unwrap()
            .snow
            .intensity_percent,
        35
    );
    assert_eq!(session.local_settings(), &local);

    session.set_follow_server(false, now).unwrap();
    assert_eq!(session.snapshot(now).settings.as_ref(), Some(&local));
    assert!(session.begin_sync(now).unwrap().is_none());
    local.rain.puddle_size_percent = 1;
    session.edit_local(local.clone(), now).unwrap();
    session.set_follow_server(true, now).unwrap();
    assert_eq!(session.snapshot(now).source, ProfileSource::Server);
    assert_eq!(session.local_settings(), &local);
    session.set_follow_server(false, now).unwrap();
    assert_eq!(session.snapshot(now).settings.as_ref(), Some(&local));
}

#[test]
fn mode_toggle_cancels_requests_and_late_results_cannot_consume_a_new_request() {
    let now = Instant::now();
    let mut session = Session::new(Settings::default()).unwrap();
    let old = session.begin_sync(now).unwrap().unwrap();
    assert!(session.begin_sync(now).unwrap().is_none());
    assert_eq!(session.set_follow_server(false, now).unwrap(), Some(old));
    session.set_follow_server(true, now).unwrap();
    let new = session.begin_sync(now).unwrap().unwrap();
    assert_ne!(old, new);
    let bytes = serde_json::to_vec(&profile()).unwrap();
    assert_eq!(
        session.receive(old, &bytes, now),
        Err(SessionError::StaleResponse)
    );
    assert!(!session.sync_failed(old));
    assert!(session.receive(new, &bytes, now).is_ok());
    assert_eq!(session.snapshot(now).source, ProfileSource::Server);
}

#[test]
fn ticket_from_another_session_is_rejected() {
    let now = Instant::now();
    let mut a = Session::new(Settings::default()).unwrap();
    let mut b = Session::new(Settings::default()).unwrap();
    let ticket_a = a.begin_sync(now).unwrap().unwrap();
    let ticket_b = b.begin_sync(now).unwrap().unwrap();
    let bytes = serde_json::to_vec(&profile()).unwrap();
    assert_eq!(
        b.receive(ticket_a, &bytes, now),
        Err(SessionError::StaleResponse)
    );
    b.receive(ticket_b, &bytes, now).unwrap();
}

#[test]
fn errors_and_old_revisions_never_refresh_the_cache() {
    let now = Instant::now();
    let mut session = Session::new(Settings::default()).unwrap();
    let mut config = profile();
    config.revision = 5;
    accept(&mut session, &config, now);
    let almost_expired = now + CACHE_LIFETIME - Duration::from_secs(1);
    for document in [
        b"{}".to_vec(),
        serde_json::to_vec(&Configuration {
            revision: 4,
            ..config.clone()
        })
        .unwrap(),
        serde_json::to_vec(&Configuration {
            global_enabled: false,
            ..config.clone()
        })
        .unwrap(),
    ] {
        let ticket = session.begin_sync(almost_expired).unwrap().unwrap();
        assert!(session.receive(ticket, &document, almost_expired).is_err());
        assert_eq!(session.server_configuration(), Some(&config));
    }
    let ticket = session.begin_sync(almost_expired).unwrap().unwrap();
    assert!(session.sync_failed(ticket));
    assert!(session.snapshot(almost_expired).should_render);
    let epoch = session.snapshot(almost_expired).clear_epoch;
    assert_eq!(session.tick(now + CACHE_LIFETIME), None);
    assert_eq!(
        session.snapshot(now + CACHE_LIFETIME).source,
        ProfileSource::WaitingForServer
    );
    assert!(!session.snapshot(now + CACHE_LIFETIME).should_render);
    assert_ne!(session.snapshot(now + CACHE_LIFETIME).clear_epoch, epoch);
    let expired_epoch = session.snapshot(now + CACHE_LIFETIME).clear_epoch;
    session.tick(now + CACHE_LIFETIME + Duration::from_secs(10));
    assert_eq!(
        session.snapshot(now + CACHE_LIFETIME).clear_epoch,
        expired_epoch
    );
    assert!(session.begin_sync(now + CACHE_LIFETIME).unwrap().is_some());
}

#[test]
fn matching_revision_refreshes_lifetime_and_new_generation_accepts_reset_revision() {
    let now = Instant::now();
    let mut session = Session::new(Settings::default()).unwrap();
    let mut config = profile();
    config.revision = 99;
    accept(&mut session, &config, now);
    accept(
        &mut session,
        &config,
        now + CACHE_LIFETIME - Duration::from_secs(1),
    );
    assert!(
        session
            .snapshot(now + CACHE_LIFETIME + Duration::from_secs(1))
            .should_render
    );
    config.generation_id = "00000000-0000-4000-8000-000000000001".into();
    config.revision = 1;
    accept(
        &mut session,
        &config,
        now + CACHE_LIFETIME + Duration::from_secs(2),
    );
    assert_eq!(session.server_configuration(), Some(&config));
}

#[test]
fn timeout_releases_the_slot_but_cannot_replace_valid_settings() {
    let now = Instant::now();
    let mut session = Session::new(Settings::default()).unwrap();
    accept(&mut session, &profile(), now);
    let ticket = session.begin_sync(now).unwrap().unwrap();
    assert_eq!(session.tick(now + REQUEST_TIMEOUT), Some(ticket));
    assert_eq!(
        session.receive(ticket, b"{}", now + REQUEST_TIMEOUT),
        Err(SessionError::StaleResponse)
    );
    assert!(session.snapshot(now + REQUEST_TIMEOUT).should_render);
    let next = session.begin_sync(now + REQUEST_TIMEOUT).unwrap().unwrap();
    assert_eq!(
        session.receive(next, b"{}", now + REQUEST_TIMEOUT * 2),
        Err(SessionError::RequestExpired)
    );
    assert!(session.snapshot(now + REQUEST_TIMEOUT * 2).should_render);
}

#[test]
fn clearing_and_pausing_are_independent_of_profile_and_polling() {
    let now = Instant::now();
    let mut session = Session::new(Settings::default()).unwrap();
    accept(&mut session, &profile(), now);
    let saved = session.server_configuration().unwrap().clone();
    let epoch = session.snapshot(now).clear_epoch;
    session.set_condition(PauseReason::Manual, true).unwrap();
    session.set_condition(PauseReason::Locked, true).unwrap();
    assert!(!session.snapshot(now).should_render);
    assert_eq!(session.snapshot(now).clear_epoch, epoch);
    session.set_condition(PauseReason::Locked, false).unwrap();
    assert_eq!(session.snapshot(now).pauses.len(), 1);
    assert!(!session.snapshot(now).should_render);
    session.clear();
    assert_ne!(session.snapshot(now).clear_epoch, epoch);
    assert_eq!(session.server_configuration(), Some(&saved));
    assert!(session.begin_sync(now).unwrap().is_some());
    session.set_condition(PauseReason::Manual, false).unwrap();
    assert!(session.snapshot(now).should_render);
}

#[test]
fn server_off_while_paused_clears_and_keeps_polling_and_pause() {
    let now = Instant::now();
    let mut session = Session::new(Settings::default()).unwrap();
    let mut config = profile();
    accept(&mut session, &config, now);
    session.set_condition(PauseReason::Manual, true).unwrap();
    let epoch = session.snapshot(now).clear_epoch;
    config.global_enabled = false;
    config.revision += 1;
    accept(&mut session, &config, now);
    let disabled_epoch = session.snapshot(now).clear_epoch;
    assert_ne!(disabled_epoch, epoch);
    assert!(!session.snapshot(now).effects_enabled);
    accept(&mut session, &config, now);
    assert_eq!(session.snapshot(now).clear_epoch, disabled_epoch);
    config.global_enabled = true;
    config.revision += 1;
    accept(&mut session, &config, now);
    assert!(session.snapshot(now).effects_enabled);
    assert!(!session.snapshot(now).should_render);
    session.set_condition(PauseReason::Manual, false).unwrap();
    assert!(session.snapshot(now).should_render);
    assert_eq!(session.snapshot(now).clear_epoch, disabled_epoch);
}

#[test]
fn local_disable_while_paused_clears_but_size_reduction_does_not() {
    let now = Instant::now();
    let mut session = Session::new(Settings::default()).unwrap();
    session.set_follow_server(false, now).unwrap();
    session.set_condition(PauseReason::Manual, true).unwrap();
    let epoch = session.snapshot(now).clear_epoch;
    let mut settings = session.local_settings().clone();
    settings.snow.drift_size_percent = 0;
    session.edit_local(settings.clone(), now).unwrap();
    assert_eq!(session.snapshot(now).clear_epoch, epoch);
    settings.effects_enabled = false;
    session.edit_local(settings, now).unwrap();
    assert_ne!(session.snapshot(now).clear_epoch, epoch);
}

#[test]
fn policy_limits_are_reversible_and_do_not_overwrite_either_profile() {
    let now = Instant::now();
    let mut session = Session::new(Settings::default()).unwrap();
    accept(&mut session, &profile(), now);
    let limits = Limits {
        max_intensity_percent: 5,
        max_drift_size_percent: 0,
        max_puddle_size_percent: 10,
        max_fps: 30,
        ..Default::default()
    };
    session.set_limits(limits, now).unwrap();
    let snapshot = session.snapshot(now);
    assert!(snapshot.limited);
    let effective = snapshot.settings.unwrap();
    assert_eq!(effective.snow.intensity_percent, 5);
    assert_eq!(effective.rain.intensity_percent, 5);
    assert_eq!(effective.snow.drift_size_percent, 0);
    assert_eq!(effective.rain.puddle_size_percent, 10);
    assert_eq!(effective.performance.target_fps, 30);
    assert_eq!(session.server_configuration().unwrap(), &profile());
    assert_eq!(session.local_settings(), &Settings::default());
    session.set_follow_server(false, now).unwrap();
    assert!(session.snapshot(now).limited);
    session.set_limits(Limits::default(), now).unwrap();
    assert!(!session.snapshot(now).limited);
    assert_eq!(session.snapshot(now).settings, Some(Settings::default()));
}

#[test]
fn mandatory_pauses_override_local_preferences_and_allowed_modes_never_switch_weather() {
    let now = Instant::now();
    let mut settings = Settings::default();
    settings.performance.pause_fullscreen = false;
    settings.performance.pause_remote_session = false;
    let mut session = Session::new(settings).unwrap();
    session.set_follow_server(false, now).unwrap();
    session
        .set_condition(PauseReason::RemoteSession, true)
        .unwrap();
    assert!(session.snapshot_for_monitor(now, true).should_render);
    session
        .set_limits(
            Limits {
                pause_on_fullscreen: true,
                allow_remote_session: false,
                ..Default::default()
            },
            now,
        )
        .unwrap();
    assert_eq!(session.snapshot_for_monitor(now, true).pauses.len(), 2);
    assert_eq!(session.snapshot_for_monitor(now, false).pauses.len(), 1);
    assert!(!session.snapshot(now).should_render);
    session
        .set_limits(
            Limits {
                allow_snow: false,
                ..Default::default()
            },
            now,
        )
        .unwrap();
    let snapshot = session.snapshot(now);
    assert!(!snapshot.effects_enabled);
    assert_eq!(snapshot.settings.unwrap().weather.mode, WeatherMode::Snow);
    assert!(session.local_settings().effects_enabled);
}

#[test]
fn forbidden_application_and_user_exit_cannot_be_revived_by_a_reply_or_policy_removal() {
    for reason in [ExitReason::Policy, ExitReason::User] {
        let now = Instant::now();
        let mut session = Session::new(Settings::default()).unwrap();
        let ticket = session.begin_sync(now).unwrap().unwrap();
        if reason == ExitReason::Policy {
            assert_eq!(
                session
                    .set_limits(
                        Limits {
                            enabled: false,
                            ..Default::default()
                        },
                        now
                    )
                    .unwrap(),
                Some(ticket)
            );
        } else {
            assert_eq!(session.close(reason), Some(ticket));
        }
        assert_eq!(session.snapshot(now).exit, Some(reason));
        assert!(!session.snapshot(now).should_render);
        assert!(session.begin_sync(now).unwrap().is_none());
        assert_eq!(
            session.receive(ticket, &serde_json::to_vec(&profile()).unwrap(), now),
            Err(SessionError::StaleResponse)
        );
        assert_eq!(
            session.set_limits(Limits::default(), now),
            Err(SessionError::Closed)
        );
        assert_eq!(
            session.set_follow_server(false, now),
            Err(SessionError::Closed)
        );
    }
}

#[test]
fn expiry_in_local_mode_does_not_clear_local_accumulation() {
    let now = Instant::now();
    let mut session = Session::new(Settings::default()).unwrap();
    accept(&mut session, &profile(), now);
    session.set_follow_server(false, now).unwrap();
    let epoch = session.snapshot(now).clear_epoch;
    session.tick(now + CACHE_LIFETIME);
    assert_eq!(session.snapshot(now + CACHE_LIFETIME).clear_epoch, epoch);
    assert!(session.snapshot(now + CACHE_LIFETIME).should_render);
    session
        .set_follow_server(true, now + CACHE_LIFETIME)
        .unwrap();
    assert!(!session.snapshot(now + CACHE_LIFETIME).should_render);
}

#[test]
fn invalid_local_values_and_policy_are_atomic() {
    let now = Instant::now();
    let mut session = Session::new(Settings::default()).unwrap();
    session.set_follow_server(false, now).unwrap();
    let original = session.snapshot(now);
    let mut settings = Settings::default();
    settings.rain.puddle_size_percent = 101;
    assert!(session.edit_local(settings, now).is_err());
    assert_eq!(
        session.set_limits(
            Limits {
                enabled: false,
                max_fps: 144,
                ..Default::default()
            },
            now
        ),
        Err(SessionError::InvalidLimits)
    );
    assert_eq!(session.snapshot(now), original);
}

#[test]
fn fixed_steps_match_at_30_and_60_fps() {
    let base = Instant::now();
    let mut totals = Vec::new();
    for fps in [30, 60] {
        let mut clock = StepClock::default();
        let mut total = 0;
        for frame in 0..=fps * 120 {
            let result = clock.advance(
                base + Duration::from_secs_f64(f64::from(frame) / f64::from(fps)),
                true,
            );
            total += result.count;
            assert_eq!(result.discarded, Duration::ZERO);
        }
        totals.push(total);
    }
    assert!(totals[0].abs_diff(totals[1]) <= 1);
    assert!(totals.iter().all(|&total| total.abs_diff(7200) <= 1));
}

#[test]
fn paused_and_sleeping_time_never_becomes_precipitation() {
    let base = Instant::now();
    let mut clock = StepClock::default();
    clock.advance(base, true);
    assert_eq!(clock.advance(base + STEP, true).count, 1);
    clock.advance(base + STEP * 2, false);
    assert_eq!(
        clock.advance(base + Duration::from_secs(3600), true).count,
        0
    );
    assert_eq!(
        clock
            .advance(base + Duration::from_secs(3600) + STEP, true)
            .count,
        1
    );
    let result = clock.advance(base + Duration::from_secs(7200), true);
    assert_eq!(result.count, 0);
    assert!(result.discarded >= DISCONTINUITY);
    assert_eq!(
        clock
            .advance(base + Duration::from_secs(7200) + STEP, true)
            .count,
        1
    );
}

#[test]
fn slow_frames_have_bounded_work_without_deferred_catchup() {
    let base = Instant::now();
    let mut clock = StepClock::default();
    clock.advance(base, true);
    let slow = base + STEP * 12;
    let result = clock.advance(slow, true);
    assert_eq!(result.count, MAX_STEPS);
    assert_eq!(result.discarded, STEP * (12 - MAX_STEPS));
    assert_eq!(clock.advance(slow + STEP, true).count, 1);
}

#[test]
fn fullscreen_only_pauses_its_monitor_and_cannot_become_a_global_condition() {
    let now = Instant::now();
    let mut session = Session::new(Settings::default()).unwrap();
    session.set_follow_server(false, now).unwrap();
    assert_eq!(
        session.set_condition(PauseReason::Fullscreen, true),
        Err(SessionError::MonitorScopedCondition)
    );
    assert!(!session.snapshot_for_monitor(now, true).should_render);
    assert!(session.snapshot_for_monitor(now, false).should_render);
    session.set_condition(PauseReason::Manual, true).unwrap();
    assert!(!session.snapshot_for_monitor(now, false).should_render);
    assert!(!session.snapshot_for_monitor(now, true).should_render);
    session.set_condition(PauseReason::Manual, false).unwrap();
    assert!(session.snapshot_for_monitor(now, false).should_render);
}

#[test]
fn mandatory_session_conditions_each_keep_rendering_stopped() {
    for reason in [
        PauseReason::Locked,
        PauseReason::Suspended,
        PauseReason::DisplaysOff,
        PauseReason::Disconnected,
        PauseReason::GraphicsUnavailable,
    ] {
        let now = Instant::now();
        let mut session = Session::new(Settings::default()).unwrap();
        session.set_follow_server(false, now).unwrap();
        session.set_condition(reason, true).unwrap();
        session.set_condition(PauseReason::Manual, true).unwrap();
        session.set_condition(PauseReason::Manual, false).unwrap();
        assert!(!session.snapshot(now).should_render);
        session.set_condition(reason, false).unwrap();
        assert!(session.snapshot(now).should_render);
    }
}
