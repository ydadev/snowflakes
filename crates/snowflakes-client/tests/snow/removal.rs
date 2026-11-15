//! Actual World removal paths with independent mass balance and time integrals.
use super::*;

#[derive(serde::Serialize)]
struct Observation {
    cases: usize,
    steps: usize,
    max_balance_error: f64,
    max_curve_relative_error: f64,
    profile_hash: u64,
}
impl Default for Observation {
    fn default() -> Self {
        Self {
            cases: 0,
            steps: 0,
            max_balance_error: 0.0,
            max_curve_relative_error: 0.0,
            profile_hash: 0xcbf29ce484222325,
        }
    }
}
impl Observation {
    fn observe(&mut self, world: &World, key: u64, initial: f64, expected: f64) {
        assert_eq!(world.introduced_mass, initial);
        assert!(world.flakes.is_empty() && world.pending_collapse_count() == 0);
        self.steps += 1;
        self.max_balance_error = self
            .max_balance_error
            .max((initial - world.removed_mass - world.total_mass()).abs());
        self.max_curve_relative_error = self
            .max_curve_relative_error
            .max((world.total_mass() - expected).abs() / initial.max(1e-6));
        // Numeric evidence for before/after comparison, not a fixed golden hash.
        // Exclude the ledger being corrected; retain every physical stock bit.
        let drift = &world.drifts[&key];
        for mass in drift.mass.iter().chain(&drift.pending) {
            self.profile_hash ^= u64::from(mass.to_bits());
            self.profile_hash = self.profile_hash.wrapping_mul(0x100000001b3);
        }
    }
    fn check(&self, name: &str) {
        println!(
            "removal_accounting {name} {}",
            serde_json::to_string(self).unwrap()
        );
        assert!(
            self.max_curve_relative_error < 5e-5,
            "removal speed/ceiling changed"
        );
        assert!(
            self.max_balance_error < 1e-9,
            "removed mass differs from actual stock loss"
        );
    }
}
fn fixture(scale: f32, roof: bool) -> (World, Settings, u64) {
    let mut desktop = scene();
    let screen = &mut desktop.screens[0];
    screen.rect.right *= scale;
    screen.rect.bottom *= scale;
    screen.floor *= scale;
    screen.scale = scale;
    // Both outer window edges lie outside the screen. This removes edge outflow
    // from the experiment without changing product forces or disabling a branch.
    desktop.windows.push(Window {
        key: 7,
        visible: true,
        collect: true,
        rect: Rect {
            left: 0.0,
            top: 300.0 * scale,
            right: 800.0 * scale,
            bottom: 500.0 * scale,
        },
    });
    let mut world = World::new(desktop, 8);
    let mut settings = quiet();
    settings.snow.drift_size_percent = 100;
    settings.interaction.cursor_enabled = false;
    // Establish the existing ceiling via a real empty step, before test stock.
    world.step(&settings, None);
    let key = if roof { 7 } else { 1 | (1 << 63) };
    (world, settings, key)
}
fn stock(world: &mut World, key: u64, scale: f32, resting: f32, pending: f32) -> f64 {
    let drift = world.drifts.get_mut(&key).unwrap();
    drift.mass.fill(resting * drift.cell_width / scale);
    drift.pending.fill(pending * drift.cell_width / scale);
    let initial = world.total_mass();
    world.introduced_mass = initial;
    initial
}
#[test]
fn disabled_stock_counts_actual_resting_and_pending_loss_over_the_original_two_seconds() {
    let mut observation = Observation::default();
    for scale in [1.0, 1.25, 2.0] {
        for (roof, zero_size) in [(true, false), (false, false), (false, true)] {
            for depth in [0.0002, std::f32::consts::PI, 31.2345] {
                for fraction in [0.0, 0.13, 0.75] {
                    let (mut world, mut settings, key) = fixture(scale, roof);
                    let initial = stock(
                        &mut world,
                        key,
                        scale,
                        depth * (1.0 - fraction),
                        depth * fraction,
                    );
                    if zero_size {
                        settings.snow.drift_size_percent = 0;
                    } else if roof {
                        settings.snow.on_windows = false;
                    } else {
                        settings.snow.on_ground = false;
                    }
                    observation.cases += 1;
                    for step in 1..=150 {
                        world.step(&settings, None);
                        let expected = initial * (1.0 - f64::from(step) / 120.0).max(0.0);
                        observation.observe(&world, key, initial, expected);
                    }
                    assert!(world.total_mass() < 1e-9);
                }
            }
        }
    }
    observation.check("disabled");
}
#[test]
fn partial_pending_removal_and_settling_keep_the_full_model_mass_budget() {
    let mut observation = Observation::default();
    for scale in [1.0, 1.25, 2.0] {
        let (mut world, settings, key) = fixture(scale, false);
        let initial = stock(&mut world, key, scale, 119.123, 1.731);
        observation.cases += 1;
        for _ in 0..150 {
            world.step(&settings, None);
            // The excess is less than the pending stock. Its partial removal
            // is followed by ordinary settling, which also rounds in f32.
            let expected = initial * 120.0 / (119.123 + 1.731);
            observation.observe(&world, key, initial, expected);
            let error = (initial - world.removed_mass - world.total_mass()).abs();
            assert!(error < initial * 1e-5);
        }
        assert!(world.removed_mass > 0.0 && world.removed_mass < initial * 0.02);
    }
    println!(
        "removal_accounting partial_pending {}",
        serde_json::to_string(&observation).unwrap()
    );
    assert!(observation.max_curve_relative_error < 1e-5);
}

#[test]
fn positive_ceiling_counts_actual_loss_without_changing_its_two_second_deadline() {
    let mut observation = Observation::default();
    for scale in [1.0, 1.25, 2.0] {
        for roof in [false, true] {
            let (mut world, mut settings, key) = fixture(scale, roof);
            let depth = if roof { 32.0 } else { 120.0 };
            let initial = stock(&mut world, key, scale, depth, 0.0);
            settings.snow.drift_size_percent = 99;
            observation.cases += 1;
            for step in 1..=150 {
                world.step(&settings, None);
                let expected = initial * (1.0 - 0.01 * (f64::from(step) / 120.0).min(1.0));
                observation.observe(&world, key, initial, expected);
            }
        }
    }
    observation.check("positive_ceiling");
}
#[test]
fn existing_overcapacity_counts_actual_loss_at_the_unchanged_capped_rate() {
    let mut observation = Observation::default();
    for scale in [1.0, 1.25, 2.0] {
        for roof in [false, true] {
            let (mut world, settings, key) = fixture(scale, roof);
            let ceiling = if roof { 32.0 } else { 120.0 };
            let depth = ceiling + 1.321;
            let initial = stock(&mut world, key, scale, depth, 0.0);
            observation.cases += 1;
            for step in 1..=10 {
                world.step(&settings, None);
                let reduction = (48.0 * f64::from(step) / 60.0).min(f64::from(depth - ceiling));
                let expected = initial * (1.0 - reduction / f64::from(depth));
                observation.observe(&world, key, initial, expected);
            }
        }
    }
    observation.check("existing_overcapacity");
}
