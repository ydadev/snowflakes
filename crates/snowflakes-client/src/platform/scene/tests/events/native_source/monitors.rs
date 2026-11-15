//! Place only an owned private-desktop window on actual Windows monitors.
use super::*;
use crate::geometry::{Point, Screen};

fn placement(screen: &Screen) -> RECT {
    let r = screen.rect;
    let margin = (96.0 * screen.scale)
        .min(r.width() / 8.0)
        .min(r.height() / 8.0);
    let width = (520.0 * screen.scale).min(r.width() - 2.0 * margin).floor();
    let height = (160.0 * screen.scale)
        .min(r.height() - 2.0 * margin)
        .floor();
    let left = (r.left + (r.width() - width) / 2.0).round();
    let top = (r.top + margin).round();
    RECT {
        left: left as i32,
        top: top as i32,
        right: (left + width) as i32,
        bottom: (top + height) as i32,
    }
}
fn bridge(a: &Screen, b: &Screen) -> Option<RECT> {
    // A shared vertical edge only; never invent an adjoining display across a gap.
    let edge = if (a.rect.right - b.rect.left).abs() < 0.5 {
        a.rect.right
    } else if (b.rect.right - a.rect.left).abs() < 0.5 {
        a.rect.left
    } else {
        return None;
    };
    let top = a.rect.top.max(b.rect.top);
    let bottom = a.rect.bottom.min(b.rect.bottom);
    let height = (160.0 * a.scale).min((bottom - top) / 2.0).floor();
    let width = (520.0 * a.scale)
        .min(a.rect.width().min(b.rect.width()))
        .floor();
    if width < 80.0 || height < 80.0 {
        return None;
    }
    let left = (edge - width / 2.0).round();
    let y = (top + (bottom - top - height) / 2.0).round();
    Some(RECT {
        left: left as i32,
        top: y as i32,
        right: (left + width) as i32,
        bottom: (y + height) as i32,
    })
}
pub(super) fn command(command: &str, windows: &[NativeWindow]) -> bool {
    if !matches!(
        command,
        "native-place-primary" | "native-place-secondary" | "native-place-bridge"
    ) {
        return false;
    }
    private(&std::env::var(DESKTOP_ENV).unwrap());
    windows[0].check();
    let mut screens = crate::platform::scene::screens().unwrap();
    screens.sort_by(|a, b| {
        a.rect
            .left
            .total_cmp(&b.rect.left)
            .then(a.rect.top.total_cmp(&b.rect.top))
    });
    let primary = screens
        .iter()
        .find(|s| s.rect.contains(Point::default()))
        .unwrap();
    let secondary = screens
        .iter()
        .find(|s| s.key != primary.key && s.rect != primary.rect);
    let target = match command {
        "native-place-primary" => placement(primary),
        "native-place-secondary" => {
            let Some(screen) = secondary else {
                reply("native-secondary-unavailable");
                return true;
            };
            placement(screen)
        }
        _ => {
            let Some(screen) = secondary else {
                reply("native-secondary-unavailable");
                return true;
            };
            let Some(rect) = bridge(primary, screen) else {
                reply("native-shared-edge-unavailable");
                return true;
            };
            rect
        }
    };
    assert!(target.right > target.left && target.bottom > target.top);
    unsafe {
        SetWindowPos(
            windows[0].0,
            None,
            target.left,
            target.top,
            target.right - target.left,
            target.bottom - target.top,
            SWP_NOZORDER | SWP_NOACTIVATE,
        )
        .unwrap();
        let mut actual = RECT::default();
        GetWindowRect(windows[0].0, &mut actual).unwrap();
        assert_eq!(
            (actual.left, actual.top, actual.right, actual.bottom),
            (target.left, target.top, target.right, target.bottom)
        );
    }
    windows[0].check();
    reply("native-placed");
    true
}

#[test]
fn native_monitor_placement_preserves_window_identity_and_untouched_control() {
    let name = format!(
        "SnowflakesNative-{}-{}-placement",
        std::process::id(),
        unsafe { GetCurrentThreadId() }
    );
    let _desktop = TestDesktop::attach(&name, true);
    private(&name);
    let mut screens = crate::platform::scene::screens().unwrap();
    screens.sort_by(|a, b| {
        a.rect
            .left
            .total_cmp(&b.rect.left)
            .then(a.rect.top.total_cmp(&b.rect.top))
    });
    let primary = screens
        .iter()
        .find(|s| s.rect.contains(Point::default()))
        .unwrap();
    let secondary = screens
        .iter()
        .find(|s| s.key != primary.key && s.rect != primary.rect);
    let mut tracker = Tracker::new().unwrap();
    let mut host = Host::new(&name);
    let captured = create(&mut tracker, &mut host);
    let key = captured.key(0);
    let control = captured.key(1);
    let target_handle = captured.info[0].handle;
    let mut scene = captured.scene.clone();
    let mut millis = 30;
    for (command, destination) in [
        ("native-place-primary", Some(primary)),
        ("native-place-secondary", secondary),
        ("native-place-primary", Some(primary)),
    ] {
        let before = info(target_handle, host.child.id()).rect;
        let response = host.command(command);
        assert_eq!(
            response,
            if destination.is_some() {
                "native-placed"
            } else {
                "native-secondary-unavailable"
            }
        );
        pump();
        millis += 30;
        if let Some(updated) = tracker
            .poll(tracker.started + Duration::from_millis(millis))
            .unwrap()
        {
            scene = updated;
        }
        let current = info(target_handle, host.child.id());
        assert_eq!(scene.windows.iter().filter(|w| w.key == key).count(), 1);
        assert!(
            scene
                .windows
                .iter()
                .any(|w| w.key == key && w.rect == current.rect)
        );
        assert!(
            scene
                .windows
                .iter()
                .any(|w| w.key == control && w.rect == captured.info[1].rect)
        );
        if let Some(screen) = destination {
            assert!(
                screen
                    .rect
                    .contains(Point::new(current.rect.left, current.rect.top))
            );
            assert!(
                current.rect.right <= screen.rect.right
                    && current.rect.bottom <= screen.rect.bottom
            );
        } else {
            assert_eq!(current.rect, before);
        }
    }
    host.finish();
    println!(
        "native_monitor_placement monitors={} distinct_secondary={} identity_preserved=true control_unchanged=true",
        screens.len(),
        secondary.is_some()
    );
}
