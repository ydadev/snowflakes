//! Literal pre-change force and mask predicate, independent of the optimized helper.
use super::*;
use crate::geometry::Window;
use std::cell::Cell;

thread_local! {
    static LITERAL: Cell<bool> = const { Cell::new(false) };
    static WORK: Cell<[usize; 2]> = const { Cell::new([0; 2]) };
}
pub(super) fn use_literal() -> bool {
    LITERAL.get()
}
pub(super) fn record_query() {
    let mut c = WORK.get();
    c[0] += 1;
    WORK.set(c);
}
pub(super) fn record_cover() {
    let mut c = WORK.get();
    c[1] += 1;
    WORK.set(c);
}

fn with_literal<R>(f: impl FnOnce() -> R) -> R {
    struct Restore(bool);
    impl Drop for Restore {
        fn drop(&mut self) {
            LITERAL.set(self.0);
        }
    }
    let restore = Restore(LITERAL.replace(true));
    let result = f();
    drop(restore);
    result
}

pub(super) fn literal(world: &World, body: &Flake, cursor: Cursor, scale: f32, air: &mut Point) {
    record_query();
    let visible = body
        .skip_support
        .and_then(|key| world.falling_masks.get(&key))
        .is_none_or(|mask| {
            !mask.hidden
                && !mask.covers.iter().any(|r| {
                    record_cover();
                    r.contains(body.position) || r.contains(cursor.position)
                })
        });
    if visible {
        let dx = (body.position.x - cursor.position.x) / scale;
        let dy = (body.position.y - cursor.position.y) / scale;
        let distance = dx.hypot(dy);
        if distance < 65.0 && distance > 0.1 {
            let weight = (1.0 - distance / 65.0).powi(2);
            air.x += (dx / distance * 36.0 + cursor.velocity.x.clamp(-120.0, 120.0) * 0.5) * weight;
            air.y +=
                (dy / distance * 24.0 + cursor.velocity.y.clamp(-100.0, 100.0) * 0.35) * weight;
        }
    }
}

fn scene() -> Scene {
    Scene {
        screens: vec![
            Screen {
                key: 1,
                persistent_id: true,
                rect: Rect {
                    left: -800.0,
                    top: 0.0,
                    right: 0.0,
                    bottom: 1200.0,
                },
                floor: 1200.0,
                scale: 1.0,
            },
            Screen {
                key: 2,
                persistent_id: true,
                rect: Rect {
                    left: 0.0,
                    top: 0.0,
                    right: 800.0,
                    bottom: 1200.0,
                },
                floor: 1200.0,
                scale: 2.5,
            },
        ],
        windows: vec![],
    }
}
fn mask(covers: Vec<Rect>, hidden: bool) -> FallingMask {
    FallingMask {
        origins: BTreeSet::new(),
        front: BTreeSet::new(),
        covers,
        hidden,
        coalesced: false,
    }
}
fn float_equal(a: f32, b: f32) {
    if a.is_nan() || b.is_nan() {
        assert!(a.is_nan() && b.is_nan());
    } else {
        assert_eq!(a.to_bits(), b.to_bits(), "{a:?} != {b:?}");
    }
}
fn force_equal(a: Point, b: Point) {
    float_equal(a.x, b.x);
    float_equal(a.y, b.y);
}

#[test]
fn cursor_force_matches_literal_at_distance_clamp_and_mask_boundaries() {
    let mut world = World::new(scene(), 73);
    let mut body = Flake::new(Point::new(0.0, 0.0), 1.0);
    let offsets = [
        -65.00001, -65.0, -64.99999, -20.0, -0.100001, -0.1, -0.099999, -0.0, 0.0, 0.099999, 0.1,
        0.100001, 20.0, 64.99999, 65.0, 65.00001,
    ];
    let velocities = [-300.0, -120.0, -100.0, -0.0, 0.0, 100.0, 120.0, 300.0];
    let mut checked = 0;
    for mode in 0..7 {
        body.skip_support = if mode == 0 { None } else { Some(7) };
        world.falling_masks.clear();
        if mode >= 2 {
            let covers = match mode {
                4 => vec![Rect {
                    left: -1.0,
                    top: -1.0,
                    right: 1.0,
                    bottom: 1.0,
                }],
                5 => vec![Rect {
                    left: 5.0,
                    top: -1.0,
                    right: 80.0,
                    bottom: 1.0,
                }],
                6 => vec![Rect {
                    left: 0.0,
                    top: 0.0,
                    right: 0.1,
                    bottom: 20.0,
                }],
                _ => vec![],
            };
            world.falling_masks.insert(7, mask(covers, mode == 3));
        }
        for scale in [0.25, 1.0, 1.25, 2.5, 8.0] {
            for offset in offsets {
                for axis in 0..3 {
                    let position = match axis {
                        0 => Point::new(offset * scale, 0.0),
                        1 => Point::new(0.0, offset * scale),
                        _ => Point::new(offset * scale, offset * scale * 0.5),
                    };
                    for vx in velocities {
                        for vy in velocities {
                            let cursor = Cursor {
                                position,
                                velocity: Point::new(vx, vy),
                            };
                            let mut actual = Point::new(-0.0, -0.0);
                            let mut expected = actual;
                            world.apply_particle_cursor(&body, cursor, scale, &mut actual);
                            literal(&world, &body, cursor, scale, &mut expected);
                            force_equal(actual, expected);
                            checked += 1;
                        }
                    }
                }
            }
        }
    }
    assert_eq!(checked, 107_520);
}

