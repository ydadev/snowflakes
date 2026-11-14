//! Contacts of a contour edge whose endpoints move linearly during one step.
use crate::geometry::{Point, Rect, Scene};
use std::collections::BTreeSet;

mod angular;
pub(super) use angular::AngularMotion;

pub(super) trait EdgeSweep {
    fn bounds(&self) -> Rect;
    fn contact(&self, y: f32, base: f32, span: [f32; 2]) -> Option<EdgeHit>;
}

#[derive(Clone, Copy)]
pub(super) struct SweptEdge {
    pub before: [Point; 2],
    pub after: [Point; 2],
}

#[derive(Clone, Copy)]
pub(super) struct EdgeHit {
    pub time: f32,
    pub x: f32,
}

impl SweptEdge {
    pub fn bounds(self) -> Rect {
        let [a, b] = self.before;
        let [c, d] = self.after;
        Rect {
            left: a.x.min(b.x).min(c.x).min(d.x),
            right: a.x.max(b.x).max(c.x).max(d.x),
            top: a.y.min(b.y).min(c.y).min(d.y),
            bottom: a.y.max(b.y).max(c.y).max(d.y),
        }
    }

    /// `span` contains closed, representable x coordinates. The caller clips
    /// the cell, visibility, exposure and monitor ownership before this call.
    pub fn contact(self, y: f32, base: f32, span: [f32; 2]) -> Option<EdgeHit> {
        let a = [f64::from(self.before[0].x), f64::from(self.before[0].y)];
        let b = [f64::from(self.before[1].x), f64::from(self.before[1].y)];
        let va = [
            f64::from(self.after[0].x) - a[0],
            f64::from(self.after[0].y) - a[1],
        ];
        let vb = [
            f64::from(self.after[1].x) - b[0],
            f64::from(self.after[1].y) - b[1],
        ];
        if va[1].max(vb[1]) <= 0.0 || span[0] > span[1] {
            return None;
        }
        let y = f64::from(y);
        let mut initial = [0.0, 1.0];
        if clip_linear(&mut initial, a[0], b[0], span.map(f64::from))
            && clip_linear(&mut initial, a[1], b[1], [y, f64::from(base)])
            && clip_linear(&mut initial, va[1], vb[1], [0.0, f64::INFINITY])
        {
            let u = (initial[0] + initial[1]) * 0.5;
            if va[1] + (vb[1] - va[1]) * u > 0.0 {
                return Some(EdgeHit {
                    time: 0.0,
                    x: (a[0] + (b[0] - a[0]) * u) as f32,
                });
            }
        }
        let e = [b[0] - a[0], b[1] - a[1]];
        let de = [vb[0] - va[0], vb[1] - va[1]];
        let mut best: Option<(f64, f64)> = None;
        let mut consider = |time: f64, u: f64| {
            if !(0.0..=1.0).contains(&time) || !(0.0..=1.0).contains(&u) {
                return;
            }
            let start_y = a[1] + e[1] * u;
            let dy = va[1] + de[1] * u;
            let x = a[0] + e[0] * u + (va[0] + de[0] * u) * time;
            if dy > 0.0
                && start_y <= y
                && x as f32 >= span[0]
                && x as f32 <= span[1]
                && best.is_none_or(|(old, _)| time < old)
            {
                best = Some((time, x));
            }
        };
        // An interval of possible contacts first appears at an endpoint of
        // the moving edge or an endpoint of the horizontal receiving span.
        for (p, v, u) in [(a, va, 0.0), (b, vb, 1.0)] {
            if v[1] > 0.0 {
                consider((y - p[1]) / v[1], u);
            }
        }
        for x in span.map(f64::from) {
            let c = [x - a[0], y - a[1]];
            for time in roots(-cross(va, de), cross(c, de) - cross(va, e), cross(c, e))
                .into_iter()
                .flatten()
            {
                if !(0.0..=1.0).contains(&time) {
                    continue;
                }
                let edge = [e[0] + de[0] * time, e[1] + de[1] * time];
                let offset = [c[0] - va[0] * time, c[1] - va[1] * time];
                let axis = usize::from(edge[1].abs() > edge[0].abs());
                if edge[axis] != 0.0 {
                    let u = offset[axis] / edge[axis];
                    let other = 1 - axis;
                    let residual = offset[other] - edge[other] * u;
                    let magnitude = c[0]
                        .abs()
                        .max(c[1].abs())
                        .max(edge[0].abs())
                        .max(edge[1].abs())
                        .max(1.0);
                    if residual.abs() <= 64.0 * f64::EPSILON * magnitude {
                        consider(time, u);
                    }
                }
            }
        }
        best.map(|(time, x)| EdgeHit {
            time: time as f32,
            x: x as f32,
        })
    }
}

