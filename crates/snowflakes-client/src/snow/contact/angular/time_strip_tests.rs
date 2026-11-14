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

#[test]
fn an_accepted_earlier_hit_skips_the_cell_equations() {
    let m = AngularMotion::new(pose(100.0, 0.0, -0.02, 1.0), pose(100.0, 10.0, 0.02, 1.0));
    let edge = m.edge(Point::new(-40.0, -1.0), Point::new(40.0, 3.0));
    assert!(
        edge.contact(5.0, 5.0, [99.0, 101.0])
            .is_some_and(|h| h.time > 0.05)
    );
    monotone_tests::CALLS.set(0);
    let full = edge.contact_in_strip::<false>(5.0, 5.0, [99.0, 101.0], Some(0.05));
    assert!(monotone_tests::CALLS.get() > 0);
    monotone_tests::CALLS.set(0);
    let filtered = edge.contact_before(5.0, 5.0, [99.0, 101.0], Some(0.05));
    assert_eq!(bits(filtered), bits(full));
    assert_eq!(monotone_tests::CALLS.get(), 0, "still built cell equations");
}

#[test]
fn initial_solid_overlap_precedes_the_surface_projection_test() {
    let m = AngularMotion::new(pose(100.0, 2.0, -0.02, 1.0), pose(100.0, 3.0, 0.02, 1.0));
    let edge = m.edge(Point::new(-2.0, 1.0), Point::new(2.0, 1.0));
    let initial = edge.local.map(|p| edge.sample(p, -HALF).0);
    assert!(!edge.may_reach_before(0.0, [99.0, 101.0], initial, 0.0));
    let expected = edge.contact_in_strip::<false>(0.0, 5.0, [99.0, 101.0], Some(0.0));
    assert!(expected.is_some_and(|h| h.time == 0.0));
    assert_eq!(
        bits(edge.contact_before(0.0, 5.0, [99.0, 101.0], Some(0.0))),
        bits(expected)
    );
}

#[test]
fn a_curved_interior_hit_outside_the_endpoint_strip_is_retained() {
    let m = AngularMotion::new(pose(0.0, 0.0, 0.0, 1.0), pose(0.0, 0.0, 0.04, 1.0));
    let edge = m.edge(Point::new(-1.001, -100.0), Point::new(-0.999, -100.0));
    let y = -100.002;
    let expected = edge.contact_in_strip::<false>(y, 0.0, [-4.0, 4.0], Some(1.0));
    assert!(expected.is_some_and(|h| h.time > 0.25 && h.time < 0.75));
    assert_eq!(
        bits(edge.contact_before(y, 0.0, [-4.0, 4.0], Some(1.0))),
        bits(expected),
        "lost a real contact between chord endpoints"
    );
}

#[test]
fn cancellation_and_solver_residual_uncertainty_keep_the_full_path() {
    let m = AngularMotion::new(
        pose(0.0, 1_000_000.0, 0.0, 1.0),
        pose(0.0, 1_000_000.0, f32::from_bits(1), 1.0),
    );
    let edge = m.edge(
        Point::new(-1.0, -1_000_000.0),
        Point::new(1.0, -1_000_000.0),
    );
    let initial = edge.local.map(|p| edge.sample(p, -HALF).0);
    assert!(edge.may_reach_before(-5e-8, [-1.0, 1.0], initial, 1.0));
}

#[test]
fn invalid_limits_and_uncertain_projections_do_not_reject_the_solver() {
    let m = AngularMotion::new(pose(100.0, 0.0, -0.02, 1.0), pose(100.0, 10.0, 0.02, 1.0));
    let edge = m.edge(Point::new(-40.0, -1.0), Point::new(40.0, 3.0));
    let expected = bits(edge.contact(5.0, 5.0, [99.0, 101.0]));
    assert!(expected.is_some());
    for limit in [-1.0, 1.01, f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
        assert_eq!(
            bits(edge.contact_before(5.0, 5.0, [99.0, 101.0], Some(limit))),
            expected
        );
    }
    let initial = edge.local.map(|p| edge.sample(p, -HALF).0);
    for (y, span) in [
        (f64::NAN, [99.0, 101.0]),
        (f64::INFINITY, [99.0, 101.0]),
        (5.0, [f32::NAN, 101.0]),
        (5.0, [f32::NEG_INFINITY, f32::INFINITY]),
        (5.0, [101.0, 99.0]),
    ] {
        assert!(edge.may_reach_before(y, span, initial, 0.05));
    }
    let point = m.edge(Point::new(0.0, 0.0), Point::new(0.0, 0.0));
    let initial = point.local.map(|p| point.sample(p, -HALF).0);
    assert!(point.may_reach_before(500.0, [900.0, 901.0], initial, 0.05));
    for (before, after) in [(-1.0, 2.0), (-10.0, -1.0), (0.0, 1.0)] {
        let m = AngularMotion::new(pose(0.0, 0.0, 0.0, before), pose(0.0, 0.0, 0.04, after));
        let edge = m.edge(Point::new(-40.0, -1.0), Point::new(40.0, 3.0));
        let initial = edge.local.map(|p| edge.sample(p, -HALF).0);
        assert!(edge.may_reach_before(500.0, [900.0, 901.0], initial, 0.05));
    }
}

