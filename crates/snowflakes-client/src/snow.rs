//! Force integration, swept contacts and mass-preserving accumulation.
//! Trajectories are state, not functions of an animation clock.
mod contact;
mod prefilter;
use crate::geometry::{Point, Rect, Scene, Screen, Support, uncovered_rectangles};
#[cfg(test)]
use contact::SweptEdge;
use contact::{AngularMotion, EdgeSweep};
use snowflakes_protocol::Settings;
use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    f32::consts::TAU,
};

pub const DT: f32 = 1.0 / 60.0;
pub const MAX_FLAKES: usize = 2400;
pub const GLOBAL_MAX_FLAKES: usize = 16_000;
pub const BASE_COLLAPSE_BUDGET: usize = 128;
const MAX_COLLAPSE_PARTS: usize = 8;
pub const ORDINARY_FLAKES_PER_SCREEN: usize = MAX_FLAKES - BASE_COLLAPSE_BUDGET;
const CELL: f32 = 6.0;
const MAX_CELLS: usize = 65_536;
const MAX_BUCKET_COLUMNS: i32 = 256;
const GRAVITY: f32 = 48.0;
const SLAB_GRAVITY: f32 = 200.0;
const EDGE_INTERVAL: f32 = 0.75;
const MAX_EDGE_CHUNKS_PER_STEP: usize = 8;
const DPI_CHANGE_PER_STEP: f32 = 0.04;
const MIN_MATERIAL_SCALE: f32 = 0.25;
const MAX_MATERIAL_SCALE: f32 = 8.0;
#[derive(Clone, Debug)]
pub struct SnowSlab {
    pub heights: Vec<f32>,
    pub cell_width: f32,
    pub center: Point,
}
impl SnowSlab {
    /// Rounded contours of occupied runs, in the body's shared local coordinates.
    /// Empty cells separate figures for both rendering and swept contacts.
    pub fn contours(&self) -> impl Iterator<Item = Vec<Point>> + '_ {
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
            Some(self.outline_range(first, next))
        })
    }
    fn outline_range(&self, first: usize, end: usize) -> Vec<Point> {
        let heights = &self.heights[first..end];
        let left = first as f32 * self.cell_width - self.center.x;
        let mut points = Vec::with_capacity(heights.len() + 4);
        points.push(Point::new(left, -self.center.y));
        points.push(Point::new(left, -heights[0] - self.center.y));
        for (index, height) in heights.iter().enumerate() {
            points.push(Point::new(
                ((first + index) as f32 + 0.5) * self.cell_width - self.center.x,
                -height - self.center.y,
            ));
        }
        let right = end as f32 * self.cell_width - self.center.x;
        points.push(Point::new(
            right,
            -heights[heights.len() - 1] - self.center.y,
        ));
        points.push(Point::new(right, -self.center.y));
        let mut rounded = Vec::with_capacity(points.len() * 5);
        for (index, &corner) in points.iter().enumerate() {
            let previous = points[(index + points.len() - 1) % points.len()];
            let next = points[(index + 1) % points.len()];
            let a = Point::new(previous.x - corner.x, previous.y - corner.y);
            let b = Point::new(next.x - corner.x, next.y - corner.y);
            let la = a.x.hypot(a.y);
            let lb = b.x.hypot(b.y);
            if la < 0.001 || lb < 0.001 || (a.x * b.x + a.y * b.y) / la / lb < -0.98 {
                rounded.push(corner);
                continue;
            }
            let cut = (la.min(lb) * 0.4).min(4.0);
            let entry = Point::new(corner.x + a.x * cut / la, corner.y + a.y * cut / la);
            let exit = Point::new(corner.x + b.x * cut / lb, corner.y + b.y * cut / lb);
            // The same quadratic contour is used for rendering and swept contact.
            for step in 0..=4 {
                let t = step as f32 / 4.0;
                let u = 1.0 - t;
                rounded.push(Point::new(
                    u * u * entry.x + 2.0 * u * t * corner.x + t * t * exit.x,
                    u * u * entry.y + 2.0 * u * t * corner.y + t * t * exit.y,
                ));
            }
        }
        rounded
    }
}
#[derive(Clone, Debug)]
pub struct Flake {
    pub position: Point,
    pub velocity: Point,
    pub radius: f32,
    pub mass: f32,
    pub angle: f32,
    pub spin: f32,
    pub age: f32,
    pub opacity: f32,
    pub shape: f32,
    /// Detached material passes in front of the ledge it separated from.
    pub skip_support: Option<u64>,
    pub slab: Option<SnowSlab>,
    display_scale: Option<f32>,
    previous_pose: Option<RenderPose>,
}
/// A visual pose between two actual physics states. Contains no mass or geometry copy.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RenderPose {
    pub position: Point,
    pub angle: f32,
    pub scale: f32,
    pub age: f32,
}
fn render_fraction(fraction: f32) -> f32 {
    if fraction.is_finite() {
        fraction.clamp(0.0, 1.0)
    } else {
        1.0
    }
}
fn render_mix(previous: f32, current: f32, fraction: f32) -> f32 {
    if fraction <= 0.0 {
        previous
    } else if fraction >= 1.0 {
        current
    } else {
        previous + (current - previous) * fraction
    }
}
impl RenderPose {
    pub fn bounds(self, body: &Flake) -> Rect {
        body.bounds_at(self.position, self.angle, self.scale)
    }
}
/// One body's transform for a single operation; never cached across steps.
#[derive(Clone, Copy)]
pub(crate) struct BodyTransform {
    origin: Point,
    sine: f32,
    cosine: f32,
    scale: f32,
}
impl BodyTransform {
    fn new(origin: Point, angle: f32, scale: f32) -> Self {
        let (sine, cosine) = angle.sin_cos();
        Self {
            origin,
            sine,
            cosine,
            scale,
        }
    }
    #[inline]
    pub(crate) fn point(self, local: Point) -> Point {
        Point::new(
            self.origin.x + (local.x * self.cosine - local.y * self.sine) * self.scale,
            self.origin.y + (local.x * self.sine + local.y * self.cosine) * self.scale,
        )
    }
}
impl Flake {
    pub fn new(position: Point, radius: f32) -> Self {
        Self {
            position,
            velocity: Point::default(),
            radius,
            mass: radius * radius * 1.20,
            angle: 0.0,
            spin: 0.0,
            age: 0.0,
            opacity: 0.85,
            shape: 1.0,
            skip_support: None,
            slab: None,
            display_scale: None,
            previous_pose: None,
        }
    }
    pub fn integrate(&mut self, air: Point, scale: f32) {
        self.previous_pose = Some(self.current_pose(scale));
        let previous = self.display_scale(scale);
        let limit = previous * DPI_CHANGE_PER_STEP;
        let scale = previous + (scale - previous).clamp(-limit, limit);
        self.set_display_scale(scale);
        // Changing DIP units must not inject physical linear velocity.
        self.velocity.x *= previous / scale;
        self.velocity.y *= previous / scale;
        let relative = Point::new(air.x - self.velocity.x, air.y - self.velocity.y);
        // Gravity kick followed by the exact quadratic-drag substep. Very light
        // fragments cannot become unstable under a large relative air velocity.
        let slab = self.slab.is_some();
        let drag = (if slab { 0.04 } else { 0.09 }) * self.radius.max(0.2) * self.shape;
        self.velocity.y += (if slab { SLAB_GRAVITY } else { GRAVITY }) * DT;
        let vx = self.velocity.x - air.x;
        let vy = self.velocity.y - air.y;
        let damping = 1.0 / (1.0 + drag / self.mass * vx.hypot(vy) * DT);
        self.velocity.x = air.x + vx * damping;
        self.velocity.y = air.y + vy * damping;
        self.velocity.x = self.velocity.x.clamp(-220.0, 220.0);
        self.velocity.y = self
            .velocity
            .y
            .clamp(-120.0, if slab { 360.0 } else { 160.0 });
        self.spin += (relative.x * 0.025 / self.radius - self.spin * 0.9) * DT;
        self.spin = self.spin.clamp(-2.5, 2.5);
        self.angle = (self.angle + self.spin * DT).rem_euclid(TAU);
        self.position.x += self.velocity.x * scale * DT;
        self.position.y += self.velocity.y * scale * DT;
        self.age += DT;
    }
    pub fn display_scale(&self, fallback: f32) -> f32 {
        self.display_scale.unwrap_or(fallback)
    }
    fn current_pose(&self, fallback: f32) -> RenderPose {
        RenderPose {
            position: self.position,
            angle: self.angle,
            scale: self.display_scale(fallback),
            age: self.age,
        }
    }
    /// Interpolation is read-only: collision, mass, radius and slab topology use current state.
    pub fn render_pose(&self, fallback: f32, fraction: f32) -> RenderPose {
        let current = self.current_pose(fallback);
        let Some(previous) = self.previous_pose else {
            return current;
        };
        let fraction = render_fraction(fraction);
        if fraction <= 0.0 {
            return previous;
        }
        if fraction >= 1.0 {
            return current;
        }
        let angle = (current.angle - previous.angle + std::f32::consts::PI).rem_euclid(TAU)
            - std::f32::consts::PI;
        RenderPose {
            position: Point::new(
                render_mix(previous.position.x, current.position.x, fraction),
                render_mix(previous.position.y, current.position.y, fraction),
            ),
            angle: (previous.angle + angle * fraction).rem_euclid(TAU),
            scale: render_mix(previous.scale, current.scale, fraction),
            age: render_mix(previous.age, current.age, fraction),
        }
    }
    pub(crate) fn set_display_scale(&mut self, scale: f32) {
        self.display_scale = Some(scale);
    }
    pub fn transformed_point(&self, local: Point, scale: f32) -> Point {
        self.transform(scale).point(local)
    }
    pub(crate) fn transform(&self, scale: f32) -> BodyTransform {
        BodyTransform::new(self.position, self.angle, scale)
    }
    /// Include the rendered contour and its outline, even during rotation.
    pub fn draw_bounds(&self, scale: f32) -> Rect {
        self.bounds_at(self.position, self.angle, scale)
    }
    fn bounds_at(&self, origin: Point, angle: f32, scale: f32) -> Rect {
        if let Some(slab) = &self.slab {
            let transform = BodyTransform::new(origin, angle, scale);
            let mut bounds = Rect {
                left: f32::INFINITY,
                top: f32::INFINITY,
                right: f32::NEG_INFINITY,
                bottom: f32::NEG_INFINITY,
            };
            for local in slab.contours().flatten() {
                let p = transform.point(local);
                bounds.left = bounds.left.min(p.x);
                bounds.top = bounds.top.min(p.y);
                bounds.right = bounds.right.max(p.x);
                bounds.bottom = bounds.bottom.max(p.y);
            }
            Rect {
                left: bounds.left - 2.0,
                top: bounds.top - 2.0,
                right: bounds.right + 2.0,
                bottom: bounds.bottom + 2.0,
            }
        } else {
            let radius = self.radius * scale * 1.5;
            Rect {
                left: origin.x - radius,
                top: origin.y - radius,
                right: origin.x + radius,
                bottom: origin.y + radius,
            }
        }
    }
}
#[derive(Clone, Copy, Default)]
pub struct Cursor {
    pub position: Point,
    pub velocity: Point,
}
#[cfg_attr(test, derive(Debug))]
pub struct Drift {
    pub support: Support,
    pub mass: Vec<f32>,
    pub pending: Vec<f32>,
    scratch: Vec<f32>,
    flux: Vec<f32>,
    disable_rate: Vec<f32>,
    shrink_remaining: Vec<f32>,
    last_size: Vec<u8>,
    edge_wait: [f32; 2],
    pub cell_width: f32,
    motion_velocity: Point,
    /// DIP unit of motion_velocity; the frame origin is independent of the ledge inset.
    motion_scale: f32,
    /// Square roots of physical area per material unit, including a DPI transition.
    scales: Vec<f32>,
    target_scales: Vec<f32>,
    previous_y: Option<f32>,
}
impl Drift {
    fn advance_scales(&mut self, scene: &Scene, paused: &BTreeSet<u64>) {
        for i in 0..self.mass.len() {
            if scene.paused_at(Point::new(self.x(i), self.support.y - 1.0), paused) {
                continue;
            }
            let target = self.target_scales[i];
            if self.mass[i] + self.pending[i] <= 0.0 {
                self.scales[i] = target;
            } else if self.scales[i] != target {
                let area = self.scales[i] * self.scales[i];
                let limit = area * DPI_CHANGE_PER_STEP;
                self.scales[i] = (area + (target * target - area).clamp(-limit, limit)).sqrt();
            }
        }
    }
    fn profile_scale(&self, first: usize, end: usize) -> f32 {
        let mut mass = 0.0_f64;
        let mut area = 0.0_f64;
        for i in first..end {
            let amount = f64::from(self.mass[i]) + f64::from(self.pending[i]);
            mass += amount;
            area += amount * f64::from(self.scales[i]).powi(2);
        }
        (area / mass).sqrt() as f32
    }
    fn edge_chunk(
        &mut self,
        right: bool,
        scene: &Scene,
        paused: &BTreeSet<u64>,
        allow_emit: bool,
    ) -> Option<Flake> {
        let side = usize::from(right);
        let edge = if right { self.mass.len() - 1 } else { 0 };
        let scale = self.scales[edge];
        let cell_width = self.cell_width / scale;
        let count = ((18.0 / cell_width).floor() as usize)
            .clamp(1, 12)
            .min(self.mass.len());
        let first = if right { self.mass.len() - count } else { 0 };
        let last = first + count;
        let outer_x = scene
            .windows
            .iter()
            .find(|w| w.key == self.support.key)
            .map(|w| {
                if right {
                    w.rect.right + scale * 0.5
                } else {
                    w.rect.left - scale * 0.5
                }
            })
            .unwrap_or(if right {
                self.support.right + scale
            } else {
                self.support.left - scale
            });
        let outside = Point::new(outer_x, self.support.y - 1.0);
        if !scene.screens.iter().any(|s| s.rect.contains(outside))
            || scene.paused_at(outside, paused)
            || (first..last).any(|i| {
                self.shrink_remaining[i] > 0.0
                    || !self.support.visible_at(self.x(i))
                    || scene.paused_at(Point::new(self.x(i), self.support.y - 1.0), paused)
                    || (self.scales[i] - scale).abs() > 0.001
            })
        {
            self.edge_wait[side] = EDGE_INTERVAL;
            return None;
        }
        self.edge_wait[side] = (self.edge_wait[side] - DT).max(0.0);
        if self.edge_wait[side] > 0.0 || !allow_emit {
            return None;
        }
        self.edge_wait[side] = EDGE_INTERVAL;
        let heights: Vec<f32> = (first..last)
            .map(|i| {
                let distance = if right { self.mass.len() - 1 - i } else { i };
                let stable = 4.0 + 0.55 * (distance as f32 + 0.5) * cell_width;
                (self.height(i, scene) / scale - stable).max(0.0) * 0.35
            })
            .collect();
        if heights.iter().copied().fold(0.0, f32::max) < 0.6 {
            return None;
        }
        let mass = heights.iter().sum::<f32>() * cell_width;
        if mass <= 0.0 {
            return None;
        }
        let mut center = Point::default();
        let mut base_height = 0.0;
        let mut top = self.support.y;
        for (local, &height) in heights.iter().enumerate() {
            let i = first + local;
            let weight = height * cell_width / mass;
            center.x += (local as f32 + 0.5) * cell_width * weight;
            center.y -= height * 0.5 * weight;
            base_height += (self.height(i, scene) / scale - height) * weight;
            top = top.min(self.support.y - self.height(i, scene));
        }
        let position = Point::new(
            self.support.left + first as f32 * self.cell_width + center.x * scale,
            self.support.y - base_height * scale + center.y * scale,
        );
        let mut body = Flake::new(position, (mass / 1.20).sqrt().clamp(0.2, 32.0));
        body.set_display_scale(scale);
        body.mass = mass;
        body.velocity = Point::new(if right { 24.0 } else { -24.0 }, 14.0);
        body.spin = if right { 0.12 } else { -0.12 };
        body.age = 1.0;
        body.skip_support = Some(self.support.key);
        body.slab = Some(SnowSlab {
            heights,
            cell_width,
            center,
        });
        let outline = body.slab.as_ref().unwrap().contours();
        let bounds = outline
            .flatten()
            .map(|p| body.transformed_point(p, scale))
            .fold(
                Rect {
                    left: self.support.left + first as f32 * self.cell_width,
                    right: self.support.left + last as f32 * self.cell_width,
                    top,
                    bottom: self.support.y,
                },
                |r, p| Rect {
                    left: r.left.min(p.x),
                    right: r.right.max(p.x),
                    top: r.top.min(p.y),
                    bottom: r.bottom.max(p.y),
                },
            );
        if !self.support.exposes_rect(bounds) {
            return None;
        }
        for (local, height) in body.slab.as_ref().unwrap().heights.iter().enumerate() {
            self.mass[first + local] -= height * cell_width;
        }
        Some(body)
    }
    fn remove_amount(&mut self, index: usize, amount: f32) -> f64 {
        let total = self.mass[index] + self.pending[index];
        let fraction = if total > 0.0 {
            (amount / total).min(1.0)
        } else {
            0.0
        };
        let resting = self.mass[index] * fraction;
        let pending = self.pending[index] * fraction;
        self.mass[index] -= resting;
        self.pending[index] -= pending;
        f64::from(resting) + f64::from(pending)
    }
    pub fn total_mass(&self) -> f64 {
        self.mass
            .iter()
            .chain(&self.pending)
            .map(|m| f64::from(*m))
            .sum()
    }
    fn contact_height(&self, index: usize, _scene: &Scene) -> f32 {
        let scale = self.scales[index];
        (self.mass[index] + self.pending[index]) * scale * scale / self.cell_width
    }
    fn capacity(&self, index: usize, scene: &Scene, settings: &Settings) -> f32 {
        let active = if self.support.ground {
            settings.snow.on_ground
        } else {
            settings.snow.on_windows
        };
        if !active {
            return 0.0;
        }
        let x = self.x(index);
        let scale = self.scales[index];
        let max_depth = if self.support.ground { 120.0 } else { 32.0 };
        let depth = (max_depth * settings.snow.drift_size_percent as f32 / 100.0
            * self.target_scales[index])
            .min(scene.headroom(x, self.support.y));
        depth / scale / scale * self.cell_width
    }
    fn deposit_capacity(&self, index: usize, scene: &Scene, settings: &Settings) -> f32 {
        let point = Point::new(
            self.x(index),
            self.support.y - self.contact_height(index, scene),
        );
        let scale = self.scales[index];
        let visible = self.support.exposed_headroom(point) * self.cell_width / (scale * scale);
        (self.capacity(index, scene, settings) - self.mass[index] - self.pending[index])
            .max(0.0)
            .min(visible)
    }
    fn relax(&mut self, scene: &Scene, paused: &BTreeSet<u64>, settings: &Settings) {
        self.scratch.fill(0.0);
        self.flux.fill(0.0);
        for index in 0..self.mass.len().saturating_sub(1) {
            if [index, index + 1]
                .iter()
                .any(|&i| scene.paused_at(Point::new(self.x(i), self.support.y - 1.0), paused))
            {
                continue;
            }
            let difference = self.height(index, scene) - self.height(index + 1, scene);
            let excess = (difference.abs() - 0.55 * self.cell_width).max(0.0);
            let left_scale = self.scales[index];
            let right_scale = self.scales[index + 1];
            let (donor, receiver) = if difference > 0.0 {
                (index, index + 1)
            } else {
                (index + 1, index)
            };
            // Both height changes matter at a DPI boundary. For equal scales
            // this is the original 6*DT relaxation rate.
            let amount = (excess * self.cell_width
                / (left_scale * left_scale + right_scale * right_scale)
                * (12.0 * DT))
                .min(self.mass[donor] * 0.25);
            self.flux[index] = amount * difference.signum();
            self.scratch[receiver] += amount;
        }
        // Reserve the receiver's free capacity once for all incoming edges.
        // Pending material also occupies space. Do not count same-step outflow
        // as free room, or favor whichever neighbour is visited first.
        for index in 0..self.mass.len() {
            self.scratch[index] = if self.scratch[index] > 0.0 {
                (self.deposit_capacity(index, scene, settings) / self.scratch[index]).min(1.0)
            } else {
                0.0
            };
        }
        for index in 0..self.flux.len() {
            let receiver = if self.flux[index] > 0.0 {
                index + 1
            } else {
                index
            };
            self.flux[index] *= self.scratch[receiver];
        }
        self.scratch.fill(0.0);
        for (index, &amount) in self.flux.iter().enumerate() {
            self.scratch[index] -= amount;
            self.scratch[index + 1] += amount;
        }
        for (mass, delta) in self.mass.iter_mut().zip(&self.scratch) {
            *mass += delta;
        }
    }
    pub fn height(&self, index: usize, _scene: &Scene) -> f32 {
        let scale = self.scales[index];
        self.mass[index] * scale * scale / self.cell_width
    }
    pub fn x(&self, index: usize) -> f32 {
        self.support.left + (index as f32 + 0.5) * self.cell_width
    }
    fn index(&self, x: f32) -> usize {
        (((x - self.support.left) / self.cell_width) as usize).min(self.mass.len() - 1)
    }
    /// Invert the actual rounded index operation, including cancellation near
    /// x=0 on a wide desktop. The common boundary needs at most one ULP fix;
    /// ambiguous plateaus use a bounded search over ordered finite floats.
    fn contact_boundary(&self, index: usize) -> f32 {
        if index == 0 {
            return self.support.left;
        }
        if index == self.mass.len() {
            return self.support.right;
        }
        let ordered = |x: f32| {
            let bits = x.to_bits();
            if bits >> 31 != 0 {
                !bits
            } else {
                bits | (1 << 31)
            }
        };
        let value = |bits: u32| {
            f32::from_bits(if bits >> 31 != 0 {
                bits & !(1 << 31)
            } else {
                !bits
            })
        };
        let start = ordered(self.support.left);
        let end = ordered(self.support.right);
        let estimate = ordered(
            (self.support.left + index as f32 * self.cell_width)
                .clamp(self.support.left, self.support.right),
        );
        let at_or_after = |bits| self.index(value(bits)) >= index;
        let (mut low, mut high) = if at_or_after(estimate) {
            if estimate == start || !at_or_after(estimate - 1) {
                return value(estimate);
            }
            (start, estimate)
        } else {
            if estimate < end && at_or_after(estimate + 1) {
                return value(estimate + 1);
            }
            (estimate + 1, end)
        };
        while low < high {
            let middle = low + (high - low) / 2;
            if at_or_after(middle) {
                high = middle
            } else {
                low = middle + 1
            }
        }
        value(low)
    }
    fn contact_span(&self, index: usize) -> Option<[f32; 2]> {
        let left = self.contact_boundary(index);
        let right = self.contact_boundary(index + 1).next_down();
        (left <= right && self.index(left) == index).then_some([left, right])
    }
}
#[cfg_attr(test, derive(Debug))]
struct Rng(u64);
impl Rng {
    fn unit(&mut self) -> f32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        ((self.0 >> 40) as u32) as f32 / 16_777_216.0
    }
    fn signed(&mut self) -> f32 {
        self.unit() * 2.0 - 1.0
    }
}
#[derive(Clone, Copy, Default)]
#[cfg_attr(test, derive(Debug))]
struct ParticleUsage {
    total: usize,
    ordinary: usize,
}
impl ParticleUsage {
    fn add(&mut self, priority: bool) {
        self.total += 1;
        self.ordinary += usize::from(!priority);
    }
}
#[cfg_attr(test, derive(Debug))]
struct PendingCollapse {
    parts: Vec<Flake>,
}
impl PendingCollapse {
    fn position(&self) -> Point {
        let mass: f64 = self.parts.iter().map(|b| f64::from(b.mass)).sum();
        let mut x = 0.0;
        let mut y = 0.0;
        for body in &self.parts {
            let weight = f64::from(body.mass) / mass;
            x += f64::from(body.position.x) * weight;
            y += f64::from(body.position.y) * weight;
        }
        Point::new(x as f32, y as f32)
    }
}
#[cfg_attr(test, derive(Debug))]
struct FallingMask {
    /// Actual live supports excluded by an auxiliary detached-layer key.
    origins: BTreeSet<u64>,
    /// Logical IDs survive source closure without retaining native handles.
    front: BTreeSet<u64>,
    covers: Vec<Rect>,
    /// An inactive source never transfers visible material to this desktop.
    /// Bodies still obey monitor pauses and become outflow on landing/exit.
    hidden: bool,
    coalesced: bool,
}
/// Candidate IDs only, scoped to one body's swept contour. Contact results
/// remain point-specific; borrowed buckets cannot survive a scene mutation.
#[derive(Default)]
struct ContactCandidates<'a> {
    key: Option<(i32, i32, i32, i32)>,
    single: &'a [u64],
    multiple: BTreeSet<u64>,
}
impl<'a> ContactCandidates<'a> {
    fn select(&mut self, world: &'a World, from: i32, to: i32, first: i32, last: i32) {
        let key = (from, to, first, last);
        if self.key == Some(key) {
            return;
        }
        self.key = Some(key);
        self.single = &[];
        self.multiple.clear();
        if from == to && first == last {
            self.single = world.buckets.get(&(from, first)).map_or(&[], Vec::as_slice);
            // rebuild_buckets visits unique drift keys in BTreeMap order.
            debug_assert!(self.single.windows(2).all(|p| p[0] < p[1]));
        } else {
            for row in from..=to {
                for column in first..=last {
                    if let Some(ids) = world.buckets.get(&(row, column)) {
                        self.multiple.extend(ids.iter().copied());
                    }
                }
            }
        }
    }
    fn iter(&self) -> impl Iterator<Item = u64> + '_ {
        self.single.iter().chain(self.multiple.iter()).copied()
    }
}
/// On-demand model diagnostics; no native handles, history or background sampling.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize)]
pub struct BudgetSnapshot {
    pub active_bodies: usize,
    pub active_slabs: usize,
    pub active_profile_cells: usize,
    pub pending_packets: usize,
    pub pending_bodies: usize,
    pub pending_profile_cells: usize,
    pub spill_bodies: usize,
    pub supports: usize,
    pub drift_cells: usize,
    pub falling_masks: usize,
    pub max_active_bodies: usize,
    /// Absolute live-body bound; max_active_bodies is the current admission target.
    pub global_max_active_bodies: usize,
    pub max_pending_packets: usize,
    pub max_pending_bodies: usize,
}
#[cfg_attr(test, derive(Debug))]
pub struct World {
    pub scene: Scene,
    pub flakes: Vec<Flake>,
    pub drifts: BTreeMap<u64, Drift>,
    rng: Rng,
    wind: [Point; 40],
    emission: f32,
    size_factor: f32,
    scene_age: f32,
    edge_cursor: u64,
    buckets: BTreeMap<(i32, i32), Vec<u64>>,
    bucket_left: f32,
    bucket_width: f32,
    spill: VecDeque<Flake>,
    collapses: VecDeque<PendingCollapse>,
    falling_masks: BTreeMap<u64, FallingMask>,
    next_falling_mask: u64,
    usage: BTreeMap<Option<u64>, ParticleUsage>,
    paused_monitors: BTreeSet<u64>,
    last_screens: Vec<Screen>,
    pub introduced_mass: f64,
    pub removed_mass: f64,
    #[cfg(test)]
    contact_work: prefilter::ContactWork,
}
impl World {
    pub fn new(scene: Scene, seed: u64) -> Self {
        let mut world = Self {
            scene: Scene::default(),
            flakes: Vec::new(),
            drifts: BTreeMap::new(),
            rng: Rng(seed.max(1)),
            wind: [Point::default(); 40],
            emission: 0.0,
            size_factor: 1.0,
            scene_age: 0.0,
            edge_cursor: 0,
            buckets: BTreeMap::new(),
            bucket_left: 0.0,
            bucket_width: 128.0,
            spill: VecDeque::new(),
            collapses: VecDeque::new(),
            falling_masks: BTreeMap::new(),
            next_falling_mask: 1 << 62,
            usage: BTreeMap::new(),
            paused_monitors: BTreeSet::new(),
            last_screens: Vec::new(),
            introduced_mass: 0.0,
            removed_mass: 0.0,
            #[cfg(test)]
            contact_work: prefilter::ContactWork::default(),
        };
        world.set_scene(scene);
        world
    }
    pub fn total_mass(&self) -> f64 {
        self.flakes
            .iter()
            .chain(self.spill.iter())
            .chain(self.collapses.iter().flat_map(|p| &p.parts))
            .map(|f| f64::from(f.mass))
            .sum::<f64>()
            + self.drifts.values().map(Drift::total_mass).sum::<f64>()
    }
    pub fn set_paused_monitors(&mut self, monitors: BTreeSet<u64>) {
        if self.paused_monitors != monitors {
            for body in &mut self.flakes {
                if self.scene.paused_at(body.position, &self.paused_monitors)
                    != self.scene.paused_at(body.position, &monitors)
                {
                    body.previous_pose = None;
                }
            }
            for drift in self.drifts.values_mut().filter(|d| d.support.ground) {
                let key = drift.support.key & !(1 << 63);
                if self.paused_monitors.contains(&key) != monitors.contains(&key) {
                    drift.previous_y = None;
                }
            }
        }
        self.paused_monitors = monitors;
    }
    /// Invalidate visual intervals at a clock discontinuity; physics and material are untouched.
    pub fn reset_interpolation(&mut self) {
        for body in &mut self.flakes {
            body.previous_pose = None;
        }
        for drift in self.drifts.values_mut() {
            drift.previous_y = None;
        }
    }
    pub fn render_pose(&self, body: &Flake, fraction: f32) -> RenderPose {
        let fraction = if self.scene.paused_at(body.position, &self.paused_monitors) {
            1.0
        } else {
            fraction
        };
        body.render_pose(self.particle_scale(body), fraction)
    }
    /// Ground follows its physical approach; real window geometry and masks remain immediate.
    pub fn render_base(&self, drift: &Drift, fraction: f32) -> f32 {
        if drift.support.ground
            && !self
                .paused_monitors
                .contains(&(drift.support.key & !(1 << 63)))
        {
            render_mix(
                drift.previous_y.unwrap_or(drift.support.y),
                drift.support.y,
                render_fraction(fraction),
            )
        } else {
            drift.support.y
        }
    }
    pub fn clear(&mut self) {
        self.removed_mass += self.total_mass();
        self.flakes.clear();
        self.spill.clear();
        self.collapses.clear();
        self.usage.clear();
        self.falling_masks.clear();
        for drift in self.drifts.values_mut() {
            drift.previous_y = None;
            drift.mass.fill(0.0);
            drift.pending.fill(0.0);
            drift.disable_rate.fill(0.0);
            drift.shrink_remaining.fill(0.0);
            drift.last_size.fill(0);
            drift.edge_wait = [EDGE_INTERVAL; 2];
        }
        self.emission = 0.0;
        self.edge_cursor = 0;
    }
    /// One scale snapshot shared by the moving body's geometry and renderer.
    pub fn particle_scale(&self, body: &Flake) -> f32 {
        body.display_scale(self.scene.scale_at(body.position))
    }
    /// Only detached material has a depth inherited from its former window.
    /// Ordinary snowfall remains in the foreground.
    pub fn falling_clips(&self, body: &Flake, bounds: Rect) -> Option<Vec<Rect>> {
        let mask = self.falling_masks.get(&body.skip_support?)?;
        Some(if mask.hidden {
            Vec::new()
        } else {
            uncovered_rectangles(bounds, mask.covers.iter().copied())
        })
    }
    /// Waiting packets, each holding at most MAX_COLLAPSE_PARTS independent bodies.
    pub fn pending_collapse_count(&self) -> usize {
        self.collapses.len()
    }
    pub fn budget_snapshot(&self) -> BudgetSnapshot {
        let profile_cells = |body: &Flake| body.slab.as_ref().map_or(0, |s| s.heights.len());
        BudgetSnapshot {
            active_bodies: self.flakes.len(),
            active_slabs: self.flakes.iter().filter(|b| b.slab.is_some()).count(),
            active_profile_cells: self.flakes.iter().map(profile_cells).sum(),
            pending_packets: self.collapses.len(),
            pending_bodies: self.collapses.iter().map(|p| p.parts.len()).sum(),
            pending_profile_cells: self
                .collapses
                .iter()
                .flat_map(|p| &p.parts)
                .map(profile_cells)
                .sum(),
            spill_bodies: self.spill.len(),
            supports: self.drifts.len(),
            drift_cells: self.drifts.values().map(|d| d.mass.len()).sum(),
            falling_masks: self.falling_masks.len(),
            max_active_bodies: self.active_budget(),
            global_max_active_bodies: GLOBAL_MAX_FLAKES,
            max_pending_packets: self.collapse_budget(),
            max_pending_bodies: self.collapse_budget().saturating_mul(MAX_COLLAPSE_PARTS),
        }
    }
    fn priority_body(&self, body: &Flake) -> bool {
        body.skip_support
            .is_some_and(|key| self.falling_masks.contains_key(&key))
    }
    fn active_budget(&self) -> usize {
        MAX_FLAKES
            .saturating_mul(self.scene.screens.len().max(1))
            .min(GLOBAL_MAX_FLAKES)
    }
    fn reserve_per_screen(&self) -> usize {
        // Retain 128 where possible; on extremely many displays reservations
        // occupy at most half the global pool, leaving room for ordinary snowfall.
        BASE_COLLAPSE_BUDGET.min(self.active_budget() / self.scene.screens.len().max(1) / 2)
    }
    fn ordinary_budget(&self) -> usize {
        let screens = self.scene.screens.len().max(1);
        ORDINARY_FLAKES_PER_SCREEN.saturating_mul(screens).min(
            self.active_budget()
                .saturating_sub(self.reserve_per_screen().saturating_mul(screens)),
        )
    }
    fn slot_available(&self, body: &Flake) -> bool {
        let budget = self.active_budget();
        if self.flakes.len() >= budget {
            return false;
        }
        let key = self.scene.nearest_screen(body.position).map(|s| s.key);
        let used = self.usage.get(&key).copied().unwrap_or_default();
        let priority = self.priority_body(body);
        if priority && used.total - used.ordinary < self.reserve_per_screen() {
            // Wind/topology may have moved ordinary bodies into this screen.
            // Its minimum material reserve survives that temporary imbalance.
            return true;
        }
        if budget - self.flakes.len() <= self.reserved_slots() {
            return false;
        }
        if priority {
            used.total < MAX_FLAKES
        } else {
            used.total < MAX_FLAKES
                && used.ordinary < ORDINARY_FLAKES_PER_SCREEN
                && self.usage.values().map(|u| u.ordinary).sum::<usize>() < self.ordinary_budget()
        }
    }
    fn reserved_slots(&self) -> usize {
        let reserve = self.reserve_per_screen();
        let missing = |key| {
            let used = self.usage.get(&key).copied().unwrap_or_default();
            reserve.saturating_sub(used.total - used.ordinary)
        };
        if self.scene.screens.is_empty() {
            missing(None)
        } else {
            self.scene.screens.iter().fold(0_usize, |n, screen| {
                n.saturating_add(missing(Some(screen.key)))
            })
        }
    }
    fn activate(&mut self, mut body: Flake) {
        body.previous_pose = None;
        if body.display_scale.is_none() {
            body.set_display_scale(self.scene.scale_at(body.position));
        }
        let key = self.scene.nearest_screen(body.position).map(|s| s.key);
        let priority = self.priority_body(&body);
        self.usage.entry(key).or_default().add(priority);
        self.flakes.push(body);
    }
    fn rebuild_usage(&mut self) {
        self.usage.clear();
        for body in &self.flakes {
            let key = self.scene.nearest_screen(body.position).map(|s| s.key);
            let priority = self.priority_body(body);
            self.usage.entry(key).or_default().add(priority);
        }
    }
    fn collapse_budget(&self) -> usize {
        BASE_COLLAPSE_BUDGET.max(self.scene.screens.len().max(1).saturating_mul(2))
    }
    fn merge_pending_collapses(&mut self, a: Flake, b: Flake) -> Flake {
        let fallback = self.scene.screens.is_empty().then(|| Scene {
            screens: self.last_screens.clone(),
            windows: Vec::new(),
        });
        let mut body = crate::collapse::merge(&a, &b, fallback.as_ref().unwrap_or(&self.scene));
        let mut front = BTreeSet::new();
        let mut origins = BTreeSet::new();
        let mut hidden = false;
        for parent in [&a, &b] {
            if let Some(mask) = parent
                .skip_support
                .and_then(|key| self.falling_masks.get(&key))
            {
                front.extend(mask.front.iter().copied());
                origins.extend(mask.origins.iter().copied());
                hidden = mask.hidden;
            }
            if let Some(key) = parent.skip_support
                && self.scene.windows.iter().any(|window| window.key == key)
            {
                origins.insert(key);
            }
        }
        // A compacted packet owns a separate layer: changing its mask must not
        // change another body from either source, including live bodies.
        let key = loop {
            let key = self.next_falling_mask;
            self.next_falling_mask = self.next_falling_mask.wrapping_add(1);
            if !self.falling_masks.contains_key(&key)
                && !self.drifts.contains_key(&key)
                && !self.scene.windows.iter().any(|w| w.key == key)
            {
                break key;
            }
        };
        let covers = self
            .scene
            .windows
            .iter()
            .filter(|w| w.visible && (front.contains(&w.key) || !w.collect))
            .map(|w| w.rect)
            .collect();
        self.falling_masks.insert(
            key,
            FallingMask {
                front,
                covers,
                hidden,
                coalesced: true,
                origins,
            },
        );
        body.skip_support = Some(key);
        for parent in [&a, &b] {
            if let Some(key) = parent.skip_support
                && self
                    .falling_masks
                    .get(&key)
                    .is_some_and(|mask| mask.coalesced)
                && !self
                    .flakes
                    .iter()
                    .chain(&self.spill)
                    .chain(self.collapses.iter().flat_map(|p| &p.parts))
                    .any(|f| f.skip_support == Some(key))
            {
                self.falling_masks.remove(&key);
            }
        }
        body
    }
    fn collapse_class(&self, body: &Flake) -> (Option<u64>, bool) {
        (
            self.scene.nearest_screen(body.position).map(|s| s.key),
            body.skip_support
                .and_then(|key| self.falling_masks.get(&key))
                .is_some_and(|mask| mask.hidden),
        )
    }
    fn merge_pending_packets(
        &mut self,
        mut a: PendingCollapse,
        b: PendingCollapse,
    ) -> PendingCollapse {
        a.parts.extend(b.parts);
        loop {
            let mut best: Option<(usize, usize, bool, f64)> = None;
            for i in 0..a.parts.len() {
                for j in i + 1..a.parts.len() {
                    let x = &a.parts[i];
                    let y = &a.parts[j];
                    let layer = |body: &Flake| {
                        body.skip_support
                            .and_then(|key| self.falling_masks.get(&key))
                    };
                    let same_layer = match (layer(x), layer(y)) {
                        (Some(x), Some(y)) => x.hidden == y.hidden && x.front == y.front,
                        (None, None) => true,
                        _ => false,
                    };
                    let adjacent = same_layer && crate::collapse::adjacent(x, y, &self.scene);
                    let dx = f64::from(x.position.x) - f64::from(y.position.x);
                    let dy = f64::from(x.position.y) - f64::from(y.position.y);
                    let distance = dx * dx + dy * dy;
                    if best.is_none_or(|(_, _, old_adjacent, old)| {
                        adjacent && !old_adjacent || adjacent == old_adjacent && distance < old
                    }) {
                        best = Some((i, j, adjacent, distance));
                    }
                }
            }
            let Some((i, j, adjacent, _)) = best else {
                break;
            };
            if !adjacent && a.parts.len() <= MAX_COLLAPSE_PARTS {
                break;
            }
            let y = a.parts.remove(j);
            let x = a.parts.remove(i);
            let merged = self.merge_pending_collapses(x, y);
            a.parts.insert(i, merged);
        }
        a
    }
    fn requeue_collapses(&mut self) {
        let pending = std::mem::take(&mut self.collapses);
        for packet in pending {
            let class = self.collapse_class(&packet.parts[0]);
            if packet
                .parts
                .iter()
                .all(|body| self.collapse_class(body) == class)
            {
                self.queue_collapse_packet(packet);
            } else {
                // A changed screen layout may split a packet's admission class.
                for body in packet.parts {
                    self.queue_collapse(body);
                }
            }
        }
    }
    fn queue_collapse(&mut self, body: Flake) {
        self.queue_collapse_packet(PendingCollapse { parts: vec![body] });
    }
    fn repack_collapses_exact(&mut self, incoming: PendingCollapse) -> Result<(), PendingCollapse> {
        let mut counts = BTreeMap::new();
        for part in self
            .collapses
            .iter()
            .chain(std::iter::once(&incoming))
            .flat_map(|p| &p.parts)
        {
            *counts.entry(self.collapse_class(part)).or_insert(0_usize) += 1;
        }
        let needed: usize = counts
            .values()
            .map(|n| n.div_ceil(MAX_COLLAPSE_PARTS))
            .sum();
        if needed > self.collapse_budget() {
            return Err(incoming);
        }
        // Stable packing within each admission class preserves old parts while
        // removing unused gaps. Profiles and masks never pass through merge().
        let pending = std::mem::take(&mut self.collapses);
        let mut open: BTreeMap<(Option<u64>, bool), usize> = BTreeMap::new();
        for part in pending
            .into_iter()
            .chain(std::iter::once(incoming))
            .flat_map(|p| p.parts)
        {
            let class = self.collapse_class(&part);
            let index = match open
                .get(&class)
                .copied()
                .filter(|&i| self.collapses[i].parts.len() < MAX_COLLAPSE_PARTS)
            {
                Some(index) => index,
                None => {
                    let index = self.collapses.len();
                    self.collapses
                        .push_back(PendingCollapse { parts: Vec::new() });
                    open.insert(class, index);
                    index
                }
            };
            self.collapses[index].parts.push(part);
        }
        Ok(())
    }
    fn queue_collapse_packet(&mut self, body: PendingCollapse) {
        if self.collapses.len() < self.collapse_budget() {
            self.collapses.push_back(body);
            return;
        }
        let candidates: Vec<_> = self
            .collapses
            .iter()
            .chain(std::iter::once(&body))
            .map(|f| {
                (
                    f.position(),
                    self.collapse_class(&f.parts[0]),
                    f.parts.len(),
                )
            })
            .collect();
        let mut best: Option<(usize, usize, usize, f64)> = None;
        for i in 0..candidates.len() {
            for j in i + 1..candidates.len() {
                if candidates[i].1 != candidates[j].1 {
                    continue;
                }
                let dx = f64::from(candidates[i].0.x) - f64::from(candidates[j].0.x);
                let dy = f64::from(candidates[i].0.y) - f64::from(candidates[j].0.y);
                let distance = dx * dx + dy * dy;
                // Prefer a pair whose independent bodies still fit. If every
                // compatible pair overflows, discard as little geometry as possible.
                let excess = (candidates[i].2 + candidates[j].2).saturating_sub(MAX_COLLAPSE_PARTS);
                if best.is_none_or(|(_, _, old_excess, old_distance)| {
                    excess < old_excess || excess == old_excess && distance < old_distance
                }) {
                    best = Some((i, j, excess, distance));
                }
            }
        }
        // Two visibility classes per screen, with at least one extra packet:
        // a compatible pair exists even when every monitor has its own class.
        let (i, j, excess, _) = best.expect("pending collapse classes fit their budget");
        let body = if excess > 0 {
            match self.repack_collapses_exact(body) {
                Ok(()) => return,
                Err(body) => body,
            }
        } else {
            body
        };
        if j == self.collapses.len() {
            let a = self.collapses.remove(i).unwrap();
            let merged = self.merge_pending_packets(a, body);
            // Coalescing must inherit the oldest parent's admission position;
            // new arrivals must not repeatedly rejuvenate already waiting snow.
            self.collapses.insert(i, merged);
        } else {
            let b = self.collapses.remove(j).unwrap();
            let a = self.collapses.remove(i).unwrap();
            let merged = self.merge_pending_packets(a, b);
            self.collapses.insert(i, merged);
            self.collapses.push_back(body);
        }
    }
    fn falling_cursor_visible(&self, body: &Flake, cursor: Point) -> bool {
        body.skip_support
            .and_then(|key| self.falling_masks.get(&key))
            .is_none_or(|mask| {
                !mask.hidden
                    && !mask
                        .covers
                        .iter()
                        .any(|r| r.contains(body.position) || r.contains(cursor))
            })
    }
    fn retire_falling_masks(&mut self) {
        if self.falling_masks.is_empty() {
            return;
        }
        let used: BTreeSet<_> = self
            .flakes
            .iter()
            .chain(&self.spill)
            .chain(self.collapses.iter().flat_map(|p| &p.parts))
            .filter_map(|body| body.skip_support)
            .collect();
        self.falling_masks.retain(|key, _| used.contains(key));
    }
    fn refresh_falling_masks(&mut self) {
        self.retire_falling_masks();
        let current: BTreeSet<_> = self.scene.windows.iter().map(|w| w.key).collect();
        for mask in self.falling_masks.values_mut() {
            mask.front.retain(|key| current.contains(key));
            mask.origins.retain(|key| current.contains(key));
            mask.covers = self
                .scene
                .windows
                .iter()
                .filter(|window| {
                    window.visible && (mask.front.contains(&window.key) || !window.collect)
                })
                .map(|window| window.rect)
                .collect();
        }
    }
    pub fn set_flake_size(&mut self, percent: u8) {
        let factor = (percent as f32 / 100.0).clamp(0.25, 2.0);
        if factor == self.size_factor {
            return;
        }
        let ratio = factor / self.size_factor;
        for flake in self.flakes.iter_mut().chain(self.spill.iter_mut()) {
            if flake.slab.is_some() {
                continue;
            }
            let old = flake.mass;
            flake.radius *= ratio;
            flake.mass *= ratio * ratio;
            if flake.mass >= old {
                self.introduced_mass += f64::from(flake.mass - old);
            } else {
                self.removed_mass += f64::from(old - flake.mass);
            }
        }
        self.size_factor = factor;
    }
    pub fn release(&mut self, mut flake: Flake) -> bool {
        if ![
            flake.position.x,
            flake.position.y,
            flake.velocity.x,
            flake.velocity.y,
            flake.radius,
            flake.mass,
            flake.shape,
            flake.angle,
            flake.spin,
            flake.age,
            flake.opacity,
        ]
        .iter()
        .all(|n| n.is_finite())
            || !(0.2..=32.0).contains(&flake.radius)
            || !(0.01..=1_000_000.0).contains(&flake.mass)
            || !(0.2..=3.0).contains(&flake.shape)
            || flake.velocity.x.abs() > 10_000.0
            || flake.velocity.y.abs() > 10_000.0
            || !self.slot_available(&flake)
        {
            return false;
        }
        self.introduced_mass += f64::from(flake.mass);
        flake.set_display_scale(self.scene.scale_at(flake.position));
        self.activate(flake);
        true
    }
    fn queue_spill(
        &mut self,
        position: Point,
        mass: f32,
        velocity: Point,
        origin: Option<u64>,
        scale: f32,
    ) {
        if mass <= 0.0 {
            return;
        }
        // Below a 0.2 DIP grain the material is subpixel outflow. Do not turn
        // arbitrarily small fractions into equally visible, high-drag flakes.
        if mass < 1.20 * 0.2 * 0.2 {
            self.removed_mass += f64::from(mass);
            return;
        }
        if self.spill.len() >= 128 {
            if let Some(last) = self.spill.iter_mut().rev().find(|body| {
                body.slab.is_none()
                    && body.skip_support == origin
                    && !self.scene.paused_at(body.position, &self.paused_monitors)
            }) {
                let total = last.mass + mass;
                let weight = mass / total;
                let old_scale = last.display_scale(scale);
                let merged_scale = ((f64::from(last.mass) * f64::from(old_scale).powi(2)
                    + f64::from(mass) * f64::from(scale).powi(2))
                    / f64::from(total))
                .sqrt() as f32;
                last.position.x += (position.x - last.position.x) * weight;
                last.position.y += (position.y - last.position.y) * weight;
                last.velocity.x = (last.velocity.x * old_scale * (1.0 - weight)
                    + velocity.x * scale * weight)
                    / merged_scale;
                last.velocity.y = (last.velocity.y * old_scale * (1.0 - weight)
                    + velocity.y * scale * weight)
                    / merged_scale;
                last.set_display_scale(merged_scale);
                last.mass += mass;
                last.radius = (last.mass / 1.20).sqrt().min(5.5);
            } else {
                self.removed_mass += f64::from(mass);
            }
            return;
        }
        let mut flake = Flake::new(position, (mass / 1.20).sqrt().min(5.5));
        flake.set_display_scale(scale);
        flake.mass = mass;
        flake.velocity = velocity;
        flake.age = 1.0;
        flake.angle = self.rng.unit() * TAU;
        flake.spin = self.rng.signed();
        flake.skip_support = origin;
        self.spill.push_back(flake);
    }
    fn release_slabs(&mut self, old: &Drift) {
        let mut first = 0;
        while first < old.mass.len() {
            if old.mass[first] + old.pending[first] <= 0.0 {
                first += 1;
                continue;
            }
            // Irregular fracture spacing, with a bounded contour per body.
            let count = 7 + (self.rng.unit() * 10.0) as usize;
            let mut end = (first + count).min(old.mass.len());
            let screen = self
                .scene
                .nearest_screen(Point::new(old.x(first), old.support.y - 1.0))
                .map(|s| s.key);
            if let Some(gap) = (first..end).find(|&i| {
                old.mass[i] + old.pending[i] <= 0.0
                    || self
                        .scene
                        .nearest_screen(Point::new(old.x(i), old.support.y - 1.0))
                        .map(|s| s.key)
                        != screen
            }) {
                end = gap;
            }
            self.release_slab_cells(
                old,
                first..end,
                old.support.left + first as f32 * old.cell_width,
                old.cell_width,
                old.support.key,
            );
            first = end;
        }
    }

