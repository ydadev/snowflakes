use super::*;
use std::cell::Cell;

thread_local! {
    pub(super) static FULL_SEARCH: Cell<bool> = const { Cell::new(false) };
    pub(super) static CALLS: Cell<usize> = const { Cell::new(0) };
}

fn solve<const CERTIFY: bool>(p: Polynomial) -> (Vec<u64>, usize) {
    CALLS.set(0);
    let roots = p
        .roots_inner::<CERTIFY>()
        .iter()
        .map(f64::to_bits)
        .collect();
    (roots, CALLS.get())
}

fn factored(roots: &[f64]) -> Polynomial {
    let mut p = Polynomial([0.0; COEFFICIENTS]);
    p.0[0] = 1.0;
    for (degree, &root) in roots.iter().enumerate() {
        for i in (0..=degree + 1).rev() {
            p.0[i] = if i == 0 { 0.0 } else { p.0[i - 1] } - root * p.0[i];
        }
    }
    p
}

#[test]
fn certified_monotone_polynomials_skip_derivatives_without_changing_root_bits() {
    for scale in [1e-300, 1e-120, 1.0, 1e120, 1e300] {
        for sign in [-1.0, 1.0] {
            for offset in [-2.0, -1.00625, -0.5, 0.0, 0.5, 0.99625, 2.0] {
                let mut p = Polynomial([0.0; COEFFICIENTS]);
                for (i, coefficient) in [offset, 2.0, 0.02, 0.01].into_iter().enumerate() {
                    p.0[i] = coefficient * scale * sign;
                }
                let fast = solve::<true>(p);
                let full = solve::<false>(p);
                assert_eq!(
                    fast.0, full.0,
                    "scale={scale}, offset={offset}, sign={sign}"
                );
                assert!(
                    fast.1 < full.1,
                    "a provably signed derivative still entered the recursive search"
                );
                assert_eq!(fast.1, 1);
            }
        }
    }
}

#[test]
fn uncertain_slopes_and_turning_roots_keep_the_complete_search() {
    for p in [
        factored(&[-0.5, -0.2, 0.0, 0.2, 0.2, 0.5]),
        factored(&[0.0, 0.0]),
        factored(&[-0.4, -0.1, 0.3]),
        factored(&[0.1, 0.1, 0.1]),
    ] {
        let fast = solve::<true>(p);
        let full = solve::<false>(p);
        assert_eq!(fast.0, full.0);
        assert!(full.1 > 1);
    }
    // The slope is mathematically positive, but the coefficient bound cannot
    // certify it. Near-zero derivatives must keep the original path.
    let mut p = Polynomial([0.0; COEFFICIENTS]);
    p.0[1] = 1e-20;
    p.0[3] = 1.0;
    assert_eq!(solve::<true>(p), solve::<false>(p));
}

#[test]
fn uncertain_nonfinite_and_subnormal_slope_bounds_fail_closed() {
    for linear in [1.0 - 4.0 * f64::EPSILON, 1.0, 1.0 + 4.0 * f64::EPSILON] {
        let mut p = Polynomial([0.0; COEFFICIENTS]);
        p.0[1] = linear;
        p.0[2] = 1.0;
        assert!(!p.derivative_is_signed(2));
        assert_eq!(solve::<true>(p), solve::<false>(p));
    }
    for (linear, quadratic) in [
        (f64::NAN, 1.0),
        (f64::INFINITY, 1.0),
        (1.0, f64::NAN),
        (1.0, f64::INFINITY),
        (f64::MAX, f64::MAX),
        (f64::from_bits(64), f64::from_bits(1)),
    ] {
        let mut p = Polynomial([0.0; COEFFICIENTS]);
        p.0[1] = linear;
        p.0[2] = quadratic;
        assert!(!p.derivative_is_signed(2));
    }
}

