use snowflakes_client::{
    geometry::{Point, Rect, Screen},
    scene_registry::{MAX_WINDOWS, Registry, Source, WindowInfo},
};
use std::{collections::BTreeMap, time::Duration};

struct Desktop {
    windows: BTreeMap<usize, WindowInfo>,
    order: Vec<usize>,
    reads: Vec<usize>,
    scans: usize,
    screen_queries: usize,
    foreground: usize,
    fail_order: bool,
    fail_screens: bool,
    screens: Vec<Screen>,
}
impl Desktop {
    fn new(count: usize) -> Self {
        Self {
            windows: (1..=count)
                .map(|handle| {
                    (
                        handle,
                        WindowInfo {
                            handle,
                            process: 100,
                            thread: handle as u32,
                            rect: Rect {
                                left: 100.0,
                                top: 200.0,
                                right: 600.0,
                                bottom: 500.0,
                            },
                            visible: true,
                            collect: true,
                            maximized: false,
                            taskbar: false,
                        },
                    )
                })
                .collect(),
            order: (1..=count).collect(),
            reads: Vec::new(),
            scans: 0,
            screen_queries: 0,
            foreground: 1,
            fail_order: false,
            fail_screens: false,
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
        }
    }
    fn reset_counts(&mut self) {
        self.reads.clear();
        self.scans = 0;
        self.screen_queries = 0;
    }
}
impl Source for Desktop {
    type Error = ();
    fn order(&mut self) -> Result<Vec<usize>, ()> {
        self.scans += 1;
        if self.fail_order {
            return Err(());
        }
        Ok(self.order.clone())
    }
    fn read(&mut self, handle: usize) -> Option<WindowInfo> {
        self.reads.push(handle);
        self.windows.get(&handle).cloned()
    }
    fn owner(&self, handle: usize) -> Option<(u32, u32)> {
        self.windows.get(&handle).map(|w| (w.process, w.thread))
    }
    fn foreground(&mut self) -> usize {
        self.foreground
    }
    fn screens(&mut self) -> Result<Vec<Screen>, ()> {
        self.screen_queries += 1;
        if self.fail_screens {
            return Err(());
        }
        Ok(self.screens.clone())
    }
}
fn ms(value: u64) -> Duration {
    Duration::from_millis(value)
}

fn failed_periodic_refresh(order_failure: bool) {
    let mut desktop = Desktop::new(1);
    let mut registry = Registry::default();
    let initial = registry.poll(ms(0), &mut desktop).unwrap().unwrap();
    let replacement = Rect {
        left: 125.0,
        top: 200.0,
        right: 625.0,
        bottom: 500.0,
    };
    desktop.windows.get_mut(&1).unwrap().rect = replacement;
    desktop.screens[0].floor = 580.0;
    // Both facts changed without WinEvent delivery: the periodic scan must recover them.
    desktop.fail_order = order_failure;
    desktop.fail_screens = !order_failure;
    assert!(registry.poll(ms(1000), &mut desktop).is_err());
    desktop.fail_order = false;
    desktop.fail_screens = false;
    desktop.reset_counts();
    registry.changes.order(); // the production Tracker's fast retry
    let recovered = registry.poll(ms(1100), &mut desktop).unwrap().unwrap();
    assert_eq!(
        (recovered.windows[0].rect, recovered.screens[0].floor),
        (replacement, 580.0),
        "an order retry must finish the interrupted full geometry refresh"
    );
    assert_eq!(recovered.windows[0].key, initial.windows[0].key);
    assert_eq!(desktop.reads, [1]);
    assert_eq!(desktop.screen_queries, 1);
    desktop.reset_counts();
    assert!(registry.poll(ms(2000), &mut desktop).unwrap().is_none());
    assert_eq!(desktop.scans, 0);
    assert!(registry.poll(ms(2100), &mut desktop).unwrap().is_some());
    assert_eq!(desktop.reads, [1]);
}

#[test]
fn failed_periodic_order_retains_full_geometry_refresh_until_success() {
    failed_periodic_refresh(true);
}

#[test]
fn failed_periodic_screens_retains_full_geometry_refresh_until_success() {
    failed_periodic_refresh(false);
}