#[test]
fn cursor_force_keeps_exceptional_input_classification_and_signed_zero() {
    let mut world = World::new(scene(), 73);
    let mut body = Flake::new(Point::new(0.0, 0.0), 1.0);
    body.skip_support = Some(7);
    let values = [
        f32::NEG_INFINITY,
        -f32::MAX,
        -1.0,
        -0.0,
        0.0,
        f32::from_bits(1),
        1.0,
        f32::MAX,
        f32::INFINITY,
        f32::NAN,
    ];
    let mut checked = 0;
    for hidden in [false, true] {
        world.falling_masks.insert(7, mask(vec![], hidden));
        for scale in values {
            for position in values {
                for velocity in values {
                    for air in [Point::new(-0.0, 0.0), Point::new(4.0, -3.0)] {
                        let cursor = Cursor {
                            position: Point::new(position, 20.0),
                            velocity: Point::new(velocity, -velocity),
                        };
                        let mut actual = air;
                        let mut expected = air;
                        world.apply_particle_cursor(&body, cursor, scale, &mut actual);
                        literal(&world, &body, cursor, scale, &mut expected);
                        force_equal(actual, expected);
                        checked += 1;
                    }
                }
            }
        }
    }
    assert_eq!(checked, 4000);
}

#[test]
fn distance_gate_skips_far_mask_scans_and_keeps_near_occlusion() {
    let mut world = World::new(scene(), 73);
    let mut body = Flake::new(Point::new(0.0, 0.0), 1.0);
    body.skip_support = Some(7);
    world.falling_masks.insert(
        7,
        mask(
            (0..64)
                .map(|i| Rect {
                    left: 200.0 + i as f32 * 20.0,
                    top: 200.0,
                    right: 210.0 + i as f32 * 20.0,
                    bottom: 300.0,
                })
                .collect(),
            false,
        ),
    );
    for x in [0.0, 0.1, 65.0, 1000.0, f32::INFINITY, f32::NAN] {
        let cursor = Cursor {
            position: Point::new(x, 0.0),
            velocity: Point::new(120.0, -60.0),
        };
        let mut actual = Point::new(4.0, -3.0);
        let mut expected = actual;
        WORK.set([0; 2]);
        world.apply_particle_cursor(&body, cursor, 1.0, &mut actual);
        assert_eq!(
            WORK.get(),
            [0, 0],
            "far or excluded distance still consulted a mask"
        );
        WORK.set([0; 2]);
        literal(&world, &body, cursor, 1.0, &mut expected);
        assert_eq!(WORK.get(), [1, 64]);
        force_equal(actual, expected);
    }
    let cursor = Cursor {
        position: Point::new(20.0, 0.0),
        velocity: Point::new(120.0, -60.0),
    };
    for mode in 0..3 {
        let mask = world.falling_masks.get_mut(&7).unwrap();
        if mode == 1 {
            mask.covers[63] = Rect {
                left: 10.0,
                top: -1.0,
                right: 30.0,
                bottom: 1.0,
            };
        }
        mask.hidden = mode == 2;
        let mut actual = Point::new(4.0, -3.0);
        let mut expected = actual;
        WORK.set([0; 2]);
        world.apply_particle_cursor(&body, cursor, 1.0, &mut actual);
        assert_eq!(WORK.get(), if mode == 2 { [1, 0] } else { [1, 64] });
        literal(&world, &body, cursor, 1.0, &mut expected);
        force_equal(actual, expected);
        if mode > 0 {
            force_equal(actual, Point::new(4.0, -3.0));
        }
    }
}