#[test]
fn monotone_filter_matches_full_roots_for_contact_polynomials() {
    let (mut checked, mut reduced) = (0, 0);
    for scale in [0.25, 1.0, 2.5, 8.0] {
        for offset in [-100_000.0, 0.0, 100_000.0] {
            for angle in [
                0.01,
                std::f32::consts::FRAC_PI_2,
                std::f32::consts::TAU - 0.01,
            ] {
                for spin in [-2.5, 0.4, 2.5] {
                    for change in [0.96, 1.04] {
                        for speed in [4.0, 240.0] {
                            let before = RenderPose {
                                position: Point::new(offset, 200.0),
                                angle,
                                scale,
                                age: 0.0,
                            };
                            let after = RenderPose {
                                position: Point::new(
                                    offset + 22.0 * scale * crate::snow::DT,
                                    200.0 + speed * scale * crate::snow::DT,
                                ),
                                angle: (angle + spin * crate::snow::DT)
                                    .rem_euclid(std::f32::consts::TAU),
                                scale: scale * change,
                                age: crate::snow::DT,
                            };
                            let m = AngularMotion::new(before, after);
                            let edge = m.edge(Point::new(-40.0, -2.0), Point::new(40.0, 6.0));
                            let e = [
                                edge.local[1][0] - edge.local[0][0],
                                edge.local[1][1] - edge.local[0][1],
                            ];
                            let cross = edge.local[0][0] * e[1] - edge.local[0][1] * e[0];
                            for delta_y in [-60.0, -2.0, 0.0, 2.0, 60.0] {
                                let y = m.center[1] + delta_y * f64::from(scale);
                                let mut polynomials = Vec::new();
                                for p in edge.local {
                                    polynomials.push(m.polynomial(
                                        [m.scale * p[1], m.scale_delta * p[1]],
                                        [m.scale * p[0], m.scale_delta * p[0]],
                                        [m.center[1] - y, m.velocity[1]],
                                    ));
                                }
                                for qx in [-20.0, 20.0] {
                                    let q = [qx * f64::from(scale), y - m.center[1]];
                                    polynomials.push(m.polynomial(
                                        [
                                            q[0] * e[1] - q[1] * e[0],
                                            -m.velocity[0] * e[1] + m.velocity[1] * e[0],
                                        ],
                                        [
                                            q[0] * e[0] + q[1] * e[1],
                                            -m.velocity[0] * e[0] - m.velocity[1] * e[1],
                                        ],
                                        [-m.scale * cross, -m.scale_delta * cross],
                                    ));
                                }
                                for p in polynomials {
                                    let fast = solve::<true>(p);
                                    let full = solve::<false>(p);
                                    assert_eq!(fast.0, full.0, "contact polynomial {checked}");
                                    reduced += usize::from(fast.1 < full.1);
                                    checked += 1;
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    assert_eq!(checked, 8640);
    assert!(
        reduced > 1000,
        "only {reduced} reduced searches among {checked}"
    );
}

fn with_full_search<T>(f: impl FnOnce() -> T) -> T {
    struct Restore(bool);
    impl Drop for Restore {
        fn drop(&mut self) {
            FULL_SEARCH.set(self.0);
        }
    }
    let _restore = Restore(FULL_SEARCH.replace(true));
    f()
}

#[test]
fn world_states_match_full_roots_through_contact_pause_resize_and_closure() {
    use crate::{
        geometry::{Scene, Screen, Window},
        snow::{DT, Flake, SnowSlab, World},
    };
    use snowflakes_protocol::Settings;
    let mut scene = Scene::default();
    for (key, left, right, scale) in [(1, -800.0, 0.0, 1.0), (2, 0.0, 800.0, 2.5)] {
        scene.screens.push(Screen {
            key,
            persistent_id: true,
            rect: Rect {
                left,
                right,
                top: 0.0,
                bottom: 900.0,
            },
            floor: 900.0,
            scale,
        });
    }
    for (key, left, right, top) in [(8, -100.0, 100.0, 300.0), (7, -700.0, 700.0, 410.0)] {
        scene.windows.push(Window {
            key,
            rect: Rect {
                left,
                right,
                top,
                bottom: top + 100.0,
            },
            visible: true,
            collect: true,
        });
    }
    let mut settings = Settings::default();
    settings.snow.intensity_percent = 0;
    settings.snow.melting = false;
    settings.snow.drift_size_percent = 100;
    let mut hits = 0;
    for seed in [17, 24301, 0xdead_beef, 99] {
        let mut current_scene = scene.clone();
        let mut fast = World::new(scene.clone(), seed);
        let mut full = World::new(scene.clone(), seed);
        for step in 0..180 {
            if step == 60 {
                for world in [&mut fast, &mut full] {
                    world.set_paused_monitors([2].into());
                }
            }
            if step == 90 {
                for world in [&mut fast, &mut full] {
                    world.set_paused_monitors(Default::default());
                }
            }
            if step == 120 {
                current_scene.windows[1].rect.left += 35.0;
                current_scene.windows[1].rect.right -= 55.0;
                fast.set_scene(current_scene.clone());
                full.set_scene(current_scene.clone());
            }
            if step == 150 {
                current_scene.windows.remove(0);
                fast.set_scene(current_scene.clone());
                full.set_scene(current_scene.clone());
            }
            if step % 15 == 0 {
                for (i, x) in [-300.0, -5.0, 300.0].into_iter().enumerate() {
                    let mut body = Flake::new(Point::new(x, 270.0), (320.0_f32 / 1.2).sqrt());
                    body.mass = 320.0;
                    body.velocity = Point::new(20.0, 150.0);
                    body.angle =
                        (i as f32 * 2.0 + step as f32 * DT).rem_euclid(std::f32::consts::TAU);
                    body.spin = if i % 2 == 0 { 2.4 } else { -2.4 };
                    body.slab = Some(SnowSlab {
                        heights: vec![4.0, 0.0, 4.0, 8.0],
                        cell_width: 20.0,
                        center: Point::new(40.0, -2.0),
                    });
                    assert!(fast.release(body.clone()));
                    assert!(full.release(body));
                }
            }
            settings.snow.flake_size_percent = if step < 90 { 25 } else { 200 };
            fast.step(&settings, None);
            with_full_search(|| full.step(&settings, None));
            hits += fast.contact_work.hits;
            assert_eq!(
                format!("{fast:?}"),
                format!("{full:?}"),
                "seed={seed}, step={step}"
            );
            assert!(
                (fast.introduced_mass - fast.removed_mass - fast.total_mass()).abs()
                    <= fast.introduced_mass.max(1.0) * 0.001
            );
        }
    }
    assert!(hits >= 12);
}
