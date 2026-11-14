//! One world over consecutive steps; external cohesive sources refill vacant slots.
use super::{Fixture, contact};
use serde::Serialize;
use snowflakes_client::{
    geometry::Point,
    snow::{Flake, GLOBAL_MAX_FLAKES, MAX_FLAKES, World},
};
use std::collections::BTreeSet;

const SCREENS: usize = 8;
const ROOFS: usize = 8;
const ROOF_Y: f32 = 1800.0;
pub(super) const PER_SCREEN: usize = GLOBAL_MAX_FLAKES / SCREENS;
pub(super) const MIN_STEPS: usize = 360;
pub(super) const FALL_DISTANCE: f32 = 1000.0;

fn screen_counts(world: &World) -> Result<[usize; SCREENS], &'static str> {
    if world.scene.screens.len() != SCREENS
        || world
            .scene
            .screens
            .iter()
            .map(|s| s.key)
            .collect::<BTreeSet<_>>()
            .len()
            != SCREENS
    {
        return Err("stream_layout_changed");
    }
    let mut counts = [0; SCREENS];
    for body in &world.flakes {
        let key = world
            .scene
            .nearest_screen(body.position)
            .ok_or("stream_body_outside_layout")?
            .key;
        let index = world
            .scene
            .screens
            .iter()
            .position(|s| s.key == key)
            .ok_or("stream_layout_changed")?;
        counts[index] += 1;
    }
    Ok(counts)
}

pub(super) fn prepare(seed: u64, per_screen: usize) -> Result<Fixture, &'static str> {
    if !(1..=PER_SCREEN).contains(&per_screen) {
        return Err("stream_target_invalid");
    }
    let mut fixture = contact::prepare_layout(seed, SCREENS, per_screen)?;
    let mut slots = [0; SCREENS];
    for body in &mut fixture.world.flakes {
        let key = fixture
            .world
            .scene
            .nearest_screen(body.position)
            .ok_or("stream_body_outside_layout")?
            .key;
        let index = fixture
            .world
            .scene
            .screens
            .iter()
            .position(|s| s.key == key)
            .ok_or("stream_layout_changed")?;
        slots[index] += 1;
        body.position.y -= FALL_DISTANCE * slots[index] as f32 / per_screen as f32;
    }
    Ok(fixture)
}

pub(super) struct Driver {
    keys: [u64; SCREENS],
    templates: Vec<Flake>,
    slots: [usize; SCREENS],
    target: usize,
}
pub(super) struct Admission {
    before: [usize; SCREENS],
    added: [usize; SCREENS],
}
impl Driver {
    pub fn new(world: &World, target: usize) -> Result<Self, &'static str> {
        if !(1..=MAX_FLAKES).contains(&target)
            || target * SCREENS > GLOBAL_MAX_FLAKES
            || screen_counts(world)? != [target; SCREENS]
        {
            return Err("stream_initial_population_invalid");
        }
        let mut templates = Vec::new();
        for screen in &world.scene.screens {
            let template = world
                .flakes
                .iter()
                .find(|b| {
                    world
                        .scene
                        .nearest_screen(b.position)
                        .is_some_and(|s| s.key == screen.key)
                })
                .ok_or("stream_template_missing")?
                .clone();
            if template.slab.is_none() {
                return Err("stream_template_not_cohesive");
            }
            // Clones keep their real closed source's live mask and material
            // admission class. No synthetic handle or budget bypass is used.
            if world
                .falling_clips(&template, template.draw_bounds(screen.scale))
                .is_none()
            {
                return Err("stream_source_mask_expired");
            }
            templates.push(template);
        }
        Ok(Self {
            keys: std::array::from_fn(|i| world.scene.screens[i].key),
            templates,
            slots: [0; SCREENS],
            target,
        })
    }
    pub fn refill(&mut self, world: &mut World) -> Result<Admission, &'static str> {
        let mut counts = screen_counts(world)?;
        if world.scene.screens.iter().map(|s| s.key).ne(self.keys) {
            return Err("stream_layout_changed");
        }
        let mut added = [0; SCREENS];
        for index in 0..SCREENS {
            if world
                .falling_clips(
                    &self.templates[index],
                    self.templates[index].draw_bounds(world.scene.screens[index].scale),
                )
                .is_none()
            {
                return Err("stream_source_mask_expired");
            }
            if counts[index] > self.target {
                return Err("stream_population_exceeded");
            }
            while counts[index] < self.target {
                let slot = self.slots[index];
                let mut body = self.templates[index].clone();
                body.age = 0.0;
                body.velocity = Point::new((slot % 3) as f32 * 20.0 - 20.0, 240.0);
                body.angle = ((slot % 7) as f32 * 0.04 - 0.12).rem_euclid(std::f32::consts::TAU);
                body.spin = if slot.is_multiple_of(2) { 0.55 } else { -0.55 };
                let screen = &world.scene.screens[index];
                let scale = body.display_scale(screen.scale);
                let bottom = body
                    .slab
                    .as_ref()
                    .ok_or("stream_template_not_cohesive")?
                    .contours()
                    .flatten()
                    .map(|p| body.transformed_point(p, scale).y - body.position.y)
                    .fold(f32::NEG_INFINITY, f32::max);
                body.position = Point::new(
                    screen.rect.left
                        + (slot % ROOFS) as f32 * 480.0
                        + 92.0
                        + ((slot / ROOFS) % 20) as f32 * (296.0 / 19.0),
                    ROOF_Y - FALL_DISTANCE - bottom,
                );
                if !world.release(body) {
                    return Err("stream_admission_failed");
                }
                self.slots[index] += 1;
                counts[index] += 1;
                added[index] += 1;
            }
        }
        Ok(Admission {
            before: counts,
            added,
        })
    }
}

