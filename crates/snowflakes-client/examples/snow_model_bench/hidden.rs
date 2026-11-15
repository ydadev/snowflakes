//! Continuous outflow attributed to original source layers, including display migration.
use super::{Fixture, stream};
use serde::Serialize;
use snowflakes_client::snow::World;
use stream::SCREENS;

pub(super) fn prepare(seed: u64, target: usize) -> Result<Fixture, &'static str> {
    let mut fixture = stream::prepare_with_visibility(seed, target, false)?;
    fixture.settings.interaction.cursor_enabled = true;
    Ok(fixture)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Scenario, StateHasher, cursor, parse};
    fn fingerprint(world: &World) -> u64 {
        let mut h = StateHasher::new();
        h.append(world, world.budget_snapshot());
        h.0
    }

    #[test]
    fn hidden_arguments_and_full_pool_preserve_actual_closed_source_masks() {
        assert!(
            parse(["--scenario", "hidden-stream", "--steps", "359"].map(str::to_owned)).is_err()
        );
        let options = parse(
            [
                "--scenario",
                "hidden-stream",
                "--steps",
                "360",
                "--cursor",
                "disabled",
            ]
            .map(str::to_owned),
        )
        .unwrap()
        .unwrap();
        assert_eq!(options.scenario, Scenario::HiddenStream);
        assert!(!options.cursor_enabled);
        let fixture = prepare(0x5eed, stream::PER_SCREEN).unwrap();
        assert!(fixture.settings.interaction.cursor_enabled);
        let budget = fixture.world.budget_snapshot();
        assert_eq!(budget.active_bodies, 16_000);
        assert_eq!(budget.falling_masks, 8);
        Activity::new(&fixture.world, stream::PER_SCREEN).unwrap();
        assert!(fixture.world.flakes.iter().all(|b| {
            fixture
                .world
                .falling_clips(b, fixture.world.scene.bounds())
                .is_some_and(|c| c.is_empty())
        }));
        for step in 0..24 {
            let input = cursor::Probe::read_hidden(&fixture.world, step)
                .unwrap()
                .input(true)
                .unwrap();
            if step % 3 == 0 {
                assert_eq!(input.position.y, -10_000.0);
            } else {
                assert!((850.0..1450.0).contains(&input.position.y));
            }
            if step % 3 == 1 {
                assert_eq!(input.velocity.x, 0.0);
                assert_eq!(input.velocity.y, 0.0);
            }
        }
    }

    #[test]
    fn consecutive_hidden_input_matches_control_and_exits_from_every_source_without_mounds() {
        let (mut actual, mut control) =
            (prepare(0x5eed, 128).unwrap(), prepare(0x5eed, 128).unwrap());
        let mut driver = stream::Driver::new(&actual.world, 128).unwrap();
        let mut control_driver = stream::Driver::new(&control.world, 128).unwrap();
        let mut activity = Activity::new(&actual.world, 128).unwrap();
        let mut cursor_activity = cursor::Activity::hidden(true);
        for step in 0..120 {
            driver.refill(&mut actual.world).unwrap();
            control_driver.refill(&mut control.world).unwrap();
            let probe = cursor::Probe::read_hidden(&actual.world, step).unwrap();
            actual.world.step(&actual.settings, probe.input(true));
            control.world.step(&control.settings, None);
        }
        assert_eq!(
            activity.samples, 0,
            "warmup observations entered the measured ledger"
        );
        assert_eq!(fingerprint(&actual.world), fingerprint(&control.world));
        for step in 0..stream::MIN_STEPS {
            let admission = driver.refill(&mut actual.world).unwrap();
            control_driver.refill(&mut control.world).unwrap();
            let before = activity.before(&actual.world).unwrap();
            let probe = cursor::Probe::read_hidden(&actual.world, step).unwrap();
            actual.world.step(&actual.settings, probe.input(true));
            control.world.step(&control.settings, None);
            activity.record(&actual.world, admission, before).unwrap();
            cursor_activity.record(probe, 1);
            if step % 30 == 0 || step + 1 == stream::MIN_STEPS {
                assert_eq!(fingerprint(&actual.world), fingerprint(&control.world));
            }
        }
        activity.finish(stream::MIN_STEPS).unwrap();
        cursor_activity.finish(stream::MIN_STEPS).unwrap();
        assert!(actual.world.removed_mass > 0.0);
        assert!(
            activity
                .per_source
                .iter()
                .all(|s| s.outflow_bodies > 0 && s.outflow_mass > 0.0)
        );
        activity.per_source[0].outflow_mass = 0.0;
        assert_eq!(
            activity.finish(stream::MIN_STEPS),
            Err("hidden_continuity_missing")
        );
    }

    #[test]
    fn visible_sources_corrupt_ledgers_and_deposited_mass_cannot_prove_hidden_outflow() {
        let visible = stream::prepare(0x5eed, 128).unwrap();
        assert_eq!(
            Activity::new(&visible.world, 128).err(),
            Some("hidden_source_not_hidden")
        );
        assert_eq!(
            cursor::Probe::read_hidden(&visible.world, 1).err(),
            Some("hidden_cursor_target_visible")
        );
        for corrupt_ledger in [false, true] {
            let mut f = prepare(0x5eed, 128).unwrap();
            let mut activity = Activity::new(&f.world, 128).unwrap();
            assert_eq!(activity.finish(360), Err("hidden_continuity_missing"));
            let mut driver = stream::Driver::new(&f.world, 128).unwrap();
            let admission = driver.refill(&mut f.world).unwrap();
            let before = activity.before(&f.world).unwrap();
            f.world.step(&f.settings, None);
            if corrupt_ledger {
                f.world.removed_mass += 1000.0;
            } else {
                f.world.flakes.pop();
            }
            assert_eq!(
                activity.record(&f.world, admission, before),
                Err("hidden_removed_mass_mismatch")
            );
        }
        let mut f = prepare(0x5eed, 128).unwrap();
        let activity = Activity::new(&f.world, 128).unwrap();
        f.world.drifts.values_mut().next().unwrap().mass[0] = 1.0;
        assert_eq!(
            activity.before(&f.world).err(),
            Some("hidden_material_accumulated")
        );
    }

    #[test]
    fn source_attribution_does_not_count_display_migration_as_outflow() {
        let mut f = prepare(0x5eed, 128).unwrap();
        let mut driver = stream::Driver::new(&f.world, 128).unwrap();
        let mut activity = Activity::new(&f.world, 128).unwrap();
        let admission = driver.refill(&mut f.world).unwrap();
        let before = activity.before(&f.world).unwrap();
        let body = f
            .world
            .flakes
            .iter_mut()
            .find(|b| b.position.x < 3840.0)
            .unwrap();
        body.position.x += 3840.0;
        activity.record(&f.world, admission, before).unwrap();
        assert!(
            activity
                .per_source
                .iter()
                .all(|s| s.outflow_bodies == 0 && s.outflow_mass == 0.0)
        );
        f.world.flakes[0].skip_support = None;
        assert_eq!(
            activity.before(&f.world).err(),
            Some("hidden_unknown_layer")
        );
    }
}

