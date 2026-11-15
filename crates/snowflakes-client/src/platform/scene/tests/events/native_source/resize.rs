//! Real DWM resize geometry and a spatial mass oracle, in production's DPI context.
use super::*;
use crate::snow::Drift;

fn resized(host: &mut Host, tracker: &mut Tracker, command: &str, millis: u64) -> Scene {
    assert_eq!(host.command(command), "native-resized");
    pump();
    poll_native(tracker, millis)
}

fn intervals(drift: &Drift) -> Vec<(f64, f64, f64)> {
    (0..drift.mass.len())
        .map(|i| {
            let left = f64::from(drift.support.left) + i as f64 * f64::from(drift.cell_width);
            (
                left,
                left + f64::from(drift.cell_width),
                f64::from(drift.mass[i]) + f64::from(drift.pending[i]),
            )
        })
        .collect()
}

fn in_range(old: &[(f64, f64, f64)], left: f64, right: f64) -> f64 {
    old.iter()
        .map(|&(start, end, mass)| {
            mass * (end.min(right) - start.max(left)).max(0.0) / (end - start)
        })
        .sum()
}

fn remapped(drift: &Drift, old: &[(f64, f64, f64)]) {
    for (left, right, mass) in intervals(drift) {
        let expected = in_range(old, left, right);
        assert!(
            (mass - expected).abs() < expected.max(1.0) * 1e-4,
            "native resize moved material outside its physical overlap: {mass} vs {expected}"
        );
    }
}

fn settle(world: &mut World, settings: &Settings) {
    for _ in 0..1800 {
        world.step(settings, None);
        conserved(world);
        if world.flakes.is_empty() && world.pending_collapse_count() == 0 {
            break;
        }
    }
    assert!(world.flakes.is_empty() && world.pending_collapse_count() == 0);
}

