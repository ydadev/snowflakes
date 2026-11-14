//! Bounded render resources keyed by exact local shape, never by motion or window masks.
use crate::{
    geometry::{Point, Scene},
    snow::{Drift, SnowSlab},
};
use std::{collections::HashMap, rc::Rc};

pub(crate) const MAX_ENTRIES: usize = 4096;
pub(crate) const MAX_POINTS: usize = 131_072;
pub(crate) const MAX_KEY_WORDS: usize = 4100;
const STROKE_VARIANTS: usize = 4;

/// Promote repeated shapes only. Any optional builder failure disables promotion
/// for this shape's lifetime, avoiding a hot retry loop while preserving direct drawing.
pub(crate) struct Realizations<R> {
    uses: u32,
    disabled: bool,
    fill: Option<R>,
    strokes: Vec<(u32, R)>,
}
impl<R: Clone> Default for Realizations<R> {
    fn default() -> Self {
        Self {
            uses: 0,
            disabled: false,
            fill: None,
            strokes: Vec::new(),
        }
    }
}
impl<R: Clone> Realizations<R> {
    #[cfg(test)]
    pub fn ready(&self) -> (usize, usize) {
        if self.disabled {
            (0, 0)
        } else {
            (usize::from(self.fill.is_some()), self.strokes.len())
        }
    }
    pub fn note_use(&mut self) {
        self.uses = self.uses.saturating_add(1);
    }
    pub fn prepare(
        &mut self,
        eligible: bool,
        width: Option<f32>,
        fill: impl FnOnce() -> Option<R>,
        stroke: impl FnOnce(f32) -> Option<R>,
    ) -> Option<(R, Option<R>)> {
        if !eligible || self.disabled || self.uses < 2 {
            return None;
        }
        if self.fill.is_none() {
            let Some(value) = fill() else {
                self.disabled = true;
                return None;
            };
            self.fill = Some(value);
        }
        let outline = if let Some(width) = width {
            let key = width.to_bits();
            if let Some(i) = self.strokes.iter().position(|entry| entry.0 == key) {
                let entry = self.strokes.remove(i);
                self.strokes.insert(0, entry);
            } else {
                let Some(value) = stroke(width) else {
                    self.disabled = true;
                    return None;
                };
                self.strokes.insert(0, (key, value));
                self.strokes.truncate(STROKE_VARIANTS);
            }
            Some(self.strokes[0].1.clone())
        } else {
            None
        };
        Some((self.fill.as_ref().unwrap().clone(), outline))
    }
}