    fn release_slab_cells(
        &mut self,
        old: &Drift,
        cells: std::ops::Range<usize>,
        left: f32,
        physical_width: f32,
        layer: u64,
    ) {
        let total: f32 = cells.clone().map(|i| old.mass[i] + old.pending[i]).sum();
        if total <= 0.0 || physical_width <= 0.0 {
            return;
        }
        let mass = total * (physical_width / old.cell_width);
        let scale = old.profile_scale(cells.start, cells.end);
        let cell_width = physical_width / scale;
        let heights: Vec<_> = cells
            .clone()
            .map(|i| old.contact_height(i, &self.scene) / scale)
            .collect();
        let mut center = Point::default();
        for (local, &height) in heights.iter().enumerate() {
            let i = cells.start + local;
            let weight = (old.mass[i] + old.pending[i]) / total;
            center.x += (local as f32 + 0.5) * cell_width * weight;
            center.y -= height * 0.5 * weight;
        }
        let mut body = Flake::new(
            Point::new(left + center.x * scale, old.support.y + center.y * scale),
            (mass / 1.20).sqrt().clamp(0.2, 32.0),
        );
        body.set_display_scale(scale);
        body.mass = mass;
        body.velocity = Point::new(
            (old.motion_velocity.x * old.motion_scale / scale).clamp(-180.0, 180.0),
            (old.motion_velocity.y * old.motion_scale / scale).clamp(0.0, 80.0) + 12.0,
        );
        body.spin = self.rng.signed() * 0.55;
        body.age = 1.0;
        body.skip_support = Some(layer);
        body.slab = Some(SnowSlab {
            heights,
            cell_width,
            center,
        });
        self.queue_collapse(body);
    }