impl EdgeSweep for SweptEdge {
    fn bounds(&self) -> Rect {
        (*self).bounds()
    }

    fn contact(&self, y: f32, base: f32, span: [f32; 2]) -> Option<EdgeHit> {
        (*self).contact(y, base, span)
    }
}

fn cross(a: [f64; 2], b: [f64; 2]) -> f64 {
    a[0] * b[1] - a[1] * b[0]
}

fn clip_linear(range: &mut [f64; 2], a: f64, b: f64, clip: [f64; 2]) -> bool {
    let delta = b - a;
    if delta == 0.0 {
        return a >= clip[0] && a <= clip[1];
    }
    let ends = [(clip[0] - a) / delta, (clip[1] - a) / delta];
    range[0] = range[0].max(ends[0].min(ends[1]));
    range[1] = range[1].min(ends[0].max(ends[1]));
    range[0] <= range[1]
}

/// Stable quadratic roots without a heap allocation. An identically zero
/// polynomial adds no event; endpoint/initial checks handle collinear motion.
fn roots(a: f64, b: f64, c: f64) -> [Option<f64>; 2] {
    if a == 0.0 {
        return [if b != 0.0 { Some(-c / b) } else { None }, None];
    }
    let four_ac = 4.0 * a * c;
    let discriminant = b * b - four_ac;
    let error = 16.0 * f64::EPSILON * (b * b).abs().max(four_ac.abs());
    if discriminant < -error {
        return [None, None];
    }
    let root = discriminant.max(0.0).sqrt();
    let q = -0.5 * (b + root.copysign(b));
    if q == 0.0 {
        [Some(-b / (2.0 * a)), None]
    } else {
        [Some(q / a), Some(c / q)]
    }
}