fn window(key: u64, left: f32, top: f32, width: f32, collect: bool) -> Window {
    Window {
        key,
        rect: Rect {
            left,
            top,
            right: left + width,
            bottom: top + 120.0,
        },
        visible: true,
        collect,
    }
}
fn fixture(per_screen: usize) -> (World, Settings) {
    source_fixture(per_screen, false)
}
fn source_fixture(per_screen: usize, hidden: bool) -> (World, Settings) {
    let mut scene = scene();
    scene.windows = vec![
        window(11, -500.0, 350.0, 250.0, false),
        window(12, 250.0, 350.0, 250.0, false),
        window(7, -650.0, 200.0, 400.0, true),
        window(8, 100.0, 200.0, 400.0, true),
        window(30, -750.0, 900.0, 700.0, true),
        window(31, 50.0, 900.0, 700.0, true),
    ];
    if hidden {
        for window in &mut scene.windows {
            if [7, 8].contains(&window.key) {
                window.visible = false;
            }
        }
    }
    let mut settings = Settings::default();
    settings.snow.intensity_percent = 0;
    settings.snow.melting = false;
    let mut world = World::new(scene.clone(), 73);
    for key in [7, 8] {
        let drift = world.drifts.get_mut(&key).unwrap();
        drift.mass.fill(drift.cell_width * 0.5);
    }
    world.introduced_mass = world.total_mass();
    scene.windows.retain(|w| w.key != 7 && w.key != 8);
    world.set_scene(scene);
    world.step(&settings, None);
    for index in 0..2 {
        let display = world.scene.screens[index].clone();
        let template = world
            .flakes
            .iter()
            .find(|b| display.rect.contains(b.position))
            .unwrap()
            .clone();
        while world
            .flakes
            .iter()
            .filter(|b| display.rect.contains(b.position))
            .count()
            < per_screen
        {
            let slot = world
                .flakes
                .iter()
                .filter(|b| display.rect.contains(b.position))
                .count();
            let mut body = template.clone();
            body.position = Point::new(
                display.rect.left + 80.0 + (slot % 13) as f32 * 48.0,
                250.0 + (slot % 37) as f32 * 16.0,
            );
            body.velocity = Point::new((slot % 3) as f32 * 20.0 - 20.0, 240.0);
            body.spin = if slot.is_multiple_of(2) { 0.55 } else { -0.55 };
            assert!(world.release(body));
        }
    }
    assert_eq!(world.falling_masks.len(), 2);
    (world, settings)
}
fn full_equal(a: &World, b: &World) {
    assert_eq!(format!("{a:?}"), format!("{b:?}"));
}
fn conserved(world: &World) {
    assert!(
        (world.introduced_mass - world.removed_mass - world.total_mass()).abs()
            <= world.introduced_mass.max(1.0) * 0.001
    );
}

#[test]
fn cursor_sequence_keeps_every_private_world_field_through_masks_pauses_and_closures() {
    let (mut actual, mut settings) = fixture(64);
    let (mut expected, _) = fixture(64);
    let mut changed = 0;
    for step in 0..240 {
        if step % 40 == 0 {
            let pauses = match step % 120 {
                0 => BTreeSet::new(),
                40 => [1].into(),
                _ => [2].into(),
            };
            actual.set_paused_monitors(pauses.clone());
            expected.set_paused_monitors(pauses);
        }
        if [60, 120, 180].contains(&step) {
            let mut scene = actual.scene.clone();
            if step == 60 {
                scene.windows[0].rect.left -= 100.0;
                scene.windows[0].rect.right -= 100.0;
            }
            if step == 120 {
                scene.windows.retain(|w| w.key != 11);
            }
            if step == 180 {
                scene.windows.retain(|w| w.key != 31);
            }
            actual.set_scene(scene.clone());
            expected.set_scene(scene);
        }
        settings.interaction.cursor_enabled = step % 17 != 0;
        let cursor = match step % 5 {
            0 => None,
            1 => Some(Cursor {
                position: Point::new(-380.0, 400.0),
                velocity: Point::new(300.0, -300.0),
            }),
            2 => Some(Cursor {
                position: Point::new(150.0, 600.0),
                velocity: Point::new(-80.0, 40.0),
            }),
            3 => Some(Cursor {
                position: Point::new(200.0, -10_000.0),
                velocity: Point::new(0.0, 0.0),
            }),
            _ => Some(Cursor {
                position: Point::new(-500.0, 900.0),
                velocity: Point::new(-200.0, 200.0),
            }),
        };
        actual.step(&settings, cursor);
        with_literal(|| expected.step(&settings, cursor));
        full_equal(&actual, &expected);
        conserved(&actual);
        changed += usize::from(actual.contact_work.hits > 0);
    }
    assert!(changed > 0, "the sequence never reached receiving contacts");
    assert!(actual.removed_mass > 0.0 || actual.drifts.values().any(|d| d.total_mass() > 0.0));
}

