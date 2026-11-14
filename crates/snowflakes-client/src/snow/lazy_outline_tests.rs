use super::*;
use std::collections::VecDeque;

fn slab(heights: Vec<f32>, cell_width: f32, center: Point) -> SnowSlab {
    SnowSlab {
        heights,
        cell_width,
        center,
    }
}

fn same_point(actual: Point, expected: Point) {
    for (actual, expected) in [(actual.x, expected.x), (actual.y, expected.y)] {
        if expected.is_nan() {
            assert!(actual.is_nan());
        } else {
            assert_eq!(actual.to_bits(), expected.to_bits());
        }
    }
}

fn check<const LAZY: bool>(body: &SnowSlab) -> usize {
    let expected: Vec<_> = body.contours().collect();
    let actual: Vec<_> = body.contact_outlines::<LAZY>().collect();
    assert_eq!(actual.len(), expected.len());
    let mut checked = 0;
    for ((start, points), expected) in actual.into_iter().zip(&expected) {
        same_point(start, *expected.last().unwrap());
        let actual: Vec<_> = points.collect();
        assert_eq!(actual.len(), expected.len());
        for (actual, &expected) in actual.into_iter().zip(expected) {
            same_point(actual, expected);
            checked += 1;
        }
    }
    // The complete order is also required after mixed front/back consumption.
    for ((start, mut points), expected) in body.contact_outlines::<LAZY>().zip(&expected) {
        same_point(start, *expected.last().unwrap());
        let mut remaining: VecDeque<_> = expected.iter().copied().collect();
        let mut back = true;
        while !remaining.is_empty() {
            let (actual, expected) = if back {
                (points.next_back(), remaining.pop_back())
            } else {
                (points.next(), remaining.pop_front())
            };
            same_point(actual.unwrap(), expected.unwrap());
            back = !back;
        }
        assert!(points.next().is_none());
        assert!(points.next_back().is_none());
    }
    for ((_, points), expected) in body.contact_outlines::<LAZY>().zip(&expected) {
        let actual: Vec<_> = points.rev().collect();
        assert_eq!(actual.len(), expected.len());
        for (actual, &expected) in actual.into_iter().zip(expected.iter().rev()) {
            same_point(actual, expected);
        }
    }
    checked
}

#[test]
fn stopping_after_the_first_edge_rounds_only_the_boundary_corners() {
    let body = slab(vec![8.0; 8192], 6.0, Point::new(123.0, -4.0));
    CORNERS.set(0);
    let (_, mut points) = body.contact_outlines::<true>().next().unwrap();
    assert!(points.next().is_some());
    let lazy = CORNERS.get();
    assert!(
        lazy > 0 && lazy <= 2,
        "rounded {lazy} corners before the first edge"
    );
    drop(points);
    CORNERS.set(0);
    let (_, mut points) = body.contact_outlines::<false>().next().unwrap();
    assert!(points.next().is_some());
    assert_eq!(CORNERS.get(), body.heights.len() + 4);
}

#[test]
fn separate_runs_and_the_closing_edge_match_the_original_contour() {
    let body = slab(
        vec![
            0.0,
            8.0,
            8.0,
            -0.0,
            -2.0,
            f32::NAN,
            4.0,
            f32::INFINITY,
            f32::NEG_INFINITY,
            9.0,
            0.0,
        ],
        6.0,
        Point::new(21.0, -3.0),
    );
    assert_eq!(body.contours().count(), 3);
    assert!(check::<true>(&body) > 30);
    check::<false>(&body);
    for ((mut previous, points), expected) in body.contact_outlines::<true>().zip(body.contours()) {
        let mut old_previous = *expected.last().unwrap();
        let mut edges = 0;
        for (point, expected) in points.zip(&expected) {
            same_point(previous, old_previous);
            same_point(point, *expected);
            previous = point;
            old_previous = *expected;
            edges += 1;
        }
        assert_eq!(edges, expected.len());
        same_point(previous, *expected.last().unwrap());
    }
}

#[test]
fn short_straight_and_tiny_corners_keep_literal_point_bits() {
    let mut points = 0;
    for heights in [
        vec![],
        vec![0.0, -0.0, f32::NAN],
        vec![1.0],
        vec![8.0; 5],
        vec![1.0, 2.0, 3.0, 4.0],
        vec![0.00001; 4],
        vec![f32::from_bits(1), f32::MIN_POSITIVE, 100.0],
    ] {
        for width in [0.0, -0.0, 0.0001, 0.3, 6.0, -6.0, 25.0] {
            for center in [
                Point::new(0.0, -0.0),
                Point::new(17.0, -2.0),
                Point::new(-100000.0, 1000000.0),
            ] {
                points += check::<true>(&slab(heights.clone(), width, center));
            }
        }
    }
    assert!(points > 1000);
}

#[test]
fn exceptional_coordinates_and_overflow_keep_the_original_classification() {
    for width in [
        f32::MAX,
        f32::MIN_POSITIVE,
        f32::from_bits(1),
        f32::INFINITY,
        f32::NAN,
    ] {
        for center in [
            Point::new(0.0, 0.0),
            Point::new(f32::MAX, -f32::MAX),
            Point::new(f32::NAN, 0.0),
        ] {
            check::<true>(&slab(vec![f32::MAX, 1.0, 0.0, 5.0], width, center));
        }
    }
}

#[test]
fn generated_profiles_keep_all_points_in_both_directions() {
    let mut state = 0x9137_a5c6_7b21_4301_u64;
    let mut random = || {
        state = state
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        (state >> 32) as u32
    };
    let mut points = 0;
    for case in 0..1024 {
        let len = (random() % 97) as usize;
        let heights = (0..len)
            .map(|_| match random() % 11 {
                0 => 0.0,
                1 => -1.0,
                2 => f32::NAN,
                3 => f32::INFINITY,
                4 => f32::from_bits(1),
                _ => f32::from_bits(random() % f32::INFINITY.to_bits()),
            })
            .collect();
        let width = [0.0001, 0.3, 1.5, 6.0, 25.0, 200000.0][case % 6];
        let center = Point::new(
            (random() as i32) as f32 * 0.001,
            (random() as i32) as f32 * 0.001,
        );
        points += check::<true>(&slab(heights, width, center));
    }
    assert!(points > 10000, "only {points} points");
    eprintln!(
        "lazy outline: profiles=1024, forward_points={points}; reverse and mixed also checked"
    );
}
