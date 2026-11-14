use super::*;
use std::cell::Cell;

thread_local! { pub(super) static PRUNED: Cell<usize> = const { Cell::new(0) }; }

fn polynomial(coefficients: &[f64]) -> Polynomial {
    let mut p = Polynomial([0.0; COEFFICIENTS]);
    p.0[..coefficients.len()].copy_from_slice(coefficients);
    p
}

fn bits(roots: &Roots) -> Vec<u64> {
    roots.iter().map(f64::to_bits).collect()
}

fn assert_improving_events(original: &Roots, filtered: &Roots, limit: f32) {
    let expected = bits(original);
    let actual = bits(filtered);
    assert!(actual.iter().all(|value| expected.contains(value)));
    for root in original.iter().filter(|x| ((*x + HALF) as f32) < limit) {
        assert!(
            actual.contains(&root.to_bits()),
            "lost root={root}, limit={limit}"
        );
    }
}

#[test]
fn known_hit_avoids_later_monotone_root_work() {
    for scale in [1e-300, 1.0, 1e300] {
        for sign in [-1.0, 1.0] {
            let p = polynomial(&[-0.2 * scale * sign, scale * sign, 0.001 * scale * sign]);
            for evaluator in [Evaluator::portable(), Evaluator::new()] {
                let original = p.roots_with(evaluator);
                assert_eq!(original.len, 1);
                assert!(original.values[0] + HALF > 0.125);
                Evaluator::reset_test_usage();
                assert_eq!(p.roots_for_hit(evaluator, Some(0.125)).len, 0);
                let calls = Evaluator::test_usage();
                assert_eq!(calls[2] + calls[3], 0, "still refined a later event");
            }
        }
    }
}

#[test]
fn earlier_roots_and_uncertain_inputs_keep_the_original_events() {
    for p in [
        polynomial(&[0.2, 1.0, 0.001]),
        polynomial(&[-0.04, 0.0, 1.0]),
        polynomial(&[0.0, 0.0, 0.0, 1.0]),
        polynomial(&[-0.125, 1.0]),
        polynomial(&[0.0]),
    ] {
        for evaluator in [Evaluator::portable(), Evaluator::new()] {
            let original = p.roots_with(evaluator);
            for limit in [-1.0, f32::NAN, f32::INFINITY, 2.0] {
                assert_eq!(
                    bits(&p.roots_for_hit(evaluator, Some(limit))),
                    bits(&original)
                );
            }
            assert_eq!(bits(&p.roots_for_hit(evaluator, None)), bits(&original));
            for limit in [0.0, 0.125, 0.5, 0.875, 1.0] {
                assert_improving_events(&original, &p.roots_for_hit(evaluator, Some(limit)), limit);
            }
        }
    }
}

#[test]
fn classified_boundary_and_subnormal_events_are_not_pruned_early() {
    let linear = 1.0 + 1024.0 * f64::EPSILON;
    for scale in [1e-300, 1.0, 1e300] {
        for sign in [-1.0, 1.0] {
            let p = polynomial(&[
                (0.5 * linear - 0.25 - f64::EPSILON) * scale * sign,
                linear * scale * sign,
                scale * sign,
            ]);
            for evaluator in [Evaluator::portable(), Evaluator::new()] {
                let original = p.roots_with(evaluator);
                assert_eq!(original.values[0].to_bits(), (-HALF).to_bits());
                let limit = f32::from_bits(1);
                assert_improving_events(&original, &p.roots_for_hit(evaluator, Some(limit)), limit);
            }
        }
    }
    for scale in [f64::MIN_POSITIVE, 1e-300, 1.0, 1e300] {
        for sign in [-1.0, 1.0] {
            for root in [-0.5, -0.125, 0.0, 0.5] {
                for offset in [-128.0, -1.0, 0.0, 1.0, 128.0] {
                    let root = root + offset * f64::EPSILON;
                    let quadratic = 0.001;
                    let p = polynomial(&[
                        -(root + quadratic * root * root) * scale * sign,
                        scale * sign,
                        quadratic * scale * sign,
                    ]);
                    for evaluator in [Evaluator::portable(), Evaluator::new()] {
                        let original = p.roots_with(evaluator);
                        let time = ((root + HALF) as f32).clamp(0.0, 1.0);
                        for limit in [time.next_down(), time, time.next_up()] {
                            assert_improving_events(
                                &original,
                                &p.roots_for_hit(evaluator, Some(limit)),
                                limit,
                            );
                        }
                    }
                }
            }
        }
    }
}