    fn resize_shift(old: &Drift, new: &Drift, fixed_display_space: bool) -> Option<f32> {
        if new.support.ground {
            return None;
        }
        if new.support.right <= new.support.left {
            return Some(0.0);
        }
        let old_inset = old.support.left - old.support.motion_origin.x;
        let new_inset = new.support.left - new.support.motion_origin.x;
        let old_right = old.support.right + old_inset;
        let new_right = new.support.right + new_inset;
        if (!fixed_display_space && (old.motion_scale - new.motion_scale).abs() > 0.001)
            || ((old_right - old.support.motion_origin.x)
                - (new_right - new.support.motion_origin.x))
                .abs()
                <= 0.001
        {
            return None;
        }
        let left = new.support.motion_origin.x - old.support.motion_origin.x;
        let right = new_right - old_right;
        // A shared edge displacement is translation; opposing movements only resize.
        Some(if left * right > 0.0 {
            if left.abs() < right.abs() {
                left
            } else {
                right
            }
        } else {
            0.0
        })
    }

    fn rebase_resize_inset(old: &mut Drift, new: &Drift) {
        let inset_delta = (new.support.left - new.support.motion_origin.x)
            - (old.support.left - old.support.motion_origin.x);
        if inset_delta.abs() <= 0.001 || new.support.right <= new.support.left {
            return;
        }
        // Rebase the synthetic DPI inset within the old native frame before
        // cutting actual frame changes. An unchanged frame edge must not shed
        // snow just because the motion reference selected another display.
        let old_width = (old.support.right - old.support.left).max(old.cell_width);
        let width = (old_width - 2.0 * inset_delta).max(1.0);
        old.support.left += inset_delta;
        old.support.right = old.support.left + width;
        old.cell_width = width / old.mass.len() as f32;
        let ratio = (width / old_width).sqrt();
        for scale in &mut old.scales {
            *scale = (*scale * ratio).clamp(MIN_MATERIAL_SCALE, MAX_MATERIAL_SCALE);
        }
    }

