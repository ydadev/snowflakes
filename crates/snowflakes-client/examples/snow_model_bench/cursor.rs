//! Synthetic cursor input for the continuous model fixture; never moves the OS cursor.
use super::{Fixture, distribution, stream, window};
use serde_json::{Value, json};
use snowflakes_client::{
    geometry::{Point, Rect},
    snow::{Cursor, World},
};

const SCREENS: usize = 8;
pub(super) const COVERS: usize = SCREENS * 8;
const PHASES: [&str; 3] = ["far", "near_visible", "near_covered"];
const HIDDEN_PHASES: [&str; 3] = ["far", "near_hidden_stationary", "near_hidden_moving"];

pub(super) fn prepare(seed: u64, per_screen: usize) -> Result<Fixture, &'static str> {
    let mut fixture = stream::prepare(seed, per_screen)?;
    let mut scene = fixture.world.scene.clone();
    for screen in &scene.screens {
        for tile in 0..8 {
            let mut cover = window(
                10_000 + screen.key * 16 + tile,
                screen.rect.left + tile as f32 * 480.0 + 144.0,
                900.0,
                192.0,
            );
            cover.rect.bottom = 1300.0;
            cover.collect = false;
            scene.windows.push(cover);
        }
    }
    fixture.world.set_scene(scene);
    fixture.settings.interaction.cursor_enabled = true;
    Ok(fixture)
}

fn covered(world: &World, point: Point) -> bool {
    world
        .scene
        .windows
        .iter()
        .any(|w| w.visible && !w.collect && w.rect.contains(point))
}

#[derive(Clone, Copy)]
pub(super) struct Probe {
    cursor: Cursor,
    phase: usize,
    screen: usize,
}
impl Probe {
    pub fn read(world: &World, step: usize) -> Result<Self, &'static str> {
        if world.scene.screens.len() != SCREENS
            || world
                .scene
                .windows
                .iter()
                .filter(|w| w.visible && !w.collect)
                .count()
                != COVERS
        {
            return Err("cursor_layout_changed");
        }
        let phase = step % PHASES.len();
        let screen = (step / PHASES.len()) % SCREENS;
        let display = &world.scene.screens[screen];
        let position = if phase == 0 {
            Point::new(display.rect.left + 1920.0, -10_000.0)
        } else {
            let want_covered = phase == 2;
            let (body, position) = world
                .flakes
                .iter()
                .filter(|b| {
                    display.rect.contains(b.position) && (850.0..1450.0).contains(&b.position.y)
                })
                .find_map(|body| {
                    let scale = world.particle_scale(body);
                    let position = Point::new(body.position.x + 20.0 * scale, body.position.y);
                    (covered(world, body.position) == want_covered
                        && covered(world, position) == want_covered)
                        .then_some((body, position))
                })
                .ok_or("cursor_target_missing")?;
            // Independently observe the inherited mask through the public clip
            // API. A nearby center alone must not count as a visible target.
            for point in [body.position, position] {
                let visible = world
                    .falling_clips(
                        body,
                        Rect {
                            left: point.x,
                            top: point.y,
                            right: point.x + 0.01,
                            bottom: point.y + 0.01,
                        },
                    )
                    .ok_or("cursor_source_mask_missing")?
                    .iter()
                    .any(|rect| rect.contains(point));
                if visible == want_covered {
                    return Err("cursor_target_mask_mismatch");
                }
            }
            position
        };
        Ok(Self {
            cursor: Cursor {
                position,
                velocity: Point::new(120.0, -60.0),
            },
            phase,
            screen,
        })
    }
    pub fn input(self, enabled: bool) -> Option<Cursor> {
        enabled.then_some(self.cursor)
    }
    pub fn read_hidden(world: &World, step: usize) -> Result<Self, &'static str> {
        if world.scene.screens.len() != SCREENS
            || world.scene.windows.len() != 64
            || world.scene.windows.iter().any(|w| !w.collect || !w.visible)
        {
            return Err("hidden_cursor_layout_changed");
        }
        let phase = step % 3;
        let screen = (step / 3) % SCREENS;
        let display = &world.scene.screens[screen];
        let position = if phase == 0 {
            Point::new(display.rect.left + 1920.0, -10_000.0)
        } else {
            let body = world
                .flakes
                .iter()
                .find(|b| {
                    display.rect.contains(b.position) && (850.0..1450.0).contains(&b.position.y)
                })
                .ok_or("hidden_cursor_target_missing")?;
            if !world
                .falling_clips(body, world.scene.bounds())
                .is_some_and(|c| c.is_empty())
            {
                return Err("hidden_cursor_target_visible");
            }
            Point::new(
                body.position.x + 20.0 * world.particle_scale(body),
                body.position.y,
            )
        };
        Ok(Self {
            cursor: Cursor {
                position,
                velocity: if phase == 1 {
                    Point::new(0.0, 0.0)
                } else {
                    Point::new(120.0, -60.0)
                },
            },
            phase,
            screen,
        })
    }
}

