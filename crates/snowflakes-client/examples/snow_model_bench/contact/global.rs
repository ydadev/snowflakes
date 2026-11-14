//! Global live-body limit with receiving evidence on every synthetic display.
use super::*;
use snowflakes_client::geometry::Point;
use std::collections::BTreeSet;

const SCREENS: usize = 8;
const PER_SCREEN: usize = GLOBAL_MAX_FLAKES / SCREENS;

pub(crate) fn prepare(seed: u64) -> Result<Fixture, &'static str> {
    prepare_layout(seed, SCREENS, PER_SCREEN)
}

pub(super) fn bodies(world: &World, key: u64) -> (usize, f64) {
    world
        .flakes
        .iter()
        .filter(|b| {
            world
                .scene
                .nearest_screen(b.position)
                .is_some_and(|s| s.key == key)
        })
        .fold((0, 0.0), |(count, mass), b| {
            (count + 1, mass + f64::from(b.mass))
        })
}

fn receivers_on(world: &World, key: u64) -> f64 {
    world
        .drifts
        .values()
        .filter(|d| {
            !d.support.ground
                && world
                    .scene
                    .nearest_screen(Point::new(
                        (d.support.left + d.support.right) * 0.5,
                        d.support.y,
                    ))
                    .is_some_and(|s| s.key == key)
        })
        .map(|d| d.total_mass())
        .sum()
}

pub(super) struct Before {
    key: u64,
    bodies: usize,
    active_mass: f64,
    receiver_mass: f64,
}
pub(super) fn before(world: &World) -> Result<Vec<Before>, &'static str> {
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
        return Err("contact_display_layout_changed");
    }
    world
        .scene
        .screens
        .iter()
        .map(|screen| {
            let (bodies, active_mass) = bodies(world, screen.key);
            let receiver_mass = receivers_on(world, screen.key);
            if bodies != PER_SCREEN
                || !active_mass.is_finite()
                || active_mass <= 0.0
                || receiver_mass != 0.0
            {
                return Err("contact_display_not_fresh_or_balanced");
            }
            Ok(Before {
                key: screen.key,
                bodies,
                active_mass,
                receiver_mass,
            })
        })
        .collect()
}

pub(super) struct Sample {
    pub key: u64,
    starting_bodies: usize,
    net_body_reduction: usize,
    receiver_mass_gain: f64,
    receiver_fraction: f64,
}
pub(super) fn after(before: Vec<Before>, world: &World) -> Result<Vec<Sample>, &'static str> {
    before
        .into_iter()
        .map(|before| {
            let reduction = before.bodies.saturating_sub(bodies(world, before.key).0);
            let received = receivers_on(world, before.key) - before.receiver_mass;
            let fraction = received / before.active_mass;
            if reduction * 10 < before.bodies * 9 || !fraction.is_finite() || fraction < 0.25 {
                return Err("contact_display_load_missing");
            }
            Ok(Sample {
                key: before.key,
                starting_bodies: before.bodies,
                net_body_reduction: reduction,
                receiver_mass_gain: received,
                receiver_fraction: fraction,
            })
        })
        .collect()
}

#[derive(Serialize)]
pub(super) struct Activity {
    pub key: u64,
    pub samples: usize,
    min_starting_bodies: usize,
    min_net_body_reduction: usize,
    min_receiver_fraction: f64,
    total_receiver_mass_gain: f64,
}
impl Activity {
    pub fn new(key: u64) -> Self {
        Self {
            key,
            samples: 0,
            min_starting_bodies: usize::MAX,
            min_net_body_reduction: usize::MAX,
            min_receiver_fraction: f64::INFINITY,
            total_receiver_mass_gain: 0.0,
        }
    }
    pub fn record(&mut self, sample: Sample) {
        self.samples += 1;
        self.min_starting_bodies = self.min_starting_bodies.min(sample.starting_bodies);
        self.min_net_body_reduction = self.min_net_body_reduction.min(sample.net_body_reduction);
        self.min_receiver_fraction = self.min_receiver_fraction.min(sample.receiver_fraction);
        self.total_receiver_mass_gain += sample.receiver_mass_gain;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn global_contact_bursts_fill_the_pool_and_land_on_every_display() {
        let mut activity = super::super::Activity::new();
        for seed in [24301, 17] {
            let mut fixture = prepare(seed).unwrap();
            let before = super::super::Before::read_global(&fixture.world).unwrap();
            fixture.world.step(&fixture.settings, None);
            activity
                .record(before.after(&fixture.world).unwrap())
                .unwrap();
        }
        activity.finish(2).unwrap();
        assert_eq!(activity.min_starting_bodies, 16000);
        assert_eq!(activity.per_screen.len(), 8);
        for screen in activity.per_screen {
            assert_eq!(screen.samples, 2);
            assert_eq!(screen.min_starting_bodies, 2000);
            assert!(screen.min_net_body_reduction >= 1800 && screen.min_receiver_fraction >= 0.25);
        }
    }

    #[test]
    fn global_contact_fixture_rejects_a_skewed_display_even_at_the_global_count() {
        let mut fixture = prepare(24301).unwrap();
        for body in &mut fixture.world.flakes {
            if body.position.x >= 7.0 * 3840.0 {
                body.position.x -= 7.0 * 3840.0;
            }
        }
        assert_eq!(fixture.world.flakes.len(), 16000);
        assert!(matches!(
            super::super::Before::read_global(&fixture.world),
            Err("contact_display_not_fresh_or_balanced")
        ));
    }

    #[test]
    fn global_receiver_evidence_rejects_accounted_outflow_on_one_display() {
        let mut fixture = prepare(24301).unwrap();
        let before = super::super::Before::read_global(&fixture.world).unwrap();
        fixture.world.step(&fixture.settings, None);
        let mut removed = 0.0;
        for drift in fixture
            .world
            .drifts
            .values_mut()
            .filter(|d| !d.support.ground && d.support.left < 3840.0)
        {
            removed += drift.total_mass();
            drift.mass.fill(0.0);
            drift.pending.fill(0.0);
        }
        fixture.world.removed_mass += removed;
        assert!(removed > 0.0 && fixture.world.flakes.is_empty());
        assert!(matches!(
            before.after(&fixture.world),
            Err("contact_display_load_missing")
        ));
    }

    #[test]
    fn a_paused_display_cannot_pass_the_global_contact_load() {
        let mut fixture = prepare(24301).unwrap();
        let before = super::super::Before::read_global(&fixture.world).unwrap();
        fixture.world.set_paused_monitors([8].into());
        fixture.world.step(&fixture.settings, None);
        assert!(before.after(&fixture.world).is_err());
    }
}
