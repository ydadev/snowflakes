use snowflakes_client::{
    control::{Command, Setting},
    ui::{CONTENT_HEIGHT, Pending, scale, scroll_limit},
};

#[test]
fn trace_tokens_follow_coalesced_values_without_crossing_source_barriers() {
    use snowflakes_client::{diagnostics::Recorder, ui::Queued};
    let dir = tempfile::tempdir().unwrap();
    let recorder = Recorder::open(&dir.path().join("trace.ndjson")).unwrap();
    let mut pending = Pending::default();
    let mut ids = Vec::new();
    for value in [11, 22] {
        let command = Command::Set {
            setting: Setting::SnowIntensity(value),
        };
        let trace = recorder.input(1);
        ids.push(trace.id);
        let (accepted, old) = pending.push_traced(Queued {
            command,
            trace: Some(trace),
        });
        assert!(accepted);
        assert_eq!(old.map(|t| t.id), (value == 22).then_some(ids[0]));
    }
    pending.push(Command::FollowServer { enabled: true });
    let trace = recorder.input(1);
    assert!(
        pending
            .push_traced(Queued {
                command: Command::Set {
                    setting: Setting::SnowIntensity(33)
                },
                trace: Some(trace)
            })
            .0
    );
    assert_eq!(pending.pop_traced().unwrap().trace.unwrap().id, ids[1]);
    assert!(pending.pop_traced().unwrap().trace.is_none());
    assert_eq!(pending.pop_traced().unwrap().trace.unwrap().id, trace.id);
    assert!(pending.is_empty());
}