    fn resize_layer(&mut self, source: u64, geometry: &Scene) -> u64 {
        let key = loop {
            let key = self.next_falling_mask;
            self.next_falling_mask = key.wrapping_add(1);
            if !self.falling_masks.contains_key(&key)
                && !self.drifts.contains_key(&key)
                && !self.scene.windows.iter().any(|w| w.key == key)
                && !geometry.windows.iter().any(|w| w.key == key)
            {
                break key;
            }
        };
        let index = geometry.windows.iter().position(|w| w.key == source);
        let front: BTreeSet<_> = index
            .into_iter()
            .flat_map(|i| &geometry.windows[..i])
            .map(|w| w.key)
            .collect();
        let covers = geometry
            .windows
            .iter()
            .filter(|w| w.visible && (front.contains(&w.key) || !w.collect))
            .map(|w| w.rect)
            .collect();
        self.falling_masks.insert(
            key,
            FallingMask {
                front,
                covers,
                hidden: index.is_some_and(|i| !geometry.windows[i].visible),
                coalesced: false,
                origins: [source].into(),
            },
        );
        key
    }

    fn release_cut(&mut self, old: &Drift, start: f64, end: f64, shift: f32, layer: u64) {
        let mut first = start.floor() as usize;
        let n = old.mass.len();
        while first < n && (first as f64) < end {
            let a = start.max(first as f64);
            let b = end.min(first as f64 + 1.0);
            if old.mass[first] + old.pending[first] <= 0.0 {
                first += 1;
                continue;
            }
            let offset = old.support.left + shift;
            let left = (f64::from(offset) + a * f64::from(old.cell_width)) as f32;
            if a > first as f64 || b < first as f64 + 1.0 {
                self.release_slab_cells(
                    old,
                    first..first + 1,
                    left,
                    ((b - a) * f64::from(old.cell_width)) as f32,
                    layer,
                );
                first += 1;
                continue;
            }
            let count = 7 + (self.rng.unit() * 10.0) as usize;
            let mut last = (first + count).min(n).min(end.floor() as usize);
            let owner = |i: usize| {
                self.scene
                    .nearest_screen(Point::new(old.x(i) + shift, old.support.y - 1.0))
                    .map(|s| s.key)
            };
            let screen = owner(first);
            if let Some(gap) =
                (first..last).find(|&i| old.mass[i] + old.pending[i] <= 0.0 || owner(i) != screen)
            {
                last = gap;
            }
            self.release_slab_cells(old, first..last, left, old.cell_width, layer);
            first = last;
        }
    }

