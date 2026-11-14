//! Independent one-step bursts with a verified, non-empty receiving workload.
use super::{Fixture, window};
use serde::Serialize;
use snowflakes_client::{
    geometry::{Point, Rect, Scene, Screen},
    snow::{MAX_FLAKES, World},
};
use snowflakes_protocol::Settings;

const ROOFS: usize = 8;
const ROOF_Y: f32 = 1800.0;
const FIRST_RECEIVER: u64 = 100;

pub(super) fn prepare(seed: u64) -> Result<Fixture, &'static str> {
    let mut scene = Scene::default();
    for i in 0..2 {
        scene.screens.push(Screen {
            key: i + 1,
            persistent_id: true,
            rect: Rect {
                left: i as f32 * 3840.0,
                right: (i + 1) as f32 * 3840.0,
                top: 0.0,
                bottom: 2160.0,
            },
            floor: 2160.0,
            scale: 2.0,
        });
    }
    // Sources are in front of receivers. Their detached layer must be able to
    // land on those receivers after the sources have actually been closed.
    for screen in &scene.screens {
        scene.windows.push(window(
            6 + screen.key,
            screen.rect.left + 100.0,
            200.0,
            400.0,
        ));
    }
    for screen in &scene.screens {
        for tile in 0..ROOFS {
            scene.windows.push(window(
                FIRST_RECEIVER + screen.key * 16 + tile as u64,
                screen.rect.left + tile as f32 * 480.0 + 16.0,
                ROOF_Y,
                448.0,
            ));
        }
    }
    let mut settings = Settings::default();
    settings.snow.intensity_percent = 0;
    settings.snow.drift_size_percent = 100;
    settings.snow.melting = false;
    settings.snow.on_ground = false;
    let mut world = World::new(scene.clone(), seed);
    for key in [7, 8] {
        let source = world.drifts.get_mut(&key).ok_or("contact_source_missing")?;
        source.mass.fill(source.cell_width * 0.2);
    }
    world.introduced_mass = world.total_mass();
    scene.windows.retain(|w| w.key >= FIRST_RECEIVER);
    world.set_scene(scene.clone());
    world.step(&settings, None);
    for screen in &scene.screens {
        let prototype = world
            .flakes
            .iter()
            .find(|b| {
                world
                    .scene
                    .nearest_screen(b.position)
                    .is_some_and(|s| s.key == screen.key)
            })
            .ok_or("contact_source_missing_body")?
            .clone();
        while world.release(prototype.clone()) {}
    }
    if world.flakes.len() != 2 * MAX_FLAKES || world.pending_collapse_count() != 0 {
        return Err("contact_fixture_not_saturated");
    }
    let mut slots = [0; 2];
    for body in &mut world.flakes {
        let screen = scene
            .screens
            .iter()
            .position(|s| s.rect.contains(body.position))
            .ok_or("contact_body_outside_source_display")?;
        let slot = slots[screen];
        slots[screen] += 1;
        let tile = slot % ROOFS;
        let lane = (slot / ROOFS) % 20;
        body.angle = ((slot % 7) as f32 * 0.04 - 0.12).rem_euclid(std::f32::consts::TAU);
        body.spin = if slot % 2 == 0 { 0.55 } else { -0.55 };
        body.velocity = Point::new((slot % 3) as f32 * 20.0 - 20.0, 240.0);
        let scale = body.display_scale(scene.screens[screen].scale);
        let slab = body.slab.as_ref().ok_or("contact_body_not_cohesive")?;
        let bottom = slab
            .contours()
            .flatten()
            .map(|local| body.transformed_point(local, scale).y - body.position.y)
            .fold(f32::NEG_INFINITY, f32::max);
        if !bottom.is_finite() {
            return Err("contact_body_invalid_contour");
        }
        let left = scene.screens[screen].rect.left + tile as f32 * 480.0 + 28.0;
        body.position = Point::new(
            left + 64.0 + lane as f32 * (296.0 / 19.0),
            ROOF_Y - 2.0 - bottom,
        );
    }
    Ok(Fixture { world, settings })
}

fn receivers(world: &World) -> f64 {
    world
        .drifts
        .values()
        .filter(|d| !d.support.ground)
        .map(|d| d.total_mass())
        .sum()
}

pub(super) struct Before {
    bodies: usize,
    active_mass: f64,
    receiver_mass: f64,
    removed_mass: f64,
}
impl Before {
    pub fn read(world: &World) -> Result<Self, &'static str> {
        let budget = world.budget_snapshot();
        let value = Self {
            bodies: world.flakes.len(),
            active_mass: world.flakes.iter().map(|b| f64::from(b.mass)).sum(),
            receiver_mass: receivers(world),
            removed_mass: world.removed_mass,
        };
        if value.bodies != 2 * MAX_FLAKES
            || budget.active_slabs != value.bodies
            || budget.pending_bodies != 0
            || budget.spill_bodies != 0
            || value.receiver_mass != 0.0
            || !value.active_mass.is_finite()
            || value.active_mass <= 0.0
        {
            return Err("contact_fixture_not_fresh");
        }
        Ok(value)
    }
    pub fn after(self, world: &World) -> Result<Sample, &'static str> {
        let reduction = self.bodies.saturating_sub(world.flakes.len());
        let received = receivers(world) - self.receiver_mass;
        let removed = world.removed_mass - self.removed_mass;
        let fraction = received / self.active_mass;
        let mass_error = (world.introduced_mass - world.removed_mass - world.total_mass()).abs()
            / world.introduced_mass.max(1.0);
        // Count reduction alone could be pure outflow. Require substantial net
        // material on the intended receivers, on every measured step.
        if reduction * 10 < self.bodies * 9
            || !fraction.is_finite()
            || fraction < 0.25
            || !removed.is_finite()
            || removed < 0.0
        {
            return Err("contact_load_missing");
        }
        if !mass_error.is_finite() || mass_error > 0.001 {
            return Err("model_mass_balance_failed");
        }
        Ok(Sample {
            starting_bodies: self.bodies,
            net_body_reduction: reduction,
            receiver_mass_gain: received,
            receiver_fraction: fraction,
            removed_mass: removed,
            mass_error,
        })
    }
}

