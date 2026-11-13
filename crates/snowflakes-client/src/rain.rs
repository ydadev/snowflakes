//! Fixed-step rain. Impact receipts carry water volume for the accumulation stage;
//! short-lived spray is decorative and never duplicates that volume.
use crate::{
    geometry::{Point, Scene, Support},
    quality::RenderQuality,
};
use snowflakes_protocol::Settings;
use std::collections::{BTreeMap, BTreeSet};

pub const DT: f32 = 1.0 / 60.0;
pub const MAX_DROPS: usize = 4096;
pub const MAX_SPLASHES: usize = 768;
const BUCKET: f32 = 128.0;

#[derive(Clone, Debug, PartialEq)]
pub struct Drop {
    pub position: Point,
    /// DIP per second, independent of the monitor scale and render frequency.
    pub velocity: Point,
    pub volume: f32,
    pub width: f32,
    pub length: f32,
    pub opacity: f32,
    pub terminal_speed: f32,
    pub age: f32,
}
impl Drop {
    pub fn new(position: Point) -> Self {
        Self {
            position,
            velocity: Point::new(0.0, 620.0),
            volume: 1.0,
            width: 1.0,
            length: 14.0,
            opacity: 0.6,
            terminal_speed: 720.0,
            age: 0.0,
        }
    }
    fn integrate(&mut self, wind: f32, scale: f32) {
        // Stable linear drag toward a size-dependent terminal fall speed.
        let damping = 1.0 - (-3.5 * DT).exp();
        self.velocity.x += (wind - self.velocity.x) * damping;
        self.velocity.y += (self.terminal_speed - self.velocity.y) * damping;
        self.position.x += self.velocity.x * scale * DT;
        self.position.y += self.velocity.y * scale * DT;
        self.age += DT;
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Splash {
    pub position: Point,
    pub velocity: Point,
    pub age: f32,
    pub lifetime: f32,
}
#[derive(Clone, Debug, PartialEq)]
pub struct Impact {
    pub support: u64,
    pub position: Point,
    pub volume: f32,
    pub speed: f32,
}
struct Rng(u64);
impl Rng {
    fn unit(&mut self) -> f32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 >> 40) as f32 / 16_777_216.0
    }
}

