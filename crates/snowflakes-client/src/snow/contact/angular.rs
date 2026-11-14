//! Continuous rigid edges over one force-integrated step. The small angular
//! interval is represented below f64 roundoff, rather than sampled in time.
use super::{EdgeHit, EdgeSweep, SweptEdge, clip_linear};
use crate::{
    geometry::{Point, Rect},
    snow::{BodyTransform, RenderPose},
};

const COEFFICIENTS: usize = 13;
const HALF: f64 = 0.5;
const WEIGHTS: [f64; COEFFICIENTS] = [
    1.0,
    0.5,
    0.25,
    0.125,
    0.0625,
    0.03125,
    0.015625,
    0.0078125,
    0.00390625,
    0.001953125,
    0.0009765625,
    0.00048828125,
    0.000244140625,
];

#[derive(Clone, Copy)]
struct Polynomial([f64; COEFFICIENTS]);

#[derive(Default)]
struct Roots {
    values: [f64; COEFFICIENTS],
    len: usize,
}
impl Roots {
    fn push(&mut self, value: f64) {
        if self.len == 0 || (self.values[self.len - 1] - value).abs() > 8.0 * f64::EPSILON {
            self.values[self.len] = value;
            self.len += 1;
        }
    }
    fn iter(&self) -> impl Iterator<Item = f64> + '_ {
        self.values[..self.len].iter().copied()
    }
}

impl Polynomial {
    fn value(self, x: f64, degree: usize) -> f64 {
        self.0[..=degree]
            .iter()
            .rev()
            .fold(0.0, |a, b| a.mul_add(x, *b))
    }

    fn roots(mut self) -> Roots {
        // The domain is [-1/2,1/2]. Discard only a tail smaller than evaluation
        // roundoff of the remaining polynomial, including its domain weight.
        let norm = self
            .0
            .iter()
            .zip(WEIGHTS)
            .map(|(c, weight)| c.abs() * weight)
            .sum::<f64>();
        let error = 32.0 * f64::EPSILON * norm;
        let mut degree = COEFFICIENTS - 1;
        let mut tail = 0.0;
        while degree > 0 && tail + self.0[degree].abs() * WEIGHTS[degree] <= error {
            tail += self.0[degree].abs() * WEIGHTS[degree];
            self.0[degree] = 0.0;
            degree -= 1;
        }
        if degree == 0 {
            return Roots::default();
        }
        if degree == 1 {
            let mut roots = Roots::default();
            let x = -self.0[0] / self.0[1];
            if (-HALF..=HALF).contains(&x) {
                roots.push(x);
            }
            return roots;
        }
        let mut derivative = Self([0.0; COEFFICIENTS]);
        for i in 1..=degree {
            derivative.0[i - 1] = self.0[i] * i as f64;
        }
        let critical = derivative.roots();
        let classify = |x| {
            let f = self.value(x, degree);
            if f.abs() <= error + tail { 0.0 } else { f }
        };
        let mut roots = Roots::default();
        let mut a = -HALF;
        let mut fa = classify(a);
        if fa == 0.0 {
            roots.push(a);
        }
        for b in critical.iter().chain(std::iter::once(HALF)) {
            let fb = classify(b);
            if fa != 0.0 && fb != 0.0 && fa.is_sign_positive() != fb.is_sign_positive() {
                let (mut low, mut high, mut f_low) = (a, b, fa);
                for _ in 0..56 {
                    let mid = (low + high) * HALF;
                    if mid == low || mid == high {
                        break;
                    }
                    let f = self.value(mid, degree);
                    if f == 0.0 {
                        low = mid;
                        high = mid;
                        break;
                    }
                    if f.is_sign_positive() == f_low.is_sign_positive() {
                        low = mid;
                        f_low = f;
                    } else {
                        high = mid;
                    }
                }
                roots.push((low + high) * HALF);
            }
            if fb == 0.0 {
                roots.push(b);
            }
            a = b;
            fa = fb;
        }
        roots
    }
}

pub(in crate::snow) struct AngularMotion {
    center: [f64; 2],
    velocity: [f64; 2],
    scale: f64,
    scale_delta: f64,
    before_transform: BodyTransform,
    after_transform: BodyTransform,
    middle_rotation: [f64; 2],
    half_rotation: [f64; 2],
    angle_delta: f64,
    bounds_factor: f64,
    bounds_offset: f64,
    cosine: [f64; 12],
    sine: [f64; 12],
}

