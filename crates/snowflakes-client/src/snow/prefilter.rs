//! A conservative whole-body query. A negative result skips geometry work only.
use super::{MAX_BUCKET_COLUMNS, RenderPose, SnowSlab, World, contact::MAX_SWEEP_ANGLE};
use crate::geometry::Rect;

impl SnowSlab {
    pub(super) fn contact_bounds(&self, before: RenderPose, after: RenderPose) -> Option<Rect> {
        if ![
            self.cell_width,
            self.center.x,
            self.center.y,
            before.position.x,
            before.position.y,
            after.position.x,
            after.position.y,
            before.scale,
            after.scale,
            before.angle,
            after.angle,
        ]
        .iter()
        .all(|x| x.is_finite())
            || before.scale <= 0.0
            || after.scale <= 0.0
        {
            return None;
        }
        let height = self
            .heights
            .iter()
            .copied()
            .filter(|h| h.is_finite() && *h > 0.0)
            .fold(0.0_f32, f32::max);
        let x0 = -f64::from(self.center.x);
        let x1 = self.heights.len() as f64 * f64::from(self.cell_width) + x0;
        let y0 = -f64::from(height) - f64::from(self.center.y);
        let y1 = -f64::from(self.center.y);
        let radius = x0.abs().max(x1.abs()) + y0.abs().max(y1.abs());
        let scale = f64::from(before.scale.max(after.scale));
        let scale_delta = (f64::from(after.scale) - f64::from(before.scale)).abs();
        // This circle covers every angle, including disconnected contour runs.
        // Also contain the expanded per-edge query boxes, not just the true arc.
        let query_margin = 2.0
            * radius
            * (scale * MAX_SWEEP_ANGLE * MAX_SWEEP_ANGLE + 2.0 * scale_delta * MAX_SWEEP_ANGLE)
            / 8.0;
        let position = [
            before.position.x,
            before.position.y,
            after.position.x,
            after.position.y,
        ]
        .into_iter()
        .map(|x| f64::from(x).abs())
        .fold(0.0_f64, f64::max);
        // Covers f32 contour construction and the non-rotating transform path.
        let rounding = 32.0 * f64::from(f32::EPSILON) * (position + radius * scale + 1.0);
        let extent = radius * scale + query_margin + rounding;
        let bounds = Rect {
            left: ((f64::from(before.position.x.min(after.position.x)) - extent) as f32)
                .next_down(),
            right: ((f64::from(before.position.x.max(after.position.x)) + extent) as f32).next_up(),
            top: ((f64::from(before.position.y.min(after.position.y)) - extent) as f32).next_down(),
            bottom: ((f64::from(before.position.y.max(after.position.y)) + extent) as f32)
                .next_up(),
        };
        [bounds.left, bounds.right, bounds.top, bounds.bottom]
            .iter()
            .all(|x| x.is_finite())
            .then_some(bounds)
    }
}

impl World {
    pub(super) fn slab_candidates(&self, bounds: Rect) -> bool {
        self.slab_candidates_visiting(bounds, || {})
    }
    fn slab_candidates_visiting(&self, bounds: Rect, mut visit: impl FnMut()) -> bool {
        let column = |x: f32| ((x - self.bucket_left) / self.bucket_width).floor() as i32;
        let first = column(bounds.left).max(0);
        let last = column(bounds.right).min(MAX_BUCKET_COLUMNS);
        let from = (bounds.top / 128.0).floor() as i32;
        let to = (bounds.bottom / 128.0).floor() as i32;
        if first > last || from > to {
            return false;
        }
        // Visit existing buckets only; an enormous gap must not create an
        // enormous loop over absent row/column coordinates.
        self.buckets
            .range((from, first)..=(to, last))
            .any(|(&(_, column), ids)| {
                visit();
                column >= first && column <= last && !ids.is_empty()
            })
    }
}

#[cfg(test)]
#[derive(Clone, Copy, Debug, Default)]
pub(super) struct ContactWork {
    pub skipped: usize,
    pub contours: usize,
    pub edges: usize,
    pub hits: usize,
}

#[cfg(test)]
mod tests {
    use crate::{
        geometry::{Point, Rect, Scene, Screen, Window},
        snow::{
            Cursor, DT, Flake, RenderPose, Rng, SnowSlab, World,
            contact::{AngularMotion, EdgeSweep},
        },
    };
    use snowflakes_protocol::Settings;

