//! Native delivery into the real snow model, without graphics or visible windows.
use super::*;
use crate::snow::World;
use snowflakes_protocol::Settings;

struct Loaded {
    source: FixtureSource,
    world: World,
    settings: Settings,
    key: u64,
    control: u64,
    millis: u64,
}
impl Loaded {
    fn new(tracker: &mut Tracker, host: &mut Host) -> Self {
        let mut source = FixtureSource {
            handles: vec![],
            pid: host.child.id(),
            reads: vec![],
            orders: 0,
        };
        assert!(poll(tracker, &mut source, 0).unwrap().windows.is_empty());
        let created = host.command("create");
        source.handles = created
            .strip_prefix("created ")
            .unwrap()
            .split_whitespace()
            .map(|handle| handle.parse().unwrap())
            .collect();
        pump();
        let scene = poll(tracker, &mut source, 30).expect("native roof creation was not delivered");
        assert_eq!(scene.windows.len(), 2);
        let key = scene
            .windows
            .iter()
            .find(|w| w.rect.left == 100.0)
            .unwrap()
            .key;
        let control = scene
            .windows
            .iter()
            .find(|w| w.rect.left == 320.0)
            .unwrap()
            .key;
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
        Self {
            source,
            world,
            settings,
            key,
            control,
            millis: 30,
        }
    }
    fn accept(&mut self, tracker: &mut Tracker, host: &mut Host, command: &str, reply: &str) {
        assert_eq!(host.command(command), reply);
        pump();
        self.millis += 30;
        assert!(
            self.millis < 1000,
            "periodic polling must not mask lost native delivery"
        );
        let scene = poll(tracker, &mut self.source, self.millis)
            .expect("native window change was not delivered to the registry");
        self.world.set_scene(scene);
        conserved(&self.world);
    }
    fn step(&mut self, count: usize) {
        for _ in 0..count {
            self.world.step(&self.settings, None);
            conserved(&self.world);
        }
    }
}

fn conserved(world: &World) {
    let difference = (world.introduced_mass - world.removed_mass - world.total_mass()).abs();
    assert!(
        difference < world.introduced_mass.max(1.0) * 1e-5,
        "mass difference {difference}"
    );
}

#[test]
fn native_window_motion_preserves_gentle_snow_and_sheds_only_after_acceleration() {
    let name = format!("SnowflakesSnowMotion-{}-{}", std::process::id(), unsafe {
        GetCurrentThreadId()
    });
    let _desktop = TestDesktop::attach(&name, true);
    let mut tracker = Tracker::new().unwrap();
    let mut host = Host::new(&name);
    let mut loaded = Loaded::new(&mut tracker, &mut host);
    let before = loaded.world.drifts[&loaded.key].total_mass();
    let control = loaded.world.drifts[&loaded.control].total_mass();
    for offset in 1..=10 {
        loaded.step(2);
        loaded.source.reads.clear();
        loaded.accept(&mut tracker, &mut host, "nudge", "nudged");
        assert_eq!(loaded.source.reads, [loaded.source.handles[0]]);
        let window = loaded
            .world
            .scene
            .windows
            .iter()
            .find(|w| w.key == loaded.key)
            .unwrap();
        assert_eq!(window.rect.left, 100.0 + offset as f32);
        assert!((loaded.world.drifts[&loaded.key].total_mass() - before).abs() < 0.01);
        assert!((loaded.world.drifts[&loaded.control].total_mass() - control).abs() < 0.01);
        assert!(loaded.world.flakes.is_empty());
        assert_eq!(loaded.world.pending_collapse_count(), 0);
    }
    loaded.step(2);
    loaded.source.reads.clear();
    loaded.accept(&mut tracker, &mut host, "move", "moved");
    assert_eq!(loaded.source.reads, [loaded.source.handles[0]]);
    let retained = loaded.world.drifts[&loaded.key].total_mass();
    assert!(retained >= before * 0.5 && retained < before * 0.95);
    loaded.step(180);
    let detached: Vec<_> = loaded
        .world
        .flakes
        .iter()
        .filter(|f| f.skip_support == Some(loaded.key))
        .collect();
    assert!(!detached.is_empty());
    assert!(
        detached.iter().all(|f| f.position.y > 120.0),
        "shed snow must fall below its old ledge"
    );
    assert!(loaded.world.drifts[&loaded.key].total_mass() <= retained + 0.01);
    assert!((loaded.world.drifts[&loaded.control].total_mass() - control).abs() < 0.01);
    host.finish();
}