pub struct World {
    pub scene: Scene,
    pub drops: Vec<Drop>,
    pub splashes: Vec<Splash>,
    /// Exactly the current physical step's impacts, never a render-frame backlog.
    pub impacts: Vec<Impact>,
    pub introduced_volume: f64,
    pub impacted_volume: f64,
    pub removed_volume: f64,
    pub suppressed_drops: u64,
    supports: Vec<Support>,
    buckets: BTreeMap<i32, Vec<usize>>,
    emission: BTreeMap<u64, f64>,
    paused: BTreeSet<u64>,
    rng: Rng,
    spray_rng: Rng,
}
impl World {
    pub fn new(scene: Scene, seed: u64) -> Self {
        let mut world = Self {
            scene: Scene::default(),
            drops: Vec::new(),
            splashes: Vec::new(),
            impacts: Vec::new(),
            introduced_volume: 0.0,
            impacted_volume: 0.0,
            removed_volume: 0.0,
            suppressed_drops: 0,
            supports: Vec::new(),
            buckets: BTreeMap::new(),
            emission: BTreeMap::new(),
            paused: BTreeSet::new(),
            rng: Rng(seed.max(1)),
            spray_rng: Rng((seed ^ 0x6a09_e667_f3bc_c909).max(1)),
        };
        world.set_scene(scene);
        world
    }
    pub fn airborne_volume(&self) -> f64 {
        self.drops.iter().map(|d| f64::from(d.volume)).sum()
    }
    pub fn release(&mut self, drop: Drop) -> bool {
        if self.drops.len() == MAX_DROPS {
            return false;
        }
        self.introduced_volume += f64::from(drop.volume);
        self.drops.push(drop);
        true
    }
    pub fn clear(&mut self) {
        self.removed_volume += self.airborne_volume();
        self.drops.clear();
        self.splashes.clear();
        self.impacts.clear();
        self.emission.clear();
    }
    pub fn set_paused_monitors(&mut self, paused: BTreeSet<u64>) {
        self.paused = paused;
    }
    pub fn set_scene(&mut self, scene: Scene) {
        let mut supports = scene.supports();
        for support in &mut supports {
            if support.ground {
                let id = support.key & !(1 << 63);
                if let Some(screen) = scene.screens.iter().find(|s| s.key == id)
                    && self
                        .scene
                        .screens
                        .iter()
                        .any(|old| old.key == id && old.rect == screen.rect)
                    && let Some(old) = self.supports.iter().find(|old| old.key == support.key)
                {
                    *support = scene.ground_support(screen, old.y);
                }
            }
        }
        self.emission
            .retain(|id, _| scene.screens.iter().any(|s| s.key == *id));
        self.drops.retain(|drop| {
            if scene.screens.iter().any(|s| s.rect.contains(drop.position)) {
                true
            } else {
                self.removed_volume += f64::from(drop.volume);
                false
            }
        });
        self.splashes.retain(|splash| {
            scene
                .screens
                .iter()
                .any(|s| s.rect.contains(splash.position))
        });
        self.supports = supports;
        self.scene = scene;
        self.rebuild_buckets();
    }
    fn rebuild_buckets(&mut self) {
        self.buckets.clear();
        for (index, support) in self.supports.iter().enumerate() {
            self.buckets
                .entry((support.y / BUCKET).floor() as i32)
                .or_default()
                .push(index);
        }
    }
    fn advance_ground(&mut self) {
        let mut changed_bucket = false;
        for support in &mut self.supports {
            if !support.ground {
                continue;
            }
            let id = support.key & !(1 << 63);
            if self.paused.contains(&id) {
                continue;
            }
            if let Some(screen) = self.scene.screens.iter().find(|s| s.key == id) {
                let y = crate::geometry::approach_floor(support.y, screen.floor, screen.scale, DT);
                if y != support.y {
                    changed_bucket |= (y / BUCKET).floor() != (support.y / BUCKET).floor();
                    *support = self.scene.ground_support(screen, y);
                }
            }
        }
        if changed_bucket {
            self.rebuild_buckets();
        }
    }
    fn contact(&self, before: Point, after: Point) -> Option<(u64, Point)> {
        let dy = after.y - before.y;
        if dy <= 0.0 {
            return None;
        }
        let mut nearest: Option<(u64, Point)> = None;
        for (_, indices) in self
            .buckets
            .range((before.y / BUCKET).floor() as i32..=(after.y / BUCKET).floor() as i32)
        {
            for &index in indices {
                let support = &self.supports[index];
                if support.y < before.y || support.y > after.y {
                    continue;
                }
                let t = (support.y - before.y) / dy;
                let p = Point::new(before.x + (after.x - before.x) * t, support.y);
                if support.visible_at(p.x)
                    && support.exposed(p)
                    && nearest.as_ref().is_none_or(|(_, old)| p.y < old.y)
                {
                    nearest = Some((support.key, p));
                }
            }
        }
        nearest
    }
    fn emit(&mut self, settings: &Settings) {
        for screen in self.scene.screens.clone() {
            if self.paused.contains(&screen.key) {
                continue;
            }
            let scale = screen.scale.clamp(0.5, 4.0);
            let fraction = self.emission.entry(screen.key).or_default();
            *fraction += f64::from(screen.rect.width() / scale / 1920.0)
                * f64::from(settings.rain.intensity_percent)
                / 100.0
                * 480.0
                / 60.0;
            let due = fraction.floor() as usize;
            *fraction -= due as f64;
            // A saturated visual budget cannot turn into delayed rain after intensity=0.
            let count = due.min(MAX_DROPS - self.drops.len());
            self.suppressed_drops += (due - count) as u64;
            for _ in 0..count {
                let position = Point::new(
                    screen.rect.left + self.rng.unit() * screen.rect.width(),
                    screen.rect.top + 0.01,
                );
                let mut drop = Drop::new(position);
                drop.width = 0.65 + self.rng.unit() * 0.65;
                drop.volume = drop.width * drop.width;
                drop.length = 9.0 + self.rng.unit() * 12.0;
                drop.opacity = 0.38 + self.rng.unit() * 0.35;
                drop.terminal_speed = 560.0 + self.rng.unit() * 360.0;
                drop.velocity = Point::new(
                    settings.rain.wind_percent as f32 * 2.7,
                    drop.terminal_speed * 0.85,
                );
                self.release(drop);
            }
        }
    }
    fn spray(&mut self, impact: &Impact, reduced: bool) {
        let limit = if reduced {
            MAX_SPLASHES / 4
        } else {
            MAX_SPLASHES
        };
        let count = if reduced { 1 } else { 3 };
        for _ in 0..count.min(limit.saturating_sub(self.splashes.len())) {
            let velocity = Point::new(
                (self.spray_rng.unit() * 2.0 - 1.0) * 65.0,
                -(28.0 + self.spray_rng.unit() * 60.0),
            );
            let scale = self
                .scene
                .scale_at(Point::new(impact.position.x, impact.position.y - 0.1));
            self.splashes.push(Splash {
                position: Point::new(impact.position.x, impact.position.y - scale),
                velocity,
                age: 0.0,
                lifetime: 0.18 + self.spray_rng.unit() * 0.22,
            });
        }
    }
    pub fn step(&mut self, settings: &Settings, quality: RenderQuality) {
        self.impacts.clear();
        if self
            .scene
            .screens
            .iter()
            .all(|s| self.paused.contains(&s.key))
        {
            return;
        }
        self.advance_ground();
        let reduced = !quality.secondary_details;
        self.splashes.truncate(if reduced {
            MAX_SPLASHES / 4
        } else {
            MAX_SPLASHES
        });
        self.splashes.retain_mut(|splash| {
            if self.scene.paused_at(splash.position, &self.paused) {
                return true;
            }
            let scale = self.scene.scale_at(splash.position);
            let before = splash.clone();
            splash.velocity.y += 460.0 * DT;
            splash.position.x += splash.velocity.x * scale * DT;
            splash.position.y += splash.velocity.y * scale * DT;
            if self.scene.paused_at(splash.position, &self.paused) {
                *splash = before;
                return true;
            }
            splash.age += DT;
            splash.age < splash.lifetime
                && self
                    .scene
                    .screens
                    .iter()
                    .any(|s| s.rect.contains(splash.position))
        });
        self.emit(settings);
        let mut survivors = Vec::with_capacity(self.drops.len());
        for mut drop in std::mem::take(&mut self.drops) {
            if self.scene.paused_at(drop.position, &self.paused) {
                survivors.push(drop);
                continue;
            }
            let before = drop.clone();
            drop.integrate(
                settings.rain.wind_percent as f32 * 2.7,
                self.scene.scale_at(drop.position),
            );
            let impact = self.contact(before.position, drop.position);
            let target = impact.as_ref().map_or(drop.position, |(_, point)| *point);
            if self.scene.paused_at(target, &self.paused) {
                survivors.push(before);
                continue;
            }
            if let Some((support, position)) = impact {
                let impact = Impact {
                    support,
                    position,
                    volume: drop.volume,
                    speed: drop.velocity.x.hypot(drop.velocity.y),
                };
                self.impacted_volume += f64::from(drop.volume);
                self.spray(&impact, reduced);
                self.impacts.push(impact);
            } else if drop.age > 15.0
                || !self
                    .scene
                    .screens
                    .iter()
                    .any(|s| s.rect.contains(drop.position))
            {
                self.removed_volume += f64::from(drop.volume);
            } else {
                survivors.push(drop);
            }
        }
        self.drops = survivors;
    }
}