pub(super) struct Before {
    counts: [usize; SCREENS],
    mass: [f64; SCREENS],
    introduced: f64,
    removed: f64,
}
#[derive(Serialize)]
pub(super) struct Activity {
    samples: usize,
    target_per_screen: usize,
    min_starting_bodies: usize,
    min_after_step_bodies: usize,
    max_mass_relative_error: f64,
    admitted_per_screen: [usize; SCREENS],
    per_source: Vec<Source>,
}
#[derive(Serialize)]
struct Source {
    initial_screen_key: u64,
    layer: u64,
    outflow_steps: usize,
    outflow_bodies: usize,
    outflow_mass: f64,
}
impl Activity {
    pub fn new(world: &World, target: usize) -> Result<Self, &'static str> {
        if world.scene.screens.len() != SCREENS || !(1..=stream::PER_SCREEN).contains(&target) {
            return Err("hidden_initial_layout_invalid");
        }
        let mut per_source = Vec::new();
        for screen in &world.scene.screens {
            let bodies = world
                .flakes
                .iter()
                .filter(|b| {
                    world
                        .scene
                        .nearest_screen(b.position)
                        .is_some_and(|s| s.key == screen.key)
                })
                .collect::<Vec<_>>();
            if bodies.len() != target {
                return Err("hidden_initial_population_invalid");
            }
            let layer = bodies[0].skip_support.ok_or("hidden_layer_missing")?;
            if bodies.iter().any(|b| b.skip_support != Some(layer))
                || per_source.iter().any(|s: &Source| s.layer == layer)
                || !world
                    .falling_clips(bodies[0], world.scene.bounds())
                    .is_some_and(|c| c.is_empty())
            {
                return Err("hidden_source_not_hidden");
            }
            per_source.push(Source {
                initial_screen_key: screen.key,
                layer,
                outflow_steps: 0,
                outflow_bodies: 0,
                outflow_mass: 0.0,
            });
        }
        let activity = Self {
            samples: 0,
            target_per_screen: target,
            min_starting_bodies: usize::MAX,
            min_after_step_bodies: usize::MAX,
            max_mass_relative_error: 0.0,
            admitted_per_screen: [0; SCREENS],
            per_source,
        };
        activity.before(world)?;
        Ok(activity)
    }
    pub fn before(&self, world: &World) -> Result<Before, &'static str> {
        let budget = world.budget_snapshot();
        if budget.pending_bodies != 0
            || budget.pending_packets != 0
            || budget.spill_bodies != 0
            || budget.active_slabs != budget.active_bodies
            || budget.falling_masks != SCREENS
        {
            return Err("hidden_unexpected_material");
        }
        if world.drifts.values().any(|d| d.total_mass() != 0.0) {
            return Err("hidden_material_accumulated");
        }
        let mut counts = [0; SCREENS];
        let mut mass = [0.0; SCREENS];
        let mut checked = [false; SCREENS];
        for body in &world.flakes {
            let index = self
                .per_source
                .iter()
                .position(|s| Some(s.layer) == body.skip_support)
                .ok_or("hidden_unknown_layer")?;
            if !checked[index] {
                if !world
                    .falling_clips(body, world.scene.bounds())
                    .is_some_and(|c| c.is_empty())
                {
                    return Err("hidden_source_not_hidden");
                }
                checked[index] = true;
            }
            counts[index] += 1;
            mass[index] += f64::from(body.mass);
        }
        if checked.contains(&false) {
            return Err("hidden_source_not_active");
        }
        Ok(Before {
            counts,
            mass,
            introduced: world.introduced_mass,
            removed: world.removed_mass,
        })
    }
    pub fn record(
        &mut self,
        world: &World,
        admission: stream::Admission,
        before: Before,
    ) -> Result<(), &'static str> {
        let after = self.before(world)?;
        if after.introduced != before.introduced
            || after.removed < before.removed
            || admission.before != [self.target_per_screen; SCREENS]
        {
            return Err("hidden_step_ledger_invalid");
        }
        let mut outflow = 0.0;
        for index in 0..SCREENS {
            let count = before.counts[index]
                .checked_sub(after.counts[index])
                .ok_or("hidden_body_created_during_step")?;
            let mass = before.mass[index] - after.mass[index];
            if !mass.is_finite()
                || mass < 0.0
                || (count == 0 && mass != 0.0)
                || (count > 0 && mass <= 0.0)
            {
                return Err("hidden_source_outflow_invalid");
            }
            let source = &mut self.per_source[index];
            source.outflow_steps += usize::from(count > 0);
            source.outflow_bodies += count;
            source.outflow_mass += mass;
            self.admitted_per_screen[index] += admission.added[index];
            outflow += mass;
        }
        if ((after.removed - before.removed) - outflow).abs() > outflow.max(1.0) * 0.001 {
            return Err("hidden_removed_mass_mismatch");
        }
        let error = (world.introduced_mass - world.removed_mass - world.total_mass()).abs()
            / world.introduced_mass.max(1.0);
        if !error.is_finite() || error > 0.001 {
            return Err("model_mass_balance_failed");
        }
        self.samples += 1;
        self.min_starting_bodies = self.min_starting_bodies.min(admission.before.iter().sum());
        self.min_after_step_bodies = self.min_after_step_bodies.min(world.flakes.len());
        self.max_mass_relative_error = self.max_mass_relative_error.max(error);
        Ok(())
    }
    pub fn finish(&self, steps: usize) -> Result<(), &'static str> {
        if steps < stream::MIN_STEPS
            || self.samples != steps
            || self.min_starting_bodies != SCREENS * self.target_per_screen
            || self.admitted_per_screen.contains(&0)
            || self.per_source.iter().any(|s| {
                s.outflow_bodies == 0
                    || s.outflow_steps == 0
                    || !s.outflow_mass.is_finite()
                    || s.outflow_mass <= 0.0
            })
        {
            return Err("hidden_continuity_missing");
        }
        Ok(())
    }
}