impl AngularMotion {
    pub fn new(before: RenderPose, after: RenderPose) -> Self {
        let mut angle_delta =
            (f64::from(after.angle) - f64::from(before.angle)).rem_euclid(std::f64::consts::TAU);
        if angle_delta > std::f64::consts::PI {
            angle_delta -= std::f64::consts::TAU;
        }
        // Flake::integrate limits spin to 2.5 rad/s at DT=1/60.
        debug_assert!(angle_delta.abs() < 0.05);
        let mut cosine = [0.0; 12];
        let mut sine = [0.0; 12];
        cosine[0] = 1.0;
        sine[1] = angle_delta;
        for i in 2..12 {
            if i % 2 == 0 {
                cosine[i] = -cosine[i - 2] * angle_delta * angle_delta / (i * (i - 1)) as f64;
            } else {
                sine[i] = -sine[i - 2] * angle_delta * angle_delta / (i * (i - 1)) as f64;
            }
        }
        let (middle_sine, middle_cosine) = (f64::from(before.angle) + angle_delta * HALF).sin_cos();
        let (half_sine, half_cosine) = (angle_delta * HALF).sin_cos();
        let center = [
            (f64::from(before.position.x) + f64::from(after.position.x)) * HALF,
            (f64::from(before.position.y) + f64::from(after.position.y)) * HALF,
        ];
        let velocity = [
            f64::from(after.position.x) - f64::from(before.position.x),
            f64::from(after.position.y) - f64::from(before.position.y),
        ];
        let scale_delta = f64::from(after.scale) - f64::from(before.scale);
        let maximum_scale = f64::from(before.scale.max(after.scale));
        Self {
            center,
            velocity,
            scale: (f64::from(before.scale) + f64::from(after.scale)) * HALF,
            scale_delta,
            before_transform: BodyTransform::new(before.position, before.angle, before.scale),
            after_transform: BodyTransform::new(after.position, after.angle, after.scale),
            middle_rotation: [middle_sine, middle_cosine],
            half_rotation: [half_sine, half_cosine],
            angle_delta,
            bounds_factor: (maximum_scale * angle_delta * angle_delta
                + 2.0 * (scale_delta * angle_delta).abs())
                / 8.0
                + 32.0 * f64::EPSILON * maximum_scale,
            bounds_offset: 32.0
                * f64::EPSILON
                * (center[0].abs().max(center[1].abs()) + velocity[0].abs().max(velocity[1].abs()))
                    .max(1.0),
            cosine,
            sine,
        }
    }

    fn polynomial(&self, cosine: [f64; 2], sine: [f64; 2], linear: [f64; 2]) -> Polynomial {
        let mut p = Polynomial([0.0; COEFFICIENTS]);
        p.0[0] = linear[0];
        p.0[1] = linear[1];
        for i in 0..12 {
            p.0[i] += cosine[0] * self.cosine[i] + sine[0] * self.sine[i];
            p.0[i + 1] += cosine[1] * self.cosine[i] + sine[1] * self.sine[i];
        }
        p
    }

    pub fn vertex(&self, p: Point) -> AngularVertex {
        let before = self.before_transform.point(p);
        let after = self.after_transform.point(p);
        if self.angle_delta == 0.0 {
            return AngularVertex {
                local: [0.0; 2],
                before,
                after,
                bounds: Rect {
                    left: before.x.min(after.x),
                    right: before.x.max(after.x),
                    top: before.y.min(after.y),
                    bottom: before.y.max(after.y),
                },
            };
        }
        let [s, c] = self.middle_rotation;
        let local = [
            f64::from(p.x) * c - f64::from(p.y) * s,
            f64::from(p.x) * s + f64::from(p.y) * c,
        ];
        // The L1 norm bounds the radius without a square root. Each vertex's
        // transforms/bounds are reused by its two neighbouring contour edges.
        let margin = self.bounds_offset + (local[0].abs() + local[1].abs()) * self.bounds_factor;
        let a = self.sample(local, -HALF).0;
        let b = self.sample(local, HALF).0;
        AngularVertex {
            local,
            before,
            after,
            bounds: Rect {
                left: ((a[0].min(b[0]) - margin) as f32).next_down(),
                right: ((a[0].max(b[0]) + margin) as f32).next_up(),
                top: ((a[1].min(b[1]) - margin) as f32).next_down(),
                bottom: ((a[1].max(b[1]) + margin) as f32).next_up(),
            },
        }
    }

