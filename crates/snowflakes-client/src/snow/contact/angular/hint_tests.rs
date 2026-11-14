use super::late_tests::full_search;
use super::*;

fn pose(x: f32, y: f32, angle: f32, scale: f32) -> RenderPose {
    RenderPose {
        position: Point::new(x, y),
        angle,
        scale,
        age: 0.0,
    }
}

fn bits(hit: Option<EdgeHit>) -> Option<(u32, u32)> {
    hit.map(|h| (h.time.to_bits(), h.x.to_bits()))
}

fn assert_real_improving_hit(original: Option<EdgeHit>, actual: Option<EdgeHit>, limit: f32) {
    if actual.is_some() {
        assert_eq!(
            bits(actual),
            bits(original),
            "returned a different or fictitious hit"
        );
    }
    if original.is_some_and(|hit| hit.time < limit) {
        assert_eq!(
            bits(actual),
            bits(original),
            "lost an improving hit at limit={limit}"
        );
    }
}

#[test]
fn external_hit_hint_reduces_refinement_without_forging_a_contact() {
    let motion = AngularMotion::new(pose(100.0, 0.0, -0.02, 1.0), pose(100.0, 10.0, 0.02, 1.0));
    let edge = motion.edge(Point::new(-40.0, -1.0), Point::new(40.0, 3.0));
    Evaluator::reset_test_usage();
    let original = edge.contact(5.0, 5.0, [99.0, 101.0]);
    let full_calls = Evaluator::test_usage();
    assert!(original.is_some_and(|hit| hit.time > 0.05));
    Evaluator::reset_test_usage();
    let actual = edge.contact_before(5.0, 5.0, [99.0, 101.0], Some(0.05));
    let hinted_calls = Evaluator::test_usage();
    assert_real_improving_hit(original, actual, 0.05);
    assert!(
        hinted_calls[2] + hinted_calls[3] < full_calls[2] + full_calls[3],
        "external time did not avoid any refinement"
    );
}

#[test]
fn hints_never_create_hits_and_invalid_times_keep_the_original_result() {
    let motion = AngularMotion::new(pose(100.0, 0.0, -0.02, 1.0), pose(100.0, 10.0, 0.02, 1.0));
    let edge = motion.edge(Point::new(-40.0, -1.0), Point::new(40.0, 3.0));
    assert!(edge.contact(1000.0, 1000.0, [99.0, 101.0]).is_none());
    for hint in [None, Some(0.0), Some(0.05), Some(0.5), Some(1.0)] {
        assert!(
            edge.contact_before(1000.0, 1000.0, [99.0, 101.0], hint)
                .is_none()
        );
    }
    for (y, base) in [(0.0, 5.0), (5.0, 5.0)] {
        let original = edge.contact(y, base, [99.0, 101.0]);
        assert!(original.is_some());
        for hint in [
            None,
            Some(-1.0),
            Some(2.0),
            Some(f32::NAN),
            Some(f32::INFINITY),
        ] {
            assert_eq!(
                bits(edge.contact_before(y, base, [99.0, 101.0], hint)),
                bits(original)
            );
        }
        if original.is_some_and(|hit| hit.time == 0.0) {
            assert_eq!(
                bits(edge.contact_before(y, base, [99.0, 101.0], Some(0.1))),
                bits(original)
            );
        }
    }
}

#[test]
fn linear_edge_and_zero_rotation_keep_the_default_contact_path() {
    let linear = SweptEdge {
        before: [Point::new(80.0, 0.0), Point::new(120.0, 0.0)],
        after: [Point::new(80.0, 20.0), Point::new(120.0, 20.0)],
    };
    let motion = AngularMotion::new(pose(100.0, 0.0, 0.0, 1.0), pose(100.0, 20.0, 0.0, 1.0));
    let angular = motion.edge(Point::new(-20.0, 0.0), Point::new(20.0, 0.0));
    for span in [[99.0, 101.0], [80.0, 80.0], [130.0, 140.0]] {
        let expected = linear.contact(10.0, 10.0, span);
        assert_eq!(bits(angular.contact(10.0, 10.0, span)), bits(expected));
        for hint in [
            None,
            Some(0.0),
            Some(0.1),
            Some(0.5),
            Some(1.0),
            Some(f32::NAN),
        ] {
            assert_eq!(
                bits(linear.contact_before(10.0, 10.0, span, hint)),
                bits(expected)
            );
            assert_eq!(
                bits(angular.contact_before(10.0, 10.0, span, hint)),
                bits(expected)
            );
        }
    }
}