#[test]
fn failed_periodic_refresh_retains_destroy_and_overflow_generations() {
    for order_failure in [true, false] {
        let mut desktop = Desktop::new(2);
        let mut registry = Registry::default();
        let before = registry.poll(ms(0), &mut desktop).unwrap().unwrap();
        registry.changes.window(1, true);
        registry.changes.window(1, false);
        for handle in 3..2100 {
            registry.changes.window(handle, false);
        }
        desktop.reset_counts();
        desktop.fail_order = order_failure;
        desktop.fail_screens = !order_failure;
        for now in [1000, 1100] {
            assert!(registry.poll(ms(now), &mut desktop).is_err());
            assert!(desktop.reads.is_empty());
        }
        desktop.fail_order = false;
        desktop.fail_screens = false;
        let after = registry.poll(ms(1200), &mut desktop).unwrap().unwrap();
        assert_eq!(after.windows.len(), 2);
        for (old, new) in before.windows.iter().zip(&after.windows) {
            assert_ne!(
                old.key, new.key,
                "overflow makes every cached generation uncertain"
            );
            assert_eq!(old.rect, new.rect);
        }
        assert_eq!(desktop.reads, [1, 2]);
        desktop.reset_counts();
        assert!(registry.poll(ms(2199), &mut desktop).unwrap().is_none());
        assert_eq!(desktop.scans, 0);
    }
}

#[test]
fn ordered_identity_replacement_refreshes_only_that_surface_without_a_destroy_event() {
    let mut desktop = Desktop::new(2);
    let mut registry = Registry::default();
    let before = registry.poll(ms(0), &mut desktop).unwrap().unwrap();
    desktop.reset_counts();
    desktop.windows.get_mut(&1).unwrap().process = 200;
    registry.changes.order();
    let after = registry.poll(ms(30), &mut desktop).unwrap().unwrap();
    assert_ne!(before.windows[0].key, after.windows[0].key);
    assert_eq!(before.windows[1].key, after.windows[1].key);
    assert_eq!(desktop.reads, [1]);
}

#[test]
fn identity_changed_between_order_and_geometry_is_excluded_instead_of_reusing_the_roof() {
    struct Reused(Desktop);
    impl Source for Reused {
        type Error = ();
        fn order(&mut self) -> Result<Vec<usize>, ()> {
            self.0.order()
        }
        fn owner(&self, _: usize) -> Option<(u32, u32)> {
            Some((100, 1))
        }
        fn read(&mut self, handle: usize) -> Option<WindowInfo> {
            self.0.read(handle).map(|mut window| {
                window.process = 200;
                window
            })
        }
        fn screens(&mut self) -> Result<Vec<Screen>, ()> {
            self.0.screens()
        }
        fn foreground(&mut self) -> usize {
            1
        }
    }
    let mut registry = Registry::default();
    let scene = registry
        .poll(ms(0), &mut Reused(Desktop::new(1)))
        .unwrap()
        .unwrap();
    assert!(scene.windows.is_empty());
    assert!(registry.fullscreen.is_empty());
}

#[test]
fn collection_eligibility_does_not_exempt_a_full_monitor_foreground_window() {
    for collect in [false, true] {
        let mut desktop = Desktop::new(1);
        let screen = desktop.screens[0].rect;
        let window = desktop.windows.get_mut(&1).unwrap();
        window.rect = screen;
        window.collect = collect;
        let mut registry = Registry::default();
        let scene = registry.poll(ms(0), &mut desktop).unwrap().unwrap();
        assert!(
            registry.fullscreen.contains(&1),
            "fullscreen cannot depend on a snow support flag"
        );
        assert_eq!(scene.windows[0].collect, collect);
    }
}

#[test]
fn maximized_monitor_sized_window_with_taskbar_above_is_not_fullscreen() {
    let mut desktop = Desktop::new(2);
    let screen = desktop.screens[0].rect;
    let window = desktop.windows.get_mut(&1).unwrap();
    window.rect = screen;
    window.collect = false; // A custom, borderless maximized application.
    window.maximized = true;
    let bar = desktop.windows.get_mut(&2).unwrap();
    bar.taskbar = true;
    bar.collect = false;
    bar.rect = Rect {
        left: 0.0,
        top: 560.0,
        right: 800.0,
        bottom: 600.0,
    };
    desktop.order = vec![2, 1];
    let mut registry = Registry::default();
    registry.poll(ms(0), &mut desktop).unwrap().unwrap();
    assert!(
        registry.fullscreen.is_empty(),
        "a maximized window with an available taskbar is not fullscreen"
    );
}

