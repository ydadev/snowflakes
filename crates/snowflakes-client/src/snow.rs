//! Force integration, swept contacts and mass-preserving accumulation.
//! Trajectories are state, not functions of an animation clock.
use crate::geometry::{Point, Rect, Scene, Support, uncovered_rectangles};
use snowflakes_protocol::Settings;
use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    f32::consts::TAU,
};

pub const DT: f32 = 1.0 / 60.0;
pub const MAX_FLAKES: usize = 2400;
pub const BASE_COLLAPSE_BUDGET: usize = 128;
pub const ORDINARY_FLAKES_PER_SCREEN: usize = MAX_FLAKES - BASE_COLLAPSE_BUDGET;
const CELL: f32 = 6.0;
const MAX_CELLS: usize = 65_536;
const GRAVITY: f32 = 48.0;
const SLAB_GRAVITY: f32 = 200.0;
const EDGE_INTERVAL: f32 = 0.75;
const MAX_EDGE_CHUNKS_PER_STEP: usize = 8;
#[derive(Clone, Debug)]
pub struct SnowSlab {
    pub heights: Vec<f32>,
    pub cell_width: f32,
    pub center: Point,
}
impl SnowSlab {
    pub fn outline(&self) -> Vec<Point> {
        let mut points = Vec::with_capacity(self.heights.len() + 4);
        points.push(Point::new(-self.center.x, -self.center.y));
        points.push(Point::new(-self.center.x, -self.heights[0] - self.center.y));
        for (index, height) in self.heights.iter().enumerate() {
            points.push(Point::new(
                (index as f32 + 0.5) * self.cell_width - self.center.x,
                -height - self.center.y,
            ));
        }
        let right = self.heights.len() as f32 * self.cell_width - self.center.x;
        points.push(Point::new(
            right,
            -self.heights[self.heights.len() - 1] - self.center.y,
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
        }
    }
    pub fn integrate(&mut self, air: Point, scale: f32) {
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
    pub fn transformed_point(&self, local: Point, scale: f32) -> Point {
        let (s, c) = self.angle.sin_cos();
        Point::new(
            self.position.x + (local.x * c - local.y * s) * scale,
            self.position.y + (local.x * s + local.y * c) * scale,
        )
    }
    /// Include the rendered contour and its outline, even during rotation.
    pub fn draw_bounds(&self, scale: f32) -> Rect {
        if let Some(slab) = &self.slab {
            let mut bounds = Rect {
                left: f32::INFINITY,
                top: f32::INFINITY,
                right: f32::NEG_INFINITY,
                bottom: f32::NEG_INFINITY,
            };
            for local in slab.outline() {
                let p = self.transformed_point(local, scale);
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
                left: self.position.x - radius,
                top: self.position.y - radius,
                right: self.position.x + radius,
                bottom: self.position.y + radius,
            }
        }
    }
}
#[derive(Clone, Copy, Default)]
pub struct Cursor {
    pub position: Point,
    pub velocity: Point,
}
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
}
impl Drift {
    fn edge_chunk(
        &mut self,
        right: bool,
        scene: &Scene,
        paused: &BTreeSet<u64>,
        allow_emit: bool,
    ) -> Option<Flake> {
        let side = usize::from(right);
        let edge = if right { self.mass.len() - 1 } else { 0 };
        let scale = scene.scale_at(Point::new(self.x(edge), self.support.y - 1.0));
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
                    || (scene.scale_at(Point::new(self.x(i), self.support.y - 1.0)) - scale).abs()
                        > 0.001
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
        let outline = body.slab.as_ref().unwrap().outline();
        let bounds = outline
            .into_iter()
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
    fn contact_height(&self, index: usize, scene: &Scene) -> f32 {
        let scale = scene.scale_at(Point::new(self.x(index), self.support.y - 1.0));
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
        let scale = scene.scale_at(Point::new(x, self.support.y - 1.0));
        let max_depth = if self.support.ground { 120.0 } else { 32.0 };
        let depth = (max_depth * settings.snow.drift_size_percent as f32 / 100.0 * scale)
            .min(scene.headroom(x, self.support.y));
        depth / scale / scale * self.cell_width
    }
    fn deposit_capacity(&self, index: usize, scene: &Scene, settings: &Settings) -> f32 {
        let point = Point::new(
            self.x(index),
            self.support.y - self.contact_height(index, scene),
        );
        let scale = scene.scale_at(Point::new(point.x, self.support.y - 1.0));
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
            let left_scale = scene.scale_at(Point::new(self.x(index), self.support.y - 1.0));
            let right_scale = scene.scale_at(Point::new(self.x(index + 1), self.support.y - 1.0));
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
    pub fn height(&self, index: usize, scene: &Scene) -> f32 {
        let x = self.support.left + (index as f32 + 0.5) * self.cell_width;
        let scale = scene.scale_at(Point::new(x, self.support.y - 1.0));
        self.mass[index] * scale * scale / self.cell_width
    }
    pub fn x(&self, index: usize) -> f32 {
        self.support.left + (index as f32 + 0.5) * self.cell_width
    }
    fn index(&self, x: f32) -> usize {
        (((x - self.support.left) / self.cell_width) as usize).min(self.mass.len() - 1)
    }
}
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
struct FallingMask {
    /// Logical IDs survive source closure without retaining native handles.
    front: BTreeSet<u64>,
    covers: Vec<Rect>,
    /// An inactive source never transfers visible material to this desktop.
    /// Bodies still obey monitor pauses and become outflow on landing/exit.
    hidden: bool,
    coalesced: bool,
}
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
    collapses: VecDeque<Flake>,
    falling_masks: BTreeMap<u64, FallingMask>,
    next_falling_mask: u64,
    usage: BTreeMap<Option<u64>, ParticleUsage>,
    paused_monitors: BTreeSet<u64>,
    pub introduced_mass: f64,
    pub removed_mass: f64,
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
            introduced_mass: 0.0,
            removed_mass: 0.0,
        };
        world.set_scene(scene);
        world
    }
    pub fn total_mass(&self) -> f64 {
        self.flakes
            .iter()
            .chain(self.spill.iter())
            .chain(self.collapses.iter())
            .map(|f| f64::from(f.mass))
            .sum::<f64>()
            + self.drifts.values().map(Drift::total_mass).sum::<f64>()
    }
    pub fn set_paused_monitors(&mut self, monitors: BTreeSet<u64>) {
        self.paused_monitors = monitors;
    }
    pub fn clear(&mut self) {
        self.removed_mass += self.total_mass();
        self.flakes.clear();
        self.spill.clear();
        self.collapses.clear();
        self.usage.clear();
        self.falling_masks.clear();
        for drift in self.drifts.values_mut() {
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
    pub fn pending_collapse_count(&self) -> usize {
        self.collapses.len()
    }
    fn priority_body(&self, body: &Flake) -> bool {
        body.skip_support
            .is_some_and(|key| self.falling_masks.contains_key(&key))
    }
    fn slot_available(&self, body: &Flake) -> bool {
        let screens = self.scene.screens.len().max(1);
        if self.flakes.len() >= MAX_FLAKES.saturating_mul(screens) {
            return false;
        }
        let key = self.scene.nearest_screen(body.position).map(|s| s.key);
        let used = self.usage.get(&key).copied().unwrap_or_default();
        let priority = self.priority_body(body);
        if priority && used.total - used.ordinary < BASE_COLLAPSE_BUDGET {
            // Wind/topology may have moved ordinary bodies into this screen.
            // Its minimum material reserve survives that temporary imbalance.
            return true;
        }
        if MAX_FLAKES.saturating_mul(screens) - self.flakes.len() <= self.reserved_slots() {
            return false;
        }
        if priority {
            used.total < MAX_FLAKES
        } else {
            used.total < MAX_FLAKES
                && used.ordinary < ORDINARY_FLAKES_PER_SCREEN
                && self.usage.values().map(|u| u.ordinary).sum::<usize>()
                    < ORDINARY_FLAKES_PER_SCREEN.saturating_mul(screens)
        }
    }
    fn reserved_slots(&self) -> usize {
        let missing = |key| {
            let used = self.usage.get(&key).copied().unwrap_or_default();
            BASE_COLLAPSE_BUDGET.saturating_sub(used.total - used.ordinary)
        };
        if self.scene.screens.is_empty() {
            missing(None)
        } else {
            self.scene.screens.iter().fold(0_usize, |n, screen| {
                n.saturating_add(missing(Some(screen.key)))
            })
        }
    }
    fn activate(&mut self, body: Flake) {
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
        let mut body = crate::collapse::merge(&a, &b, &self.scene);
        let mut front = BTreeSet::new();
        let mut hidden = false;
        for parent in [&a, &b] {
            if let Some(mask) = parent
                .skip_support
                .and_then(|key| self.falling_masks.get(&key))
            {
                front.extend(mask.front.iter().copied());
                hidden = mask.hidden;
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
                    .chain(&self.collapses)
                    .any(|f| f.skip_support == Some(key))
            {
                self.falling_masks.remove(&key);
            }
        }
        body
    }
    fn queue_collapse(&mut self, body: Flake) {
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
                    f.position,
                    self.scene.nearest_screen(f.position).map(|s| s.key),
                    f.skip_support
                        .and_then(|key| self.falling_masks.get(&key))
                        .is_some_and(|mask| mask.hidden),
                )
            })
            .collect();
        let mut best: Option<(usize, usize, f64)> = None;
        for i in 0..candidates.len() {
            for j in i + 1..candidates.len() {
                if candidates[i].1 != candidates[j].1 || candidates[i].2 != candidates[j].2 {
                    continue;
                }
                let dx = f64::from(candidates[i].0.x) - f64::from(candidates[j].0.x);
                let dy = f64::from(candidates[i].0.y) - f64::from(candidates[j].0.y);
                let distance = dx * dx + dy * dy;
                if best.is_none_or(|(_, _, old)| distance < old) {
                    best = Some((i, j, distance));
                }
            }
        }
        // Two visibility classes per screen, with at least one extra packet:
        // a compatible pair exists even when every monitor has its own class.
        let (i, j, _) = best.expect("pending collapse classes fit their budget");
        if j == self.collapses.len() {
            let a = self.collapses.remove(i).unwrap();
            let merged = self.merge_pending_collapses(a, body);
            // Coalescing must inherit the oldest parent's admission position;
            // new arrivals must not repeatedly rejuvenate already waiting snow.
            self.collapses.insert(i, merged);
        } else {
            let b = self.collapses.remove(j).unwrap();
            let a = self.collapses.remove(i).unwrap();
            let merged = self.merge_pending_collapses(a, b);
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
            .chain(&self.collapses)
            .filter_map(|body| body.skip_support)
            .collect();
        self.falling_masks.retain(|key, _| used.contains(key));
    }
    fn refresh_falling_masks(&mut self) {
        self.retire_falling_masks();
        let current: BTreeSet<_> = self.scene.windows.iter().map(|w| w.key).collect();
        for mask in self.falling_masks.values_mut() {
            mask.front.retain(|key| current.contains(key));
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
    pub fn release(&mut self, flake: Flake) -> bool {
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
        self.activate(flake);
        true
    }
    fn queue_spill(&mut self, position: Point, mass: f32, velocity: Point, origin: Option<u64>) {
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
                last.position.x += (position.x - last.position.x) * weight;
                last.position.y += (position.y - last.position.y) * weight;
                last.velocity.x += (velocity.x - last.velocity.x) * weight;
                last.velocity.y += (velocity.y - last.velocity.y) * weight;
                last.mass += mass;
                last.radius = (last.mass / 1.20).sqrt().min(5.5);
            } else {
                self.removed_mass += f64::from(mass);
            }
            return;
        }
        let mut flake = Flake::new(position, (mass / 1.20).sqrt().min(5.5));
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
            let scale = self
                .scene
                .scale_at(Point::new(old.x(first), old.support.y - 1.0));
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
                    || (self
                        .scene
                        .scale_at(Point::new(old.x(i), old.support.y - 1.0))
                        - scale)
                        .abs()
                        > 0.001
            }) {
                end = gap;
            }
            let mass: f32 = (first..end).map(|i| old.mass[i] + old.pending[i]).sum();
            let cell_width = old.cell_width / scale;
            let heights: Vec<f32> = (first..end)
                .map(|i| old.contact_height(i, &self.scene) / scale)
                .collect();
            let mut center = Point::default();
            for (i, &height) in heights.iter().enumerate() {
                let weight = (old.mass[first + i] + old.pending[first + i]) / mass;
                center.x += (i as f32 + 0.5) * cell_width * weight;
                center.y -= height * 0.5 * weight;
            }
            let position = Point::new(
                old.support.left + first as f32 * old.cell_width + center.x * scale,
                old.support.y + center.y * scale,
            );
            let velocity = Point::new(
                old.motion_velocity.x.clamp(-180.0, 180.0),
                old.motion_velocity.y.clamp(0.0, 80.0) + 12.0,
            );
            let mut body = Flake::new(position, (mass / 1.20).sqrt().clamp(0.2, 32.0));
            body.mass = mass;
            body.velocity = velocity;
            body.spin = self.rng.signed() * 0.55;
            body.age = 1.0;
            body.skip_support = Some(old.support.key);
            body.slab = Some(SnowSlab {
                heights,
                cell_width,
                center,
            });
            self.queue_collapse(body);
            first = end;
        }
    }
    pub fn set_scene(&mut self, scene: Scene) {
        let sample_age = self.scene_age;
        self.scene_age = 0.0;
        let mut previous = std::mem::take(&mut self.drifts);
        let mut cells = 0;
        for mut support in scene.supports() {
            if support.ground
                && let Some(old) = previous.get(&support.key)
                && let Some(screen) = scene
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
                support = scene.ground_support(screen, old.support.y);
            }
            let width = (support.right - support.left).max(1.0);
            let count = (width / CELL).ceil().clamp(1.0, 4096.0) as usize;
            if cells + count > MAX_CELLS {
                continue;
            }
            cells += count;
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
                cell_width: width / count as f32,
                motion_velocity: Point::default(),
            };
            if let Some(mut old) = previous.remove(&drift.support.key) {
                drift.edge_wait = old.edge_wait;
                if !drift.support.ground && sample_age > 0.0 {
                    let scale = self
                        .scene
                        .scale_at(Point::new(old.support.left, old.support.y - 1.0));
                    let dx = (drift.support.left - old.support.left) / scale;
                    let dy = (drift.support.y - old.support.y) / scale;
                    let distance = dx.hypot(dy);
                    let ratio = width / (old.support.right - old.support.left).max(1.0);
                    let teleport = distance > 480.0 || !(0.65..=1.55).contains(&ratio);
                    if !teleport && !(distance < 0.01 && sample_age > 0.15) {
                        let dt = sample_age.clamp(DT, 0.06);
                        let alpha = dt / (0.07 + dt);
                        let velocity = Point::new(
                            old.motion_velocity.x + (dx / dt - old.motion_velocity.x) * alpha,
                            old.motion_velocity.y + (dy / dt - old.motion_velocity.y) * alpha,
                        );
                        let acceleration = (velocity.x - old.motion_velocity.x)
                            .hypot(velocity.y - old.motion_velocity.y)
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
                                    old.motion_velocity.x.clamp(-180.0, 180.0),
                                    old.motion_velocity.y.clamp(0.0, 80.0) + 12.0,
                                );
                                self.queue_spill(position, mass, velocity, Some(old.support.key));
                            }
                        }
                    }
                }
                if old.mass.len() == count && (old.cell_width - drift.cell_width).abs() < 0.001 {
                    drift.mass = old.mass;
                    drift.pending = old.pending;
                    drift.disable_rate = old.disable_rate;
                    drift.shrink_remaining = old.shrink_remaining;
                    drift.last_size = old.last_size;
                } else {
                    // Preserve resting and just-arrived material during resampling.
                    for index in 0..old.mass.len() {
                        let a = index as f32 / old.mass.len() as f32 * count as f32;
                        let b = (index + 1) as f32 / old.mass.len() as f32 * count as f32;
                        for target in (a.floor() as usize)..(b.ceil() as usize).min(count) {
                            let fraction = (b.min((target + 1) as f32) - a.max(target as f32))
                                .max(0.0)
                                / (b - a);
                            drift.mass[target] += old.mass[index] * fraction;
                            drift.pending[target] += old.pending[index] * fraction;
                            drift.disable_rate[target] += old.disable_rate[index] * fraction;
                            drift.shrink_remaining[target] =
                                drift.shrink_remaining[target].max(old.shrink_remaining[index]);
                            drift.last_size[target] =
                                drift.last_size[target].max(old.last_size[index]);
                        }
                    }
                }
            }
            self.drifts.insert(drift.support.key, drift);
        }
        for old in previous.into_values() {
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
                            front: self.scene.windows[..index].iter().map(|w| w.key).collect(),
                            covers: Vec::new(),
                            hidden: !self.scene.windows[index].visible,
                            coalesced: false,
                        },
                    );
                }
                self.release_slabs(&old);
                continue;
            }
            for index in 0..old.mass.len() {
                let mass = old.mass[index] + old.pending[index];
                let velocity = Point::new(
                    old.motion_velocity.x.clamp(-180.0, 180.0) + self.rng.signed() * 6.0,
                    old.motion_velocity.y.clamp(0.0, 80.0) + 12.0,
                );
                let height = old.contact_height(index, &self.scene);
                let position = Point::new(old.x(index), old.support.y - height * self.rng.unit());
                self.queue_spill(position, mass, velocity, Some(old.support.key));
            }
        }
        self.scene = scene;
        let pending = std::mem::take(&mut self.collapses);
        for body in pending {
            self.queue_collapse(body);
        }
        self.refresh_falling_masks();
        self.rebuild_buckets();
        self.rebuild_usage();
    }
    fn rebuild_buckets(&mut self) {
        self.buckets.clear();
        let bounds = self.scene.bounds();
        self.bucket_left = bounds.left;
        // Bound the horizontal index even on very wide virtual desktops.
        self.bucket_width = (bounds.width() / 256.0).max(128.0);
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
        if self
            .scene
            .screens
            .iter()
            .all(|s| self.paused_monitors.contains(&s.key))
        {
            return;
        }
        self.advance_ground();
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
            let body = self.collapses.pop_front().unwrap();
            if self.scene.paused_at(body.position, &self.paused_monitors)
                || !self.slot_available(&body)
            {
                self.collapses.push_back(body);
            } else {
                self.activate(body);
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
        let flakes = std::mem::take(&mut self.flakes);
        for mut flake in flakes {
            if self.scene.paused_at(flake.position, &self.paused_monitors) {
                alive.push(flake);
                continue;
            }
            let before = flake.position;
            let before_angle = flake.angle;
            let scale = self.scene.scale_at(before);
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
            flake.integrate(air, scale);
            let radius = flake.radius * scale;
            let contact = if let Some(slab) = &flake.slab {
                let (s, c) = before_angle.sin_cos();
                slab.outline()
                    .into_iter()
                    .filter_map(|local| {
                        let a = Point::new(
                            before.x + (local.x * c - local.y * s) * scale,
                            before.y + (local.x * s + local.y * c) * scale,
                        );
                        self.trace_contact(
                            a,
                            flake.transformed_point(local, scale),
                            flake.skip_support,
                            settings,
                        )
                    })
                    .min_by(|a, b| a.2.total_cmp(&b.2))
            } else {
                self.trace_contact(
                    Point::new(before.x, before.y + radius),
                    Point::new(flake.position.x, flake.position.y + radius),
                    flake.skip_support,
                    settings,
                )
            };
            if let Some((key, index, _)) = contact {
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
                        swept.push((Point::new(position.x, position.y - 3.0), mass));
                    }
                }
            }
            // Apply all pairwise fluxes together, avoiding a travelling-wave bias.
            if accumulating {
                drift.relax(&self.scene, &self.paused_monitors, settings);
            }
        }
        for (position, mass) in swept {
            let velocity = Point::new(self.rng.signed() * 18.0, -8.0);
            self.queue_spill(position, mass, velocity, None);
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
        let screens = self.scene.screens.len().max(1);
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
                    && MAX_FLAKES.saturating_mul(screens).saturating_sub(total) > reserved
                    && ordinary < ORDINARY_FLAKES_PER_SCREEN.saturating_mul(screens)
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
    fn trace_contact(
        &self,
        a: Point,
        b: Point,
        skip: Option<u64>,
        settings: &Settings,
    ) -> Option<(u64, usize, f32)> {
        if b.y <= a.y {
            return None;
        }
        let from = (a.y / 128.0).floor() as i32;
        let to = (b.y / 128.0).floor() as i32;
        let column = ((b.x - self.bucket_left) / self.bucket_width).floor() as i32;
        let candidates: BTreeSet<u64> = (from..=to)
            .filter_map(|r| self.buckets.get(&(r, column)))
            .flatten()
            .copied()
            .collect();
        let mut contact: Option<(u64, usize, f32)> = None;
        let behind = skip.and_then(|key| self.falling_masks.get(&key));
        for key in candidates {
            if skip == Some(key) || behind.is_some_and(|mask| mask.front.contains(&key)) {
                continue;
            }
            let drift = &self.drifts[&key];
            if (drift.support.ground && !settings.snow.on_ground)
                || (!drift.support.ground && !settings.snow.on_windows)
            {
                continue;
            }
            let x = b.x;
            if x < drift.support.left || x >= drift.support.right || !drift.support.visible_at(x) {
                continue;
            }
            let index = drift.index(x);
            let y = drift.support.y - drift.contact_height(index, &self.scene);
            if !drift.support.exposed(Point::new(x, y)) {
                continue;
            }
            if self
                .scene
                .paused_at(Point::new(x, drift.support.y - 1.0), &self.paused_monitors)
            {
                continue;
            }
            if b.y >= y && a.y <= drift.support.y {
                // Another impact may have raised this surface into the body
                // since its last step. Resolve that solid overlap immediately;
                // particles already below a window's base still pass freely.
                let t = ((y - a.y) / (b.y - a.y)).max(0.0);
                if contact.is_none_or(|(_, _, old)| t < old) {
                    contact = Some((key, index, t));
                }
            }
        }
        contact
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
        let center =
            if x >= drift.support.left && x < drift.support.right && drift.support.visible_at(x) {
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