#[test]
fn preset_barriers_preserve_intensity_changes_that_custom_must_keep() {
    use snowflakes_client::{
        config::SnowPreset,
        control::Controller,
        storage::{CacheClock, UserStore},
    };
    let dir = tempfile::tempdir().unwrap();
    let now = std::time::Instant::now();
    let mut app = Controller::open(
        UserStore::at(dir.path()),
        1,
        CacheClock {
            unix_ms: 2_000_000_000_000,
            uptime_ms: 100_000,
        },
        now,
    )
    .unwrap();
    app.handle(Command::FollowServer { enabled: false }, now);
    let mut pending = Pending::default();
    for setting in [
        Setting::SnowIntensity(33),
        Setting::SnowPreset(SnowPreset::Light),
        Setting::SnowPreset(SnowPreset::Custom),
    ] {
        assert!(pending.push(Command::Set { setting }));
    }
    while let Some(command) = pending.pop() {
        assert_eq!(app.handle(command, now).code, 0);
    }
    assert_eq!(app.config().settings.snow.intensity_percent, 20);
    assert_eq!(app.config().snow_preset, SnowPreset::Custom);
}
#[test]
fn snow_option_changes_coalesce_independently() {
    let mut pending = Pending::default();
    for value in [false, true, false] {
        for setting in [
            Setting::SnowOnWindows(value),
            Setting::SnowOnGround(value),
            Setting::SnowMelting(value),
            Setting::CursorEnabled(value),
        ] {
            assert!(pending.push(Command::Set { setting }));
        }
    }
    let commands: Vec<_> = std::iter::from_fn(|| pending.pop()).collect();
    assert_eq!(commands.len(), 4);
    for command in commands {
        assert_eq!(
            serde_json::to_value(command).unwrap()["setting"]["value"],
            false
        );
    }
}
#[test]
fn fast_slider_input_keeps_latest_independent_values() {
    let mut pending = Pending::default();
    for value in 0..=100 {
        for setting in [
            Setting::SnowIntensity(value),
            Setting::RainIntensity(100 - value),
            Setting::DriftSize(value / 2),
            Setting::PuddleSize(value / 4),
        ] {
            assert!(pending.push(Command::Set { setting }));
        }
    }
    let json = std::iter::from_fn(|| pending.pop())
        .map(|command| serde_json::to_value(command).unwrap())
        .collect::<Vec<_>>();
    assert_eq!(json.len(), 4);
    let values = json
        .iter()
        .map(|command| command["setting"]["value"].as_u64().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(values, [100, 0, 50, 25]);
    assert!(pending.is_empty());
}

#[test]
fn source_switches_preserve_edits_between_them() {
    let mut pending = Pending::default();
    pending.push(Command::FollowServer { enabled: false });
    pending.push(Command::Set {
        setting: Setting::SnowIntensity(17),
    });
    pending.push(Command::FollowServer { enabled: true });
    pending.push(Command::ServerUrl {
        value: "http://127.0.0.1:805/".into(),
    });
    pending.push(Command::FollowServer { enabled: false });
    pending.push(Command::Set {
        setting: Setting::SnowIntensity(38),
    });
    assert!(matches!(
        pending.pop(),
        Some(Command::FollowServer { enabled: false })
    ));
    assert!(matches!(
        pending.pop(),
        Some(Command::Set {
            setting: Setting::SnowIntensity(17)
        })
    ));
    assert!(matches!(
        pending.pop(),
        Some(Command::FollowServer { enabled: true })
    ));
    assert!(matches!(pending.pop(), Some(Command::ServerUrl { .. })));
    assert!(matches!(
        pending.pop(),
        Some(Command::FollowServer { enabled: false })
    ));
    assert!(matches!(
        pending.pop(),
        Some(Command::Set {
            setting: Setting::SnowIntensity(38)
        })
    ));
    assert!(pending.is_empty());
}

#[test]
fn queue_overload_is_explicit_without_losing_accepted_commands() {
    let mut pending = Pending::default();
    for _ in 0..32 {
        assert!(pending.push(Command::FollowServer { enabled: false }));
    }
    assert!(!pending.push(Command::Quit {}));
    assert_eq!(std::iter::from_fn(|| pending.pop()).count(), 32);
    assert!(pending.push(Command::Quit {}));
}

#[test]
fn short_and_scaled_windows_can_reach_the_bottom_controls() {
    for dpi in [96, 120, 144, 192] {
        assert_eq!(scale(96, dpi), dpi as i32);
        let content = scale(CONTENT_HEIGHT, dpi);
        for height in [200, 500, 900] {
            let scroll = scroll_limit(height, dpi);
            assert!(scroll >= 0);
            assert!(content - scroll <= height);
        }
        assert_eq!(scroll_limit(content + 1, dpi), 0);
    }
}

#[test]
fn monitor_rows_preserve_identity_and_disconnected_selections() {
    use snowflakes_client::{
        geometry::Rect,
        ui::{Monitor, monitor_rows},
    };
    use snowflakes_protocol::{Display, MonitorSelection};
    let primary = Monitor {
        id: "display-a".into(),
        rect: Rect {
            left: 0.0,
            top: 0.0,
            right: 800.0,
            bottom: 600.0,
        },
        primary: true,
        persistent: true,
    };
    let secondary = Monitor {
        id: "display-b".into(),
        rect: Rect {
            left: -800.0,
            top: 0.0,
            right: 0.0,
            bottom: 600.0,
        },
        primary: false,
        persistent: false,
    };
    let chosen = Display {
        selection: MonitorSelection::Selected,
        monitor_ids: vec![secondary.id.clone()],
    };
    let rows = monitor_rows(
        &[primary.clone(), secondary.clone()],
        chosen.monitor_ids.clone().into_iter(),
    );
    assert_eq!(rows[0].id, "display-b");
    assert!(rows[0].selected(&chosen));
    assert!(!rows[1].selected(&chosen));
    assert_eq!(
        rows,
        monitor_rows(
            &[secondary, primary.clone()],
            chosen.monitor_ids.clone().into_iter()
        )
    );
    let missing = monitor_rows(&[primary], chosen.monitor_ids.clone().into_iter());
    assert_eq!(missing[1].id, "display-b");
    assert!(!missing[1].connected);
    assert!(missing[1].selected(&chosen));
    assert!(!missing[0].selected(&chosen));
    let unknown = monitor_rows(&[], chosen.monitor_ids.clone().into_iter());
    assert_eq!(unknown.len(), 1);
    assert!(unknown[0].selected(&chosen));
    let all = Display {
        selection: MonitorSelection::All,
        monitor_ids: vec![],
    };
    let main = Display {
        selection: MonitorSelection::Primary,
        monitor_ids: vec![],
    };
    assert!(missing[0].selected(&all) && missing[0].selected(&main));
    assert!(!missing[1].selected(&all) && !missing[1].selected(&main));
}

#[test]
fn pending_display_changes_remain_atomic_across_source_barriers() {
    use snowflakes_protocol::{Display, MonitorSelection};
    let command = |id: &str| Command::Set {
        setting: Setting::Display(Display {
            selection: MonitorSelection::Selected,
            monitor_ids: vec![id.into()],
        }),
    };
    let mut pending = Pending::default();
    pending.push(command("display-a"));
    pending.push(command("display-b"));
    pending.push(Command::FollowServer { enabled: true });
    pending.push(command("display-c"));
    let first = pending.pop().unwrap();
    assert!(
        matches!(first, Command::Set { setting: Setting::Display(d) } if d.monitor_ids == ["display-b"])
    );
    assert!(matches!(
        pending.pop(),
        Some(Command::FollowServer { enabled: true })
    ));
    assert!(
        matches!(pending.pop(), Some(Command::Set { setting: Setting::Display(d) }) if d.monitor_ids == ["display-c"])
    );
    assert!(pending.is_empty());
}
#[test]
fn capture_requests_coalesce_independently_and_keep_source_barriers() {
    let mut pending = Pending::default();
    pending.push(Command::ExcludeFromCapture { enabled: true });
    pending.push(Command::Pause {});
    pending.push(Command::ExcludeFromCapture { enabled: false });
    pending.push(Command::FollowServer { enabled: true });
    pending.push(Command::ExcludeFromCapture { enabled: true });
    assert!(matches!(pending.pop(), Some(Command::Pause {})));
    assert!(matches!(
        pending.pop(),
        Some(Command::ExcludeFromCapture { enabled: false })
    ));
    assert!(matches!(
        pending.pop(),
        Some(Command::FollowServer { enabled: true })
    ));
    assert!(matches!(
        pending.pop(),
        Some(Command::ExcludeFromCapture { enabled: true })
    ));
    assert!(pending.is_empty());
}