fn taskbar_desktop(
    collect: bool,
    maximized: bool,
    bar_rect: Rect,
    visible: bool,
    above: bool,
) -> Desktop {
    let mut desktop = Desktop::new(2);
    let screen = desktop.screens[0].rect;
    let window = desktop.windows.get_mut(&1).unwrap();
    window.rect = screen;
    window.collect = collect;
    window.maximized = maximized;
    let bar = desktop.windows.get_mut(&2).unwrap();
    bar.taskbar = true;
    bar.collect = false;
    bar.visible = visible;
    bar.rect = bar_rect;
    desktop.order = if above { vec![2, 1] } else { vec![1, 2] };
    desktop
}

#[test]
fn maximized_work_area_does_not_pause_either_collection_mode() {
    for collect in [false, true] {
        for frame in [
            Rect {
                left: 0.0,
                top: 0.0,
                right: 800.0,
                bottom: 560.0,
            },
            Rect {
                left: 0.0,
                top: 40.0,
                right: 800.0,
                bottom: 600.0,
            },
            Rect {
                left: 40.0,
                top: 0.0,
                right: 800.0,
                bottom: 600.0,
            },
            Rect {
                left: 0.0,
                top: 0.0,
                right: 760.0,
                bottom: 600.0,
            },
        ] {
            let mut desktop = Desktop::new(1);
            let window = desktop.windows.get_mut(&1).unwrap();
            window.collect = collect;
            window.maximized = true;
            window.rect = frame;
            let mut registry = Registry::default();
            registry.poll(ms(0), &mut desktop).unwrap().unwrap();
            assert!(registry.fullscreen.is_empty());
        }
    }
}

#[test]
fn taskbar_exception_requires_maximization_visibility_order_and_positive_intersection() {
    let bar = Rect {
        left: 0.0,
        top: 560.0,
        right: 800.0,
        bottom: 600.0,
    };
    // Expected fullscreen state is specified independently for each observed desktop.
    for collect in [false, true] {
        for (maximized, visible, above, rect, paused) in [
            (true, true, true, bar, false),
            (true, true, false, bar, true),
            (true, false, true, bar, true),
            (false, true, true, bar, true),
            (
                true,
                true,
                true,
                Rect {
                    left: 800.0,
                    top: 560.0,
                    right: 1600.0,
                    bottom: 600.0,
                },
                true,
            ),
            (
                true,
                true,
                true,
                Rect {
                    left: 0.0,
                    top: 600.0,
                    right: 800.0,
                    bottom: 640.0,
                },
                true,
            ),
            (
                true,
                true,
                true,
                Rect {
                    left: 0.0,
                    top: 599.0,
                    right: 800.0,
                    bottom: 639.0,
                },
                false,
            ),
            (
                true,
                true,
                true,
                Rect {
                    left: 0.0,
                    top: -39.0,
                    right: 800.0,
                    bottom: 1.0,
                },
                false,
            ),
            (
                true,
                true,
                true,
                Rect {
                    left: -39.0,
                    top: 0.0,
                    right: 1.0,
                    bottom: 600.0,
                },
                false,
            ),
            (
                true,
                true,
                true,
                Rect {
                    left: 799.0,
                    top: 0.0,
                    right: 839.0,
                    bottom: 600.0,
                },
                false,
            ),
        ] {
            let mut desktop = taskbar_desktop(collect, maximized, rect, visible, above);
            let mut registry = Registry::default();
            let scene = registry.poll(ms(0), &mut desktop).unwrap().unwrap();
            assert_eq!(registry.fullscreen.contains(&1), paused);
            assert_eq!(
                scene
                    .windows
                    .iter()
                    .find(|w| w.rect == screen_sized_rect())
                    .unwrap()
                    .collect,
                collect
            );
        }
    }
}

fn screen_sized_rect() -> Rect {
    Rect {
        left: 0.0,
        top: 0.0,
        right: 800.0,
        bottom: 600.0,
    }
}

