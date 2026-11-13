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
