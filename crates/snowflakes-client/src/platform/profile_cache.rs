//! GPU-thread-only local paths and device-bound tessellations. No desktop interaction.
#[cfg(test)]
use crate::profile_cache::Stats;
use crate::profile_cache::{Cache, MAX_ENTRIES, MAX_POINTS, Realizations, Shape};
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
};
use windows::{
    Win32::Graphics::Direct2D::{Common::*, *},
    core::{Interface, Result},
};
use windows_numerics::Vector2;

const FLATTENING_TOLERANCE: f32 = 1.0 / 32.0; // at most 1/4 pixel through 8x scaling
const SAFE_COORDINATE: f32 = 524_000.0; // realization pre-transform safe range, with stroke margin

#[derive(Default)]
struct Counters {
    fills: Cell<u64>,
    strokes: Cell<u64>,
}
pub(super) struct Profile {
    path: ID2D1PathGeometry1,
    safe: bool,
    realizations: RefCell<Realizations<ID2D1GeometryRealization>>,
    counters: Rc<Counters>,
}
impl Profile {
    fn new(factory: &ID2D1Factory1, shape: Shape<'_>, counters: Rc<Counters>) -> Result<Self> {
        unsafe {
            let path = factory.CreatePathGeometry()?;
            let sink = path.Open()?;
            let mut safe = true;
            for figure in shape.figures() {
                let Some(first) = figure.first() else {
                    continue;
                };
                sink.BeginFigure(
                    Vector2 {
                        X: first.x,
                        Y: first.y,
                    },
                    D2D1_FIGURE_BEGIN_FILLED,
                );
                for p in &figure {
                    safe &= p.x.is_finite()
                        && p.y.is_finite()
                        && p.x.abs() < SAFE_COORDINATE
                        && p.y.abs() < SAFE_COORDINATE;
                }
                for p in &figure[1..] {
                    sink.AddLine(Vector2 { X: p.x, Y: p.y });
                }
                sink.EndFigure(D2D1_FIGURE_END_CLOSED);
            }
            sink.Close()?;
            Ok(Self {
                path,
                safe,
                realizations: RefCell::default(),
                counters,
            })
        }
    }
    /// The first use draws the path directly: one-off changing profiles do not pay for realizations.
    pub fn paint(
        &self,
        context: &ID2D1DeviceContext,
        realized: Option<&ID2D1DeviceContext1>,
        fill_brush: &ID2D1LinearGradientBrush,
        stroke_brush: &ID2D1SolidColorBrush,
        stroke_width: Option<f32>,
    ) {
        unsafe {
            let cached = self.realizations.borrow_mut().prepare(
                self.safe && realized.is_some(),
                stroke_width,
                || {
                    self.counters
                        .fills
                        .set(self.counters.fills.get().saturating_add(1));
                    realized?
                        .CreateFilledGeometryRealization(&self.path, FLATTENING_TOLERANCE)
                        .ok()
                },
                |width| {
                    self.counters
                        .strokes
                        .set(self.counters.strokes.get().saturating_add(1));
                    realized?
                        .CreateStrokedGeometryRealization(
                            &self.path,
                            FLATTENING_TOLERANCE,
                            width,
                            None,
                        )
                        .ok()
                },
            );
            if let Some(width) = stroke_width {
                if let (Some(dc), Some((_, Some(stroke)))) = (realized, &cached) {
                    dc.DrawGeometryRealization(stroke, stroke_brush);
                } else {
                    context.DrawGeometry(&self.path, stroke_brush, width, None);
                }
            }
            if let (Some(dc), Some((fill, _))) = (realized, &cached) {
                dc.DrawGeometryRealization(fill, fill_brush);
            } else {
                context.FillGeometry(&self.path, fill_brush, None);
            }
        }
    }
}
pub(super) struct Profiles {
    factory: ID2D1Factory1,
    realized: Option<ID2D1DeviceContext1>,
    cache: RefCell<Cache<Rc<Profile>>>,
    scratch: RefCell<Vec<u32>>,
    counters: Rc<Counters>,
}
impl Profiles {
    pub fn new(factory: &ID2D1Factory1, context: &ID2D1DeviceContext) -> Self {
        Self {
            factory: factory.clone(),
            realized: context.cast().ok(),
            cache: RefCell::new(Cache::new(MAX_ENTRIES, MAX_POINTS)),
            scratch: RefCell::new(Vec::new()),
            counters: Rc::default(),
        }
    }
    pub fn begin_frame(&self) {
        self.cache.borrow_mut().begin_frame();
    }
    pub fn end_frame(&self) {
        self.cache.borrow_mut().end_frame();
    }
    pub fn clear(&self) {
        self.cache.borrow_mut().clear();
    }
    pub fn get(&self, shape: Shape<'_>) -> Result<Rc<Profile>> {
        let mut scratch = self.scratch.borrow_mut();
        let points = shape.key(&mut scratch).unwrap_or(usize::MAX);
        let profile = self.cache.borrow_mut().get(&scratch, points, || {
            Profile::new(&self.factory, shape, self.counters.clone()).map(Rc::new)
        })?;
        profile.realizations.borrow_mut().note_use();
        Ok(profile)
    }
    pub fn paint(
        &self,
        profile: &Profile,
        context: &ID2D1DeviceContext,
        fill: &ID2D1LinearGradientBrush,
        stroke: &ID2D1SolidColorBrush,
        stroke_width: Option<f32>,
    ) {
        profile.paint(context, self.realized.as_ref(), fill, stroke, stroke_width);
    }
    #[cfg(test)]
    pub fn disable_realizations(&mut self) {
        self.clear();
        self.realized = None;
    }
    #[cfg(test)]
    pub fn stats(&self) -> (Stats, u64, u64, (usize, usize)) {
        let cache = self.cache.borrow();
        (
            cache.stats,
            self.counters.fills.get(),
            self.counters.strokes.get(),
            cache.usage(),
        )
    }
    #[cfg(test)]
    pub fn ready(&self) -> (usize, usize) {
        self.cache
            .borrow()
            .values()
            .map(|profile| profile.realizations.borrow().ready())
            .fold((0, 0), |total, entry| {
                (total.0 + entry.0, total.1 + entry.1)
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{geometry::Point, snow::SnowSlab};

    #[test]
    fn native_local_paths_reuse_exact_figures_without_a_device_or_overlay() {
        let factory: ID2D1Factory1 =
            unsafe { D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None) }.unwrap();
        let profiles = Profiles {
            factory,
            realized: None,
            cache: RefCell::new(Cache::new(MAX_ENTRIES, MAX_POINTS)),
            scratch: RefCell::new(Vec::new()),
            counters: Rc::default(),
        };
        let mut slab = SnowSlab {
            heights: vec![3.0, 8.0, 0.0, 0.0, 9.0, 4.0],
            cell_width: 5.0,
            center: Point::new(15.0, -2.0),
        };
        profiles.begin_frame();
        let first = profiles.get(Shape::Slab(&slab)).unwrap();
        assert_eq!(unsafe { first.path.GetFigureCount() }.unwrap(), 2);
        let bounds = unsafe { first.path.GetBounds(None) }.unwrap();
        for p in slab.contours().flatten() {
            assert!(p.x >= bounds.left - 0.0001 && p.x <= bounds.right + 0.0001);
            assert!(p.y >= bounds.top - 0.0001 && p.y <= bounds.bottom + 0.0001);
        }
        for _ in 0..20 {
            assert_eq!(
                profiles.get(Shape::Slab(&slab)).unwrap().path.as_raw(),
                first.path.as_raw()
            );
        }
        slab.heights[0] += 1.0;
        let changed = profiles.get(Shape::Slab(&slab)).unwrap();
        assert_ne!(changed.path.as_raw(), first.path.as_raw());
        let (stats, fills, strokes, usage) = profiles.stats();
        assert_eq!((stats.builds, stats.hits, fills, strokes), (2, 20, 0, 0));
        assert_eq!(usage.0, 2);
        profiles.clear();
        assert_eq!(profiles.stats().3, (0, 0));
    }
}