    pub fn between(&self, a: AngularVertex, b: AngularVertex) -> AngularEdge<'_> {
        AngularEdge {
            motion: self,
            local: [a.local, b.local],
            linear: SweptEdge {
                before: [a.before, b.before],
                after: [a.after, b.after],
            },
            bounds: Rect {
                left: a.bounds.left.min(b.bounds.left),
                right: a.bounds.right.max(b.bounds.right),
                top: a.bounds.top.min(b.bounds.top),
                bottom: a.bounds.bottom.max(b.bounds.bottom),
            },
        }
    }

    #[cfg(test)]
    fn edge(&self, a: Point, b: Point) -> AngularEdge<'_> {
        self.between(self.vertex(a), self.vertex(b))
    }

    fn sample(&self, local: [f64; 2], x: f64) -> ([f64; 2], f64) {
        let m = self;
        let [s, c] = if x == -HALF {
            [-m.half_rotation[0], m.half_rotation[1]]
        } else if x == HALF {
            m.half_rotation
        } else {
            let (s, c) = (m.angle_delta * x).sin_cos();
            [s, c]
        };
        let r = [local[0] * c - local[1] * s, local[0] * s + local[1] * c];
        let scale = m.scale + m.scale_delta * x;
        (
            [
                m.center[0] + m.velocity[0] * x + scale * r[0],
                m.center[1] + m.velocity[1] * x + scale * r[1],
            ],
            m.velocity[1] + m.scale_delta * r[1] + scale * m.angle_delta * r[0],
        )
    }
}

#[derive(Clone, Copy)]
pub(in crate::snow) struct AngularVertex {
    local: [f64; 2],
    before: Point,
    after: Point,
    bounds: Rect,
}

pub(in crate::snow) struct AngularEdge<'a> {
    motion: &'a AngularMotion,
    local: [[f64; 2]; 2],
    linear: SweptEdge,
    bounds: Rect,
}

impl AngularEdge<'_> {
    fn sample(&self, local: [f64; 2], x: f64) -> ([f64; 2], f64) {
        self.motion.sample(local, x)
    }
}