#[test]
fn generated_root_queries_preserve_every_improving_event() {
    let mut random = 0x63_5eed_u64;
    let mut checked = 0;
    for degree in 2..=12 {
        for scale in [1e-300, 1e-120, 1.0, 1e120, 1e300] {
            for sign in [-1.0, 1.0] {
                for case in 0..32 {
                    let mut coefficients = Vec::new();
                    for i in 0..=degree {
                        random = random.wrapping_mul(6364136223846793005).wrapping_add(1);
                        let sample = (random >> 11) as f64 / (1_u64 << 53) as f64 - 0.5;
                        coefficients.push(match i {
                            0 => sample * scale,
                            1 => scale * sign,
                            2 if case % 8 == 0 => sample * scale * 8.0,
                            _ => sample * scale * 0.04,
                        });
                    }
                    let p = polynomial(&coefficients);
                    for evaluator in [Evaluator::portable(), Evaluator::new()] {
                        let original = p.roots_with(evaluator);
                        for limit in [0.0, f32::from_bits(1), 0.125, 0.5, 0.875, 1.0] {
                            assert_improving_events(
                                &original,
                                &p.roots_for_hit(evaluator, Some(limit)),
                                limit,
                            );
                            checked += 1;
                        }
                    }
                }
            }
        }
    }
    assert_eq!(checked, 42240);
}

fn full_search<T>(f: impl FnOnce() -> T) -> T {
    struct Restore(bool);
    impl Drop for Restore {
        fn drop(&mut self) {
            super::monotone_tests::FULL_SEARCH.set(self.0);
        }
    }
    let _restore = Restore(super::monotone_tests::FULL_SEARCH.replace(true));
    f()
}

#[test]
fn curved_contacts_keep_the_first_hit_across_dpi_and_clipped_spans() {
    PRUNED.set(0);
    let mut checked = 0;
    for scale in [0.5, 1.0, 2.0, 2.5] {
        for center in [-1600.0_f32, 100.0] {
            for direction in [-1.0, 1.0] {
                let pose = |y, angle| RenderPose {
                    position: Point::new(center, y),
                    angle,
                    scale,
                    age: 0.0,
                };
                let motion =
                    AngularMotion::new(pose(0.0, -0.02), pose(10.0 * scale * direction, 0.02));
                let edge = motion.edge(Point::new(-40.0, -1.0), Point::new(40.0, 3.0));
                for height in [-10.0, -0.001, 0.0, 0.001, 5.0, 10.0, 15.0] {
                    for span in [
                        [center - scale, center + scale],
                        [center.next_up(), center + scale],
                        [center - 50.0 * scale, center + 50.0 * scale],
                    ] {
                        let y = height * scale;
                        let original = full_search(|| edge.contact(y, y + 3.0 * scale, span));
                        let actual = edge.contact(y, y + 3.0 * scale, span);
                        assert_eq!(
                            actual.map(|h| (h.time.to_bits(), h.x.to_bits())),
                            original.map(|h| (h.time.to_bits(), h.x.to_bits()))
                        );
                        checked += 1;
                    }
                }
            }
        }
    }
    assert_eq!(checked, 336);
    assert!(PRUNED.get() > 0, "geometry corpus never used the rejection");
}