#[test]
fn fullscreen_is_cleared_by_cloaking_taskbar_focus_missing_foreground_and_failed_read() {
    let mut desktop = Desktop::new(2);
    desktop.windows.get_mut(&1).unwrap().rect = screen_sized_rect();
    desktop.windows.get_mut(&2).unwrap().taskbar = true;
    let mut registry = Registry::default();
    registry.poll(ms(0), &mut desktop).unwrap().unwrap();
    assert!(registry.fullscreen.contains(&1));
    desktop.windows.get_mut(&1).unwrap().visible = false;
    registry.changes.window(1, false);
    registry.poll(ms(30), &mut desktop).unwrap().unwrap();
    assert!(registry.fullscreen.is_empty());
    desktop.windows.get_mut(&1).unwrap().visible = true;
    registry.changes.window(1, false);
    registry.poll(ms(60), &mut desktop).unwrap().unwrap();
    assert!(registry.fullscreen.contains(&1));
    for (time, foreground) in [(90, 2), (120, 0), (150, 999)] {
        desktop.foreground = foreground;
        registry.changes.order();
        registry.poll(ms(time), &mut desktop).unwrap().unwrap();
        assert!(registry.fullscreen.is_empty());
    }
    desktop.foreground = 1;
    desktop.windows.remove(&1); // The native query lost this HWND mid-refresh.
    registry.changes.window(1, false);
    registry.poll(ms(180), &mut desktop).unwrap().unwrap();
    assert!(registry.fullscreen.is_empty());
}

#[test]
fn fullscreen_coverage_and_taskbar_availability_are_independent_per_monitor() {
    let mut desktop = Desktop::new(3);
    desktop.screens.push(Screen {
        key: 2,
        persistent_id: true,
        rect: Rect {
            left: -800.0,
            top: 0.0,
            right: 0.0,
            bottom: 600.0,
        },
        floor: 600.0,
        scale: 2.0,
    });
    desktop.screens.push(Screen {
        key: 3,
        persistent_id: true,
        rect: Rect {
            left: 0.0,
            top: -600.0,
            right: 800.0,
            bottom: 0.0,
        },
        floor: 0.0,
        scale: 1.5,
    });
    desktop.windows.get_mut(&1).unwrap().rect = desktop.screens[0].rect;
    desktop.windows.get_mut(&2).unwrap().rect = desktop.screens[1].rect;
    let mut registry = Registry::default();
    registry.poll(ms(0), &mut desktop).unwrap().unwrap();
    assert_eq!(registry.fullscreen, [1].into());
    desktop.foreground = 2;
    registry.changes.order();
    registry.poll(ms(30), &mut desktop).unwrap().unwrap();
    assert_eq!(registry.fullscreen, [2].into());
    desktop.foreground = 3; // Both monitor-sized windows are now in the background.
    registry.changes.order();
    registry.poll(ms(60), &mut desktop).unwrap().unwrap();
    assert!(registry.fullscreen.is_empty());
    desktop.foreground = 1;
    let window = desktop.windows.get_mut(&1).unwrap();
    window.rect.left = -800.0;
    window.maximized = true;
    let bar = desktop.windows.get_mut(&3).unwrap();
    bar.taskbar = true;
    bar.rect = Rect {
        left: 0.0,
        top: 599.0,
        right: 800.0,
        bottom: 639.0,
    };
    desktop.order = vec![3, 1, 2];
    registry.changes.window(1, false);
    registry.changes.window(3, false);
    registry.changes.order();
    registry.poll(ms(90), &mut desktop).unwrap().unwrap();
    assert_eq!(registry.fullscreen, [2].into());
    desktop.windows.get_mut(&3).unwrap().visible = false;
    registry.changes.window(3, false);
    registry.poll(ms(120), &mut desktop).unwrap().unwrap();
    assert_eq!(registry.fullscreen, [1, 2].into());
}

#[test]
fn maximize_and_taskbar_changes_update_fullscreen_without_requerying_other_windows() {
    let bar = Rect {
        left: 0.0,
        top: 560.0,
        right: 800.0,
        bottom: 600.0,
    };
    let mut desktop = taskbar_desktop(true, true, bar, true, true);
    let mut registry = Registry::default();
    let scene = registry.poll(ms(0), &mut desktop).unwrap().unwrap();
    let key = scene.windows.iter().find(|w| w.collect).unwrap().key;
    assert!(registry.fullscreen.is_empty());
    desktop.reset_counts();
    desktop.windows.get_mut(&1).unwrap().maximized = false;
    registry.changes.window(1, false);
    let scene = registry.poll(ms(30), &mut desktop).unwrap().unwrap();
    assert_eq!(registry.fullscreen, [1].into());
    assert_eq!(scene.windows.iter().find(|w| w.collect).unwrap().key, key);
    assert_eq!(desktop.reads, [1]);
    assert_eq!(desktop.scans, 0);
    desktop.windows.get_mut(&1).unwrap().maximized = true;
    registry.changes.window(1, false);
    registry.poll(ms(60), &mut desktop).unwrap().unwrap();
    assert!(registry.fullscreen.is_empty());
    desktop.reset_counts();
    desktop.order = vec![1, 2];
    registry.changes.order();
    registry.poll(ms(90), &mut desktop).unwrap().unwrap();
    assert_eq!(registry.fullscreen, [1].into());
    assert!(desktop.reads.is_empty());
    assert_eq!(desktop.scans, 1);
}