/// Split only when monitor pauses exist. Breakpoints include screen edges
/// and nearest-screen ownership changes in the off-screen emission margin.
pub(super) fn active_spans(
    scene: &Scene,
    paused: &BTreeSet<u64>,
    y: f32,
    span: [f32; 2],
) -> Vec<[f32; 2]> {
    if !scene.screens.iter().any(|s| paused.contains(&s.key)) {
        return vec![span];
    }
    if scene.screens.iter().all(|s| paused.contains(&s.key)) {
        return Vec::new();
    }
    let mut cuts = vec![span[0], span[1]];
    for screen in &scene.screens {
        for x in [screen.rect.left, screen.rect.right] {
            if x > span[0] && x < span[1] {
                cuts.push(x);
            }
        }
    }
    cuts.sort_unstable_by(f32::total_cmp);
    cuts.dedup();
    let mut ownership = Vec::new();
    for strip in cuts.windows(2) {
        let origin = f64::from(strip[0]);
        let width = f64::from(strip[1]) - origin;
        let middle = origin + width * 0.5;
        if strip[0].next_up() >= strip[1]
            || scene
                .screens
                .iter()
                .any(|s| s.rect.contains(Point::new(middle as f32, y)))
        {
            // Actual containment wins over nearest-distance ownership.
            continue;
        }
        let distance = |rect: Rect| {
            let dy = f64::from(y) - f64::from(y.clamp(rect.top, rect.bottom));
            let edge = if middle < f64::from(rect.left) {
                Some(rect.left)
            } else if middle > f64::from(rect.right) {
                Some(rect.right)
            } else {
                None
            };
            edge.map_or([0.0, 0.0, dy * dy], |edge| {
                let offset = origin - f64::from(edge);
                [1.0, 2.0 * offset, offset * offset + dy * dy]
            })
        };
        for (i, a) in scene.screens.iter().enumerate() {
            for b in &scene.screens[i + 1..] {
                if paused.contains(&a.key) == paused.contains(&b.key) {
                    continue;
                }
                let a = distance(a.rect);
                let b = distance(b.rect);
                for root in roots(a[0] - b[0], a[1] - b[1], a[2] - b[2])
                    .into_iter()
                    .flatten()
                {
                    if root > 0.0 && root < width {
                        let x = (origin + root) as f32;
                        for x in [x.next_down(), x, x.next_up()] {
                            if x > span[0] && x < span[1] {
                                ownership.push(x);
                            }
                        }
                    }
                }
            }
        }
    }
    cuts.extend(ownership);
    cuts.sort_unstable_by(f32::total_cmp);
    cuts.dedup();
    let active = |x| !scene.paused_at(Point::new(x, y), paused);
    let mut spans = Vec::new();
    for &x in &cuts {
        if active(x) {
            spans.push([x, x]);
        }
    }
    for strip in cuts.windows(2) {
        let mut left = strip[0].next_up();
        let mut right = strip[1].next_down();
        if left > right {
            continue;
        }
        let middle = (f64::from(left) + (f64::from(right) - f64::from(left)) * 0.5) as f32;
        if !active(middle) {
            continue;
        }
        // Resolve floating-point ties with the actual owner used by Scene.
        if !active(left) {
            let mut high = middle;
            while left.next_up() < high {
                let mid = (f64::from(left) + (f64::from(high) - f64::from(left)) * 0.5) as f32;
                if active(mid) { high = mid } else { left = mid }
            }
            left = high;
        }
        if !active(right) {
            let mut low = middle;
            while low.next_up() < right {
                let mid = (f64::from(low) + (f64::from(right) - f64::from(low)) * 0.5) as f32;
                if active(mid) { low = mid } else { right = mid }
            }
            right = low;
        }
        spans.push([left, right]);
    }
    spans
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geometry::Screen;

    #[test]
    fn edge_contact_finds_interior_and_reversed_slopes_across_dpi() {
        for scale in [1.0_f32, 2.5] {
            for origin in [Point::default(), Point::new(-900.0, -700.0)] {
                let p = |x: f32, y: f32| Point::new(origin.x + x * scale, origin.y + y * scale);
                for reverse in [false, true] {
                    let mut edge = SweptEdge {
                        before: [p(0.0, -3.0), p(10.0, -1.0)],
                        after: [p(0.0, 1.0), p(10.0, 3.0)],
                    };
                    if reverse {
                        edge.before.reverse();
                        edge.after.reverse();
                    }
                    let hit = edge
                        .contact(origin.y, origin.y, [p(4.0, 0.0).x, p(6.0, 0.0).x])
                        .unwrap();
                    assert!((hit.time - 0.45).abs() < 0.000_001);
                    assert!((hit.x - p(6.0, 0.0).x).abs() < 0.000_1);
                }
            }
        }
    }

    #[test]
    fn edge_contact_finds_the_first_of_two_corner_roots() {
        let mut edge = SweptEdge {
            before: [Point::new(0.0, -1.0), Point::new(2.0, -3.0)],
            after: [Point::new(2.0, 3.0), Point::new(0.0, -1.0)],
        };
        // For x=0.9, collinearity gives 12t² - 14.2t + 3.8 = 0.
        let expected = (14.2_f64 - 19.24_f64.sqrt()) / 24.0;
        for _ in 0..2 {
            let hit = edge.contact(0.0, 0.0, [0.9, 0.95]).unwrap();
            assert!((f64::from(hit.time) - expected).abs() < 0.000_001);
            assert!((hit.x - 0.9).abs() < 0.000_001);
            edge.before.reverse();
            edge.after.reverse();
        }
    }

    #[test]
    fn edge_contact_rejects_upward_material_while_the_other_end_descends() {
        let edge = SweptEdge {
            before: [Point::new(0.0, -2.0), Point::new(10.0, -2.0)],
            after: [Point::new(0.0, -3.0), Point::new(10.0, 2.0)],
        };
        let hit = edge.contact(0.0, 0.0, [4.0, 6.0]).unwrap();
        assert_eq!((hit.time, hit.x), (1.0, 6.0));
        assert!(edge.contact(0.0, 0.0, [4.0, 5.0]).is_none());
        assert!(edge.contact(0.0, 0.0, [0.0, 1.0]).is_none());
    }

    #[test]
    fn edge_contact_preserves_initial_overlap_and_degenerate_points() {
        let edge = |before, after| SweptEdge {
            before: [Point::new(0.0, before), Point::new(10.0, before)],
            after: [Point::new(0.0, after), Point::new(10.0, after)],
        };
        assert_eq!(
            edge(-1.0, -0.5)
                .contact(-2.0, 0.0, [4.0, 6.0])
                .unwrap()
                .time,
            0.0
        );
        assert!(edge(1.0, 2.0).contact(0.0, 0.0, [4.0, 6.0]).is_none());
        assert!(edge(-1.0, -2.0).contact(0.0, 0.0, [4.0, 6.0]).is_none());
        assert!(edge(0.0, 0.0).contact(0.0, 0.0, [4.0, 6.0]).is_none());
        let point = SweptEdge {
            before: [Point::new(5.0, -1.0); 2],
            after: [Point::new(5.0, 1.0); 2],
        };
        assert_eq!(point.contact(0.0, 0.0, [5.0, 5.0]).unwrap().time, 0.5);
        assert!(point.contact(0.0, 0.0, [5.0_f32.next_up(), 6.0]).is_none());
        let vertical = SweptEdge {
            before: [Point::new(5.0, -2.0), Point::new(5.0, -1.0)],
            after: [Point::new(5.0, 1.0), Point::new(5.0, 2.0)],
        };
        assert!(
            (vertical.contact(0.0, 0.0, [5.0, 5.0]).unwrap().time - 1.0 / 3.0).abs() < 0.000_001
        );
    }

    #[test]
    fn edge_contact_agrees_with_independent_material_point_samples() {
        let mut seed = 29_u32;
        let mut number = || {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            (seed >> 16) as f32 / 65_536.0
        };
        for case in 0..512 {
            let edge = SweptEdge {
                before: [
                    Point::new(number() * 20.0 - 10.0, -1.0 - number() * 7.0),
                    Point::new(number() * 20.0 - 10.0, -1.0 - number() * 7.0),
                ],
                after: [
                    Point::new(number() * 20.0 - 10.0, number() * 16.0 - 8.0),
                    Point::new(number() * 20.0 - 10.0, number() * 16.0 - 8.0),
                ],
            };
            let left = number() * 12.0 - 8.0;
            let span = [left, left + 0.5 + number() * 3.0];
            let hit = edge.contact(0.0, 0.0, span);
            // Independently trace fixed material points. These samples are an
            // upper bound on first contact, not the production collision path.
            for i in 0..=256 {
                let u = f64::from(i) / 256.0;
                let interpolate = |points: [Point; 2], axis: fn(Point) -> f32| {
                    f64::from(axis(points[0])) * (1.0 - u) + f64::from(axis(points[1])) * u
                };
                let ay = interpolate(edge.before, |p| p.y);
                let by = interpolate(edge.after, |p| p.y);
                if by <= 0.0 {
                    continue;
                }
                let t = -ay / (by - ay);
                let x = interpolate(edge.before, |p| p.x) * (1.0 - t)
                    + interpolate(edge.after, |p| p.x) * t;
                if x as f32 >= span[0] && x as f32 <= span[1] {
                    assert!(
                        hit.is_some_and(|hit| f64::from(hit.time) <= t + 0.000_01),
                        "missed case {case}"
                    );
                }
            }
            if let Some(hit) = hit {
                assert!(hit.time >= 0.0 && hit.time <= 1.0);
                assert!(hit.x >= span[0] && hit.x <= span[1]);
                let t = f64::from(hit.time);
                let a = f64::from(edge.before[0].y) * (1.0 - t) + f64::from(edge.after[0].y) * t;
                let b = f64::from(edge.before[1].y) * (1.0 - t) + f64::from(edge.after[1].y) * t;
                assert!(
                    a.min(b) <= 0.000_01 && a.max(b) >= -0.000_01,
                    "false plane contact in case {case}"
                );
            }
        }
    }

    fn screen(key: u64, left: f32, right: f32, top: f32) -> Screen {
        Screen {
            key,
            persistent_id: true,
            rect: Rect {
                left,
                right,
                top,
                bottom: 100.0,
            },
            floor: 100.0,
            scale: 1.0,
        }
    }

    #[test]
    fn active_spans_follow_nearest_ownership_above_an_edge() {
        let scene = Scene {
            screens: vec![screen(1, 0.0, 100.0, 0.0), screen(2, 100.0, 200.0, -10.0)],
            windows: Vec::new(),
        };
        let paused = [1].into();
        let spans = active_spans(&scene, &paused, -0.25, [99.0, 101.0]);
        for x in [99.5, 99.75, 99.75_f32.next_up(), 99.875, 100.0, 101.0] {
            assert_eq!(
                spans.iter().any(|span| x >= span[0] && x <= span[1]),
                !scene.paused_at(Point::new(x, -0.25), &paused),
                "ownership at x={x}"
            );
        }
    }

    #[test]
    fn active_spans_preserve_narrow_monitors_and_overlap_order() {
        let mut scene = Scene {
            screens: vec![
                screen(1, 0.0, 49.999, 0.0),
                screen(2, 49.999, 50.001, 0.0),
                screen(3, 50.001, 100.0, 0.0),
            ],
            windows: Vec::new(),
        };
        let paused = [1, 3].into();
        let spans = active_spans(&scene, &paused, 50.0, [48.0, 52.0]);
        for x in [48.0, 49.999, 50.0, 50.001, 52.0] {
            assert_eq!(
                spans.iter().any(|s| x >= s[0] && x <= s[1]),
                !scene.paused_at(Point::new(x, 50.0), &paused)
            );
        }
        scene.screens = vec![screen(1, 0.0, 100.0, 0.0), screen(2, 0.0, 100.0, 0.0)];
        assert!(active_spans(&scene, &paused, 50.0, [48.0, 52.0]).is_empty());
        scene.screens.reverse();
        assert!(!active_spans(&scene, &paused, 50.0, [48.0, 52.0]).is_empty());
    }
}