    fn scene() -> Scene {
        Scene {
            screens: vec![Screen {
                key: 1,
                persistent_id: true,
                rect: Rect {
                    left: 0.0,
                    top: 0.0,
                    right: 800.0,
                    bottom: 1000.0,
                },
                floor: 1000.0,
                scale: 1.0,
            }],
            windows: vec![],
        }
    }
    fn body() -> Flake {
        let mut body = Flake::new(Point::new(400.0, 200.0), (320.0_f32 / 1.2).sqrt());
        body.mass = 320.0;
        body.spin = 0.55;
        body.velocity.y = 12.0;
        body.slab = Some(SnowSlab {
            heights: vec![4.0; 14],
            cell_width: 80.0 / 14.0,
            center: Point::new(40.0, -2.0),
        });
        body
    }
    fn quiet() -> Settings {
        let mut settings = Settings::default();
        settings.snow.intensity_percent = 0;
        settings.snow.melting = false;
        settings
    }
    fn equal_state(a: &mut World, b: &mut World) {
        a.contact_work = Default::default();
        b.contact_work = Default::default();
        assert_eq!(format!("{a:?}"), format!("{b:?}"));
    }

    fn conserved(world: &World) {
        assert!(
            (world.introduced_mass - world.removed_mass - world.total_mass()).abs()
                <= world.introduced_mass.max(1.0) * 0.001
        );
    }

    fn mixed_scene() -> Scene {
        let mut scene = scene();
        scene.screens[0].rect.left = -800.0;
        scene.screens[0].rect.right = 0.0;
        let mut right = scene.screens[0].clone();
        right.key = 2;
        right.rect.left = 0.0;
        right.rect.right = 800.0;
        right.scale = 2.5;
        scene.screens.push(right);
        for (key, left, right, top, collect) in [
            (8, -100.0, 100.0, 280.0, false),
            (7, -720.0, 720.0, 300.0, true),
            (9, -650.0, 650.0, 600.0, true),
        ] {
            scene.windows.push(Window {
                key,
                rect: Rect {
                    left,
                    right,
                    top,
                    bottom: top + 100.0,
                },
                visible: true,
                collect,
            });
        }
        scene
    }

    #[test]
    fn whole_body_bounds_contain_every_angular_edge_query() {
        let mut separated = vec![0.0; 128];
        separated[0] = 4.0;
        separated[127] = 1.0;
        let profiles = [
            vec![4.0; 14],
            vec![0.0, 10.0, 2.0, 0.0, 8.0, 0.1],
            vec![0.000001; 3],
            vec![40.0, 0.0, 0.0, 1.0],
            vec![0.0, f32::NAN, 5.0],
            vec![1.0; 128],
            separated,
        ];
        let mut checked = 0;
        let mut poses = 0;
        for heights in profiles {
            for width in [0.125, 6.0, 1000.0] {
                for scale in [0.25, 1.0, 1.25, 1.5, 2.5, 8.0] {
                    for offset in [-100_000.0, 0.0, 100_000.0] {
                        for spin in [-2.5, 0.0, 2.5] {
                            let slab = SnowSlab {
                                cell_width: width,
                                center: Point::new(width * 1.75, -3.0),
                                heights: heights.clone(),
                            };
                            let before = RenderPose {
                                position: Point::new(offset + 400.0, 200.0),
                                angle: std::f32::consts::TAU - 0.02,
                                scale,
                                age: 1.0,
                            };
                            let after = RenderPose {
                                position: Point::new(
                                    before.position.x + 220.0 * scale * DT,
                                    before.position.y + 360.0 * scale * DT,
                                ),
                                angle: (before.angle + spin * DT).rem_euclid(std::f32::consts::TAU),
                                scale: scale * if spin < 0.0 { 0.96 } else { 1.04 },
                                age: 1.0 + DT,
                            };
                            let bounds = slab.contact_bounds(before, after).unwrap();
                            poses += 1;
                            let motion = AngularMotion::new(before, after);
                            for outline in slab.contours() {
                                let mut previous = motion.vertex(*outline.last().unwrap());
                                for point in outline {
                                    let current = motion.vertex(point);
                                    let edge = motion.between(previous, current).bounds();
                                    assert!(
                                        bounds.left <= edge.left
                                            && bounds.right >= edge.right
                                            && bounds.top <= edge.top
                                            && bounds.bottom >= edge.bottom
                                    );
                                    previous = current;
                                    checked += 1;
                                }
                            }
                        }
                    }
                }
            }
        }
        assert!(checked > 50_000);
        assert_eq!(poses, 1134);
    }