impl EdgeSweep for AngularEdge<'_> {
    fn bounds(&self) -> Rect {
        self.bounds
    }

    fn contact(&self, y: f32, base: f32, span: [f32; 2]) -> Option<EdgeHit> {
        let m = self.motion;
        if m.angle_delta == 0.0 {
            return self.linear.contact(y, base, span);
        }
        let y = f64::from(y);
        let a = self.sample(self.local[0], -HALF);
        let b = self.sample(self.local[1], -HALF);
        let mut initial = [0.0, 1.0];
        if clip_linear(&mut initial, a.0[0], b.0[0], span.map(f64::from))
            && clip_linear(&mut initial, a.0[1], b.0[1], [y, f64::from(base)])
            && clip_linear(&mut initial, a.1, b.1, [0.0, f64::INFINITY])
        {
            let u = (initial[0] + initial[1]) * HALF;
            if a.1 + (b.1 - a.1) * u > 0.0 {
                return Some(EdgeHit {
                    time: 0.0,
                    x: (a.0[0] + (b.0[0] - a.0[0]) * u) as f32,
                });
            }
        }
        let mut best: Option<EdgeHit> = None;
        let mut consider = |x: f64, u: f64| {
            if !(-HALF..=HALF).contains(&x) || !(0.0..=1.0).contains(&u) {
                return;
            }
            let local = [
                self.local[0][0] + (self.local[1][0] - self.local[0][0]) * u,
                self.local[0][1] + (self.local[1][1] - self.local[0][1]) * u,
            ];
            let (p, vy) = self.sample(local, x);
            let magnitude = m.center[1].abs()
                + m.velocity[1].abs()
                + m.scale.abs() * local[0].hypot(local[1])
                + y.abs();
            let time = (x + HALF) as f32;
            if vy > 0.0
                && (p[1] - y).abs() <= 256.0 * f64::EPSILON * magnitude.max(1.0)
                && p[0] as f32 >= span[0]
                && p[0] as f32 <= span[1]
                && best.is_none_or(|old| time < old.time)
            {
                best = Some(EdgeHit {
                    time,
                    x: p[0] as f32,
                });
            }
        };
        for (p, u) in [(self.local[0], 0.0), (self.local[1], 1.0)] {
            let roots = m
                .polynomial(
                    [m.scale * p[1], m.scale_delta * p[1]],
                    [m.scale * p[0], m.scale_delta * p[0]],
                    [m.center[1] - y, m.velocity[1]],
                )
                .roots();
            for x in roots.iter() {
                consider(x, u);
            }
        }
        let e = [
            self.local[1][0] - self.local[0][0],
            self.local[1][1] - self.local[0][1],
        ];
        let cross = self.local[0][0] * e[1] - self.local[0][1] * e[0];
        for qx in span.map(f64::from) {
            let q = [qx - m.center[0], y - m.center[1]];
            let roots = m
                .polynomial(
                    [
                        q[0] * e[1] - q[1] * e[0],
                        -m.velocity[0] * e[1] + m.velocity[1] * e[0],
                    ],
                    [
                        q[0] * e[0] + q[1] * e[1],
                        -m.velocity[0] * e[0] - m.velocity[1] * e[1],
                    ],
                    [-m.scale * cross, -m.scale_delta * cross],
                )
                .roots();
            for x in roots.iter() {
                let a = self.sample(self.local[0], x).0;
                let b = self.sample(self.local[1], x).0;
                let axis = usize::from((b[1] - a[1]).abs() > (b[0] - a[0]).abs());
                if b[axis] != a[axis] {
                    let q = [qx, y];
                    consider(x, (q[axis] - a[axis]) / (b[axis] - a[axis]));
                }
            }
        }
        best
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pose(x: f32, y: f32, angle: f32, scale: f32) -> RenderPose {
        RenderPose {
            position: Point::new(x, y),
            angle,
            scale,
            age: 0.0,
        }
    }
    fn factored(roots: &[f64], scale: f64) -> Polynomial {
        let mut p = Polynomial([0.0; COEFFICIENTS]);
        p.0[0] = scale;
        for (degree, &root) in roots.iter().enumerate() {
            for i in (0..=degree + 1).rev() {
                p.0[i] = if i == 0 { 0.0 } else { p.0[i - 1] } - root * p.0[i];
            }
        }
        p
    }

    #[test]
    fn polynomial_roots_include_turning_touches_and_domain_ends() {
        let roots = factored(&[-0.5, -0.2, 0.0, 0.2, 0.2, 0.5], 1.0).roots();
        assert_eq!(roots.len, 5);
        for (actual, expected) in roots.iter().zip([-0.5, -0.2, 0.0, 0.2, 0.5]) {
            assert!(
                (actual - expected).abs() < 1e-8,
                "{actual} instead of {expected}"
            );
        }
    }

    #[test]
    fn polynomial_root_storage_and_order_hold_through_degree_twelve_and_rescaling() {
        for degree in 1..COEFFICIENTS {
            let expected: Vec<_> = (0..degree)
                .map(|i| {
                    if degree == 1 {
                        0.123
                    } else {
                        -0.45 + 0.9 * i as f64 / (degree - 1) as f64
                    }
                })
                .collect();
            for scale in [1e-120, 1.0, 1e120] {
                let roots = factored(&expected, scale).roots();
                assert_eq!(roots.len, degree, "degree={degree}, scale={scale}");
                for (a, b) in roots.iter().zip(&expected) {
                    assert!((a - b).abs() < 1e-6);
                }
            }
        }
    }

    #[test]
    fn constant_collinear_and_positive_polynomials_create_no_root_events() {
        assert_eq!(Polynomial([0.0; COEFFICIENTS]).roots().len, 0);
        let mut p = Polynomial([0.0; COEFFICIENTS]);
        p.0[0] = 0.01;
        p.0[2] = 1.0;
        assert_eq!(p.roots().len, 0);
        p.0[2] = 0.0;
        assert_eq!(p.roots().len, 0);
    }

    #[test]
    fn curved_bounds_include_midstep_bulges_with_scale_change_and_angle_wrap() {
        for sign in [-1.0, 1.0] {
            for (before, after) in [(0.25, 0.26), (1.0, 1.0), (2.5, 2.4), (8.0, 7.68)] {
                let angle = if sign > 0.0 {
                    std::f32::consts::TAU - 0.02
                } else {
                    0.02
                };
                let next = (angle + sign * 2.5 * crate::snow::DT).rem_euclid(std::f32::consts::TAU);
                let motion = AngularMotion::new(
                    pose(-1600.0, -30.0, angle, before),
                    pose(-1596.0, -25.0, next, after),
                );
                let edge = motion.edge(Point::new(-100.0, 1.0), Point::new(120.0, -2.0));
                let bounds = edge.bounds();
                for i in 0..=1000 {
                    for local in edge.local {
                        let p = edge.sample(local, i as f64 / 1000.0 - HALF).0;
                        assert!(p[0] >= f64::from(bounds.left) && p[0] <= f64::from(bounds.right));
                        assert!(p[1] >= f64::from(bounds.top) && p[1] <= f64::from(bounds.bottom));
                    }
                }
            }
        }
    }

    #[test]
    fn curved_interior_contact_matches_an_independent_line_height_oracle() {
        let motion = AngularMotion::new(pose(100.0, 0.0, -0.02, 1.0), pose(100.0, 10.0, 0.02, 1.0));
        let edge = motion.edge(Point::new(-40.0, -1.0), Point::new(40.0, 3.0));
        let hit = edge.contact(5.0, 5.0, [99.0, 101.0]).unwrap();
        let mut first = 1.0_f64;
        for qx in [99.0, 101.0] {
            let (mut a, mut b) = (0.0, 1.0);
            for _ in 0..60 {
                let t = (a + b) * HALF;
                let angle = -0.02 + 0.04 * t;
                let (s, c) = angle.sin_cos();
                let p = [100.0 - 40.0 * c + s, 10.0 * t - 40.0 * s - c];
                let q = [100.0 + 40.0 * c - 3.0 * s, 10.0 * t + 40.0 * s + 3.0 * c];
                let height = p[1] + (qx - p[0]) / (q[0] - p[0]) * (q[1] - p[1]);
                if height < 5.0 {
                    a = t;
                } else {
                    b = t;
                }
            }
            first = first.min((a + b) * HALF);
        }
        assert!((f64::from(hit.time) - first).abs() < 1e-6);
        assert!((99.0..=101.0).contains(&hit.x));
    }

    #[test]
    fn curved_contacts_reject_upward_material_and_handle_initial_pile_overlap() {
        let motion = AngularMotion::new(pose(100.0, 10.0, -0.02, 1.0), pose(100.0, 0.0, 0.02, 1.0));
        assert!(
            motion
                .edge(Point::new(-40.0, 0.0), Point::new(40.0, 0.0))
                .contact(5.0, 5.0, [99.0, 101.0])
                .is_none()
        );
        let motion = AngularMotion::new(pose(100.0, 2.0, -0.02, 1.0), pose(100.0, 3.0, 0.02, 1.0));
        let edge = motion.edge(Point::new(-2.0, 1.0), Point::new(2.0, 1.0));
        assert_eq!(edge.contact(0.0, 5.0, [99.0, 101.0]).unwrap().time, 0.0);
        assert!(edge.contact(0.0, 0.0, [99.0, 101.0]).is_none());
    }

    #[test]
    fn curved_contact_keeps_representable_half_open_span_boundaries() {
        let motion = AngularMotion::new(
            pose(100.0, 100.0, -0.01, 1.0),
            pose(100.0, 110.0, 0.01, 1.0),
        );
        let edge = motion.edge(Point::default(), Point::default());
        assert!(
            edge.contact(105.0, 105.0, [100.0_f32.next_up(), 101.0])
                .is_none()
        );
        assert!(
            edge.contact(105.0, 105.0, [99.0, 100.0_f32.next_down()])
                .is_none()
        );
        assert!(edge.contact(105.0, 105.0, [100.0, 100.0]).is_some());
    }
}