    fn remap_resize(&mut self, old: &Drift, new: &mut Drift, shift: f32, geometry: &Scene) {
        let old_left = f64::from(old.support.left) + f64::from(shift);
        let n = old.mass.len() as f64;
        if new.support.right <= new.support.left {
            let layer = self.resize_layer(old.support.key, geometry);
            self.release_cut(old, 0.0, n, shift, layer);
            return;
        }
        let old_cell = f64::from((old.support.right - old.support.left).max(old.cell_width)) / n;
        let new_cell = f64::from(new.support.right - new.support.left) / new.mass.len() as f64;
        // Unchanged/enclosing frame edges retain the complete boundary cell.
        // Dividing its width by its cell width could otherwise yield n - one ULP.
        let start = if f64::from(new.support.left) <= old_left {
            0.0
        } else {
            ((f64::from(new.support.left) - old_left) / old_cell).clamp(0.0, n)
        };
        let end = if f64::from(new.support.right) >= f64::from(old.support.right) + f64::from(shift)
        {
            n
        } else {
            ((f64::from(new.support.right) - old_left) / old_cell).clamp(0.0, n)
        };
        if start > 0.0 || end < n {
            let layer = self.resize_layer(old.support.key, geometry);
            self.release_cut(old, 0.0, start, shift, layer);
            self.release_cut(old, end, n, shift, layer);
        }
        let mut areas = vec![0.0_f64; new.mass.len()];
        for i in 0..old.mass.len() {
            let a = (old_left + i as f64 * old_cell - f64::from(new.support.left)) / new_cell;
            let b = a + old_cell / new_cell;
            let first = a.floor().max(0.0) as usize;
            let last = (b.ceil().max(0.0) as usize).min(new.mass.len());
            for (j, area) in areas.iter_mut().enumerate().take(last).skip(first) {
                let fraction =
                    ((b.min(j as f64 + 1.0) - a.max(j as f64)).max(0.0) / (b - a)) as f32;
                new.mass[j] += old.mass[i] * fraction;
                new.pending[j] += old.pending[i] * fraction;
                *area += f64::from(old.mass[i] + old.pending[i])
                    * f64::from(fraction)
                    * f64::from(old.scales[i]).powi(2);
                new.disable_rate[j] += old.disable_rate[i] * fraction;
                new.shrink_remaining[j] = new.shrink_remaining[j].max(old.shrink_remaining[i]);
                new.last_size[j] = new.last_size[j].max(old.last_size[i]);
            }
        }
        for (i, area) in areas.into_iter().enumerate() {
            let mass = f64::from(new.mass[i] + new.pending[i]);
            if mass > 0.0 {
                new.scales[i] =
                    ((area / mass).sqrt() as f32).clamp(MIN_MATERIAL_SCALE, MAX_MATERIAL_SCALE);
            }
        }
    }
    pub fn set_scene(&mut self, scene: Scene) {
        if self.scene.screens.is_empty() != scene.screens.is_empty() {
            self.reset_interpolation();
        }
        // No selected display suspends physics, rather than dismantling its
        // resting surfaces. Keep the last units for closures during that gap.
        if self.scene.screens.is_empty() {
            self.scene.screens.clone_from(&self.last_screens);
            self.requeue_collapses();
        }
        let current: BTreeSet<_> = scene.windows.iter().map(|w| w.key).collect();
        for mask in self.falling_masks.values_mut() {
            mask.front.retain(|key| current.contains(key));
        }
        let fallback = scene.screens.is_empty().then(|| Scene {
            screens: self.last_screens.clone(),
            windows: scene.windows.clone(),
        });
        let geometry = fallback.as_ref().unwrap_or(&scene);
        let fixed_display_space = self.scene.screens.len() == geometry.screens.len()
            && self.scene.screens.iter().all(|old| {
                geometry
                    .screens
                    .iter()
                    .any(|new| old.key == new.key && old.rect == new.rect && old.scale == new.scale)
            });
        let sample_age = self.scene_age;
        self.scene_age = 0.0;
        let mut previous = std::mem::take(&mut self.drifts);
        let mut cells = 0;
        for mut support in geometry.supports() {
            if support.ground
                && let Some(old) = previous.get(&support.key)
                && let Some(screen) = geometry
                    .screens
                    .iter()
                    .find(|s| s.key | (1 << 63) == support.key)
                && self
                    .scene
                    .screens
                    .iter()
                    .any(|s| s.key == screen.key && s.rect == screen.rect)
            {
                // Taskbar/work-area movement changes only the target. A changed
                // display rectangle is rebased immediately into its new space.
                support = geometry.ground_support(screen, old.support.y);
            }
            let width = (support.right - support.left).max(1.0);
            let count = (width / CELL).ceil().clamp(1.0, 4096.0) as usize;
            if cells + count > MAX_CELLS {
                continue;
            }
            cells += count;
            let cell_width = width / count as f32;
            let target_scales: Vec<_> = (0..count)
                .map(|i| {
                    geometry.scale_at(Point::new(
                        support.left + (i as f32 + 0.5) * cell_width,
                        support.y - 1.0,
                    ))
                })
                .collect();
            let motion_scale = geometry.scale_at(Point::new(
                support.motion_origin.x + 10.0,
                support.motion_origin.y,
            ));
            let mut drift = Drift {
                support,
                mass: vec![0.0; count],
                pending: vec![0.0; count],
                scratch: vec![0.0; count],
                flux: vec![0.0; count.saturating_sub(1)],
                disable_rate: vec![0.0; count],
                shrink_remaining: vec![0.0; count],
                last_size: vec![0; count],
                edge_wait: [EDGE_INTERVAL; 2],
                cell_width,
                motion_velocity: Point::default(),
                motion_scale,
                scales: target_scales.clone(),
                target_scales,
                previous_y: None,
            };
            if let Some(mut old) = previous.remove(&drift.support.key) {
                let resize_shift = Self::resize_shift(&old, &drift, fixed_display_space);
                if drift.support.ground && drift.support.y == old.support.y {
                    drift.previous_y = old.previous_y;
                }
                drift.edge_wait = old.edge_wait;
                if sample_age > 0.15 {
                    old.motion_velocity = Point::default();
                }
                if !drift.support.ground {
                    drift.motion_velocity = Point::new(
                        old.motion_velocity.x * old.motion_scale / drift.motion_scale,
                        old.motion_velocity.y * old.motion_scale / drift.motion_scale,
                    );
                }
                if !drift.support.ground && sample_age > 0.0 {
                    let scale = drift.motion_scale;
                    let dx = resize_shift
                        .unwrap_or(drift.support.motion_origin.x - old.support.motion_origin.x)
                        / scale;
                    let dy = (drift.support.motion_origin.y - old.support.motion_origin.y) / scale;
                    let distance = dx.hypot(dy);
                    let ratio = width / (old.support.right - old.support.left).max(1.0);
                    let logical_ratio = ratio * old.motion_scale / drift.motion_scale;
                    let teleport = distance > 480.0
                        || (!(0.65..=1.55).contains(&ratio)
                            && !(0.65..=1.55).contains(&logical_ratio));
                    if teleport {
                        drift.motion_velocity = Point::default();
                    }
                    if !teleport && !(distance < 0.01 && sample_age > 0.15) {
                        let dt = sample_age.clamp(DT, 0.06);
                        let alpha = dt / (0.07 + dt);
                        let velocity = Point::new(
                            drift.motion_velocity.x + (dx / dt - drift.motion_velocity.x) * alpha,
                            drift.motion_velocity.y + (dy / dt - drift.motion_velocity.y) * alpha,
                        );
                        let acceleration = (velocity.x - drift.motion_velocity.x)
                            .hypot(velocity.y - drift.motion_velocity.y)
                            / dt;
                        let fraction =
                            ((acceleration - 1800.0).max(0.0) * dt * 0.0006).clamp(0.0, 0.40);
                        drift.motion_velocity = velocity;
                        if fraction > 0.0 {
                            for index in 0..old.mass.len() {
                                if self.scene.paused_at(
                                    Point::new(old.x(index), old.support.y - 1.0),
                                    &self.paused_monitors,
                                ) {
                                    continue;
                                }
                                let height = old.contact_height(index, &self.scene);
                                let mass = (old.mass[index] + old.pending[index]) * fraction;
                                old.mass[index] *= 1.0 - fraction;
                                old.pending[index] *= 1.0 - fraction;
                                let position = Point::new(old.x(index), old.support.y - height);
                                let velocity = Point::new(
                                    (old.motion_velocity.x * old.motion_scale / old.scales[index])
                                        .clamp(-180.0, 180.0),
                                    (old.motion_velocity.y * old.motion_scale / old.scales[index])
                                        .clamp(0.0, 80.0)
                                        + 12.0,
                                );
                                self.queue_spill(
                                    position,
                                    mass,
                                    velocity,
                                    Some(old.support.key),
                                    old.scales[index],
                                );
                            }
                        }
                    }
                }
                if let Some(shift) = resize_shift {
                    Self::rebase_resize_inset(&mut old, &drift);
                    self.remap_resize(&old, &mut drift, shift, geometry);
                } else if old.mass.len() == count
                    && (old.cell_width - drift.cell_width).abs() < 0.001
                {
                    drift.scales = old.scales;
                    drift.mass = old.mass;
                    drift.pending = old.pending;
                    drift.disable_rate = old.disable_rate;
                    drift.shrink_remaining = old.shrink_remaining;
                    drift.last_size = old.last_size;
                } else {
                    // Preserve resting and just-arrived material during resampling.
                    let mut physical_areas = vec![0.0_f64; count];
                    let reference_dpi_changed = !drift.support.ground
                        && (old.motion_scale - drift.motion_scale).abs() > 0.001;
                    for index in 0..old.mass.len() {
                        let a = index as f32 / old.mass.len() as f32 * count as f32;
                        let b = (index + 1) as f32 / old.mass.len() as f32 * count as f32;
                        for (target, physical_area) in physical_areas
                            .iter_mut()
                            .enumerate()
                            .take((b.ceil() as usize).min(count))
                            .skip(a.floor() as usize)
                        {
                            let fraction = (b.min((target + 1) as f32) - a.max(target as f32))
                                .max(0.0)
                                / (b - a);
                            drift.mass[target] += old.mass[index] * fraction;
                            drift.pending[target] += old.pending[index] * fraction;
                            // The reference also controls the inset on both ends,
                            // including cells whose own display DPI did not change.
                            let dpi_changed = reference_dpi_changed
                                || (old.target_scales[index] - drift.target_scales[target]).abs()
                                    > 0.001;
                            let stretch = if dpi_changed {
                                f64::from(width) / f64::from(old.support.right - old.support.left)
                            } else {
                                1.0
                            };
                            *physical_area += (f64::from(old.mass[index])
                                + f64::from(old.pending[index]))
                                * f64::from(fraction)
                                * f64::from(old.scales[index]).powi(2)
                                * stretch;
                            drift.disable_rate[target] += old.disable_rate[index] * fraction;
                            drift.shrink_remaining[target] =
                                drift.shrink_remaining[target].max(old.shrink_remaining[index]);
                            drift.last_size[target] =
                                drift.last_size[target].max(old.last_size[index]);
                        }
                    }
                    for (i, area) in physical_areas.into_iter().enumerate() {
                        let mass = f64::from(drift.mass[i]) + f64::from(drift.pending[i]);
                        if mass > 0.0 {
                            drift.scales[i] = ((area / mass).sqrt() as f32)
                                .clamp(MIN_MATERIAL_SCALE, MAX_MATERIAL_SCALE);
                        }
                    }
                }
            }
            self.drifts.insert(drift.support.key, drift);
        }
        for mut old in previous.into_values() {
            if sample_age > 0.15 {
                old.motion_velocity = Point::default();
            }
            if !old.support.ground {
                if let Some(index) = self
                    .scene
                    .windows
                    .iter()
                    .position(|w| w.key == old.support.key)
                {
                    self.falling_masks.insert(
                        old.support.key,
                        FallingMask {
                            front: self.scene.windows[..index]
                                .iter()
                                .filter(|w| current.contains(&w.key))
                                .map(|w| w.key)
                                .collect(),
                            covers: Vec::new(),
                            hidden: !self.scene.windows[index].visible,
                            coalesced: false,
                            origins: BTreeSet::new(),
                        },
                    );
                }
                self.release_slabs(&old);
                continue;
            }
            for index in 0..old.mass.len() {
                let mass = old.mass[index] + old.pending[index];
                let velocity = Point::new(
                    (old.motion_velocity.x * old.motion_scale / old.scales[index])
                        .clamp(-180.0, 180.0)
                        + self.rng.signed() * 6.0,
                    (old.motion_velocity.y * old.motion_scale / old.scales[index]).clamp(0.0, 80.0)
                        + 12.0,
                );
                let height = old.contact_height(index, &self.scene);
                let position = Point::new(old.x(index), old.support.y - height * self.rng.unit());
                self.queue_spill(
                    position,
                    mass,
                    velocity,
                    Some(old.support.key),
                    old.scales[index],
                );
            }
        }
        if !scene.screens.is_empty() {
            self.last_screens.clone_from(&scene.screens);
        }
        self.scene = scene;
        self.requeue_collapses();
        self.refresh_falling_masks();
        self.rebuild_buckets();
        self.rebuild_usage();
    }
    fn rebuild_buckets(&mut self) {
        self.buckets.clear();
        let bounds = self.scene.bounds();
        self.bucket_left = bounds.left;
        // Bound the horizontal index even on very wide virtual desktops.
        self.bucket_width = (bounds.width() / MAX_BUCKET_COLUMNS as f32).max(128.0);
        for drift in self.drifts.values() {
            let left = drift.support.left.max(bounds.left);
            let right = drift.support.right.min(bounds.right);
            if right <= left {
                continue;
            }
            let first = ((left - self.bucket_left) / self.bucket_width).floor() as i32;
            let last = ((right - self.bucket_left) / self.bucket_width).floor() as i32;
            // Ground: 120 DIP; window ledges: 32 DIP, at up to 400% DPI.
            let reach = if drift.support.ground { 490.0 } else { 140.0 };
            let from = ((drift.support.y - reach) / 128.0).floor() as i32;
            let to = (drift.support.y / 128.0).floor() as i32;
            for row in from..=to {
                for column in first..=last {
                    self.buckets
                        .entry((row, column))
                        .or_default()
                        .push(drift.support.key);
                }
            }
        }
    }
    fn advance_ground(&mut self) {
        let mut buckets_changed = false;
        for screen in &self.scene.screens {
            if self.paused_monitors.contains(&screen.key) {
                continue;
            }
            let Some(drift) = self.drifts.get_mut(&(screen.key | (1 << 63))) else {
                continue;
            };
            let old = drift.support.y;
            drift.previous_y = Some(old);
            let next = crate::geometry::approach_floor(old, screen.floor, screen.scale, DT);
            if next != old {
                buckets_changed |= ((old - 490.0) / 128.0).floor()
                    != ((next - 490.0) / 128.0).floor()
                    || (old / 128.0).floor() != (next / 128.0).floor();
                drift.support = self.scene.ground_support(screen, next);
            }
        }
        if buckets_changed {
            self.rebuild_buckets();
        }
    }
    fn random_flake(&mut self, position: Point) -> Flake {
        let size = self.rng.unit();
        let radius = (1.2 + size * size * 5.5) * self.size_factor;
        let mut flake = Flake::new(position, radius);
        flake.opacity = 0.38 + size * 0.58;
        flake.shape = 0.75 + self.rng.unit() * 0.65;
        flake.velocity = Point::new(
            self.rng.signed() * 4.0,
            (GRAVITY * flake.mass / (0.09 * radius * flake.shape)).sqrt(),
        );
        flake.angle = self.rng.unit() * TAU;
        flake.spin = self.rng.signed() * 0.65;
        flake
    }
    pub fn seed_air(&mut self, settings: &Settings) {
        self.set_flake_size(settings.snow.flake_size_percent);
        self.rebuild_usage();
        for screen in self.scene.screens.clone() {
            if self.paused_monitors.contains(&screen.key) {
                continue;
            }
            let area = screen.rect.width() * (screen.floor - screen.rect.top).max(0.0)
                / 2_073_600.0
                / screen.scale.powi(2);
            let count = (area * settings.snow.intensity_percent as f32 * 7.0).min(1600.0) as usize;
            for _ in 0..count {
                let point = Point::new(
                    screen.rect.left + self.rng.unit() * screen.rect.width(),
                    screen.rect.top + self.rng.unit() * (screen.floor - screen.rect.top),
                );
                let mut flake = self.random_flake(point);
                flake.age = 1.0;
                self.release(flake);
            }
        }
    }
    fn air(&self, position: Point) -> Point {
        let bounds = self.scene.bounds();
        let gx = ((position.x - bounds.left) / bounds.width().max(1.0) * 7.0).clamp(0.0, 6.999);
        let gy = ((position.y - bounds.top) / bounds.height().max(1.0) * 4.0).clamp(0.0, 3.999);
        let (x, y) = (gx as usize, gy as usize);
        let (tx, ty) = (gx - x as f32, gy - y as f32);
        let samples = [
            (self.wind[y * 8 + x], (1.0 - tx) * (1.0 - ty)),
            (self.wind[y * 8 + x + 1], tx * (1.0 - ty)),
            (self.wind[(y + 1) * 8 + x], (1.0 - tx) * ty),
            (self.wind[(y + 1) * 8 + x + 1], tx * ty),
        ];
        samples.into_iter().fold(Point::default(), |p, (v, w)| {
            Point::new(p.x + v.x * w, p.y + v.y * w)
        })
    }
    pub fn step(&mut self, settings: &Settings, cursor: Option<Cursor>) {
        self.step_inner::<true>(settings, cursor);
    }
    fn step_inner<const OPTIMIZE_CONTACTS: bool>(
        &mut self,
        settings: &Settings,
        cursor: Option<Cursor>,
    ) {
        #[cfg(test)]
        {
            self.contact_work = prefilter::ContactWork::default();
        }
        if self
            .scene
            .screens
            .iter()
            .all(|s| self.paused_monitors.contains(&s.key))
        {
            return;
        }
        self.advance_ground();
        for drift in self.drifts.values_mut() {
            drift.advance_scales(&self.scene, &self.paused_monitors);
        }
        self.scene_age = (self.scene_age + DT).min(2.0);
        self.set_flake_size(settings.snow.flake_size_percent);
        self.rebuild_usage();
        let bounds = self.scene.bounds();
        for (index, wind) in self.wind.iter_mut().enumerate() {
            let point = Point::new(
                bounds.left + (index % 8) as f32 * bounds.width() / 7.0,
                bounds.top + (index / 8) as f32 * bounds.height() / 4.0,
            );
            if self.scene.paused_at(point, &self.paused_monitors) {
                continue;
            }
            // Bounded Ornstein-Uhlenbeck gusts: persistent stochastic air velocity.
            wind.x += ((settings.snow.wind_percent as f32 * 0.9 - wind.x) * 0.45) * DT
                + self.rng.signed() * 14.0 * DT.sqrt();
            wind.y += -wind.y * 0.7 * DT + self.rng.signed() * 7.0 * DT.sqrt();
            wind.x = wind.x.clamp(-150.0, 150.0);
            wind.y = wind.y.clamp(-28.0, 28.0);
        }
        for _ in 0..self.collapses.len() {
            let mut packet = self.collapses.pop_front().unwrap();
            let mut index = 0;
            while index < packet.parts.len() {
                let body = &packet.parts[index];
                if self.scene.paused_at(body.position, &self.paused_monitors)
                    || !self.slot_available(body)
                {
                    index += 1;
                } else {
                    let body = packet.parts.remove(index);
                    self.activate(body);
                }
            }
            if !packet.parts.is_empty() {
                self.collapses.push_back(packet);
            }
        }
        for _ in 0..self.spill.len() {
            let flake = self.spill.pop_front().unwrap();
            if self.scene.paused_at(flake.position, &self.paused_monitors)
                || !self.slot_available(&flake)
            {
                self.spill.push_back(flake);
            } else {
                self.activate(flake);
            }
        }
        let bounds = self.scene.bounds();
        let total_width = self
            .scene
            .screens
            .iter()
            .filter(|s| !self.paused_monitors.contains(&s.key))
            .map(|s| s.rect.width() / s.scale)
            .sum::<f32>();
        self.emission +=
            (total_width / 1920.0) * settings.snow.intensity_percent as f32 * 0.65 * DT;
        self.emission = self.emission.min(8.0);
        while self.emission >= 1.0 {
            self.emission -= 1.0;
            let mut sample = self.rng.unit() * total_width;
            let screen = self.scene.screens.iter().find(|s| {
                if self.paused_monitors.contains(&s.key) {
                    return false;
                }
                sample -= s.rect.width() / s.scale;
                sample < 0.0
            });
            let Some(screen) = screen else {
                break;
            };
            let point = Point::new(
                screen.rect.left + self.rng.unit() * screen.rect.width(),
                screen.rect.top - 12.0 * screen.scale,
            );
            let flake = self.random_flake(point);
            self.release(flake);
        }
        let mut alive = Vec::with_capacity(self.flakes.len());
        #[cfg(test)]
        let mut contact_work = prefilter::ContactWork::default();
        let flakes = std::mem::take(&mut self.flakes);
        for mut flake in flakes {
            if self.scene.paused_at(flake.position, &self.paused_monitors) {
                alive.push(flake);
                continue;
            }
            let before = flake.position;
            let target_scale = self.scene.scale_at(before);
            let before_pose = flake.current_pose(target_scale);
            let before_scale = flake.display_scale(target_scale);
            let scale = before_scale;
            let mut air = self.air(before);
            if settings.interaction.cursor_enabled
                && let Some(cursor) = cursor
                && self.falling_cursor_visible(&flake, cursor.position)
            {
                let dx = (before.x - cursor.position.x) / scale;
                let dy = (before.y - cursor.position.y) / scale;
                let distance = dx.hypot(dy);
                if distance < 65.0 && distance > 0.1 {
                    let weight = (1.0 - distance / 65.0).powi(2);
                    air.x += (dx / distance * 36.0 + cursor.velocity.x.clamp(-120.0, 120.0) * 0.5)
                        * weight;
                    air.y += (dy / distance * 24.0 + cursor.velocity.y.clamp(-100.0, 100.0) * 0.35)
                        * weight;
                }
            }
            flake.integrate(air, target_scale);
            let scale = flake.display_scale(target_scale);
            let radius = flake.radius * scale;
            let contact = {
                let mut candidates = ContactCandidates::default();
                if let Some(slab) = &flake.slab {
                    let after_pose = flake.current_pose(scale);
                    if OPTIMIZE_CONTACTS
                        && slab
                            .contact_bounds(before_pose, after_pose)
                            .is_some_and(|bounds| !self.slab_candidates(bounds))
                    {
                        #[cfg(test)]
                        {
                            contact_work.skipped += 1;
                        }
                        None
                    } else {
                        let motion = AngularMotion::new(before_pose, after_pose);
                        let mut contact: Option<(u64, usize, f32)> = None;
                        'contours: for outline in slab.contours() {
                            #[cfg(test)]
                            {
                                contact_work.contours += 1;
                            }
                            let last = *outline.last().unwrap();
                            let mut previous = motion.vertex(last);
                            for local in outline {
                                let current = motion.vertex(local);
                                let edge = motion.between(previous, current);
                                #[cfg(test)]
                                {
                                    contact_work.edges += 1;
                                }
                                if let Some(hit) = self
                                    .trace_edge_contact_inner::<OPTIMIZE_CONTACTS>(
                                        edge,
                                        flake.skip_support,
                                        settings,
                                        &mut candidates,
                                    )
                                    && contact.is_none_or(|old| hit.2 < old.2)
                                {
                                    contact = Some(hit);
                                }
                                if OPTIMIZE_CONTACTS && contact.is_some_and(|hit| hit.2 == 0.0) {
                                    // Times are non-negative and ties never replace
                                    // the first winner, so no later edge can improve it.
                                    break 'contours;
                                }
                                previous = current;
                            }
                        }
                        contact
                    }
                } else {
                    self.trace_contact(
                        Point::new(before.x, before.y + flake.radius * before_scale),
                        Point::new(flake.position.x, flake.position.y + radius),
                        flake.skip_support,
                        settings,
                        &mut candidates,
                    )
                }
            };
            if let Some((key, index, _)) = contact {
                #[cfg(test)]
                {
                    contact_work.hits += 1;
                }
                if settings.snow.drift_size_percent > 0
                    && !flake
                        .skip_support
                        .and_then(|key| self.falling_masks.get(&key))
                        .is_some_and(|mask| mask.hidden)
                {
                    if flake.slab.is_some() {
                        self.deposit_mound(key, index, flake.position.x, flake.mass, settings);
                    } else {
                        self.deposit(key, index, flake.mass, flake.radius * scale, settings);
                    }
                } else {
                    self.removed_mass += f64::from(flake.mass);
                }
            } else if flake.position.y > bounds.bottom + 32.0
                || flake.position.x < bounds.left - 80.0
                || flake.position.x > bounds.right + 80.0
                || flake.age > 240.0
            {
                self.removed_mass += f64::from(flake.mass);
            } else {
                alive.push(flake);
            }
        }
        self.flakes = alive;
        #[cfg(test)]
        {
            self.contact_work = contact_work;
        }
        self.rebuild_usage();
        let mut swept = Vec::new();
        for drift in self.drifts.values_mut() {
            let accumulating = (if drift.support.ground {
                settings.snow.on_ground
            } else {
                settings.snow.on_windows
            }) && settings.snow.drift_size_percent > 0;
            for index in 0..drift.mass.len() {
                if self.scene.paused_at(
                    Point::new(drift.x(index), drift.support.y - 1.0),
                    &self.paused_monitors,
                ) {
                    continue;
                }
                if !accumulating {
                    drift.shrink_remaining[index] = 0.0;
                    drift.last_size[index] = settings.snow.drift_size_percent;
                    let total = drift.mass[index] + drift.pending[index];
                    if drift.disable_rate[index] == 0.0 {
                        drift.disable_rate[index] = total / 2.0;
                    }
                    self.removed_mass += drift.remove_amount(index, drift.disable_rate[index] * DT);
                    continue;
                }
                drift.disable_rate[index] = 0.0;
                let scale = self
                    .scene
                    .scale_at(Point::new(drift.x(index), drift.support.y - 1.0));
                let capacity = drift.capacity(index, &self.scene, settings);
                // A user-reduced ceiling removes existing excess over active
                // simulation time; scene refreshes preserve the remaining time.
                let excess = (drift.mass[index] + drift.pending[index] - capacity).max(0.0);
                let size = settings.snow.drift_size_percent;
                if size < drift.last_size[index] {
                    drift.shrink_remaining[index] = 2.0;
                } else if size > drift.last_size[index] {
                    drift.shrink_remaining[index] = 0.0;
                }
                drift.last_size[index] = size;
                if drift.shrink_remaining[index] > 0.0 {
                    let amount = excess * (DT / drift.shrink_remaining[index]).min(1.0);
                    self.removed_mass += drift.remove_amount(index, amount);
                    drift.shrink_remaining[index] = (drift.shrink_remaining[index] - DT).max(0.0);
                } else {
                    let loose = drift.pending[index].min(excess);
                    drift.pending[index] -= loose;
                    self.removed_mass += f64::from(loose);
                    let removed = (drift.mass[index] - capacity)
                        .max(0.0)
                        .min(48.0 * DT * drift.cell_width / scale);
                    drift.mass[index] -= removed;
                    self.removed_mass += f64::from(removed);
                }
                // Only snow that made contact enters this short settling phase.
                let settling = drift.pending[index] * (DT / 0.18).min(1.0);
                drift.pending[index] -= settling;
                drift.mass[index] += settling;
                if settings.snow.melting {
                    let melt = drift.mass[index].min(0.002 * DT * drift.cell_width);
                    drift.mass[index] -= melt;
                    self.removed_mass += f64::from(melt);
                }
                if settings.interaction.cursor_enabled
                    && let Some(cursor) = cursor
                {
                    let position = Point::new(
                        drift.x(index),
                        drift.support.y - drift.height(index, &self.scene),
                    );
                    let distance = (position.x - cursor.position.x)
                        .hypot(position.y - cursor.position.y)
                        / scale;
                    if distance < 28.0
                        && cursor.velocity.x.hypot(cursor.velocity.y) > 15.0
                        && drift.support.exposed(position)
                    {
                        let fraction = (1.0 - distance / 28.0) * 0.12;
                        let mass = (drift.mass[index] + drift.pending[index]) * fraction;
                        drift.mass[index] *= 1.0 - fraction;
                        drift.pending[index] *= 1.0 - fraction;
                        swept.push((
                            Point::new(position.x, position.y - 3.0),
                            mass,
                            drift.scales[index],
                        ));
                    }
                }
            }
            // Apply all pairwise fluxes together, avoiding a travelling-wave bias.
            if accumulating {
                drift.relax(&self.scene, &self.paused_monitors, settings);
            }
        }
        for (position, mass, scale) in swept {
            let velocity = Point::new(self.rng.signed() * 18.0, -8.0);
            self.queue_spill(position, mass, velocity, None, scale);
        }
        self.shed_edges(settings);
        self.retire_falling_masks();
    }
    fn shed_edges(&mut self, settings: &Settings) {
        let mut slots = 128_usize
            .saturating_sub(self.spill.len())
            .min(MAX_EDGE_CHUNKS_PER_STEP);
        let mut queued = BTreeMap::<Option<u64>, ParticleUsage>::new();
        for body in &self.spill {
            let key = self.scene.nearest_screen(body.position).map(|s| s.key);
            queued.entry(key).or_default().add(self.priority_body(body));
        }
        let active_budget = self.active_budget();
        let ordinary_budget = self.ordinary_budget();
        let reserved = self.reserved_slots();
        let mut total = self.flakes.len() + self.spill.len();
        let mut ordinary = self
            .usage
            .values()
            .chain(queued.values())
            .map(|u| u.ordinary)
            .sum::<usize>();
        let keys: Vec<_> = self.drifts.keys().copied().collect();
        let start = keys.partition_point(|key| *key <= self.edge_cursor);
        for offset in 0..keys.len() {
            let key = keys[(start + offset) % keys.len()];
            let drift = self.drifts.get_mut(&key).unwrap();
            if drift.support.ground {
                continue;
            }
            if !settings.snow.on_windows || settings.snow.drift_size_percent == 0 {
                drift.edge_wait = [EDGE_INTERVAL; 2];
                continue;
            }
            for right in [false, true] {
                let index = if right { drift.mass.len() - 1 } else { 0 };
                let screen_key = self
                    .scene
                    .nearest_screen(Point::new(drift.x(index), drift.support.y - 1.0))
                    .map(|s| s.key);
                let used = self.usage.get(&screen_key).copied().unwrap_or_default();
                let waiting = queued.get(&screen_key).copied().unwrap_or_default();
                let allowed = slots > 0
                    && active_budget.saturating_sub(total) > reserved
                    && ordinary < ordinary_budget
                    && used.total + waiting.total < MAX_FLAKES
                    && used.ordinary + waiting.ordinary < ORDINARY_FLAKES_PER_SCREEN;
                if let Some(body) =
                    drift.edge_chunk(right, &self.scene, &self.paused_monitors, allowed)
                {
                    let screen_key = self.scene.nearest_screen(body.position).map(|s| s.key);
                    queued.entry(screen_key).or_default().add(false);
                    self.spill.push_back(body);
                    slots -= 1;
                    total += 1;
                    ordinary += 1;
                    self.edge_cursor = key;
                }
            }
        }
    }
    fn trace_contact<'a>(
        &'a self,
        a: Point,
        b: Point,
        skip: Option<u64>,
        settings: &Settings,
        candidates: &mut ContactCandidates<'a>,
    ) -> Option<(u64, usize, f32)> {
        if b.y <= a.y {
            return None;
        }
        let from = (a.y / 128.0).floor() as i32;
        let to = (b.y / 128.0).floor() as i32;
        let min_x = a.x.min(b.x);
        let max_x = a.x.max(b.x);
        let column = |x: f32| ((x - self.bucket_left) / self.bucket_width).floor() as i32;
        // Only registered columns can contain supports, even for a long sweep
        // that starts or ends far outside the virtual desktop.
        let first = column(min_x).max(0);
        let last = column(max_x).min(MAX_BUCKET_COLUMNS);
        candidates.select(self, from, to, first, last);
        let mut contact: Option<(u64, usize, f32)> = None;
        let behind = skip.and_then(|key| self.falling_masks.get(&key));
        for key in candidates.iter() {
            if skip == Some(key)
                || behind
                    .is_some_and(|mask| mask.front.contains(&key) || mask.origins.contains(&key))
            {
                continue;
            }
            let drift = &self.drifts[&key];
            if (drift.support.ground && !settings.snow.on_ground)
                || (!drift.support.ground && !settings.snow.on_windows)
            {
                continue;
            }
            if a.y > drift.support.y || max_x < drift.support.left || min_x >= drift.support.right {
                continue;
            }
            // Each cell has its own top plane. Test the actual crossing of
            // that plane, not the cell or visibility of the final position.
            for index in drift.index(min_x)..=drift.index(max_x) {
                let y = drift.support.y - drift.contact_height(index, &self.scene);
                if b.y < y {
                    continue;
                }
                // Another impact may have raised this surface into the body
                // since its last step. Resolve that solid overlap immediately;
                // particles already below a window's base still pass freely.
                let t = ((y - a.y) / (b.y - a.y)).max(0.0);
                if contact.is_some_and(|(_, _, old)| t >= old) {
                    continue;
                }
                let x = a.x + (b.x - a.x) * t;
                if x < drift.support.left
                    || x >= drift.support.right
                    || drift.index(x) != index
                    || !drift.support.visible_at(x)
                    || !drift.support.exposed(Point::new(x, y))
                    || self
                        .scene
                        .paused_at(Point::new(x, drift.support.y - 1.0), &self.paused_monitors)
                {
                    continue;
                }
                contact = Some((key, index, t));
            }
        }
        contact
    }
    #[cfg(test)]
    fn trace_edge_contact<'a>(
        &'a self,
        edge: impl EdgeSweep,
        skip: Option<u64>,
        settings: &Settings,
        candidates: &mut ContactCandidates<'a>,
    ) -> Option<(u64, usize, f32)> {
        self.trace_edge_contact_inner::<true>(edge, skip, settings, candidates)
    }
    fn trace_edge_contact_inner<'a, const EARLY_ZERO: bool>(
        &'a self,
        edge: impl EdgeSweep,
        skip: Option<u64>,
        settings: &Settings,
        candidates: &mut ContactCandidates<'a>,
    ) -> Option<(u64, usize, f32)> {
        let bounds = edge.bounds();
        let column = |x: f32| ((x - self.bucket_left) / self.bucket_width).floor() as i32;
        candidates.select(
            self,
            (bounds.top / 128.0).floor() as i32,
            (bounds.bottom / 128.0).floor() as i32,
            column(bounds.left).max(0),
            column(bounds.right).min(MAX_BUCKET_COLUMNS),
        );
        let behind = skip.and_then(|key| self.falling_masks.get(&key));
        let mut best: Option<(u64, usize, f32)> = None;
        for key in candidates.iter() {
            if skip == Some(key)
                || behind
                    .is_some_and(|mask| mask.front.contains(&key) || mask.origins.contains(&key))
            {
                continue;
            }
            let drift = &self.drifts[&key];
            let support = &drift.support;
            if (support.ground && !settings.snow.on_ground)
                || (!support.ground && !settings.snow.on_windows)
                || bounds.top > support.y
                || bounds.right < support.left
                || bounds.left >= support.right
            {
                continue;
            }
            for index in drift.index(bounds.left)..=drift.index(bounds.right) {
                let y = support.y - drift.contact_height(index, &self.scene);
                if bounds.bottom < y {
                    continue;
                }
                let Some(cell) = drift.contact_span(index) else {
                    continue;
                };
                for visible in &support.visible {
                    for clip in &support.draw_clips {
                        if y < clip.top || y > clip.bottom {
                            continue;
                        }
                        let span = [
                            cell[0].max(visible.left).max(clip.left).max(bounds.left),
                            cell[1]
                                .min(visible.right.min(clip.right).next_down())
                                .min(bounds.right),
                        ];
                        if span[0] > span[1] {
                            continue;
                        }
                        let mut consider = |span| {
                            if let Some(hit) = edge.contact(y, support.y, span)
                                && drift.index(hit.x) == index
                                && support.visible_at(hit.x)
                                && support.exposed(Point::new(hit.x, y))
                                && !self.scene.paused_at(
                                    Point::new(hit.x, support.y - 1.0),
                                    &self.paused_monitors,
                                )
                                && best.is_none_or(|(_, _, old)| hit.time < old)
                            {
                                best = Some((key, index, hit.time));
                                return EARLY_ZERO && hit.time == 0.0;
                            }
                            false
                        };
                        if self.paused_monitors.is_empty() {
                            if consider(span) {
                                return best;
                            }
                        } else {
                            for active in contact::active_spans(
                                &self.scene,
                                &self.paused_monitors,
                                support.y - 1.0,
                                span,
                            ) {
                                if consider(active) {
                                    return best;
                                }
                            }
                        }
                    }
                }
            }
        }
        best
    }
    fn deposit_mound(
        &mut self,
        key: u64,
        impact_index: usize,
        x: f32,
        mass: f32,
        settings: &Settings,
    ) {
        let drift = self.drifts.get_mut(&key).unwrap();
        let center = if x >= drift.support.left
            && x < drift.support.right
            && drift.support.visible_at(x)
            && drift.support.exposed(Point::new(
                x,
                drift.support.y - drift.contact_height(drift.index(x), &self.scene),
            ))
            && !self
                .scene
                .paused_at(Point::new(x, drift.support.y - 1.0), &self.paused_monitors)
        {
            x
        } else {
            drift.x(impact_index)
        };
        let scale = self
            .scene
            .scale_at(Point::new(center, drift.support.y - 1.0));
        // A granular mound has area = half_width * height, with its slope
        // close to the same angle of repose used by the conservative relaxation.
        let half_width = (mass / 0.55).sqrt().max(drift.cell_width / scale * 1.5) * scale;
        let first = drift.index(center - half_width);
        let last = (drift.index(center + half_width) + 1).min(drift.mass.len());
        let weights: Vec<f32> = (first..last)
            .map(|i| {
                if drift.support.visible_at(drift.x(i))
                    && drift.support.exposed(Point::new(
                        drift.x(i),
                        drift.support.y - drift.contact_height(i, &self.scene),
                    ))
                    && !self.scene.paused_at(
                        Point::new(drift.x(i), drift.support.y - 1.0),
                        &self.paused_monitors,
                    )
                {
                    (1.0 - (drift.x(i) - center).abs() / half_width).max(0.0)
                } else {
                    0.0
                }
            })
            .collect();
        let total = weights.iter().sum::<f32>();
        let mut accepted = 0.0_f64;
        if total > 0.0 {
            for (index, weight) in (first..last).zip(weights) {
                let available = drift.deposit_capacity(index, &self.scene, settings);
                let added = (mass * weight / total).min(available);
                // The cohesive bulk remains visible at impact. Only its loose
                // surface settles over time; no secondary spray is generated.
                drift.mass[index] += added * 0.85;
                drift.pending[index] += added * 0.15;
                accepted += f64::from(added);
            }
        }
        self.removed_mass += (f64::from(mass) - accepted).max(0.0);
    }
    fn deposit(&mut self, key: u64, index: usize, mass: f32, radius: f32, settings: &Settings) {
        let drift = self.drifts.get_mut(&key).unwrap();
        let reach = (radius * 2.5 / drift.cell_width).ceil().clamp(1.0, 8.0) as usize;
        let first = index.saturating_sub(reach);
        let last = (index + reach + 1).min(drift.mass.len());
        let weight = |i: usize| {
            if drift.support.visible_at(drift.x(i))
                && drift.support.exposed(Point::new(
                    drift.x(i),
                    drift.support.y - drift.contact_height(i, &self.scene),
                ))
                && !self.scene.paused_at(
                    Point::new(drift.x(i), drift.support.y - 1.0),
                    &self.paused_monitors,
                )
            {
                (reach + 1 - index.abs_diff(i)) as f32
            } else {
                0.0
            }
        };
        let weights: Vec<f32> = (first..last).map(weight).collect();
        let total = weights.iter().sum::<f32>();
        let mut accepted = 0.0_f64;
        if total > 0.0 {
            for (i, weight) in (first..last).zip(weights) {
                let part = mass * weight / total;
                let available = drift.deposit_capacity(i, &self.scene, settings);
                let added = part.min(available);
                drift.pending[i] += added;
                accepted += f64::from(added);
            }
        }
        // Outflow leaves a full surface instead of being thrown above it again.
        self.removed_mass += (f64::from(mass) - accepted).max(0.0);
    }
}

