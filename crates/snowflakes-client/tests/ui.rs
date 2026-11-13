use snowflakes_client::{
    control::{Command, Setting},
    ui::{CONTENT_HEIGHT, Pending, scale, scroll_limit},
};

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