fn receiver_mass(world: &World) -> [f64; SCREENS] {
    let mut mass = [0.0; SCREENS];
    for drift in world.drifts.values().filter(|d| !d.support.ground) {
        let point = Point::new(
            (drift.support.left + drift.support.right) * 0.5,
            drift.support.y,
        );
        if let Some(index) = world.scene.nearest_screen(point).and_then(|s| {
            world
                .scene
                .screens
                .iter()
                .position(|screen| screen.key == s.key)
        }) {
            mass[index] += drift.total_mass();
        }
    }
    mass
}

#[derive(Serialize)]
pub(super) struct Activity {
    samples: usize,
    target_per_screen: usize,
    min_starting_bodies: usize,
    min_after_step_bodies: usize,
    max_mass_relative_error: f64,
    per_screen: Vec<ScreenActivity>,
    #[serde(skip)]
    last_receiver_mass: [f64; SCREENS],
}
#[derive(Serialize)]
struct ScreenActivity {
    key: u64,
    min_starting_bodies: usize,
    admitted_bodies: usize,
    receiving_steps: usize,
    positive_net_receiver_gain: f64,
    initial_receiver_mass: f64,
    final_receiver_mass: f64,
}
impl Activity {
    pub fn new(world: &World, target: usize) -> Result<Self, &'static str> {
        screen_counts(world)?;
        if !(1..=PER_SCREEN).contains(&target) {
            return Err("stream_target_invalid");
        }
        let mass = receiver_mass(world);
        Ok(Self {
            samples: 0,
            target_per_screen: target,
            min_starting_bodies: usize::MAX,
            min_after_step_bodies: usize::MAX,
            max_mass_relative_error: 0.0,
            per_screen: world
                .scene
                .screens
                .iter()
                .enumerate()
                .map(|(i, s)| ScreenActivity {
                    key: s.key,
                    min_starting_bodies: usize::MAX,
                    admitted_bodies: 0,
                    receiving_steps: 0,
                    positive_net_receiver_gain: 0.0,
                    initial_receiver_mass: mass[i],
                    final_receiver_mass: mass[i],
                })
                .collect(),
            last_receiver_mass: mass,
        })
    }
    pub fn record(&mut self, world: &World, admission: Admission) -> Result<(), &'static str> {
        let budget = world.budget_snapshot();
        if budget.active_bodies > budget.global_max_active_bodies
            || budget.pending_packets > budget.max_pending_packets
            || budget.pending_bodies > budget.max_pending_bodies
        {
            return Err("model_count_bound_failed");
        }
        let error = (world.introduced_mass - world.removed_mass - world.total_mass()).abs()
            / world.introduced_mass.max(1.0);
        if !error.is_finite() || error > 0.001 {
            return Err("model_mass_balance_failed");
        }
        let mass = receiver_mass(world);
        self.samples += 1;
        self.min_starting_bodies = self.min_starting_bodies.min(admission.before.iter().sum());
        self.min_after_step_bodies = self.min_after_step_bodies.min(world.flakes.len());
        self.max_mass_relative_error = self.max_mass_relative_error.max(error);
        for (index, item) in self.per_screen.iter_mut().enumerate() {
            item.min_starting_bodies = item.min_starting_bodies.min(admission.before[index]);
            item.admitted_bodies += admission.added[index];
            let gain = mass[index] - self.last_receiver_mass[index];
            if gain > 0.0 {
                item.receiving_steps += 1;
                item.positive_net_receiver_gain += gain;
            }
            item.final_receiver_mass = mass[index];
        }
        self.last_receiver_mass = mass;
        Ok(())
    }
    pub fn finish(&self, steps: usize) -> Result<(), &'static str> {
        if steps < MIN_STEPS
            || self.samples != steps
            || self.per_screen.len() != SCREENS
            || self.min_starting_bodies != self.target_per_screen * SCREENS
            || self.per_screen.iter().any(|s| {
                s.min_starting_bodies != self.target_per_screen
                    || s.admitted_bodies == 0
                    || s.receiving_steps == 0
                    || !s.positive_net_receiver_gain.is_finite()
                    || s.positive_net_receiver_gain <= 0.0
            })
        {
            return Err("stream_continuity_missing");
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Scenario, parse};

    #[test]
    fn stream_arguments_require_consecutive_work_and_the_full_fixture_is_balanced() {
        for steps in ["60", "359"] {
            assert_eq!(
                parse(["--scenario", "contact-stream", "--steps", steps].map(str::to_owned)).err(),
                Some("stream_steps_too_short")
            );
        }
        let options = parse(
            [
                "--scenario",
                "contact-stream",
                "--steps",
                "360",
                "--warmup",
                "0",
            ]
            .map(str::to_owned),
        )
        .unwrap()
        .unwrap();
        assert_eq!(options.scenario, Scenario::ContactStream);
        assert_eq!(options.scenario.episode_steps(), None);
        assert!(!options.scenario.is_contact());
        let fixture = prepare(0x5eed, PER_SCREEN).unwrap();
        assert_eq!(screen_counts(&fixture.world).unwrap(), [2000; SCREENS]);
        assert_eq!(fixture.world.flakes.len(), GLOBAL_MAX_FLAKES);
        assert!(fixture.world.flakes.iter().all(|b| b.slab.is_some()));
    }

    #[test]
    fn consecutive_steps_keep_old_receivers_and_admit_new_material_on_every_screen() {
        let mut fixture = prepare(0x5eed, 16).unwrap();
        let mut driver = Driver::new(&fixture.world, 16).unwrap();
        let mut activity = Activity::new(&fixture.world, 16).unwrap();
        let initial_mass = fixture.world.introduced_mass;
        for _ in 0..MIN_STEPS {
            let admission = driver.refill(&mut fixture.world).unwrap();
            assert_eq!(screen_counts(&fixture.world).unwrap(), [16; SCREENS]);
            if admission.added.iter().sum::<usize>() > 0 {
                for body in fixture.world.flakes.iter().filter(|b| b.age == 0.0) {
                    let scale = body.display_scale(2.0);
                    let bottom = body
                        .slab
                        .as_ref()
                        .unwrap()
                        .contours()
                        .flatten()
                        .map(|p| body.transformed_point(p, scale).y)
                        .fold(f32::NEG_INFINITY, f32::max);
                    assert!((bottom - (ROOF_Y - FALL_DISTANCE)).abs() < 0.001);
                }
            }
            fixture.world.step(&fixture.settings, None);
            activity.record(&fixture.world, admission).unwrap();
        }
        activity.finish(MIN_STEPS).unwrap();
        assert_eq!(activity.samples, MIN_STEPS);
        assert_eq!(activity.min_starting_bodies, 128);
        assert!(activity.min_after_step_bodies > 0);
        assert!(fixture.world.introduced_mass > initial_mass);
        for screen in &activity.per_screen {
            assert_eq!(screen.min_starting_bodies, 16);
            assert_eq!(screen.initial_receiver_mass, 0.0);
            assert!(screen.final_receiver_mass > 0.0);
            assert!(screen.admitted_bodies > 0 && screen.receiving_steps > 1);
        }
    }

    #[test]
    fn outflow_without_receiving_snow_cannot_prove_a_contact_stream() {
        let mut fixture = prepare(0x5eed, 16).unwrap();
        fixture.settings.snow.on_windows = false;
        let mut driver = Driver::new(&fixture.world, 16).unwrap();
        let mut activity = Activity::new(&fixture.world, 16).unwrap();
        for _ in 0..MIN_STEPS {
            let admission = driver.refill(&mut fixture.world).unwrap();
            fixture.world.step(&fixture.settings, None);
            activity.record(&fixture.world, admission).unwrap();
        }
        assert!(fixture.world.removed_mass > 0.0);
        assert_eq!(activity.finish(MIN_STEPS), Err("stream_continuity_missing"));
    }

    #[test]
    fn a_finite_initial_burst_without_refills_cannot_pass_as_continuous() {
        let mut fixture = prepare(0x5eed, 16).unwrap();
        let mut activity = Activity::new(&fixture.world, 16).unwrap();
        for _ in 0..MIN_STEPS {
            let admission = Admission {
                before: screen_counts(&fixture.world).unwrap(),
                added: [0; SCREENS],
            };
            fixture.world.step(&fixture.settings, None);
            activity.record(&fixture.world, admission).unwrap();
        }
        assert!(activity.per_screen.iter().any(|s| s.receiving_steps > 0));
        assert_eq!(activity.finish(MIN_STEPS), Err("stream_continuity_missing"));
    }

    #[test]
    fn driver_rejects_changed_layout_and_never_discards_an_excess_body() {
        let mut fixture = prepare(0x5eed, 16).unwrap();
        let mut driver = Driver::new(&fixture.world, 16).unwrap();
        fixture.world.scene.screens.swap(0, 1);
        assert_eq!(
            driver.refill(&mut fixture.world).err(),
            Some("stream_layout_changed")
        );
        fixture.world.scene.screens.swap(0, 1);
        assert!(fixture.world.release(fixture.world.flakes[0].clone()));
        assert_eq!(
            driver.refill(&mut fixture.world).err(),
            Some("stream_population_exceeded")
        );
        assert_eq!(fixture.world.flakes.len(), 129);
        fixture.world.clear();
        assert_eq!(
            driver.refill(&mut fixture.world).err(),
            Some("stream_source_mask_expired")
        );
    }

    #[test]
    fn mass_corruption_and_short_or_missing_observations_are_rejected() {
        let mut fixture = prepare(0x5eed, 16).unwrap();
        let mut driver = Driver::new(&fixture.world, 16).unwrap();
        let mut activity = Activity::new(&fixture.world, 16).unwrap();
        assert_eq!(activity.finish(MIN_STEPS), Err("stream_continuity_missing"));
        let admission = driver.refill(&mut fixture.world).unwrap();
        fixture.world.step(&fixture.settings, None);
        activity.record(&fixture.world, admission).unwrap();
        assert_eq!(activity.finish(1), Err("stream_continuity_missing"));
        assert_eq!(activity.finish(MIN_STEPS), Err("stream_continuity_missing"));
        fixture.world.introduced_mass += 1e6;
        let admission = driver.refill(&mut fixture.world).unwrap();
        assert_eq!(
            activity.record(&fixture.world, admission),
            Err("model_mass_balance_failed")
        );
    }
}
