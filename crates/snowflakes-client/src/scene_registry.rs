//! Bounded event coalescing and an incremental desktop registry.
//! The source supplies geometry only; no captions or process names are retained.
use crate::geometry::{Rect, Scene, Screen, Window};
use std::{
    collections::{BTreeMap, BTreeSet},
    time::Duration,
};

pub const MAX_WINDOWS: usize = 512;
const MAX_PENDING: usize = 2048;
const REFRESH: Duration = Duration::from_secs(1);
const COALESCE: Duration = Duration::from_millis(30);

#[derive(Clone, Copy, Default)]
struct Update {
    reset: bool,
    read: bool,
}

#[derive(Default)]
pub struct Changes {
    windows: BTreeMap<usize, Update>,
    order: bool,
    all: bool,
    overflow: bool,
}
impl Changes {
    pub fn window(&mut self, handle: usize, destroyed: bool) {
        if self.windows.len() >= MAX_PENDING && !self.windows.contains_key(&handle) {
            self.overflow = true;
            return;
        }
        let entry = self.windows.entry(handle).or_default();
        entry.reset |= destroyed;
        entry.read = !destroyed;
    }
    pub fn order(&mut self) {
        self.order = true;
    }
    pub fn all(&mut self) {
        self.all = true;
        self.order = true;
    }
    pub fn merge(&mut self, other: Self) {
        self.order |= other.order;
        self.all |= other.all;
        self.overflow |= other.overflow;
        for (handle, update) in other.windows {
            if update.reset {
                self.window(handle, true);
            }
            if update.read {
                self.window(handle, false);
            }
        }
    }
    fn is_empty(&self) -> bool {
        self.windows.is_empty() && !self.order && !self.all && !self.overflow
    }
}

#[derive(Clone)]
pub struct WindowInfo {
    pub handle: usize,
    pub process: u32,
    pub thread: u32,
    pub rect: Rect,
    pub visible: bool,
    pub collect: bool,
    pub taskbar: bool,
}
pub trait Source {
    type Error;
    /// Front to back, bounded to MAX_WINDOWS by the native adapter.
    fn order(&mut self) -> Result<Vec<usize>, Self::Error>;
    fn read(&mut self, handle: usize) -> Option<WindowInfo>;
    fn screens(&mut self) -> Result<Vec<Screen>, Self::Error>;
    fn foreground(&mut self) -> usize;
}
struct Entry {
    info: WindowInfo,
    key: u64,
}
#[derive(Default)]
pub struct Registry {
    pub changes: Changes,
    entries: BTreeMap<usize, Entry>,
    order: Vec<usize>,
    screens: Vec<Screen>,
    serial: u64,
    last_poll: Option<Duration>,
    last_full: Option<Duration>,
    pub fullscreen: BTreeSet<u64>,
}
impl Registry {
    fn read(&mut self, source: &mut impl Source, handle: usize) {
        let Some(info) = source.read(handle) else {
            self.entries.remove(&handle);
            return;
        };
        if !self.entries.contains_key(&handle) && self.entries.len() >= MAX_WINDOWS {
            return;
        }
        let key = self
            .entries
            .get(&handle)
            .filter(|old| old.info.process == info.process && old.info.thread == info.thread)
            .map(|old| old.key)
            .unwrap_or_else(|| {
                self.serial += 1;
                self.serial
            });
        self.entries.insert(handle, Entry { info, key });
    }
    pub fn poll<S: Source>(
        &mut self,
        now: Duration,
        source: &mut S,
    ) -> Result<Option<Scene>, S::Error> {
        let full = self
            .last_full
            .is_none_or(|last| now.saturating_sub(last) >= REFRESH);
        if !full
            && (self.changes.is_empty()
                || self
                    .last_poll
                    .is_some_and(|last| now.saturating_sub(last) < COALESCE))
        {
            return Ok(None);
        }
        // Obtain fallible snapshots before consuming events. A failed query retains
        // pending changes; the periodic scan is still attempted at most once/sec.
        if full {
            self.last_full = Some(now);
        }
        let reorder = full
            || self.changes.order
            || self.changes.all
            || self
                .changes
                .windows
                .keys()
                .any(|id| !self.entries.contains_key(id));
        let order = if reorder { Some(source.order()?) } else { None };
        let screens = if full || self.changes.all {
            Some(source.screens()?)
        } else {
            None
        };
        self.last_poll = Some(now);
        let mut changes = std::mem::take(&mut self.changes);
        // If destroy notifications were lost to overflow, never reuse an uncertain
        // generation after reconciliation. Keep this bounded under event storms.
        if changes.overflow {
            if full {
                self.entries.clear();
            } else {
                self.changes.overflow = true;
            }
        }
        for (handle, update) in &changes.windows {
            if update.reset {
                self.entries.remove(handle);
            }
        }
        if let Some(screens) = screens {
            self.screens = screens;
        }
        if let Some(mut order) = order {
            order.truncate(MAX_WINDOWS);
            let seen: BTreeSet<_> = order.iter().copied().collect();
            self.entries.retain(|id, _| seen.contains(id));
            for &handle in &order {
                if full || changes.all || !self.entries.contains_key(&handle) {
                    self.read(source, handle);
                    changes.windows.remove(&handle);
                }
            }
            self.order = order;
        }
        for (handle, update) in changes.windows {
            if update.read && self.order.contains(&handle) {
                self.read(source, handle);
            }
        }
        let foreground = source.foreground();
        self.fullscreen.clear();
        let mut scene = Scene {
            screens: self.screens.clone(),
            windows: Vec::new(),
        };
        for handle in &self.order {
            let Some(entry) = self.entries.get(handle) else {
                continue;
            };
            let item = &entry.info;
            if *handle == foreground && !item.collect && item.visible && !item.taskbar {
                for screen in &scene.screens {
                    if item.rect.left <= screen.rect.left + 1.0
                        && item.rect.top <= screen.rect.top + 1.0
                        && item.rect.right >= screen.rect.right - 1.0
                        && item.rect.bottom >= screen.rect.bottom - 1.0
                    {
                        self.fullscreen.insert(screen.key);
                    }
                }
            }
            if item.taskbar {
                if item.visible && item.rect.width() > item.rect.height() {
                    for screen in &mut scene.screens {
                        if item.rect.right > screen.rect.left
                            && item.rect.left < screen.rect.right
                            && item.rect.top > screen.rect.top + screen.rect.height() * 0.5
                            && item.rect.bottom >= screen.rect.bottom - 4.0
                            && screen.rect.bottom - item.rect.top > 8.0
                        {
                            screen.floor = screen.floor.min(item.rect.top);
                        }
                    }
                }
            } else {
                scene.windows.push(Window {
                    key: entry.key,
                    rect: item.rect,
                    visible: item.visible,
                    collect: item.collect,
                });
            }
        }
        Ok(Some(scene))
    }
}