#[test]
fn fullscreen_coverage_preserves_the_physical_one_pixel_tolerance() {
    for collect in [false, true] {
        for (frame, paused) in [
            (screen_sized_rect(), true),
            (
                Rect {
                    left: 1.0,
                    top: 1.0,
                    right: 799.0,
                    bottom: 599.0,
                },
                true,
            ),
            (
                Rect {
                    left: -20.0,
                    top: -20.0,
                    right: 820.0,
                    bottom: 620.0,
                },
                true,
            ),
            (
                Rect {
                    left: 1.25,
                    top: 0.0,
                    right: 800.0,
                    bottom: 600.0,
                },
                false,
            ),
            (
                Rect {
                    left: 0.0,
                    top: 1.25,
                    right: 800.0,
                    bottom: 600.0,
                },
                false,
            ),
            (
                Rect {
                    left: 0.0,
                    top: 0.0,
                    right: 798.75,
                    bottom: 600.0,
                },
                false,
            ),
            (
                Rect {
                    left: 0.0,
                    top: 0.0,
                    right: 800.0,
                    bottom: 598.75,
                },
                false,
            ),
        ] {
            let mut desktop = Desktop::new(1);
            let window = desktop.windows.get_mut(&1).unwrap();
            window.rect = frame;
            window.collect = collect;
            let mut registry = Registry::default();
            registry.poll(ms(0), &mut desktop).unwrap().unwrap();
            assert_eq!(registry.fullscreen.contains(&1), paused);
        }
    }
}

#[test]
fn movement_queries_only_the_changed_window_and_coalesces_bursts() {
    let mut desktop = Desktop::new(500);
    let mut registry = Registry::default();
    registry.poll(ms(0), &mut desktop).unwrap();
    desktop.reset_counts();
    for _ in 0..10_000 {
        registry.changes.window(7, false);
    }
    desktop.windows.get_mut(&7).unwrap().rect.top = 250.0;
    assert!(registry.poll(ms(29), &mut desktop).unwrap().is_none());
    let scene = registry.poll(ms(30), &mut desktop).unwrap().unwrap();
    assert_eq!(scene.windows[6].rect.top, 250.0);
    assert_eq!(desktop.reads, [7]);
    assert_eq!((desktop.scans, desktop.screen_queries), (0, 0));
    assert!(registry.poll(ms(999), &mut desktop).unwrap().is_none());
    registry.poll(ms(1000), &mut desktop).unwrap();
    assert_eq!(desktop.reads.len(), 501);
    assert_eq!((desktop.scans, desktop.screen_queries), (1, 1));
}

#[test]
fn destroyed_then_reused_handle_gets_a_new_surface_even_in_one_batch() {
    let mut desktop = Desktop::new(2);
    let mut registry = Registry::default();
    let before = registry.poll(ms(0), &mut desktop).unwrap().unwrap();
    registry.changes.window(1, true);
    registry.changes.window(1, false);
    let after = registry.poll(ms(30), &mut desktop).unwrap().unwrap();
    assert_ne!(before.windows[0].key, after.windows[0].key);
    assert_eq!(before.windows[1].key, after.windows[1].key);
    // A final destroy must not resurrect the cached object.
    registry.changes.window(1, false);
    registry.changes.window(1, true);
    let removed = registry.poll(ms(60), &mut desktop).unwrap().unwrap();
    assert_eq!(removed.windows.len(), 1);
}