#[cfg(test)]
mod unit_tests {
    use super::*;
    use crate::geometry::Window;

    fn contact_fixture(floor: f32, roofs: &[(u64, f32, f32)]) -> World {
        World::new(
            Scene {
                screens: vec![Screen {
                    key: 1,
                    persistent_id: true,
                    rect: Rect {
                        left: 0.0,
                        top: 0.0,
                        right: 800.0,
                        bottom: 1000.0,
                    },
                    floor,
                    scale: 1.0,
                }],
                windows: roofs
                    .iter()
                    .map(|&(key, left, top)| Window {
                        key,
                        rect: Rect {
                            left,
                            top,
                            right: left + 100.0,
                            bottom: top + 100.0,
                        },
                        visible: true,
                        collect: true,
                    })
                    .collect(),
            },
            41,
        )
    }

    #[test]
    fn swept_contact_hits_a_ledge_between_endpoint_columns() {
        let world = contact_fixture(1000.0, &[(7, 20.0, 300.0)]);
        let settings = Settings::default();
        let mut cache = ContactCandidates::default();
        for (start, end) in [(90.0, 130.0), (130.0, 90.0)] {
            let hit = world
                .trace_contact(
                    Point::new(start, 290.0),
                    Point::new(end, 310.0),
                    None,
                    &settings,
                    &mut cache,
                )
                .expect("the segment crosses the visible ledge at x=110, y=300");
            assert_eq!(hit.0, 7);
            assert!((hit.2 - 0.5).abs() < 0.000_001);
        }
    }

    #[test]
    fn swept_contact_checks_the_visible_gap_at_impact() {
        let mut scene = contact_fixture(1000.0, &[(7, 20.0, 300.0)]).scene;
        for (key, left, right) in [(8, 0.0, 110.75), (9, 111.75, 200.0)] {
            scene.windows.insert(
                0,
                Window {
                    key,
                    rect: Rect {
                        left,
                        right,
                        top: 280.0,
                        bottom: 330.0,
                    },
                    visible: true,
                    collect: false,
                },
            );
        }
        let world = World::new(scene, 41);
        let settings = Settings::default();
        let mut cache = ContactCandidates::default();
        for (start, end) in [(110.0, 113.0), (113.0, 110.0)] {
            let hit = world
                .trace_contact(
                    Point::new(start, 299.5),
                    Point::new(end, 300.5),
                    None,
                    &settings,
                    &mut cache,
                )
                .expect("x=111.5 is exposed at the crossing, both endpoints are covered");
            assert_eq!(hit.0, 7);
            assert!((hit.2 - 0.5).abs() < 0.000_001);
        }
        assert!(
            world
                .trace_contact(
                    Point::new(108.5, 299.5),
                    Point::new(111.5, 300.5),
                    None,
                    &settings,
                    &mut cache,
                )
                .is_none(),
            "an exposed endpoint cannot make the covered crossing at x=110 collect"
        );
    }

    #[test]
    fn swept_contact_uses_the_height_of_the_crossed_cell() {
        let mut world = contact_fixture(1000.0, &[(7, 20.0, 300.0)]);
        let roof = world.drifts.get_mut(&7).unwrap();
        let boundary = roof.support.left + 11.0 * roof.cell_width;
        let half_cell = roof.cell_width * 0.5;
        roof.mass[10] = 6.0 * roof.cell_width;
        let settings = Settings::default();
        let mut cache = ContactCandidates::default();
        let hit = world
            .trace_contact(
                Point::new(boundary - half_cell, 291.0),
                Point::new(boundary + half_cell, 298.0),
                None,
                &settings,
                &mut cache,
            )
            .expect("the segment hits the snowy cell before entering its bare neighbour");
        assert_eq!((hit.0, hit.1), (7, 10));
        assert!((hit.2 - 3.0 / 7.0).abs() < 0.000_001);

        // A high final cell must not report a crossing that happened above
        // its bare neighbour before the segment entered the high cell.
        let roof = world.drifts.get_mut(&7).unwrap();
        roof.mass[10] = 0.0;
        roof.mass[11] = 6.0 * roof.cell_width;
        assert!(
            world
                .trace_contact(
                    Point::new(boundary - half_cell, 291.0),
                    Point::new(boundary + half_cell, 299.0),
                    None,
                    &settings,
                    &mut ContactCandidates::default(),
                )
                .is_none()
        );
    }

    #[test]
    fn swept_contact_preserves_half_open_edges_and_top_only_motion_across_dpi() {
        let settings = Settings::default();
        for scale in [1.0, 2.5] {
            for origin in [Point::default(), Point::new(-900.0, -700.0)] {
                let point = |x, y| Point::new(origin.x + x * scale, origin.y + y * scale);
                let mut scene = contact_fixture(1000.0, &[(7, 20.0, 300.0)]).scene;
                let map_rect = |r: &mut Rect| {
                    r.left = origin.x + r.left * scale;
                    r.right = origin.x + r.right * scale;
                    r.top = origin.y + r.top * scale;
                    r.bottom = origin.y + r.bottom * scale;
                };
                for screen in &mut scene.screens {
                    map_rect(&mut screen.rect);
                    screen.floor = origin.y + screen.floor * scale;
                    screen.scale = scale;
                }
                for window in &mut scene.windows {
                    map_rect(&mut window.rect);
                }
                let mut world = World::new(scene, 41);
                let mut cache = ContactCandidates::default();
                for (x, expected) in [(26.0, Some(7)), (114.0, None)] {
                    let hit = world.trace_contact(
                        point(x - 2.0, 299.0),
                        point(x + 2.0, 301.0),
                        None,
                        &settings,
                        &mut cache,
                    );
                    assert_eq!(hit.map(|h| h.0), expected, "scale={scale}, edge={x}");
                }
                for (a, b) in [
                    (point(10.0, 290.0), point(30.0, 310.0)),
                    (point(130.0, 290.0), point(110.0, 310.0)),
                    (point(60.0, 301.0), point(61.0, 302.0)),
                    (point(60.0, 301.0), point(61.0, 299.0)),
                    (point(60.0, 300.0), point(61.0, 300.0)),
                ] {
                    assert!(
                        world
                            .trace_contact(a, b, None, &settings, &mut cache)
                            .is_none()
                    );
                }
                let roof = world.drifts.get_mut(&7).unwrap();
                roof.mass.fill(4.0 * roof.cell_width / scale);
                let hit = world
                    .trace_contact(
                        point(60.0, 298.0),
                        point(61.0, 299.0),
                        None,
                        &settings,
                        &mut ContactCandidates::default(),
                    )
                    .expect("a newly raised profile still resolves existing overlap at t=0");
                assert_eq!((hit.0, hit.2), (7, 0.0));
            }
        }
    }