#[test]
fn full_two_display_pool_keeps_private_state_with_visible_and_covered_cursor_input() {
    let (mut actual, settings) = fixture(MAX_FLAKES);
    let (mut expected, _) = fixture(MAX_FLAKES);
    assert_eq!(actual.flakes.len(), 4800);
    for step in 0..24 {
        let body = &actual.flakes[step * 97 % actual.flakes.len()];
        let cursor = Some(Cursor {
            position: Point::new(
                body.position.x + 20.0 * actual.particle_scale(body),
                body.position.y,
            ),
            velocity: Point::new(120.0, -60.0),
        });
        actual.step(&settings, cursor);
        with_literal(|| expected.step(&settings, cursor));
        full_equal(&actual, &expected);
        conserved(&actual);
    }
}

#[test]
fn absent_disabled_and_fully_paused_input_never_queries_particle_masks() {
    for mode in 0..3 {
        let (mut actual, mut settings) = fixture(64);
        let (mut expected, _) = fixture(64);
        if mode == 1 {
            settings.interaction.cursor_enabled = false;
        }
        if mode == 2 {
            actual.set_paused_monitors([1, 2].into());
            expected.set_paused_monitors([1, 2].into());
        }
        let cursor = (mode != 0).then_some(Cursor {
            position: Point::new(-380.0, 400.0),
            velocity: Point::new(120.0, -60.0),
        });
        WORK.set([0; 2]);
        actual.step(&settings, cursor);
        assert_eq!(WORK.get(), [0, 0]);
        with_literal(|| expected.step(&settings, cursor));
        full_equal(&actual, &expected);
    }
}

#[test]
fn invisible_sources_keep_hidden_material_and_foreground_snow_in_full_state_agreement() {
    let prepare = || {
        let (mut world, settings) = source_fixture(64, true);
        assert!(world.falling_masks.values().all(|m| m.hidden));
        for index in 0..80 {
            let mut body = Flake::new(
                Point::new(
                    -700.0 + index as f32 * 17.0,
                    300.0 + (index % 5) as f32 * 100.0,
                ),
                2.0,
            );
            body.velocity = Point::new(0.0, 240.0);
            assert!(world.release(body));
        }
        (world, settings)
    };
    let (mut actual, settings) = prepare();
    let (mut expected, _) = prepare();
    for step in 0..120 {
        let cursor = Some(Cursor {
            position: Point::new(
                -450.0 + (step % 2) as f32 * 800.0,
                300.0 + (step % 5) as f32 * 100.0,
            ),
            velocity: Point::new(-200.0, 200.0),
        });
        actual.step(&settings, cursor);
        with_literal(|| expected.step(&settings, cursor));
        full_equal(&actual, &expected);
        conserved(&actual);
    }
    assert!(
        actual.removed_mass > 0.0,
        "hidden material never reached outflow"
    );
}

#[test]
fn hidden_air_input_above_roofs_keeps_every_control_world_field_unchanged() {
    let (mut actual, settings) = source_fixture(64, true);
    let (mut control, _) = source_fixture(64, true);
    assert!(actual.falling_masks.values().all(|m| m.hidden));
    for step in 0..240 {
        let cursor = Some(Cursor {
            position: Point::new(
                if step % 3 == 0 {
                    -10_000.0
                } else {
                    -450.0 + (step % 2) as f32 * 800.0
                },
                400.0,
            ),
            velocity: if step % 3 == 1 {
                Point::new(0.0, 0.0)
            } else {
                Point::new(120.0, -60.0)
            },
        });
        actual.step(&settings, cursor);
        control.step(&settings, None);
        full_equal(&actual, &control);
        conserved(&actual);
    }
    assert!(actual.removed_mass > 0.0);
    assert!(actual.drifts.values().all(|d| d.total_mass() == 0.0));
}
