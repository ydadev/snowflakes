//! Actual visibility transitions through Native/Tracker/World on a non-input desktop.
use super::*;
use crate::snow::Flake;

pub(super) fn stocked(scene: Scene, key: u64, control: u64) -> (World, Settings) {
    let mut world = World::new(scene, 101);
    for key in [key, control] {
        let drift = world.drifts.get_mut(&key).unwrap();
        drift.mass.fill(5.0 * drift.cell_width);
    }
    world.introduced_mass = world.total_mass();
    let mut settings = Settings::default();
    settings.snow.intensity_percent = 0;
    settings.snow.wind_percent = 0;
    settings.snow.melting = false;
    settings.snow.drift_size_percent = 100;
    world.step(&settings, None);
    world.step(&settings, None);
    (world, settings)
}

fn transition(host: &mut Host, tracker: &mut Tracker, command: &str, millis: u64) -> Scene {
    assert_eq!(host.command(command), "native-state-changed");
    pump();
    poll_native(tracker, millis)
}

fn cloak(handle: usize) -> u32 {
    let mut state = 0u32;
    unsafe {
        DwmGetWindowAttribute(
            HWND(handle as *mut _),
            DWMWA_CLOAKED,
            (&mut state as *mut u32).cast(),
            4,
        )
        .unwrap();
    }
    state
}

#[test]
fn native_hide_and_minimize_release_slabs_and_restore_empty_generation() {
    for (hide, show) in [
        ("native-hide", "native-show"),
        ("native-minimize", "native-restore"),
    ] {
        for paused in [false, true] {
            let name = format!(
                "SnowflakesNative-{}-{}-{hide}-{paused}",
                std::process::id(),
                unsafe { GetCurrentThreadId() }
            );
            let _desktop = TestDesktop::attach(&name, true);
            private(&name);
            let mut tracker = Tracker::new().unwrap();
            let mut host = Host::new(&name);
            let captured = create(&mut tracker, &mut host);
            let key = captured.key(0);
            let control = captured.key(1);
            let original = captured.info[0].clone();
            let center = Point::new(
                (original.rect.left + original.rect.right) * 0.5,
                original.rect.top,
            );
            let screen = captured
                .scene
                .screens
                .iter()
                .find(|s| s.rect.contains(center))
                .unwrap()
                .clone();
            let (mut world, settings) = stocked(captured.scene, key, control);
            let mass = world.drifts[&key].total_mass();
            let control_mass = world.drifts[&control].total_mass();
            if paused {
                world.set_paused_monitors([screen.key].into());
            }
            let excluded = transition(&mut host, &mut tracker, hide, 60);
            assert!(Native::default().read(original.handle).is_none());
            assert!(excluded.windows.iter().all(|w| w.key != key));
            assert_eq!(
                excluded
                    .windows
                    .iter()
                    .find(|w| w.key == control)
                    .unwrap()
                    .rect,
                captured.info[1].rect
            );
            world.set_scene(excluded);
            assert!(!world.drifts.contains_key(&key));
            conserved(&world);
            if paused {
                for _ in 0..120 {
                    world.step(&settings, None);
                    conserved(&world);
                }
                assert!(world.flakes.is_empty() && world.pending_collapse_count() > 0);
                world.set_paused_monitors(Default::default());
            }
            world.step(&settings, None);
            assert!(!world.flakes.is_empty());
            assert!(
                world
                    .flakes
                    .iter()
                    .all(|f| f.slab.is_some() && f.skip_support == Some(key))
            );
            for _ in 0..1800 {
                world.step(&settings, None);
                conserved(&world);
                if world.flakes.is_empty() && world.pending_collapse_count() == 0 {
                    break;
                }
            }
            assert!(world.flakes.is_empty() && world.pending_collapse_count() == 0);
            let floor_key = screen.key | (1 << 63);
            assert!((world.drifts[&floor_key].total_mass() - mass).abs() < mass * 1e-5);
            let restored = transition(&mut host, &mut tracker, show, 90);
            let native = info(original.handle, host.child.id());
            assert_eq!(native.rect, original.rect);
            let roof = restored
                .windows
                .iter()
                .find(|w| w.rect == native.rect && w.collect && w.visible)
                .unwrap();
            assert_ne!(roof.key, key);
            let restored_key = roof.key;
            world.set_scene(restored);
            assert_eq!(world.drifts[&restored_key].total_mass(), 0.0);
            assert!((world.drifts[&control].total_mass() - control_mass).abs() < 0.01);
            assert!((world.drifts[&floor_key].total_mass() - mass).abs() < mass * 1e-5);
            conserved(&world);
            host.finish();
            private(&name);
        }
    }
}