    #[test]
    fn swept_contact_rechecks_both_candidate_columns_when_only_the_start_changes() {
        let world = contact_fixture(1000.0, &[(11, 20.0, 300.0), (7, 220.0, 300.0)]);
        let settings = Settings::default();
        let mut cache = ContactCandidates::default();
        for (start, expected) in [(260.0, 7), (-180.0, 11), (260.0, 7)] {
            let hit = world
                .trace_contact(
                    Point::new(start, 290.0),
                    Point::new(300.0, 310.0),
                    None,
                    &settings,
                    &mut cache,
                )
                .unwrap();
            assert_eq!(hit.0, expected);
        }
    }

    #[test]
    fn swept_contact_checks_pause_and_source_filters_at_the_crossing() {
        let mut scene = contact_fixture(1000.0, &[(7, 20.0, 300.0)]).scene;
        let mut right = scene.screens[0].clone();
        right.key = 2;
        right.rect.left = 100.0;
        scene.screens[0].rect.right = 100.0;
        scene.screens.push(right);
        let mut world = World::new(scene, 41);
        world.set_paused_monitors([2].into());
        world.falling_masks.insert(
            900,
            FallingMask {
                front: [7].into(),
                covers: Vec::new(),
                hidden: false,
                coalesced: false,
                origins: BTreeSet::new(),
            },
        );
        let settings = Settings::default();
        let a = Point::new(85.0, 299.0);
        let b = Point::new(105.0, 301.0);
        let mut cache = ContactCandidates::default();
        let hit = world
            .trace_contact(a, b, None, &settings, &mut cache)
            .unwrap();
        assert_eq!((hit.0, hit.2), (7, 0.5));
        assert!(
            world
                .trace_contact(a, b, Some(7), &settings, &mut cache)
                .is_none()
        );
        assert!(
            world
                .trace_contact(a, b, Some(900), &settings, &mut cache)
                .is_none()
        );
        assert!(
            world
                .trace_contact(
                    Point::new(105.0, 299.5),
                    Point::new(85.0, 301.5),
                    None,
                    &settings,
                    &mut cache,
                )
                .is_none(),
            "the crossing is paused even though the endpoint is active"
        );
    }

    #[test]
    fn swept_contact_checks_the_exposed_height_not_the_endpoint_mask() {
        let mut scene = contact_fixture(1000.0, &[(7, 20.0, 300.0)]).scene;
        scene.windows.insert(
            0,
            Window {
                key: 8,
                rect: Rect {
                    left: 110.0,
                    right: 112.0,
                    top: 280.0,
                    bottom: 299.0,
                },
                visible: true,
                collect: false,
            },
        );
        let mut world = World::new(scene, 41);
        let roof = world.drifts.get_mut(&7).unwrap();
        roof.mass.fill(6.0 * roof.cell_width);
        assert!(roof.support.visible_at(111.0));
        assert!(!roof.support.exposed(Point::new(111.0, 294.0)));
        let settings = Settings::default();
        assert!(
            world
                .trace_contact(
                    Point::new(109.0, 293.0),
                    Point::new(113.0, 295.0),
                    None,
                    &settings,
                    &mut ContactCandidates::default(),
                )
                .is_none()
        );
    }

    #[test]
    fn swept_contact_deposits_a_fast_flake_crossing_a_narrow_gap() {
        let mut scene = contact_fixture(1000.0, &[(7, 20.0, 300.0)]).scene;
        for (key, left, right) in [(8, 0.0, 110.75), (9, 111.75, 200.0)] {
            scene.windows.insert(
                0,
                Window {
                    key,
                    rect: Rect {
                        left,
                        right,
                        top: 280.0,
                        bottom: 330.0,
                    },
                    visible: true,
                    collect: false,
                },
            );
        }
        let mut world = World::new(scene, 41);
        let mut body = Flake::new(Point::new(110.25, 297.5), 2.0);
        body.velocity = Point::new(220.0, 160.0);
        assert!(world.release(body));
        let mut settings = Settings::default();
        settings.snow.intensity_percent = 0;
        settings.snow.melting = false;
        world.step(&settings, None);
        assert!(
            world.flakes.is_empty(),
            "the falling body must land in this step"
        );
        assert!(world.drifts[&7].total_mass() > 0.0);
        assert!((world.introduced_mass - world.removed_mass - world.total_mass()).abs() < 0.000_01);
    }

    fn edge_gap_scene() -> Scene {
        let mut scene = contact_fixture(1000.0, &[(7, 20.0, 300.0)]).scene;
        for (key, left, right) in [(8, 0.0, 110.75), (9, 111.75, 200.0)] {
            scene.windows.insert(
                0,
                Window {
                    key,
                    rect: Rect {
                        left,
                        right,
                        top: 280.0,
                        bottom: 330.0,
                    },
                    visible: true,
                    collect: false,
                },
            );
        }
        scene
    }

    #[test]
    fn edge_contact_lands_when_only_the_interior_overlaps_a_visible_ledge() {
        let mut world = World::new(edge_gap_scene(), 41);
        let mut body = Flake::new(Point::new(100.0, 296.0), 5.0);
        body.velocity = Point::new(0.0, 160.0);
        body.mass = 320.0;
        body.slab = Some(SnowSlab {
            heights: vec![4.0],
            cell_width: 80.0,
            center: Point::new(40.0, -2.0),
        });
        assert!(world.release(body));
        let mut settings = Settings::default();
        settings.snow.intensity_percent = 0;
        settings.snow.melting = false;
        world.step(&settings, None);
        assert!(
            world.flakes.is_empty(),
            "the bottom edge crosses the visible gap even though its vertices are covered"
        );
        assert!(world.drifts[&7].total_mass() > 0.0);
        assert!((world.introduced_mass - world.removed_mass - world.total_mass()).abs() < 0.000_1);
    }

    #[test]
    fn edge_contact_does_not_bridge_disconnected_parts_of_a_profile() {
        let mut world = World::new(edge_gap_scene(), 41);
        let mut body = Flake::new(Point::new(100.0, 296.0), 5.0);
        body.velocity = Point::new(0.0, 160.0);
        body.mass = 320.0;
        body.slab = Some(SnowSlab {
            heights: vec![4.0, 0.0, 4.0],
            cell_width: 40.0,
            center: Point::new(60.0, -2.0),
        });
        assert!(world.release(body));
        let mut settings = Settings::default();
        settings.snow.intensity_percent = 0;
        settings.snow.melting = false;
        world.step(&settings, None);
        assert_eq!(
            world.flakes.len(),
            1,
            "the only exposed ledge is inside the empty profile run"
        );
        assert_eq!(world.drifts[&7].total_mass(), 0.0);
        assert!((world.introduced_mass - world.removed_mass - world.total_mass()).abs() < 0.000_1);
    }

    #[test]
    fn edge_contact_chooses_the_first_cell_and_keeps_source_and_switch_filters() {
        let mut world = contact_fixture(300.0, &[(7, 20.0, 300.0)]);
        let roof = world.drifts.get_mut(&7).unwrap();
        roof.mass[5] = 8.0 * roof.cell_width;
        world.falling_masks.insert(
            900,
            FallingMask {
                front: [7].into(),
                covers: Vec::new(),
                hidden: false,
                coalesced: false,
                origins: BTreeSet::new(),
            },
        );
        let edge = SweptEdge {
            before: [Point::new(40.0, 290.0), Point::new(100.0, 290.0)],
            after: [Point::new(40.0, 310.0), Point::new(100.0, 310.0)],
        };
        let mut settings = Settings::default();
        let mut cache = ContactCandidates::default();
        let hit = world
            .trace_edge_contact(edge, None, &settings, &mut cache)
            .unwrap();
        assert_eq!((hit.0, hit.1), (7, 5));
        assert!((hit.2 - 0.1).abs() < 0.000_001);
        for skip in [Some(7), Some(900)] {
            let hit = world
                .trace_edge_contact(edge, skip, &settings, &mut cache)
                .unwrap();
            assert_eq!((hit.0, hit.2), (1_u64 | (1 << 63), 0.5));
        }
        settings.snow.on_windows = false;
        assert_eq!(
            world
                .trace_edge_contact(edge, None, &settings, &mut cache)
                .unwrap()
                .0,
            1_u64 | (1 << 63)
        );
        settings.snow.on_ground = false;
        assert!(
            world
                .trace_edge_contact(edge, None, &settings, &mut cache)
                .is_none()
        );
    }

    #[test]
    fn edge_contact_keeps_cells_at_rounded_grid_boundaries_and_half_open_edges() {
        let settings = Settings::default();
        for index in 0..15 {
            let mut world = contact_fixture(1000.0, &[(7, 20.0, 300.0)]);
            let roof = world.drifts.get_mut(&7).unwrap();
            roof.mass[index] = 8.0 * roof.cell_width;
            let edge = SweptEdge {
                before: [Point::new(20.0, 290.0), Point::new(120.0, 290.0)],
                after: [Point::new(20.0, 310.0), Point::new(120.0, 310.0)],
            };
            let hit = world
                .trace_edge_contact(edge, None, &settings, &mut ContactCandidates::default())
                .unwrap();
            assert_eq!((hit.0, hit.1), (7, index), "lost the snowy cell {index}");
            assert!((hit.2 - 0.1).abs() < 0.000_001);
        }
        let world = contact_fixture(1000.0, &[(7, 20.0, 300.0)]);
        for (x, expected) in [
            (26.0, Some(7)),
            (114.0_f32.next_down(), Some(7)),
            (114.0, None),
        ] {
            let edge = SweptEdge {
                before: [Point::new(x, 298.0), Point::new(x, 299.0)],
                after: [Point::new(x, 300.0), Point::new(x, 301.0)],
            };
            assert_eq!(
                world
                    .trace_edge_contact(edge, None, &settings, &mut ContactCandidates::default())
                    .map(|h| h.0),
                expected
            );
        }
        // Check inverse boundaries at fractional negative coordinates and at
        // the cancellation plateau around zero on a very wide desktop.
        let mut world = contact_fixture(1000.0, &[(7, 20.0, 300.0)]);
        let roof = world.drifts.get_mut(&7).unwrap();
        for (left, width, cells) in [(65.0, 220.0, 15), (-835.0, 220.0, 15), (-1.0e9, 2.0e9, 16)] {
            roof.support.left = left;
            roof.support.right = left + width;
            roof.cell_width = width / cells as f32;
            roof.mass.resize(cells, 0.0);
            for index in 0..cells {
                let span = roof.contact_span(index).unwrap();
                assert_eq!(roof.index(span[0]), index);
                assert_eq!(roof.index(span[1]), index);
                if index > 0 {
                    assert_eq!(roof.index(span[0].next_down()), index - 1);
                }
                if index + 1 < cells {
                    assert_eq!(roof.index(span[1].next_up()), index + 1);
                }
            }
        }
    }

    #[test]
    fn edge_contact_finds_a_narrow_active_monitor_between_paused_ones() {
        let mut scene = contact_fixture(1000.0, &[(7, 20.0, 300.0)]).scene;
        let mut left = scene.screens[0].clone();
        left.rect.right = 99.999;
        let mut middle = left.clone();
        middle.key = 2;
        middle.rect.left = 99.999;
        middle.rect.right = 100.001;
        let mut right = scene.screens[0].clone();
        right.key = 3;
        right.rect.left = 100.001;
        scene.screens = vec![left, middle, right];
        let mut world = World::new(scene, 41);
        world.set_paused_monitors([1, 3].into());
        let edge = SweptEdge {
            before: [Point::new(50.0, 299.0), Point::new(150.0, 299.0)],
            after: [Point::new(50.0, 301.0), Point::new(150.0, 301.0)],
        };
        let settings = Settings::default();
        let hit = world
            .trace_edge_contact(edge, None, &settings, &mut ContactCandidates::default())
            .unwrap();
        assert_eq!((hit.0, hit.2), (7, 0.5));
        world.set_paused_monitors([1, 2, 3].into());
        assert!(
            world
                .trace_edge_contact(edge, None, &settings, &mut ContactCandidates::default())
                .is_none()
        );
    }

    #[test]
    fn contact_candidates_follow_columns_and_both_swept_rows() {
        let world = contact_fixture(
            1000.0,
            &[
                (11, 20.0, 300.0),
                (7, 220.0, 300.0),
                (3, 20.0, 700.0),
                (27, 460.0, 700.0),
            ],
        );
        let settings = Settings::default();
        let mut cache = ContactCandidates::default();
        assert_eq!(
            world
                .trace_contact(
                    Point::new(60.0, 292.0),
                    Point::new(60.0, 308.0),
                    None,
                    &settings,
                    &mut cache
                )
                .map(|h| h.0),
            Some(11)
        );
        assert_eq!(
            world
                .trace_contact(
                    Point::new(260.0, 292.0),
                    Point::new(260.0, 308.0),
                    None,
                    &settings,
                    &mut cache
                )
                .map(|h| h.0),
            Some(7)
        );
        assert_eq!(
            world
                .trace_contact(
                    Point::new(60.0, 692.0),
                    Point::new(60.0, 708.0),
                    None,
                    &settings,
                    &mut cache
                )
                .map(|h| h.0),
            Some(3)
        );
        // Change only the first row: the high roof must be considered again.
        assert_eq!(
            world
                .trace_contact(
                    Point::new(60.0, 100.0),
                    Point::new(60.0, 708.0),
                    None,
                    &settings,
                    &mut cache
                )
                .map(|h| h.0),
            Some(11)
        );
        assert!(
            world
                .trace_contact(
                    Point::new(500.0, 100.0),
                    Point::new(500.0, 350.0),
                    None,
                    &settings,
                    &mut cache
                )
                .is_none()
        );
        // Change only the last row: a previously absent low roof becomes reachable.
        assert_eq!(
            world
                .trace_contact(
                    Point::new(500.0, 100.0),
                    Point::new(500.0, 708.0),
                    None,
                    &settings,
                    &mut cache
                )
                .map(|h| h.0),
            Some(27)
        );
    }

    #[test]
    fn contact_candidates_keep_equal_time_order_and_recheck_filters() {
        let mut world = contact_fixture(300.0, &[(7, 20.0, 300.0)]);
        world.falling_masks.insert(
            900,
            FallingMask {
                front: [7].into(),
                covers: Vec::new(),
                hidden: false,
                coalesced: false,
                origins: BTreeSet::new(),
            },
        );
        let mut settings = Settings::default();
        let mut cache = ContactCandidates::default();
        let ground = 1_u64 | (1 << 63);
        let a = Point::new(60.0, 292.0);
        let b = Point::new(60.0, 308.0);
        assert_eq!(
            world
                .trace_contact(a, b, None, &settings, &mut cache)
                .map(|h| h.0),
            Some(7)
        );
        assert_eq!(
            world
                .trace_contact(a, b, Some(7), &settings, &mut cache)
                .map(|h| h.0),
            Some(ground)
        );
        assert_eq!(
            world
                .trace_contact(a, b, Some(900), &settings, &mut cache)
                .map(|h| h.0),
            Some(ground)
        );
        settings.snow.on_windows = false;
        assert_eq!(
            world
                .trace_contact(a, b, None, &settings, &mut cache)
                .map(|h| h.0),
            Some(ground)
        );
        settings.snow.on_ground = false;
        assert!(
            world
                .trace_contact(a, b, None, &settings, &mut cache)
                .is_none()
        );
    }

    #[test]
    fn contact_candidates_recheck_height_inside_one_cell() {
        let mut world = contact_fixture(300.0, &[(7, 20.0, 300.0)]);
        let roof = world.drifts.get_mut(&7).unwrap();
        let index = roof.index(50.0);
        roof.mass[index] = 6.0 * roof.cell_width;
        world.introduced_mass = world.total_mass();
        let settings = Settings::default();
        let mut cache = ContactCandidates::default();
        assert_eq!(
            world
                .trace_contact(
                    Point::new(50.0, 291.0),
                    Point::new(50.0, 298.0),
                    None,
                    &settings,
                    &mut cache
                )
                .map(|h| h.0),
            Some(7)
        );
        assert!(
            world
                .trace_contact(
                    Point::new(90.0, 291.0),
                    Point::new(90.0, 298.0),
                    None,
                    &settings,
                    &mut cache
                )
                .is_none()
        );
    }