#[test]
fn native_window_close_preserves_mass_and_lands_connected_slabs_after_monitor_resume() {
    for paused in [false, true] {
        let name = format!(
            "SnowflakesSnowClose-{}-{}-{paused}",
            std::process::id(),
            unsafe { GetCurrentThreadId() }
        );
        let _desktop = TestDesktop::attach(&name, true);
        let mut tracker = Tracker::new().unwrap();
        let mut host = Host::new(&name);
        let mut loaded = Loaded::new(&mut tracker, &mut host);
        let mass = loaded.world.drifts[&loaded.key].total_mass();
        let control = loaded.world.drifts[&loaded.control].total_mass();
        loaded.step(2);
        if paused {
            loaded.world.set_paused_monitors([1].into());
        }
        loaded.accept(&mut tracker, &mut host, "close", "closed");
        assert!(!loaded.world.drifts.contains_key(&loaded.key));
        assert_eq!(loaded.world.scene.windows.len(), 1);
        assert_eq!(loaded.world.scene.windows[0].key, loaded.control);
        if paused {
            loaded.step(120);
            assert!(loaded.world.flakes.is_empty());
            assert!(loaded.world.pending_collapse_count() > 0);
            loaded.world.set_paused_monitors(Default::default());
        }
        loaded.step(1);
        assert!(!loaded.world.flakes.is_empty() && loaded.world.flakes.len() <= 12);
        for body in &loaded.world.flakes {
            let slab = body
                .slab
                .as_ref()
                .expect("closed roof must release slabs, not individual flakes");
            assert_eq!(body.skip_support, Some(loaded.key));
            assert!(slab.cell_width * slab.heights.len() as f32 > 15.0);
            assert!(slab.heights.iter().all(|h| (*h - 5.0).abs() < 0.001));
            let area = f64::from(slab.cell_width)
                * slab.heights.iter().map(|h| f64::from(*h)).sum::<f64>();
            assert!((area - f64::from(body.mass)).abs() < 0.001);
            assert!(body.velocity.y > 12.0);
        }
        for _ in 0..1800 {
            loaded.step(1);
            if loaded.world.flakes.is_empty() && loaded.world.pending_collapse_count() == 0 {
                break;
            }
        }
        assert!(
            loaded.world.flakes.is_empty(),
            "closed roof slabs must settle on the ground"
        );
        assert_eq!(loaded.world.pending_collapse_count(), 0);
        let floor = &loaded.world.drifts[&(1 | (1 << 63))];
        assert!((floor.total_mass() - mass).abs() < mass * 1e-5);
        assert!((loaded.world.drifts[&loaded.control].total_mass() - control).abs() < 0.01);
        let peak = (0..floor.mass.len())
            .map(|i| floor.height(i, &loaded.world.scene))
            .fold(0.0, f32::max);
        let occupied = floor.mass.iter().filter(|mass| **mass > 0.001).count();
        assert!(peak > 1.0 && occupied >= 3);
        assert!(
            occupied as f32 * floor.cell_width < 400.0,
            "landing must form a local mound"
        );
        // Contact rounding can explicitly account for a tiny numerical remainder.
        assert!(loaded.world.removed_mass < loaded.world.introduced_mass * 1e-6);
        host.finish();
    }
}