#[test]
fn all_improving_contacts_survive_scales_wraps_and_adjacent_float_limits() {
    let mut fixtures = 0;
    let mut queries = 0;
    let mut improving = 0;
    let mut rejected = 0;
    for evaluator in [Evaluator::portable(), Evaluator::new()] {
        for offset in [-1600.0, 100000.0] {
            for scale in [0.25, 1.0, 1.5, 2.5] {
                for angle in [
                    0.0,
                    0.01,
                    std::f32::consts::FRAC_PI_2,
                    std::f32::consts::TAU - 0.01,
                ] {
                    for spin in [-2.5, 2.5] {
                        for change in [0.96, 1.04] {
                            for speed in [4.0, 240.0] {
                                let m = AngularMotion::with_evaluator(
                                    pose(offset, 200.0, angle, scale),
                                    pose(
                                        offset + 22.0 * scale * crate::snow::DT,
                                        200.0 + speed * scale * crate::snow::DT,
                                        (angle + spin * crate::snow::DT)
                                            .rem_euclid(std::f32::consts::TAU),
                                        scale * change,
                                    ),
                                    evaluator,
                                );
                                for [a, b] in [
                                    [Point::new(-40.0, -2.0), Point::new(40.0, 6.0)],
                                    [Point::new(0.0, -40.0), Point::new(0.0, 40.0)],
                                    [Point::new(1.0, -3.0), Point::new(1.0, -3.0)],
                                ] {
                                    let edge = m.edge(a, b);
                                    let bounds = edge.bounds();
                                    let initial = edge.local.map(|p| edge.sample(p, -HALF).0);
                                    for dy in [-8.0, 0.0, 8.0] {
                                        let y = 200.0 + dy * scale;
                                        let base = (y + 100.0 * scale).max(bounds.bottom + scale);
                                        for span in [
                                            [offset - 2.0 * scale, offset + 2.0 * scale],
                                            [bounds.left, bounds.right],
                                            [bounds.left.next_down(), bounds.left.next_up()],
                                        ] {
                                            let original =
                                                edge.contact_in_strip::<false>(y, base, span, None);
                                            let mut limits = vec![0.0, 0.05, 0.5, 1.0];
                                            if let Some(hit) = original {
                                                limits.extend([
                                                    hit.time.next_down(),
                                                    hit.time,
                                                    hit.time.next_up(),
                                                ]);
                                            }
                                            fixtures += 1;
                                            for limit in limits {
                                                let expected = edge.contact_in_strip::<false>(
                                                    y,
                                                    base,
                                                    span,
                                                    Some(limit),
                                                );
                                                let actual =
                                                    edge.contact_before(y, base, span, Some(limit));
                                                if actual.is_some() {
                                                    assert_eq!(
                                                        bits(actual),
                                                        bits(expected),
                                                        "invented or changed contact"
                                                    );
                                                }
                                                if expected.is_some_and(|h| h.time < limit) {
                                                    improving += 1;
                                                    assert_eq!(
                                                        bits(actual),
                                                        bits(expected),
                                                        "lost earlier contact at {limit}"
                                                    );
                                                }
                                                if (0.0..=1.0).contains(&limit)
                                                    && !edge.may_reach_before(
                                                        f64::from(y),
                                                        span,
                                                        initial,
                                                        limit,
                                                    )
                                                {
                                                    if let Some(hit) = actual {
                                                        assert_eq!(
                                                            hit.time, 0.0,
                                                            "only initial overlap may precede this test"
                                                        );
                                                    } else {
                                                        rejected += 1;
                                                    }
                                                }
                                                queries += 1;
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    assert_eq!(fixtures, 13824);
    assert!(
        queries >= 55296 && improving > 100 && rejected > 100,
        "queries={queries}, improving={improving}, rejected={rejected}"
    );
    eprintln!(
        "time strip: fixtures={fixtures}, queries={queries}, improving={improving}, rejected={rejected}"
    );
}