#[test]
fn generated_external_hints_preserve_every_improving_curved_contact() {
    let mut checked = 0;
    for scale in [0.5, 1.0, 2.0, 2.5] {
        for center in [-1600.0_f32, 100.0] {
            for direction in [-1.0, 1.0] {
                let motion = AngularMotion::new(
                    pose(center, 0.0, -0.02, scale),
                    pose(center, 10.0 * scale * direction, 0.02, scale),
                );
                let edge = motion.edge(Point::new(-40.0, -1.0), Point::new(40.0, 3.0));
                for height in [-10.0, -0.001, 0.0, 0.001, 5.0, 10.0, 15.0] {
                    for span in [
                        [center - scale, center + scale],
                        [center.next_up(), center + scale],
                        [center - 50.0 * scale, center + 50.0 * scale],
                    ] {
                        let y = height * scale;
                        let original = full_search(|| edge.contact(y, y + 3.0 * scale, span));
                        let time = original.map_or(0.5, |hit| hit.time);
                        for limit in [
                            0.0,
                            0.05,
                            0.25,
                            0.5,
                            0.75,
                            1.0,
                            time.next_down(),
                            time,
                            time.next_up(),
                        ] {
                            let actual = edge.contact_before(y, y + 3.0 * scale, span, Some(limit));
                            assert_real_improving_hit(original, actual, limit);
                            checked += 1;
                        }
                    }
                }
            }
        }
    }
    assert_eq!(checked, 3024);
}

#[test]
fn cell_hints_use_only_accepted_hits_and_keep_the_first_equal_winner() {
    use crate::{
        geometry::{Scene, Screen, Window},
        snow::{ContactCandidates, FallingMask, World},
    };
    use snowflakes_protocol::Settings;
    use std::cell::RefCell;

    struct Probe<'a>(&'a RefCell<Vec<(Option<f32>, [f32; 2])>>);
    impl EdgeSweep for Probe<'_> {
        fn bounds(&self) -> Rect {
            Rect {
                left: 20.0,
                top: 90.0,
                right: 280.0,
                bottom: 110.0,
            }
        }
        fn contact(&self, y: f32, base: f32, span: [f32; 2]) -> Option<EdgeHit> {
            self.contact_before(y, base, span, None)
        }
        fn contact_before(
            &self,
            _: f32,
            _: f32,
            span: [f32; 2],
            hint: Option<f32>,
        ) -> Option<EdgeHit> {
            let index = self.0.borrow().len();
            self.0.borrow_mut().push((hint, span));
            Some(EdgeHit {
                time: match index {
                    0 => 0.01,
                    1 => 0.7,
                    _ => 0.4,
                },
                x: if index == 0 {
                    f32::INFINITY
                } else {
                    (span[0] + span[1]) * 0.5
                },
            })
        }
    }
    let scene = Scene {
        screens: vec![Screen {
            key: 1,
            persistent_id: true,
            rect: Rect {
                left: 0.0,
                top: 0.0,
                right: 900.0,
                bottom: 900.0,
            },
            floor: 900.0,
            scale: 1.0,
        }],
        windows: vec![Window {
            key: 7,
            rect: Rect {
                left: 20.0,
                top: 100.0,
                right: 300.0,
                bottom: 300.0,
            },
            visible: true,
            collect: true,
        }],
    };
    let mut world = World::new(scene, 64);
    let mut settings = Settings::default();
    settings.snow.on_ground = false;
    for (outer, expected) in [
        (Some(0.9), [Some(0.9), Some(0.9), Some(0.7), Some(0.4)]),
        (Some(0.3), [Some(0.3); 4]),
        (Some(f32::NAN), [None, None, Some(0.7), Some(0.4)]),
    ] {
        let observed = RefCell::new(Vec::new());
        let hit = world
            .trace_edge_contact_limited::<true>(
                Probe(&observed),
                None,
                &settings,
                &mut ContactCandidates::default(),
                outer,
            )
            .unwrap();
        let samples = observed.borrow();
        assert!(samples.len() >= 4);
        assert_eq!(
            samples[..4]
                .iter()
                .map(|sample| sample.0)
                .collect::<Vec<_>>(),
            expected
        );
        let first_equal_index = world.drifts[&7].index((samples[2].1[0] + samples[2].1[1]) * 0.5);
        assert_eq!((hit.0, hit.1, hit.2), (7, first_equal_index, 0.4));
    }
    world.falling_masks.insert(
        900,
        FallingMask {
            front: [7].into(),
            covers: Vec::new(),
            hidden: false,
            coalesced: false,
            origins: Default::default(),
        },
    );
    for skip in [Some(7), Some(900)] {
        let observed = RefCell::new(Vec::new());
        assert!(
            world
                .trace_edge_contact_limited::<true>(
                    Probe(&observed),
                    skip,
                    &settings,
                    &mut ContactCandidates::default(),
                    Some(0.9),
                )
                .is_none()
        );
        assert!(observed.borrow().is_empty());
    }
    world.set_paused_monitors([1].into());
    let observed = RefCell::new(Vec::new());
    assert!(
        world
            .trace_edge_contact_limited::<true>(
                Probe(&observed),
                None,
                &settings,
                &mut ContactCandidates::default(),
                Some(0.9),
            )
            .is_none()
    );
    assert!(observed.borrow().is_empty());
}