#[test]
fn cloaking_preserves_identity_and_hides_collection_until_uncloaked() {
    let mut desktop = Desktop::new(1);
    let mut registry = Registry::default();
    let before = registry.poll(ms(0), &mut desktop).unwrap().unwrap();
    desktop.windows.get_mut(&1).unwrap().visible = false;
    registry.changes.window(1, false);
    let hidden = registry.poll(ms(30), &mut desktop).unwrap().unwrap();
    assert_eq!(before.windows[0].key, hidden.windows[0].key);
    assert!(hidden.supports()[0].visible.is_empty());
    desktop.windows.get_mut(&1).unwrap().visible = true;
    registry.changes.window(1, false);
    let shown = registry.poll(ms(60), &mut desktop).unwrap().unwrap();
    assert_eq!(before.windows[0].key, shown.windows[0].key);
    assert!(!shown.supports()[0].visible.is_empty());
}

#[test]
fn reorder_changes_occlusion_without_requerying_window_geometry() {
    let mut desktop = Desktop::new(2);
    let popup = desktop.windows.get_mut(&2).unwrap();
    popup.collect = false;
    popup.rect.top = 100.0;
    let mut registry = Registry::default();
    let before = registry.poll(ms(0), &mut desktop).unwrap().unwrap();
    assert!(before.supports()[0].visible_at(300.0));
    desktop.reset_counts();
    desktop.order.reverse();
    registry.changes.order();
    let covered = registry.poll(ms(30), &mut desktop).unwrap().unwrap();
    assert!(!covered.supports()[0].visible_at(300.0));
    assert_eq!(desktop.scans, 1);
    assert!(desktop.reads.is_empty());
}

#[test]
fn new_windows_and_failed_queries_do_not_discard_unaffected_surfaces() {
    let mut desktop = Desktop::new(2);
    desktop.order = vec![1];
    let mut registry = Registry::default();
    let before = registry.poll(ms(0), &mut desktop).unwrap().unwrap();
    desktop.order.push(2);
    registry.changes.window(2, false);
    let added = registry.poll(ms(30), &mut desktop).unwrap().unwrap();
    assert_eq!(added.windows.len(), 2);
    assert_eq!(before.windows[0].key, added.windows[0].key);
    desktop.windows.remove(&2);
    registry.changes.window(2, false);
    let removed = registry.poll(ms(60), &mut desktop).unwrap().unwrap();
    assert_eq!(removed.windows.len(), 1);
    assert_eq!(before.windows[0].key, removed.windows[0].key);
}

#[test]
fn reconciliation_recovers_missed_close_and_identity_changes() {
    let mut desktop = Desktop::new(2);
    let mut registry = Registry::default();
    let before = registry.poll(ms(0), &mut desktop).unwrap().unwrap();
    desktop.windows.get_mut(&1).unwrap().process += 1;
    desktop.order.pop();
    let after = registry.poll(ms(1000), &mut desktop).unwrap().unwrap();
    assert_eq!(after.windows.len(), 1);
    assert_ne!(after.windows[0].key, before.windows[0].key);
}

#[test]
fn overflow_is_bounded_and_reconciliation_resets_uncertain_generations() {
    let mut desktop = Desktop::new(MAX_WINDOWS);
    let mut registry = Registry::default();
    let before = registry.poll(ms(0), &mut desktop).unwrap().unwrap();
    desktop.reset_counts();
    for id in 1..=10_000 {
        registry.changes.window(id, false);
    }
    let changed = registry.poll(ms(30), &mut desktop).unwrap().unwrap();
    assert_eq!(changed.windows.len(), MAX_WINDOWS);
    assert!(desktop.reads.len() <= MAX_WINDOWS);
    assert_eq!(desktop.scans, 1);
    let refreshed = registry.poll(ms(1000), &mut desktop).unwrap().unwrap();
    assert_ne!(before.windows[0].key, refreshed.windows[0].key);
    assert_eq!(refreshed.windows.len(), MAX_WINDOWS);
}

#[test]
fn failed_order_query_retains_pending_movement_for_retry() {
    let mut desktop = Desktop::new(1);
    let mut registry = Registry::default();
    registry.poll(ms(0), &mut desktop).unwrap();
    desktop.windows.get_mut(&1).unwrap().rect.top = 300.0;
    registry.changes.window(1, false);
    registry.changes.order();
    desktop.fail_order = true;
    assert!(registry.poll(ms(30), &mut desktop).is_err());
    desktop.fail_order = false;
    let recovered = registry.poll(ms(31), &mut desktop).unwrap().unwrap();
    assert_eq!(recovered.windows[0].rect.top, 300.0);
}