    #[test]
    fn contact_candidates_support_negative_rows_and_coordinates() {
        let mut scene = Scene::default();
        scene.screens.push(Screen {
            key: 1,
            persistent_id: true,
            rect: Rect {
                left: -400.0,
                top: -800.0,
                right: 400.0,
                bottom: 200.0,
            },
            floor: 0.0,
            scale: 1.0,
        });
        scene.windows.push(Window {
            key: 7,
            rect: Rect {
                left: -100.0,
                top: -100.0,
                right: 100.0,
                bottom: 0.0,
            },
            visible: true,
            collect: true,
        });
        let world = World::new(scene, 41);
        let settings = Settings::default();
        let mut cache = ContactCandidates::default();
        assert_eq!(
            world
                .trace_contact(
                    Point::new(-50.0, -110.0),
                    Point::new(-50.0, -90.0),
                    None,
                    &settings,
                    &mut cache
                )
                .map(|h| h.0),
            Some(7)
        );
        assert!(
            world
                .trace_contact(
                    Point::new(-50.0, -90.0),
                    Point::new(-50.0, -110.0),
                    None,
                    &settings,
                    &mut cache
                )
                .is_none()
        );
        assert_eq!(
            world
                .trace_contact(
                    Point::new(-50.0, -700.0),
                    Point::new(-50.0, 10.0),
                    None,
                    &settings,
                    &mut cache
                )
                .map(|h| h.0),
            Some(7)
        );
    }

    #[test]
    fn budget_diagnostics_distinguish_waiting_parts_from_packets_and_live_bodies() {
        let (mut world, incoming) = pending_capacity_fixture(false);
        world.queue_collapse(incoming);
        world.introduced_mass = world.total_mass();
        assert!(world.release(Flake::new(Point::new(500.0, 50.0), 1.0)));
        let snapshot = world.budget_snapshot();
        assert_eq!(snapshot.active_bodies, 1);
        assert_eq!(snapshot.active_slabs, 0);
        assert_eq!(snapshot.pending_packets, 128);
        assert_eq!(snapshot.pending_bodies, 143);
        assert_eq!(snapshot.pending_profile_cells, 143);
        assert_eq!(snapshot.max_pending_bodies, 1024);
        assert!(snapshot.drift_cells > 0);
        world.clear();
        let cleared = world.budget_snapshot();
        assert_eq!(
            cleared.active_bodies + cleared.pending_bodies + cleared.falling_masks,
            0
        );
        assert_eq!(cleared.drift_cells, snapshot.drift_cells);
    }

    #[test]
    fn pending_packet_preserves_opposing_physical_motion() {
        let mut scene = desktop();
        scene.windows.clear();
        scene.screens[0].scale = 1.0;
        let mut world = World::new(scene, 41);
        let parts = [-40.0, 40.0].map(|vx| {
            let mut body = Flake::new(Point::new(400.0, 100.0), 1.0);
            body.mass = 12.0;
            body.velocity = Point::new(vx, 12.0);
            body.slab = Some(SnowSlab {
                heights: vec![2.0],
                cell_width: 6.0,
                center: Point::new(3.0, -1.0),
            });
            PendingCollapse { parts: vec![body] }
        });
        let [a, b] = parts;
        let packet = world.merge_pending_packets(a, b);
        world.collapses.push_back(packet);
        world.introduced_mass = world.total_mass();
        let mut settings = Settings::default();
        settings.snow.intensity_percent = 0;
        settings.snow.melting = false;
        world.step(&settings, None);
        assert_eq!(world.flakes.len(), 2);
        assert!(world.flakes[0].position.x < 400.0 && world.flakes[0].velocity.x < -35.0);
        assert!(world.flakes[1].position.x > 400.0 && world.flakes[1].velocity.x > 35.0);
        assert_eq!(world.total_mass(), 24.0);
        assert_eq!(world.removed_mass, 0.0);
    }

    fn pending_capacity_fixture(full: bool) -> (World, Flake) {
        let mut scene = desktop();
        scene.screens[0].rect.right = 200_000.0;
        scene.screens[0].scale = 1.0;
        let mut world = World::new(scene, 41);
        world.falling_masks.insert(
            9,
            FallingMask {
                front: BTreeSet::new(),
                covers: Vec::new(),
                hidden: true,
                coalesced: false,
                origins: BTreeSet::new(),
            },
        );
        let body = |x, y| {
            let mut body = Flake::new(Point::new(x, y), 1.0);
            body.mass = 12.0;
            body.slab = Some(SnowSlab {
                heights: vec![2.0],
                cell_width: 6.0,
                center: Point::new(3.0, -1.0),
            });
            body
        };
        for offset in [0.0, 15.0] {
            world.collapses.push_back(PendingCollapse {
                parts: (0..MAX_COLLAPSE_PARTS)
                    .map(|i| body(100.0, 50.0 + offset + i as f32 * 30.0))
                    .collect(),
            });
        }
        for i in 2..BASE_COLLAPSE_BUDGET {
            world.collapses.push_back(PendingCollapse {
                parts: (0..if full { MAX_COLLAPSE_PARTS } else { 1 })
                    .map(|j| {
                        let mut body = body(2000.0 + i as f32 * 1000.0, 100.0 + j as f32 * 30.0);
                        body.skip_support = Some(9);
                        body
                    })
                    .collect(),
            });
        }
        (world, body(160_000.0, 100.0))
    }

    #[test]
    fn pending_packets_use_spare_capacity_before_coarsening_other_layers() {
        let (mut world, incoming) = pending_capacity_fixture(false);
        let states = |world: &World| {
            world
                .collapses
                .iter()
                .flat_map(|p| &p.parts)
                .map(|b| (b.position.x, b.position.y, b.mass, b.skip_support))
                .collect::<Vec<_>>()
        };
        let mut expected = states(&world);
        expected.push((
            incoming.position.x,
            incoming.position.y,
            incoming.mass,
            incoming.skip_support,
        ));
        world.introduced_mass = world.total_mass() + f64::from(incoming.mass);
        world.queue_collapse(incoming);
        let mut actual = states(&world);
        let order = |a: &(f32, f32, f32, Option<u64>), b: &(f32, f32, f32, Option<u64>)| {
            a.0.total_cmp(&b.0).then_with(|| a.1.total_cmp(&b.1))
        };
        expected.sort_by(order);
        actual.sort_by(order);
        assert!(
            actual == expected,
            "unused packet capacity did not protect the original sources"
        );
        assert_eq!(world.collapses.len(), BASE_COLLAPSE_BUDGET);
        assert_eq!(world.collapses[0].parts.len(), MAX_COLLAPSE_PARTS);
        assert_eq!(world.collapses[1].parts.len(), MAX_COLLAPSE_PARTS);
        assert_eq!(
            world.collapses.back().unwrap().parts[0].position.x,
            160_000.0
        );
        assert_eq!(
            world.falling_masks.len(),
            1,
            "spare capacity created conservative replacement layers"
        );
        assert_eq!(world.total_mass(), world.introduced_mass);
        assert_eq!(world.removed_mass, 0.0);
    }

    #[test]
    fn pending_packets_repack_fragmented_capacity_without_changing_sources() {
        let (mut world, template) = pending_capacity_fixture(false);
        for (i, packet) in world.collapses.iter_mut().enumerate() {
            let origin = packet.parts[0].skip_support;
            packet.parts = (0..5)
                .map(|j| {
                    let mut body = template.clone();
                    body.position = Point::new(100.0 + i as f32 * 1000.0, 50.0 + j as f32 * 30.0);
                    body.skip_support = origin;
                    body
                })
                .collect();
        }
        let incoming = PendingCollapse {
            parts: (0..MAX_COLLAPSE_PARTS)
                .map(|j| {
                    let mut body = template.clone();
                    body.position.y += j as f32 * 30.0;
                    body
                })
                .collect(),
        };
        let states = |b: &Flake| (b.position.x, b.position.y, b.mass, b.skip_support);
        let mut expected: Vec<_> = world
            .collapses
            .iter()
            .flat_map(|p| &p.parts)
            .map(states)
            .chain(incoming.parts.iter().map(states))
            .collect();
        world.introduced_mass = world.total_mass()
            + incoming
                .parts
                .iter()
                .map(|b| f64::from(b.mass))
                .sum::<f64>();
        world.queue_collapse_packet(incoming);
        let mut actual: Vec<_> = world
            .collapses
            .iter()
            .flat_map(|p| &p.parts)
            .map(states)
            .collect();
        let order = |a: &(f32, f32, f32, Option<u64>), b: &(f32, f32, f32, Option<u64>)| {
            a.0.total_cmp(&b.0).then_with(|| a.1.total_cmp(&b.1))
        };
        expected.sort_by(order);
        actual.sort_by(order);
        assert!(
            actual == expected,
            "fragmented capacity changed source positions, masses or layers"
        );
        assert_eq!(world.collapses.len(), 82);
        assert_eq!(world.falling_masks.len(), 1);
        assert!(
            world.collapses[0].parts[..5]
                .iter()
                .all(|b| b.position.x == 100.0)
        );
        assert!(
            world
                .collapses
                .iter()
                .all(|p| p.parts.len() <= MAX_COLLAPSE_PARTS)
        );
        assert_eq!(world.total_mass(), world.introduced_mass);
        assert_eq!(world.removed_mass, 0.0);
    }

    #[test]
    fn pending_packet_overflow_keeps_a_bounded_material_representation() {
        let (mut world, incoming) = pending_capacity_fixture(true);
        let initial = world.total_mass();
        world.introduced_mass = initial + 12.0;
        let moment = |world: &World| {
            world
                .collapses
                .iter()
                .flat_map(|p| &p.parts)
                .map(|b| f64::from(b.mass) * f64::from(b.position.y))
                .sum::<f64>()
        };
        let expected_moment = moment(&world) + 12.0 * 100.0;
        world.queue_collapse(incoming);
        assert_eq!(world.collapses.len(), BASE_COLLAPSE_BUDGET);
        assert!(
            world
                .collapses
                .iter()
                .all(|p| !p.parts.is_empty() && p.parts.len() <= MAX_COLLAPSE_PARTS)
        );
        assert_eq!(world.collapses[0].parts.len(), MAX_COLLAPSE_PARTS);
        assert!((world.total_mass() - initial - 12.0).abs() < 0.001);
        assert!((moment(&world) - expected_moment).abs() < 0.001);
        assert_eq!(world.removed_mass, 0.0);
        assert_eq!(
            world.collapses.iter().map(|p| p.parts.len()).sum::<usize>(),
            BASE_COLLAPSE_BUDGET * MAX_COLLAPSE_PARTS,
            "a single arrival discarded more geometry than required by the full capacity"
        );
        assert_eq!(world.falling_masks.len(), 2);
        world.clear();
        assert!(world.falling_masks.is_empty());
        assert_eq!(world.total_mass(), 0.0);
        assert_eq!(world.introduced_mass, world.removed_mass);
    }

    #[test]
    fn sparse_snow_contours_exclude_empty_regions() {
        let slab = SnowSlab {
            heights: vec![2.0, 0.0, 0.0, 2.0],
            cell_width: 10.0,
            center: Point::new(20.0, -1.0),
        };
        let contours: Vec<_> = slab.contours().collect();
        assert_eq!(
            contours.len(),
            2,
            "empty cells joined disconnected material"
        );
        assert!(contours[0].iter().all(|p| p.x >= -20.0 && p.x <= -10.0));
        assert!(contours[1].iter().all(|p| p.x >= 10.0 && p.x <= 20.0));
        let empty = SnowSlab {
            heights: Vec::new(),
            cell_width: 10.0,
            center: Point::default(),
        };
        assert_eq!(empty.contours().count(), 0);
        let zero = SnowSlab {
            heights: vec![0.0; 16],
            cell_width: 10.0,
            center: Point::default(),
        };
        assert_eq!(zero.contours().count(), 0);
        let alternating = SnowSlab {
            heights: (0..16)
                .map(|i| if i % 2 == 0 { 1.0 } else { 0.0 })
                .collect(),
            cell_width: 10.0,
            center: Point::default(),
        };
        assert_eq!(alternating.contours().count(), 8);
    }

    #[test]
    fn sparse_snow_mound_uses_an_active_impact_instead_of_a_paused_center() {
        let mut scene = desktop();
        scene.screens[0].rect.right = 800.0;
        scene.screens[0].scale = 1.0;
        scene.screens.push(Screen {
            key: 2,
            persistent_id: true,
            rect: Rect {
                left: 800.0,
                top: 0.0,
                right: 1600.0,
                bottom: 1000.0,
            },
            floor: 1000.0,
            scale: 1.0,
        });
        scene.windows[0].rect = Rect {
            left: 0.0,
            right: 1600.0,
            top: 400.0,
            bottom: 700.0,
        };
        let mut world = World::new(scene, 41);
        world.set_paused_monitors([1].into());
        let impact = world.drifts[&7].index(1000.0);
        let mut settings = Settings::default();
        settings.snow.drift_size_percent = 100;
        world.deposit_mound(7, impact, 790.0, 100.0, &settings);
        let roof = &world.drifts[&7];
        let mass = roof.total_mass();
        let center = (0..roof.mass.len())
            .map(|i| f64::from(roof.x(i)) * f64::from(roof.mass[i] + roof.pending[i]))
            .sum::<f64>()
            / mass;
        assert!(
            center > 950.0,
            "mound moved from the impact toward a paused center: {center}"
        );
        assert!((mass - 100.0).abs() < 0.001);
        assert_eq!(world.removed_mass, 0.0);
        assert!(
            (0..roof.mass.len())
                .filter(|&i| roof.x(i) < 800.0)
                .all(|i| roof.mass[i] == 0.0 && roof.pending[i] == 0.0)
        );
    }

    fn desktop() -> Scene {
        Scene {
            screens: vec![Screen {
                key: 1,
                persistent_id: true,
                rect: Rect {
                    left: 0.0,
                    top: 0.0,
                    right: 1600.0,
                    bottom: 1000.0,
                },
                floor: 1000.0,
                scale: 2.5,
            }],
            windows: vec![Window {
                key: 7,
                rect: Rect {
                    left: 100.0,
                    top: 200.0,
                    right: 900.0,
                    bottom: 600.0,
                },
                visible: true,
                collect: true,
            }],
        }
    }

    #[test]
    fn transitioning_edge_chunk_preserves_source_physical_area() {
        let scene = desktop();
        let mut world = World::new(scene.clone(), 41);
        let roof = world.drifts.get_mut(&7).unwrap();
        roof.scales.fill(2.0);
        roof.mass.fill(12.0 * roof.cell_width / 2.0);
        roof.edge_wait[0] = 0.0;
        let before_mass = roof.total_mass();
        let before_area = before_mass * 4.0;
        let body = roof
            .edge_chunk(false, &scene, &BTreeSet::new(), true)
            .expect("edge should shed");
        let slab = body.slab.as_ref().unwrap();
        let emitted_area = f64::from(slab.heights.iter().sum::<f32>() * slab.cell_width)
            * f64::from(body.display_scale(2.5)).powi(2);
        assert!((roof.total_mass() + f64::from(body.mass) - before_mass).abs() < 0.001);
        assert!((roof.total_mass() * 4.0 + emitted_area - before_area).abs() < 0.01);
        assert_eq!(body.display_scale(2.5), 2.0);
    }

    #[test]
    fn paused_geometry_updates_keep_snow_units_finite() {
        let mut scene = desktop();
        scene.screens[0].scale = 1.0;
        let mut world = World::new(scene.clone(), 41);
        let roof = world.drifts.get_mut(&7).unwrap();
        roof.mass.fill(roof.cell_width);
        world.introduced_mass = world.total_mass();
        let mass = world.total_mass();
        world.set_paused_monitors([1].into());
        for _ in 0..250 {
            scene.screens[0].scale = 2.5;
            scene.windows[0].rect.right = 2100.0;
            world.set_scene(scene.clone());
            scene.windows[0].rect.right = 900.0;
            world.set_scene(scene.clone());
            scene.screens[0].scale = 1.0;
            world.set_scene(scene.clone());
            let roof = &world.drifts[&7];
            assert!(
                (0..roof.mass.len()).all(|i| roof.height(i, &world.scene).is_finite()),
                "paused DPI/resize sequence overflowed snow geometry"
            );
        }
        assert!((world.total_mass() - mass).abs() < mass * 0.001);
        assert!(world.flakes.is_empty());
        assert_eq!(world.removed_mass, 0.0);
    }

    #[test]
    fn full_spill_queue_preserves_physical_momentum_across_source_units() {
        let mut world = World::new(desktop(), 41);
        for _ in 0..127 {
            world.queue_spill(
                Point::new(100.0, 100.0),
                1.0,
                Point::default(),
                Some(1),
                1.0,
            );
        }
        world.queue_spill(
            Point::new(200.0, 100.0),
            2.0,
            Point::new(20.0, 12.0),
            Some(7),
            2.0,
        );
        world.queue_spill(
            Point::new(220.0, 100.0),
            2.0,
            Point::new(30.0, 12.0),
            Some(7),
            1.0,
        );
        assert_eq!(world.spill.len(), 128);
        let body = world.spill.back().unwrap();
        let scale = body.display_scale(1.0);
        assert_eq!(body.mass, 4.0);
        assert_eq!(body.position, Point::new(210.0, 100.0));
        assert!((body.velocity.x * scale - 35.0).abs() < 0.0001);
        assert!((body.velocity.y * scale - 18.0).abs() < 0.0001);
        assert!((body.mass * scale * scale - 10.0).abs() < 0.0001);
    }
}