#[test]
fn native_cloak_uncloak_keeps_generation_and_stock_without_receiving_snow() {
    let name = format!("SnowflakesNative-{}-{}-cloak", std::process::id(), unsafe {
        GetCurrentThreadId()
    });
    let _desktop = TestDesktop::attach(&name, true);
    private(&name);
    let mut tracker = Tracker::new().unwrap();
    let mut host = Host::new(&name);
    let captured = create(&mut tracker, &mut host);
    let key = captured.key(0);
    let control = captured.key(1);
    let handle = captured.info[0].handle;
    let roof = captured.info[0].rect;
    let (mut world, settings) = stocked(captured.scene, key, control);
    let mass = world.drifts[&key].total_mass();
    let control_mass = world.drifts[&control].total_mass();
    let cloaked = transition(&mut host, &mut tracker, "native-cloak", 60);
    assert!(cloak(handle) & 1 != 0);
    let native = Native::default().read(handle).unwrap();
    assert!(!native.visible && native.collect);
    let hidden = cloaked.windows.iter().find(|w| w.key == key).unwrap();
    assert!(!hidden.visible && hidden.collect);
    world.set_scene(cloaked);
    assert!(world.drifts[&key].support.visible.is_empty());
    assert!(world.release(Flake::new(
        Point::new((roof.left + roof.right) * 0.5, roof.top - 20.0),
        3.0
    )));
    for _ in 0..120 {
        world.step(&settings, None);
        conserved(&world);
    }
    assert!((world.drifts[&key].total_mass() - mass).abs() < 0.01);
    assert!(world.flakes.iter().any(|f| f.position.y > roof.top + 15.0));
    let shown = transition(&mut host, &mut tracker, "native-uncloak", 90);
    assert_eq!(cloak(handle), 0);
    assert!(
        shown
            .windows
            .iter()
            .any(|w| w.key == key && w.visible && w.collect)
    );
    world.set_scene(shown);
    assert!(!world.drifts[&key].support.visible.is_empty());
    assert!((world.drifts[&key].total_mass() - mass).abs() < 0.01);
    assert!((world.drifts[&control].total_mass() - control_mass).abs() < 0.01);
    assert_eq!(world.pending_collapse_count(), 0);
    conserved(&world);
    host.finish();
    private(&name);
}

#[test]
fn native_cloaked_close_never_deposits_its_hidden_stock_on_visible_ground() {
    let name = format!(
        "SnowflakesNative-{}-{}-hidden-close",
        std::process::id(),
        unsafe { GetCurrentThreadId() }
    );
    let _desktop = TestDesktop::attach(&name, true);
    private(&name);
    let mut tracker = Tracker::new().unwrap();
    let mut host = Host::new(&name);
    let captured = create(&mut tracker, &mut host);
    let key = captured.key(0);
    let control = captured.key(1);
    let (mut world, settings) = stocked(captured.scene, key, control);
    let mass = world.drifts[&key].total_mass();
    let control_mass = world.drifts[&control].total_mass();
    world.set_scene(transition(&mut host, &mut tracker, "native-cloak", 60));
    assert_eq!(host.command("native-close"), "native-closed");
    pump();
    world.set_scene(poll_native(&mut tracker, 90));
    world.step(&settings, None);
    assert!(!world.flakes.is_empty());
    for body in &world.flakes {
        assert!(body.slab.is_some());
        assert!(
            world
                .falling_clips(body, body.draw_bounds(world.particle_scale(body)))
                .is_some_and(|clips| clips.is_empty())
        );
    }
    for _ in 0..1800 {
        world.step(&settings, None);
        conserved(&world);
        if world.flakes.is_empty() && world.pending_collapse_count() == 0 {
            break;
        }
    }
    assert!(world.flakes.is_empty() && world.pending_collapse_count() == 0);
    for screen in &world.scene.screens {
        assert_eq!(world.drifts[&(screen.key | (1 << 63))].total_mass(), 0.0);
    }
    assert!((world.removed_mass - mass).abs() < mass * 1e-5);
    assert!((world.drifts[&control].total_mass() - control_mass).abs() < 0.01);
    host.finish();
    private(&name);
}