#[test]
fn fullscreen_and_taskbar_follow_foreground_and_geometry_events() {
    let mut desktop = Desktop::new(2);
    let fullscreen = desktop.windows.get_mut(&1).unwrap();
    fullscreen.collect = false;
    fullscreen.rect = Rect {
        left: 0.0,
        top: 0.0,
        right: 800.0,
        bottom: 600.0,
    };
    let taskbar = desktop.windows.get_mut(&2).unwrap();
    taskbar.taskbar = true;
    taskbar.rect = Rect {
        left: 0.0,
        top: 560.0,
        right: 800.0,
        bottom: 600.0,
    };
    let mut registry = Registry::default();
    let scene = registry.poll(ms(0), &mut desktop).unwrap().unwrap();
    assert_eq!(scene.screens[0].floor, 560.0);
    assert!(registry.fullscreen.contains(&1));
    desktop.foreground = 2;
    desktop.windows.get_mut(&2).unwrap().visible = false;
    registry.changes.window(2, false);
    registry.changes.order();
    let scene = registry.poll(ms(30), &mut desktop).unwrap().unwrap();
    assert_eq!(scene.screens[0].floor, 600.0);
    assert!(registry.fullscreen.is_empty());
    assert!(scene.screens[0].rect.contains(Point::new(10.0, 10.0)));
}

#[test]
fn taskbar_changes_refresh_work_area_and_keep_the_panel_as_a_noncollecting_cover() {
    let mut desktop = Desktop::new(1);
    desktop.screens[0].floor = 560.0;
    let bar = desktop.windows.get_mut(&1).unwrap();
    bar.taskbar = true;
    bar.rect = Rect {
        left: 0.0,
        top: 560.0,
        right: 800.0,
        bottom: 600.0,
    };
    let mut registry = Registry::default();
    let shown = registry.poll(ms(0), &mut desktop).unwrap().unwrap();
    assert_eq!(
        shown.windows.len(),
        1,
        "taskbar geometry must mask a moving floor"
    );
    assert!(!shown.windows[0].collect);
    desktop.reset_counts();
    desktop.screens[0].floor = 598.0;
    desktop.windows.get_mut(&1).unwrap().rect.top = 598.0;
    desktop.windows.get_mut(&1).unwrap().rect.bottom = 638.0;
    registry.changes.window(1, false);
    let hidden = registry.poll(ms(30), &mut desktop).unwrap().unwrap();
    assert_eq!(
        hidden.screens[0].floor, 598.0,
        "do not keep the old reserved work area"
    );
    assert_eq!(desktop.screen_queries, 1);
    assert_eq!(desktop.reads, [1]);
    desktop.screens[0].floor = 600.0;
    desktop.order.clear();
    registry.changes.window(1, true);
    let destroyed = registry.poll(ms(60), &mut desktop).unwrap().unwrap();
    assert_eq!(destroyed.screens[0].floor, 600.0);
    assert!(destroyed.windows.is_empty());
}

#[test]
fn taskbar_support_is_per_monitor_and_respects_other_reserved_work_area() {
    let mut desktop = Desktop::new(1);
    desktop.screens[0].floor = 580.0;
    desktop.screens.push(Screen {
        key: 2,
        persistent_id: true,
        rect: Rect {
            left: -800.0,
            top: 0.0,
            right: 0.0,
            bottom: 600.0,
        },
        floor: 550.0,
        scale: 2.0,
    });
    let bar = desktop.windows.get_mut(&1).unwrap();
    bar.taskbar = true;
    bar.rect = Rect {
        left: 0.0,
        top: 560.0,
        right: 800.0,
        bottom: 600.0,
    };
    let mut registry = Registry::default();
    let shown = registry.poll(ms(0), &mut desktop).unwrap().unwrap();
    assert_eq!(
        shown.screens.iter().map(|s| s.floor).collect::<Vec<_>>(),
        [560.0, 550.0]
    );
    for rect in [
        Rect {
            left: 0.0,
            top: 0.0,
            right: 40.0,
            bottom: 600.0,
        },
        Rect {
            left: 0.0,
            top: 0.0,
            right: 800.0,
            bottom: 40.0,
        },
    ] {
        desktop.windows.get_mut(&1).unwrap().rect = rect;
        registry.changes.window(1, false);
        let scene = registry
            .poll(ms(100 + rect.width() as u64), &mut desktop)
            .unwrap()
            .unwrap();
        assert_eq!(
            scene.screens.iter().map(|s| s.floor).collect::<Vec<_>>(),
            [580.0, 550.0]
        );
    }
}