pub(super) struct Activity {
    enabled: bool,
    per_screen: [[usize; 3]; SCREENS],
    times: [Vec<u64>; 3],
    phases: [&'static str; 3],
}
impl Activity {
    pub fn new(enabled: bool) -> Self {
        Self {
            enabled,
            per_screen: [[0; 3]; SCREENS],
            times: Default::default(),
            phases: PHASES,
        }
    }
    pub fn hidden(enabled: bool) -> Self {
        Self {
            phases: HIDDEN_PHASES,
            ..Self::new(enabled)
        }
    }
    pub fn record(&mut self, probe: Probe, nanos: u64) {
        self.per_screen[probe.screen][probe.phase] += 1;
        self.times[probe.phase].push(nanos);
    }
    pub fn finish(self, steps: usize) -> Result<Value, &'static str> {
        if steps < stream::MIN_STEPS
            || self.per_screen.iter().flatten().any(|&count| count == 0)
            || self.per_screen.iter().flatten().sum::<usize>() != steps
        {
            return Err("cursor_coverage_missing");
        }
        Ok(json!({
            "input_enabled": self.enabled,
            "samples": steps,
            "phases": self.phases.into_iter().zip(self.times).map(|(phase, times)| json!({
                "phase": phase, "samples": times.len(), "step_time": distribution(times)
            })).collect::<Vec<_>>(),
            "per_screen_phase_samples": self.per_screen,
            "observation_semantics": "pre-step-target-conditions-not-force-counts",
            "selection_timing": "outside-step-included-in-wall"
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Scenario, StateHasher, parse};

    fn fingerprint(world: &World) -> u64 {
        let mut hasher = StateHasher::new();
        hasher.append(world, world.budget_snapshot());
        hasher.0
    }

    #[test]
    fn cursor_arguments_bound_work_and_keep_the_control_explicit() {
        for args in [
            vec!["--cursor", "disabled"],
            vec!["--scenario", "cursor-stream", "--cursor", "maybe"],
            vec!["--scenario", "cursor-stream", "--steps", "359"],
            vec![
                "--scenario",
                "cursor-stream",
                "--cursor",
                "enabled",
                "--cursor",
                "disabled",
            ],
        ] {
            assert!(parse(args.into_iter().map(str::to_owned)).is_err());
        }
        let options = parse(
            [
                "--scenario",
                "cursor-stream",
                "--steps",
                "360",
                "--cursor",
                "disabled",
            ]
            .map(str::to_owned),
        )
        .unwrap()
        .unwrap();
        assert_eq!(options.scenario, Scenario::CursorStream);
        assert!(options.scenario.is_stream());
        assert_eq!(options.scenario.episode_steps(), None);
        assert!(!options.cursor_enabled);
        let fixture = prepare(0x5eed, stream::PER_SCREEN).unwrap();
        let budget = fixture.world.budget_snapshot();
        assert_eq!(budget.active_bodies, 16_000);
        assert_eq!(budget.active_slabs, 16_000);
        assert_eq!(budget.falling_masks, SCREENS);
        assert_eq!(fixture.world.scene.windows.len(), 128);
        assert_eq!(
            fixture
                .world
                .drifts
                .values()
                .filter(|d| !d.support.ground)
                .count(),
            64
        );
        for step in 0..24 {
            let probe = Probe::read(&fixture.world, step)
                .unwrap_or_else(|reason| panic!("step {step}: {reason}"));
            assert_eq!(probe.phase, step % 3);
            assert_eq!(probe.screen, step / 3);
            assert!(probe.input(false).is_none());
        }
    }

    #[test]
    fn visible_input_changes_motion_while_far_and_covered_input_keep_the_control_state() {
        for phase in 0..3 {
            let mut actual = prepare(0x5eed, 128).unwrap();
            let mut control = prepare(0x5eed, 128).unwrap();
            let probe = Probe::read(&actual.world, phase).unwrap();
            actual.world.step(&actual.settings, probe.input(true));
            control.world.step(&control.settings, None);
            if phase == 1 {
                assert_ne!(fingerprint(&actual.world), fingerprint(&control.world));
                assert!(
                    actual
                        .world
                        .flakes
                        .iter()
                        .zip(&control.world.flakes)
                        .any(|(a, b)| a.velocity != b.velocity)
                );
            } else {
                assert_eq!(fingerprint(&actual.world), fingerprint(&control.world));
            }
            assert_eq!(actual.world.introduced_mass, control.world.introduced_mass);
            assert_eq!(actual.world.removed_mass, control.world.removed_mass);
        }
    }

    #[test]
    fn consecutive_cursor_phases_keep_real_receiving_work_and_balanced_material() {
        let mut fixture = prepare(0x5eed, 512).unwrap();
        let mut driver = stream::Driver::new(&fixture.world, 512).unwrap();
        let mut stream = stream::Activity::new(&fixture.world, 512).unwrap();
        let mut cursor = Activity::new(true);
        for step in 0..stream::MIN_STEPS {
            let admission = driver.refill(&mut fixture.world).unwrap();
            let probe = Probe::read(&fixture.world, step)
                .unwrap_or_else(|reason| panic!("step {step}: {reason}"));
            fixture.world.step(&fixture.settings, probe.input(true));
            stream.record(&fixture.world, admission).unwrap();
            cursor.record(probe, 1);
        }
        stream.finish(stream::MIN_STEPS).unwrap();
        let report = cursor.finish(stream::MIN_STEPS).unwrap();
        assert_eq!(report["samples"], 360);
        assert_eq!(
            report["per_screen_phase_samples"],
            serde_json::to_value([[15; 3]; SCREENS]).unwrap()
        );
    }

    #[test]
    fn disabled_interaction_and_a_paused_target_display_keep_the_control_motion() {
        for paused in [false, true] {
            let mut actual = prepare(0x5eed, 128).unwrap();
            let mut control = prepare(0x5eed, 128).unwrap();
            let probe = Probe::read(&actual.world, 1).unwrap();
            if paused {
                actual.world.set_paused_monitors([1].into());
                control.world.set_paused_monitors([1].into());
            } else {
                actual.settings.interaction.cursor_enabled = false;
                control.settings.interaction.cursor_enabled = false;
            }
            actual.world.step(&actual.settings, probe.input(true));
            control.world.step(&control.settings, None);
            assert_eq!(fingerprint(&actual.world), fingerprint(&control.world));
        }
    }

    #[test]
    fn missing_masks_targets_and_phase_observations_cannot_pass() {
        let mut fixture = prepare(0x5eed, 128).unwrap();
        assert_eq!(
            Activity::new(true).finish(360).err(),
            Some("cursor_coverage_missing")
        );
        let mut incomplete = Activity::new(true);
        let probe = Probe::read(&fixture.world, 0).unwrap();
        for _ in 0..360 {
            incomplete.record(probe, 1);
        }
        assert_eq!(
            incomplete.finish(360).err(),
            Some("cursor_coverage_missing")
        );
        fixture.world.scene.windows.retain(|w| w.collect);
        assert_eq!(
            Probe::read(&fixture.world, 1).err(),
            Some("cursor_layout_changed")
        );
        let mut fixture = prepare(0x5eed, 128).unwrap();
        for body in &mut fixture.world.flakes {
            body.position.y = 700.0;
        }
        assert_eq!(
            Probe::read(&fixture.world, 1).err(),
            Some("cursor_target_missing")
        );
        let mut fixture = prepare(0x5eed, 128).unwrap();
        for body in &mut fixture.world.flakes {
            body.skip_support = None;
        }
        assert_eq!(
            Probe::read(&fixture.world, 1).err(),
            Some("cursor_source_mask_missing")
        );
        let mut fixture = prepare(0x5eed, 128).unwrap();
        let complete = fixture.world.scene.clone();
        let mut without_covers = complete.clone();
        without_covers.windows.retain(|w| w.collect);
        fixture.world.set_scene(without_covers);
        // A broken fixture must not infer a live mask merely from scene rects.
        fixture.world.scene = complete;
        assert_eq!(
            Probe::read(&fixture.world, 2).err(),
            Some("cursor_target_mask_mismatch")
        );
    }
}