#[derive(Clone, Copy)]
pub(crate) enum Shape<'a> {
    Slab(&'a SnowSlab),
    Drift(&'a Drift, &'a Scene),
}
impl Shape<'_> {
    /// Fill a reused scratch buffer. A too-large profile is rendered without retention.
    pub fn key(self, scratch: &mut Vec<u32>) -> Option<usize> {
        scratch.clear();
        let count = match self {
            Self::Slab(s) => s.heights.len(),
            Self::Drift(d, _) => d.mass.len(),
        };
        if count.saturating_add(4) > MAX_KEY_WORDS {
            return None;
        }
        match self {
            Self::Slab(s) => {
                scratch.extend([
                    1,
                    s.cell_width.to_bits(),
                    s.center.x.to_bits(),
                    s.center.y.to_bits(),
                ]);
                scratch.extend(s.heights.iter().map(|v| v.to_bits()));
                Some(count.saturating_mul(25)) // conservative rounded-contour vertex bound
            }
            Self::Drift(d, scene) => {
                scratch.extend([
                    2,
                    (d.support.right - d.support.left).to_bits(),
                    d.cell_width.to_bits(),
                ]);
                scratch.extend((0..count).map(|i| d.height(i, scene).to_bits()));
                Some(count.saturating_add(2))
            }
        }
    }
    pub fn figures(self) -> Vec<Vec<Point>> {
        match self {
            Self::Slab(s) => s.contours().collect(),
            Self::Drift(d, scene) => {
                let mut points = Vec::with_capacity(d.mass.len() + 2);
                points.push(Point::default());
                points.extend(
                    (0..d.mass.len())
                        .map(|i| Point::new((i as f32 + 0.5) * d.cell_width, -d.height(i, scene))),
                );
                points.push(Point::new(d.support.right - d.support.left, 0.0));
                vec![points]
            }
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Stats {
    pub hits: u64,
    pub builds: u64,
    pub bypasses: u64,
    pub evictions: u64,
}
struct Entry<V> {
    key: Rc<[u32]>,
    value: V,
    points: usize,
    frame: u64,
    previous: Option<usize>,
    next: Option<usize>,
}
pub(crate) struct Cache<V> {
    index: HashMap<Rc<[u32]>, usize>,
    entries: Vec<Option<Entry<V>>>,
    free: Vec<usize>,
    first: Option<usize>,
    last: Option<usize>,
    frame: u64,
    points: usize,
    max_entries: usize,
    max_points: usize,
    pub stats: Stats,
}
impl<V: Clone> Cache<V> {
    pub fn new(max_entries: usize, max_points: usize) -> Self {
        Self {
            index: HashMap::with_capacity(max_entries),
            entries: Vec::new(),
            free: Vec::new(),
            first: None,
            last: None,
            frame: 0,
            points: 0,
            max_entries,
            max_points,
            stats: Stats::default(),
        }
    }
    pub fn begin_frame(&mut self) {
        if self.frame == u64::MAX {
            self.clear();
            self.frame = 0;
        }
        self.frame += 1;
    }
    pub fn clear(&mut self) {
        self.index.clear();
        self.entries.clear();
        self.free.clear();
        self.first = None;
        self.last = None;
        self.points = 0;
    }
    pub fn end_frame(&mut self) {
        // LRU order groups untouched older frames at the tail. No full-map scan.
        while let Some(last) = self.last {
            if self.entries[last].as_ref().unwrap().frame == self.frame {
                break;
            }
            self.evict(last);
        }
    }
    fn detach(&mut self, i: usize) {
        let entry = self.entries[i].as_ref().unwrap();
        let (previous, next) = (entry.previous, entry.next);
        if let Some(p) = previous {
            self.entries[p].as_mut().unwrap().next = next;
        } else {
            self.first = next;
        }
        if let Some(n) = next {
            self.entries[n].as_mut().unwrap().previous = previous;
        } else {
            self.last = previous;
        }
    }
    fn touch(&mut self, i: usize) {
        if self.first == Some(i) {
            return;
        }
        self.detach(i);
        let first = self.first;
        let entry = self.entries[i].as_mut().unwrap();
        entry.previous = None;
        entry.next = first;
        if let Some(first) = first {
            self.entries[first].as_mut().unwrap().previous = Some(i);
        } else {
            self.last = Some(i);
        }
        self.first = Some(i);
    }
    fn evict(&mut self, i: usize) {
        self.detach(i);
        let entry = self.entries[i].take().unwrap();
        self.points -= entry.points;
        self.index.remove(entry.key.as_ref());
        self.free.push(i);
        self.stats.evictions = self.stats.evictions.saturating_add(1);
    }
    pub fn get<E>(
        &mut self,
        key: &[u32],
        points: usize,
        build: impl FnOnce() -> Result<V, E>,
    ) -> Result<V, E> {
        if let Some(&i) = self.index.get(key) {
            let entry = self.entries[i].as_mut().unwrap();
            entry.frame = self.frame;
            let value = entry.value.clone();
            self.touch(i);
            self.stats.hits = self.stats.hits.saturating_add(1);
            return Ok(value);
        }
        // Failure does not evict a still-valid cached resource.
        let value = build()?;
        self.stats.builds = self.stats.builds.saturating_add(1);
        if self.max_entries == 0 || points > self.max_points || key.len() > MAX_KEY_WORDS {
            self.stats.bypasses = self.stats.bypasses.saturating_add(1);
            return Ok(value);
        }
        while self.index.len() >= self.max_entries
            || self.points.saturating_add(points) > self.max_points
        {
            self.evict(self.last.unwrap());
        }
        let key: Rc<[u32]> = key.into();
        let i = self.free.pop().unwrap_or_else(|| {
            self.entries.push(None);
            self.entries.len() - 1
        });
        let next = self.first;
        self.entries[i] = Some(Entry {
            key: key.clone(),
            value: value.clone(),
            points,
            frame: self.frame,
            previous: None,
            next,
        });
        if let Some(next) = next {
            self.entries[next].as_mut().unwrap().previous = Some(i);
        } else {
            self.last = Some(i);
        }
        self.first = Some(i);
        self.index.insert(key, i);
        self.points += points;
        Ok(value)
    }
    #[cfg(test)]
    pub fn usage(&self) -> (usize, usize) {
        (self.index.len(), self.points)
    }
    #[cfg(test)]
    pub fn values(&self) -> impl Iterator<Item = &V> {
        self.entries
            .iter()
            .filter_map(|entry| entry.as_ref().map(|entry| &entry.value))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        geometry::{Rect, Screen},
        snow::World,
    };
    use std::cell::Cell;

    #[test]
    fn exact_local_shape_reuses_resources_and_changed_profile_gets_a_new_one() {
        let mut slab = SnowSlab {
            heights: vec![2.0, 5.0, 0.0, 7.0],
            cell_width: 6.0,
            center: Point::new(12.0, -2.0),
        };
        let mut key = Vec::new();
        let mut cache = Cache::new(4, 1000);
        cache.begin_frame();
        let mut created = 0;
        for _ in 0..100 {
            let cost = Shape::Slab(&slab).key(&mut key).unwrap();
            let value = cache
                .get(&key, cost, || {
                    created += 1;
                    Ok::<_, ()>(created)
                })
                .unwrap();
            assert_eq!(value, 1);
        }
        slab.heights[1] = f32::from_bits(slab.heights[1].to_bits() + 1);
        let cost = Shape::Slab(&slab).key(&mut key).unwrap();
        assert_eq!(
            cache
                .get(&key, cost, || {
                    created += 1;
                    Ok::<_, ()>(created)
                })
                .unwrap(),
            2
        );
        assert_eq!(created, 2);
        assert_eq!(cache.stats.hits, 99);
        assert_eq!(
            Shape::Slab(&slab).figures(),
            slab.contours().collect::<Vec<_>>()
        );
        assert_eq!(
            Shape::Slab(&slab).figures().len(),
            2,
            "empty runs must not be bridged"
        );
    }

    #[test]
    fn drift_key_ignores_motion_masks_and_shadow_but_tracks_physical_height_and_spacing() {
        let scene = Scene {
            screens: vec![Screen {
                key: 1,
                persistent_id: true,
                rect: Rect {
                    left: -800.0,
                    top: -600.0,
                    right: 0.0,
                    bottom: 0.0,
                },
                floor: -40.0,
                scale: 2.5,
            }],
            windows: vec![],
        };
        let mut world = World::new(scene, 701);
        let key = *world.drifts.keys().next().unwrap();
        let drift = world.drifts.get_mut(&key).unwrap();
        drift.mass.fill(3.0 * drift.cell_width);
        let mut before = Vec::new();
        let cost = Shape::Drift(drift, &world.scene).key(&mut before).unwrap();
        let figures = Shape::Drift(drift, &world.scene).figures();
        assert_eq!(figures.len(), 1);
        for (i, p) in figures[0][1..figures[0].len() - 1].iter().enumerate() {
            assert!((drift.support.left + p.x - drift.x(i)).abs() < 0.0001);
            assert_eq!(p.y, -drift.height(i, &world.scene));
        }
        drift.support.left += 200.0;
        drift.support.right += 200.0;
        drift.support.y += 30.0;
        drift.support.draw_clips.clear();
        let mut after = Vec::new();
        assert_eq!(
            Shape::Drift(drift, &world.scene).key(&mut after),
            Some(cost)
        );
        assert_eq!(before, after);
        drift.mass[0] += 1.0;
        Shape::Drift(drift, &world.scene).key(&mut after);
        assert_ne!(before, after);
        let changed = after.clone();
        drift.cell_width *= 1.25;
        Shape::Drift(drift, &world.scene).key(&mut after);
        assert_ne!(changed, after);
    }

    #[test]
    fn lru_eviction_frame_expiry_failure_and_oversize_keep_resources_bounded() {
        let mut cache = Cache::new(2, 6);
        cache.begin_frame();
        for key in [1, 2] {
            cache.get(&[key], 3, || Ok::<_, ()>(key)).unwrap();
        }
        cache
            .get::<()>(&[1], 3, || panic!("expected cached resource"))
            .unwrap();
        cache.get(&[3], 3, || Ok::<_, ()>(3)).unwrap();
        assert_eq!(cache.usage(), (2, 6));
        assert_eq!(
            cache.get(&[4], 3, || Err::<u32, _>("builder_failed")),
            Err("builder_failed")
        );
        cache
            .get::<()>(&[1], 3, || panic!("failure evicted a valid entry"))
            .unwrap();
        cache.get(&[99], 100, || Ok::<_, ()>(99)).unwrap();
        assert_eq!(cache.usage(), (2, 6));
        assert_eq!(cache.stats.bypasses, 1);
        cache.begin_frame();
        cache
            .get::<()>(&[3], 3, || panic!("most recent entry missing"))
            .unwrap();
        cache.end_frame();
        assert_eq!(cache.usage(), (1, 3));
        cache.begin_frame();
        cache.end_frame();
        assert_eq!(cache.usage(), (0, 0));
    }

    #[test]
    fn thousands_of_births_and_deaths_release_resources_and_reuse_arena_slots() {
        struct Resource(Rc<Cell<usize>>);
        impl Drop for Resource {
            fn drop(&mut self) {
                self.0.set(self.0.get() - 1);
            }
        }
        let live = Rc::new(Cell::new(0));
        let mut cache = Cache::new(7, 35);
        for frame in 0..1000 {
            cache.begin_frame();
            for index in 0..21 {
                cache
                    .get(&[frame, index % 11], 5, || {
                        live.set(live.get() + 1);
                        Ok::<_, ()>(Rc::new(Resource(live.clone())))
                    })
                    .unwrap();
                assert!(live.get() <= 7);
                assert!(cache.usage().0 <= 7 && cache.usage().1 <= 35);
                assert!(cache.entries.len() <= 7);
            }
            cache.end_frame();
        }
        cache.clear();
        assert_eq!(live.get(), 0);
        assert_eq!(cache.usage(), (0, 0));
    }

    #[test]
    fn promotion_is_lazy_stroke_variants_are_bounded_and_optional_failure_does_not_retry() {
        let mut state = Realizations::<u32>::default();
        state.note_use();
        assert!(
            state
                .prepare(
                    true,
                    Some(1.5),
                    || panic!("one-off fill"),
                    |_| panic!("one-off stroke")
                )
                .is_none()
        );
        state.note_use();
        assert_eq!(
            state.prepare(true, Some(1.5), || Some(10), |_| Some(20)),
            Some((10, Some(20)))
        );
        for width in [1.5, 1.2, 1.0, 0.75, 0.6, 0.5] {
            state.prepare(
                true,
                Some(width),
                || panic!("fill was rebuilt"),
                |_| Some(30),
            );
            assert!(state.strokes.len() <= 4);
        }
        assert!(
            state
                .prepare(true, Some(0.25), || unreachable!(), |_| None)
                .is_none()
        );
        for _ in 0..100 {
            state.note_use();
            assert!(
                state
                    .prepare(
                        true,
                        Some(1.5),
                        || panic!("fill retry"),
                        |_| panic!("stroke retry")
                    )
                    .is_none()
            );
        }
        let mut failed_fill = Realizations::<u32>::default();
        failed_fill.note_use();
        failed_fill.note_use();
        assert!(
            failed_fill
                .prepare(true, None, || None, |_| unreachable!())
                .is_none()
        );
        assert!(
            failed_fill
                .prepare(true, None, || panic!("fill retry"), |_| unreachable!())
                .is_none()
        );
        let mut unsupported = Realizations::<u32>::default();
        unsupported.note_use();
        unsupported.note_use();
        assert!(
            unsupported
                .prepare(
                    false,
                    Some(1.5),
                    || panic!("unsupported fill"),
                    |_| panic!("unsupported stroke")
                )
                .is_none()
        );
    }
}