pub(super) struct Sample {
    starting_bodies: usize,
    net_body_reduction: usize,
    receiver_mass_gain: f64,
    receiver_fraction: f64,
    removed_mass: f64,
    mass_error: f64,
}

#[derive(Serialize)]
pub(super) struct Activity {
    samples: usize,
    min_starting_bodies: usize,
    min_net_body_reduction: usize,
    total_net_body_reduction: usize,
    min_receiver_mass_gain: f64,
    max_receiver_mass_gain: f64,
    total_receiver_mass_gain: f64,
    min_receiver_fraction: f64,
    total_removed_mass: f64,
    max_mass_relative_error: f64,
}
impl Activity {
    pub fn new() -> Self {
        Self {
            samples: 0,
            min_starting_bodies: usize::MAX,
            min_net_body_reduction: usize::MAX,
            total_net_body_reduction: 0,
            min_receiver_mass_gain: f64::INFINITY,
            max_receiver_mass_gain: 0.0,
            total_receiver_mass_gain: 0.0,
            min_receiver_fraction: f64::INFINITY,
            total_removed_mass: 0.0,
            max_mass_relative_error: 0.0,
        }
    }
    pub fn record(&mut self, sample: Sample) {
        self.samples += 1;
        self.min_starting_bodies = self.min_starting_bodies.min(sample.starting_bodies);
        self.min_net_body_reduction = self.min_net_body_reduction.min(sample.net_body_reduction);
        self.total_net_body_reduction += sample.net_body_reduction;
        self.min_receiver_mass_gain = self.min_receiver_mass_gain.min(sample.receiver_mass_gain);
        self.max_receiver_mass_gain = self.max_receiver_mass_gain.max(sample.receiver_mass_gain);
        self.total_receiver_mass_gain += sample.receiver_mass_gain;
        self.min_receiver_fraction = self.min_receiver_fraction.min(sample.receiver_fraction);
        self.total_removed_mass += sample.removed_mass;
        self.max_mass_relative_error = self.max_mass_relative_error.max(sample.mass_error);
    }
    pub fn finish(&self, expected: usize) -> Result<(), &'static str> {
        if self.samples == 0 || self.samples != expected {
            return Err("contact_samples_missing");
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_fresh_contact_burst_proves_receiver_gain_and_balanced_material() {
        let mut activity = Activity::new();
        assert_eq!(activity.finish(1), Err("contact_samples_missing"));
        for seed in [1, 24301, 0xdead_beef] {
            let mut fixture = prepare(seed).unwrap();
            let before = Before::read(&fixture.world).unwrap();
            fixture.world.step(&fixture.settings, None);
            let sample = before.after(&fixture.world).unwrap();
            activity.record(sample);
            assert!(
                Before::read(&fixture.world).is_err(),
                "the empty second step was accepted as fresh"
            );
        }
        activity.finish(3).unwrap();
        assert_eq!(activity.min_starting_bodies, 4800);
        assert!(activity.min_net_body_reduction >= 4320);
        assert!(activity.min_receiver_mass_gain > 0.0);
        assert!(activity.min_receiver_fraction >= 0.25);
        assert!(activity.max_mass_relative_error <= 0.001);
        assert_eq!(activity.finish(4), Err("contact_samples_missing"));
    }

    #[test]
    fn contact_evidence_rejects_pure_outflow_even_if_every_body_disappears() {
        let mut fixture = prepare(24301).unwrap();
        let before = Before::read(&fixture.world).unwrap();
        fixture.world.clear();
        assert_eq!(fixture.world.flakes.len(), 0);
        assert!(fixture.world.removed_mass > 0.0);
        assert!(matches!(
            before.after(&fixture.world),
            Err("contact_load_missing")
        ));
    }

    #[test]
    fn disabled_or_paused_receivers_cannot_pass_the_contact_workload_gate() {
        for paused in [false, true] {
            let mut fixture = prepare(24301).unwrap();
            let before = Before::read(&fixture.world).unwrap();
            if paused {
                fixture.world.set_paused_monitors([1, 2].into());
            } else {
                fixture.settings.snow.on_windows = false;
            }
            fixture.world.step(&fixture.settings, None);
            assert!(matches!(
                before.after(&fixture.world),
                Err("contact_load_missing")
            ));
        }
    }
}
