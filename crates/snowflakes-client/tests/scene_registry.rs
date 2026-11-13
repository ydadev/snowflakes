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
    fn foreground(&mut self) -> usize {
        self.foreground
    }
    fn screens(&mut self) -> Result<Vec<Screen>, ()> {
        self.screen_queries += 1;
        Ok(vec![Screen {
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
        }])
    }
}
fn ms(value: u64) -> Duration {
    Duration::from_millis(value)
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