    #[test]
    fn unprovable_or_overflowing_bounds_fall_back_instead_of_rejecting() {
        let slab = body().slab.unwrap();
        let pose = RenderPose {
            position: Point::new(400.0, 200.0),
            angle: 0.0,
            scale: 1.0,
            age: 1.0,
        };
        for invalid in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            let mut changed = slab.clone();
            changed.cell_width = invalid;
            assert!(changed.contact_bounds(pose, pose).is_none());
            changed = slab.clone();
            changed.center.x = invalid;
            assert!(changed.contact_bounds(pose, pose).is_none());
        }
        for invalid in [0.0, -1.0, f32::NAN] {
            let changed = RenderPose {
                scale: invalid,
                ..pose
            };
            assert!(slab.contact_bounds(pose, changed).is_none());
        }
        let mut enormous = slab;
        enormous.cell_width = f32::MAX;
        assert!(enormous.contact_bounds(pose, pose).is_none());
    }

    #[test]
    fn bucket_query_visits_only_existing_entries_across_huge_gaps() {
        let mut scene = mixed_scene();
        scene.windows.clear();
        scene.screens[0].rect.left = -64_000.0;
        scene.screens[0].rect.right = -62_000.0;
        scene.screens[1].rect.left = 62_000.0;
        scene.screens[1].rect.right = 64_000.0;
        let world = World::new(scene, 42);
        let mut visited = 0;
        assert!(!world.slab_candidates_visiting(
            Rect {
                left: 0.0,
                right: 1.0,
                top: -1e30,
                bottom: 1e30
            },
            || visited += 1
        ));
        assert_eq!(visited, world.buckets.len());
        visited = 0;
        assert!(world.slab_candidates_visiting(
            Rect {
                left: -1e30,
                right: 1e30,
                top: -1e30,
                bottom: 1e30
            },
            || visited += 1
        ));
        assert!(visited > 0 && visited <= world.buckets.len());
    }

    #[test]
    fn filtered_steps_match_the_full_world_on_seeded_scenes_and_real_hits() {
        let mut skipped = 0;
        let mut hits = 0;
        for seed in 1..=16 {
            let mut filtered = World::new(mixed_scene(), seed);
            let mut full = World::new(mixed_scene(), seed);
            let mut random = Rng(seed + 100);
            for i in 0..24 {
                let count = 7 + (random.unit() * 10.0) as usize;
                let heights: Vec<_> = (0..count)
                    .map(|j| {
                        if j % 5 == 0 {
                            0.0
                        } else {
                            1.0 + random.unit() * 9.0
                        }
                    })
                    .collect();
                let x = -700.0 + random.unit() * 1400.0;
                let scale = if x < 0.0 { 1.0 } else { 2.5 };
                let cell_width = 6.0 / scale;
                let sum = heights.iter().sum::<f32>();
                let mass = sum * cell_width;
                let center = Point::new(
                    heights
                        .iter()
                        .enumerate()
                        .map(|(i, h)| (i as f32 + 0.5) * cell_width * h)
                        .sum::<f32>()
                        / sum,
                    -heights.iter().map(|h| h * h * 0.5).sum::<f32>() / sum,
                );
                let mut body = Flake::new(
                    Point::new(
                        x,
                        if i % 3 == 0 {
                            -100.0 * scale
                        } else {
                            294.0 - random.unit() * 8.0
                        },
                    ),
                    (mass / 1.2).sqrt().clamp(0.2, 32.0),
                );
                body.mass = mass;
                body.velocity = Point::new(random.signed() * 220.0, 12.0 + random.unit() * 348.0);
                body.spin = random.signed() * 2.5;
                body.angle = random.unit() * std::f32::consts::TAU;
                body.set_display_scale(scale * if i % 4 == 0 { 0.96 } else { 1.0 });
                body.slab = Some(SnowSlab {
                    heights,
                    cell_width,
                    center,
                });
                if i % 5 == 0 {
                    body.skip_support = Some(7);
                }
                assert!(filtered.release(body.clone()));
                assert!(full.release(body));
            }
            for tick in 0..30 {
                let cursor = (tick % 10 == 0).then_some(Cursor {
                    position: Point::new(200.0, 295.0),
                    velocity: Point::new(120.0, -40.0),
                });
                filtered.step(&quiet(), cursor);
                full.step_inner::<false>(&quiet(), cursor);
                skipped += filtered.contact_work.skipped;
                hits += full.contact_work.hits;
                conserved(&filtered);
                equal_state(&mut filtered, &mut full);
            }
        }
        assert!(skipped > 100 && hits > 100, "skips={skipped}, hits={hits}");
    }

    #[test]
    fn filtered_closures_pauses_and_topology_changes_match_the_full_world() {
        let mut desktop = mixed_scene();
        for i in 0..17 {
            desktop.windows.push(Window {
                key: 1000 + i,
                rect: Rect {
                    left: -720.0 + i as f32 * 80.0,
                    right: -620.0 + i as f32 * 80.0,
                    top: 200.0 + (i % 3) as f32 * 70.0,
                    bottom: 350.0,
                },
                visible: true,
                collect: true,
            });
        }
        let mut filtered = World::new(desktop.clone(), 43);
        let mut full = World::new(desktop.clone(), 43);
        for world in [&mut filtered, &mut full] {
            for drift in world.drifts.values_mut().filter(|d| !d.support.ground) {
                for i in 0..drift.mass.len() {
                    drift.mass[i] = 5.0 * drift.cell_width / drift.scales[i].powi(2);
                }
            }
            world.introduced_mass = world.total_mass();
        }
        desktop.windows.retain(|w| w.key < 1000);
        let mut skipped = 0;
        let mut hits = 0;
        for tick in 0..240 {
            match tick {
                10 => {
                    filtered.set_paused_monitors([1].into());
                    full.set_paused_monitors([1].into());
                }
                20 => {
                    let mut empty = desktop.clone();
                    empty.screens.clear();
                    filtered.set_scene(empty.clone());
                    full.set_scene(empty);
                }
                30 => {
                    filtered.set_paused_monitors(Default::default());
                    full.set_paused_monitors(Default::default());
                }
                40 => {
                    let mut partial = desktop.clone();
                    partial.screens.remove(0);
                    filtered.set_scene(partial.clone());
                    full.set_scene(partial);
                }
                60 => {
                    desktop.windows[1].rect.left += 40.0;
                }
                70 => {
                    desktop.windows.retain(|w| w.key != 7);
                }
                80 => {
                    desktop.windows.reverse();
                }
                200 => {
                    filtered.set_paused_monitors([1, 2].into());
                    full.set_paused_monitors([1, 2].into());
                }
                230 => {
                    filtered.clear();
                    full.clear();
                }
                _ => {}
            }
            if [0, 30, 50, 60, 70, 80].contains(&tick) {
                filtered.set_scene(desktop.clone());
                full.set_scene(desktop.clone());
            }
            filtered.step(&quiet(), None);
            full.step_inner::<false>(&quiet(), None);
            skipped += filtered.contact_work.skipped;
            hits += full.contact_work.hits;
            conserved(&filtered);
            equal_state(&mut filtered, &mut full);
        }
        assert!(skipped > 0 && hits > 0, "skips={skipped}, hits={hits}");
    }

    #[test]
    fn an_unreachable_slab_keeps_its_full_state_without_building_contours() {
        let mut filtered = World::new(scene(), 41);
        let mut full = World::new(scene(), 41);
        assert!(filtered.release(body()));
        assert!(full.release(body()));
        filtered.step(&quiet(), None);
        full.step_inner::<false>(&quiet(), None);
        assert_eq!(filtered.flakes.len(), 1);
        assert_eq!(
            filtered.contact_work.contours, 0,
            "an unreachable slab still built a contour"
        );
        assert_eq!(filtered.contact_work.edges, 0);
        assert_eq!(filtered.contact_work.skipped, 1);
        assert!(full.contact_work.contours > 0 && full.contact_work.edges > 0);
        equal_state(&mut filtered, &mut full);
    }
}
