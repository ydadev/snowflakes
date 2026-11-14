//! Traverse the existing rounded contact contours without building their Vecs.
use super::{Point, SnowSlab};

#[cfg(test)]
thread_local! { static CORNERS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) }; }

#[cfg(test)]
pub(super) fn constructed_corner() {
    CORNERS.set(CORNERS.get() + 1);
}

impl SnowSlab {
    pub(super) fn contact_outlines<'a, const LAZY: bool>(
        &'a self,
    ) -> impl Iterator<Item = (Point, impl DoubleEndedIterator<Item = Point> + 'a)> + 'a {
        let mut next = 0;
        let occupied = |i: usize| self.heights[i].is_finite() && self.heights[i] > 0.0;
        std::iter::from_fn(move || {
            while next < self.heights.len() && !occupied(next) {
                next += 1;
            }
            if next == self.heights.len() {
                return None;
            }
            let first = next;
            while next < self.heights.len() && occupied(next) {
                next += 1;
            }
            let end = next;
            let points =
                (0..end - first + 4).flat_map(move |index| Corner::new(self, first, end, index));
            #[cfg(test)]
            let mut points = if LAZY {
                Outline::Lazy(points)
            } else {
                Outline::Literal(self.outline_range(first, end).into_iter())
            };
            #[cfg(not(test))]
            let mut points = Outline::Lazy(points);
            // The physics walk starts at the last point and then visits the
            // complete original sequence, including this closing point.
            let last = points.next_back().unwrap();
            Some((last, points.chain(std::iter::once(last))))
        })
    }
}

enum Outline<I> {
    Lazy(I),
    #[cfg(test)]
    Literal(std::vec::IntoIter<Point>),
}
impl<I: DoubleEndedIterator<Item = Point>> Iterator for Outline<I> {
    type Item = Point;
    fn next(&mut self) -> Option<Point> {
        match self {
            Self::Lazy(points) => points.next(),
            #[cfg(test)]
            Self::Literal(points) => points.next(),
        }
    }
}
impl<I: DoubleEndedIterator<Item = Point>> DoubleEndedIterator for Outline<I> {
    fn next_back(&mut self) -> Option<Point> {
        match self {
            Self::Lazy(points) => points.next_back(),
            #[cfg(test)]
            Self::Literal(points) => points.next_back(),
        }
    }
}

struct Corner {
    entry: Point,
    point: Point,
    exit: Point,
    front: u8,
    back: u8,
    rounded: bool,
}
impl Corner {
    fn new(slab: &SnowSlab, first: usize, end: usize, index: usize) -> Self {
        #[cfg(test)]
        constructed_corner();
        let count = end - first;
        // Same unrounded corner positions as SnowSlab::outline_range.
        let raw = |index: usize| {
            if index == 0 {
                Point::new(
                    first as f32 * slab.cell_width - slab.center.x,
                    -slab.center.y,
                )
            } else if index == 1 {
                Point::new(
                    first as f32 * slab.cell_width - slab.center.x,
                    -slab.heights[first] - slab.center.y,
                )
            } else if index < count + 2 {
                let cell = first + (index - 2);
                Point::new(
                    (cell as f32 + 0.5) * slab.cell_width - slab.center.x,
                    -slab.heights[cell] - slab.center.y,
                )
            } else if index == count + 2 {
                Point::new(
                    end as f32 * slab.cell_width - slab.center.x,
                    -slab.heights[end - 1] - slab.center.y,
                )
            } else {
                Point::new(end as f32 * slab.cell_width - slab.center.x, -slab.center.y)
            }
        };
        let point = raw(index);
        let previous = raw(if index == 0 { count + 3 } else { index - 1 });
        let next = raw(if index == count + 3 { 0 } else { index + 1 });
        let a = Point::new(previous.x - point.x, previous.y - point.y);
        let b = Point::new(next.x - point.x, next.y - point.y);
        let la = a.x.hypot(a.y);
        let lb = b.x.hypot(b.y);
        if la < 0.001 || lb < 0.001 || (a.x * b.x + a.y * b.y) / la / lb < -0.98 {
            return Self {
                entry: point,
                point,
                exit: point,
                front: 0,
                back: 1,
                rounded: false,
            };
        }
        let cut = (la.min(lb) * 0.4).min(4.0);
        let entry = Point::new(point.x + a.x * cut / la, point.y + a.y * cut / la);
        let exit = Point::new(point.x + b.x * cut / lb, point.y + b.y * cut / lb);
        Self {
            entry,
            point,
            exit,
            front: 0,
            back: 5,
            rounded: true,
        }
    }
    fn at(&self, step: u8) -> Point {
        if !self.rounded {
            return self.point;
        }
        let t = step as f32 / 4.0;
        let u = 1.0 - t;
        Point::new(
            u * u * self.entry.x + 2.0 * u * t * self.point.x + t * t * self.exit.x,
            u * u * self.entry.y + 2.0 * u * t * self.point.y + t * t * self.exit.y,
        )
    }
}
impl Iterator for Corner {
    type Item = Point;
    fn next(&mut self) -> Option<Point> {
        if self.front == self.back {
            return None;
        }
        let step = self.front;
        self.front += 1;
        Some(self.at(step))
    }
    fn size_hint(&self) -> (usize, Option<usize>) {
        let count = usize::from(self.back - self.front);
        (count, Some(count))
    }
}
impl DoubleEndedIterator for Corner {
    fn next_back(&mut self) -> Option<Point> {
        if self.front == self.back {
            return None;
        }
        self.back -= 1;
        Some(self.at(self.back))
    }
}
impl ExactSizeIterator for Corner {}

#[cfg(test)]
#[path = "lazy_outline_tests.rs"]
mod tests;