#[test]
fn native_resize_growth_and_right_cut_preserve_spatial_stock_and_pause_the_cut() {
    for paused in [false, true] {
        let name = format!(
            "SnowflakesNative-{}-{}-resize-{paused}",
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
        let wide = resized(&mut host, &mut tracker, "native-wide", 60);
        let wide_rect = wide.windows.iter().find(|w| w.key == key).unwrap().rect;
        let screen = wide
            .screens
            .iter()
            .find(|s| {
                s.rect
                    .contains(Point::new(wide_rect.left + 10.0, wide_rect.top))
            })
            .unwrap()
            .clone();
        let (mut world, settings) = visibility::stocked(wide, key, control);
        let initial = world.drifts[&key].total_mass();
        let control_mass = world.drifts[&control].total_mass();
        let old = intervals(&world.drifts[&key]);
        let grown = resized(&mut host, &mut tracker, "native-grow-right", 90);
        let grown_rect = grown.windows.iter().find(|w| w.key == key).unwrap().rect;
        assert_eq!(grown_rect.left, wide_rect.left);
        assert_eq!(grown_rect.right, wide_rect.right + 120.0);
        assert_eq!(
            grown
                .windows
                .iter()
                .find(|w| w.key == control)
                .unwrap()
                .rect,
            captured.info[1].rect
        );
        world.set_scene(grown);
        remapped(&world.drifts[&key], &old);
        assert!((world.drifts[&key].total_mass() - initial).abs() < initial * 1e-5);
        assert!(world.flakes.is_empty() && world.pending_collapse_count() == 0);
        conserved(&world);
        world.step(&settings, None);
        world.step(&settings, None);
        let old = intervals(&world.drifts[&key]);
        let before_cut = world.drifts[&key].total_mass();
        if paused {
            world.set_paused_monitors([screen.key].into());
        }
        let cut = resized(&mut host, &mut tracker, "native-shrink-right", 120);
        let cut_rect = cut.windows.iter().find(|w| w.key == key).unwrap().rect;
        assert_eq!(cut_rect.left, wide_rect.left);
        assert_eq!(cut_rect.right, wide_rect.right - 160.0);
        world.set_scene(cut);
        let retained = &world.drifts[&key];
        let expected = in_range(
            &old,
            f64::from(retained.support.left),
            f64::from(retained.support.right),
        );
        remapped(retained, &old);
        assert!((retained.total_mass() - expected).abs() < initial * 1e-5);
        let cut_mass = before_cut - expected;
        assert!(cut_mass > 0.0 && world.pending_collapse_count() > 0);
        let roof_mass = retained.total_mass();
        if paused {
            for _ in 0..120 {
                world.step(&settings, None);
                conserved(&world);
            }
            assert!(world.flakes.is_empty() && world.pending_collapse_count() > 0);
            assert_eq!(world.drifts[&key].total_mass(), roof_mass);
            world.set_paused_monitors(Default::default());
        }
        world.step(&settings, None);
        assert!(!world.flakes.is_empty());
        assert!(world.flakes.iter().all(|body| body.slab.is_some()
            && body.skip_support.is_some()
            && body.skip_support != Some(key)));
        settle(&mut world, &settings);
        assert!(
            (world.drifts[&(screen.key | (1 << 63))].total_mass() - cut_mass).abs()
                < initial * 1e-5
        );
        assert!((world.drifts[&key].total_mass() - roof_mass).abs() < initial * 1e-5);
        assert!((world.drifts[&control].total_mass() - control_mass).abs() < 0.01);
        host.finish();
        private(&name);
    }
}

#[test]
fn native_left_and_right_cuts_release_removed_tail_instead_of_squeezing_it() {
    for left in [false, true] {
        let name = format!(
            "SnowflakesNative-{}-{}-tail-{left}",
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
        let wide = resized(&mut host, &mut tracker, "native-wide", 60);
        let rect = wide.windows.iter().find(|w| w.key == key).unwrap().rect;
        let screen = wide
            .screens
            .iter()
            .find(|s| s.rect.contains(Point::new(rect.left + 10.0, rect.top)))
            .unwrap()
            .clone();
        let (mut world, settings) = visibility::stocked(wide, key, control);
        let drift = world.drifts.get_mut(&key).unwrap();
        drift.mass.fill(0.0);
        drift.pending.fill(0.0);
        for i in 0..drift.mass.len() {
            if if left {
                drift.x(i) < drift.support.left + 80.0
            } else {
                drift.x(i) > drift.support.right - 80.0
            } {
                drift.mass[i] = 5.0 * drift.cell_width;
            }
        }
        world.introduced_mass = world.total_mass() + world.removed_mass;
        let tail = world.drifts[&key].total_mass();
        let control_mass = world.drifts[&control].total_mass();
        assert!(tail > 0.0);
        let cut = resized(
            &mut host,
            &mut tracker,
            if left {
                "native-cut-left"
            } else {
                "native-shrink-right"
            },
            90,
        );
        let after = cut.windows.iter().find(|w| w.key == key).unwrap().rect;
        if left {
            assert_eq!(after.right, rect.right);
            assert_eq!(after.left, rect.left + 160.0);
        } else {
            assert_eq!(after.left, rect.left);
            assert_eq!(after.right, rect.right - 160.0);
        }
        world.set_scene(cut);
        assert!(
            world.drifts[&key].total_mass() < 1e-4,
            "removed tail was compressed onto the retained roof"
        );
        assert!(world.pending_collapse_count() > 0);
        conserved(&world);
        world.step(&settings, None);
        assert!(!world.flakes.is_empty());
        assert!(world.flakes.iter().all(|body| body.slab.is_some()
            && body.skip_support.is_some()
            && body.skip_support != Some(key)));
        settle(&mut world, &settings);
        assert!((world.drifts[&(screen.key | (1 << 63))].total_mass() - tail).abs() < tail * 1e-5);
        assert!(world.drifts[&key].total_mass() < 1e-4);
        assert!((world.drifts[&control].total_mass() - control_mass).abs() < 0.01);
        host.finish();
        private(&name);
    }
}
