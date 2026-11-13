//! Force integration, swept contacts and mass-preserving accumulation.
//! Trajectories are state, not functions of an animation clock.
use crate::geometry::{Point, Scene, Support};
use snowflakes_protocol::Settings;
use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    f32::consts::TAU,
};

pub const DT: f32 = 1.0 / 60.0;
pub const MAX_FLAKES: usize = 2400;
const CELL: f32 = 6.0;
const MAX_CELLS: usize = 65_536;
const GRAVITY: f32 = 48.0;
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
        points
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
        let drag = 0.09 * self.radius.max(0.2) * self.shape;
        self.velocity.y += GRAVITY * DT;
        let vx = self.velocity.x - air.x;
        let vy = self.velocity.y - air.y;
        let damping = 1.0 / (1.0 + drag / self.mass * vx.hypot(vy) * DT);
        self.velocity.x = air.x + vx * damping;
        self.velocity.y = air.y + vy * damping;
        self.velocity.x = self.velocity.x.clamp(-220.0, 220.0);
        self.velocity.y = self.velocity.y.clamp(-120.0, 160.0);
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
    pub cell_width: f32,
    motion_velocity: Point,
}
impl Drift {
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
    fn relax(&mut self, scene: &Scene, paused: &BTreeSet<u64>) {
        self.scratch.fill(0.0);
        for index in 0..self.mass.len().saturating_sub(1) {
            if [index, index + 1]
                .iter()
                .any(|&i| scene.paused_at(Point::new(self.x(i), self.support.y - 1.0), paused))
            {
                continue;
            }
            let difference = self.height(index, scene) - self.height(index + 1, scene);
            let excess = (difference.abs() - 0.55 * self.cell_width).max(0.0);
            let scale = scene.scale_at(Point::new(self.x(index), self.support.y - 1.0));
            let donor = if difference > 0.0 {
                self.mass[index]
            } else {
                self.mass[index + 1]
            };
            let amount = (excess * self.cell_width / scale / scale * (6.0 * DT)).min(donor * 0.25)
                * difference.signum();
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
pub struct World {
    pub scene: Scene,
    pub flakes: Vec<Flake>,
    pub drifts: BTreeMap<u64, Drift>,
    rng: Rng,
    wind: [Point; 40],
    emission: f32,
    size_factor: f32,
    scene_age: f32,
    buckets: BTreeMap<i32, Vec<u64>>,
    spill: VecDeque<Flake>,
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
            buckets: BTreeMap::new(),
            spill: VecDeque::new(),
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
        for drift in self.drifts.values_mut() {
            drift.mass.fill(0.0);
            drift.pending.fill(0.0);
        }
        self.emission = 0.0;
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
        if self.flakes.len() >= MAX_FLAKES
            || ![
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
        {
            return false;
        }
        self.introduced_mass += f64::from(flake.mass);
        self.flakes.push(flake);
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
            if let Some(last) = self.spill.back_mut() {
                let total = last.mass + mass;
                let weight = mass / total;
                last.position.x += (position.x - last.position.x) * weight;
                last.position.y += (position.y - last.position.y) * weight;
                last.velocity.x += (velocity.x - last.velocity.x) * weight;
                last.velocity.y += (velocity.y - last.velocity.y) * weight;
                if last.skip_support != origin {
                    last.skip_support = None;
                }
                last.mass += mass;
                last.slab = None;
                last.radius = (last.mass / 1.20).sqrt().min(5.5);
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
            if let Some(gap) = (first..end).find(|&i| old.mass[i] + old.pending[i] <= 0.0) {
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
            if self.spill.len() >= 128 || mass < cell_width * heights.len() as f32 * 0.2 {
                self.queue_spill(position, mass, velocity, Some(old.support.key));
            } else {
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
                self.spill.push_back(body);
            }
            first = end;
        }
    }
    pub fn set_scene(&mut self, scene: Scene) {
        let sample_age = self.scene_age;
        self.scene_age = 0.0;
        let mut previous = std::mem::take(&mut self.drifts);
        let mut cells = 0;
        for support in scene.supports() {
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
                cell_width: width / count as f32,
                motion_velocity: Point::default(),
            };
            if let Some(mut old) = previous.remove(&drift.support.key) {
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
                        }
                    }
                }
            }
            self.drifts.insert(drift.support.key, drift);
        }
        for old in previous.into_values() {
            if !old.support.ground {
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
        self.buckets.clear();
        for drift in self.drifts.values() {
            // Ground: 120 DIP; window ledges: 32 DIP, at up to 400% DPI.
            let reach = if drift.support.ground { 490.0 } else { 140.0 };
            let from = ((drift.support.y - reach) / 128.0).floor() as i32;
            let to = (drift.support.y / 128.0).floor() as i32;
            for row in from..=to {
                self.buckets.entry(row).or_default().push(drift.support.key);
            }
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
        self.scene_age = (self.scene_age + DT).min(2.0);
        self.set_flake_size(settings.snow.flake_size_percent);
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
        for _ in 0..self.spill.len() {
            if self.flakes.len() >= MAX_FLAKES {
                break;
            }
            let flake = self.spill.pop_front().unwrap();
            if self.scene.paused_at(flake.position, &self.paused_monitors) {
                self.spill.push_back(flake);
            } else {
                self.flakes.push(flake);
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
        while self.emission >= 1.0 && self.flakes.len() < MAX_FLAKES {
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
                if settings.snow.drift_size_percent > 0 {
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
        let mut swept = Vec::new();
        for drift in self.drifts.values_mut() {
            for index in 0..drift.mass.len() {
                if self.scene.paused_at(
                    Point::new(drift.x(index), drift.support.y - 1.0),
                    &self.paused_monitors,
                ) {
                    continue;
                }
                let scale = self
                    .scene
                    .scale_at(Point::new(drift.x(index), drift.support.y - 1.0));
                let capacity = drift.capacity(index, &self.scene, settings);
                // A reduced ceiling removes excess, without a snow debt above it.
                let excess = (drift.mass[index] + drift.pending[index] - capacity).max(0.0);
                let loose = drift.pending[index].min(excess);
                drift.pending[index] -= loose;
                self.removed_mass += f64::from(loose);
                let removed = (drift.mass[index] - capacity)
                    .max(0.0)
                    .min(48.0 * DT * drift.cell_width / scale);
                drift.mass[index] -= removed;
                self.removed_mass += f64::from(removed);
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
                    if distance < 28.0 && cursor.velocity.x.hypot(cursor.velocity.y) > 15.0 {
                        let fraction = (1.0 - distance / 28.0) * 0.12;
                        let mass = (drift.mass[index] + drift.pending[index]) * fraction;
                        drift.mass[index] *= 1.0 - fraction;
                        drift.pending[index] *= 1.0 - fraction;
                        swept.push((Point::new(position.x, position.y - 3.0), mass));
                    }
                }
            }
            // Apply all pairwise fluxes together, avoiding a travelling-wave bias.
            drift.relax(&self.scene, &self.paused_monitors);
        }
        for (position, mass) in swept {
            let velocity = Point::new(self.rng.signed() * 18.0, -8.0);
            self.queue_spill(position, mass, velocity, None);
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
        let candidates: BTreeSet<u64> = (from..=to)
            .filter_map(|r| self.buckets.get(&r))
            .flatten()
            .copied()
            .collect();
        let mut contact: Option<(u64, usize, f32)> = None;
        for key in candidates {
            if skip == Some(key) {
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
            if self
                .scene
                .paused_at(Point::new(x, drift.support.y - 1.0), &self.paused_monitors)
            {
                continue;
            }
            if a.y <= y && b.y >= y {
                let t = (y - a.y) / (b.y - a.y);
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
                let available = (drift.capacity(index, &self.scene, settings)
                    - drift.mass[index]
                    - drift.pending[index])
                    .max(0.0);
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
                let available =
                    (drift.capacity(i, &self.scene, settings) - drift.mass[i] - drift.pending[i])
                        .max(0.0);
                let added = part.min(available);
                drift.pending[i] += added;
                accepted += f64::from(added);
            }
        }
        // Outflow leaves a full surface instead of being thrown above it again.
        self.removed_mass += (f64::from(mass) - accepted).max(0.0);
    }
}
