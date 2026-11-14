//! Skip only a certified prefix of the original dyadic bisection tree.
use super::horner;

pub(super) struct Jump {
    pub low: f64,
    pub high: f64,
    pub depth: usize,
}

#[inline(always)]
pub(super) fn enclose(coefficients: &[f64], interval: [f64; 2], f_low: f64) -> Option<Jump> {
    if interval != [-0.5, 0.5]
        || !(2..=13).contains(&coefficients.len())
        || !f_low.is_finite()
        || f_low == 0.0
    {
        return None;
    }
    let (mut norm, mut nonlinear, mut weight) = (0.0, 0.0, 1.0);
    for (i, &coefficient) in coefficients.iter().enumerate() {
        // This also keeps every Horner/derivative intermediate far from overflow.
        if !coefficient.is_finite() || coefficient.abs() > f64::MAX / 64.0 {
            return None;
        }
        norm += coefficient.abs() * weight;
        if i >= 2 {
            nonlinear += i as f64 * coefficient.abs() * (weight * 2.0);
        }
        weight *= 0.5;
    }
    let linear = coefficients[1].abs();
    let derivative_norm = linear + nonlinear;
    let margin = 256.0 * f64::EPSILON * derivative_norm.max(f64::MIN_POSITIVE);
    let lower = (linear - nonlinear - margin).next_down();
    if !lower.is_finite()
        || lower <= 0.0
        || f_low.is_sign_positive() == coefficients[1].is_sign_positive()
    {
        return None;
    }

    // Newton only proposes a centre. Its accuracy is never trusted for skipping.
    let mut centre = (-coefficients[0] / coefficients[1]).clamp(-0.5, 0.5);
    for _ in 0..2 {
        let (mut value, mut slope) = (0.0_f64, 0.0_f64);
        for &coefficient in coefficients.iter().rev() {
            slope = slope.mul_add(centre, value);
            value = value.mul_add(centre, coefficient);
        }
        if !slope.is_finite() || slope == 0.0 {
            break;
        }
        let next = centre - value / slope;
        if !(-0.5..=0.5).contains(&next) {
            break;
        }
        centre = next;
    }
    let value = horner(coefficients, centre);
    if !value.is_finite() {
        return None;
    }
    // For <=13 fused terms, E dominates Horner's absolute evaluation error on
    // this domain, including rounded bounds and gradual-underflow error.
    let error = (128.0 * f64::EPSILON * norm.max(f64::MIN_POSITIVE)).next_up();
    let radius = ((value.abs() + 2.0 * error).next_up() / lower).next_up();
    let left = (centre - radius).next_down();
    let right = (centre + radius).next_up();
    if !left.is_finite() || !right.is_finite() || left <= -0.5 || right >= 0.5 {
        return None;
    }
    // Outside this interval, |p| > E with the derivative's fixed sign. Thus no
    // skipped computed midpoint can be zero or choose the other binary branch.
    let normalized = [(left + 0.5).next_down(), (right + 0.5).next_up()];
    if normalized[0] <= 0.0 || normalized[1] >= 1.0 {
        return None;
    }
    let (mut depth, mut end) = (0, 49);
    while depth + 1 < end {
        let middle = (depth + end) / 2;
        if cell(normalized, middle).is_some() {
            depth = middle;
        } else {
            end = middle;
        }
    }
    if depth < 8 {
        return None;
    }
    let index = cell(normalized, depth)?;
    let step = power_of_two(-(depth as i32));
    // These dyadic boundaries are exact at depth <=48, well before adjacent
    // floats could terminate the original search. Caller deducts this depth.
    Some(Jump {
        low: index * step - 0.5,
        high: (index + 1.0) * step - 0.5,
        depth,
    })
}

#[inline(always)]
fn power_of_two(exponent: i32) -> f64 {
    f64::from_bits(((exponent + 1023) as u64) << 52)
}

#[inline(always)]
fn cell(normalized: [f64; 2], depth: usize) -> Option<f64> {
    let scale = power_of_two(depth as i32);
    let left = (normalized[0] * scale).floor();
    let right = (normalized[1] * scale).floor();
    (left == right && left >= 0.0 && left < scale).then_some(left)
}

#[cfg(test)]
thread_local! { static WORK: std::cell::Cell<[usize; 2]> = const { std::cell::Cell::new([0; 2]) }; }
#[cfg(test)]
pub(super) fn record_jump(depth: usize) {
    WORK.with(|v| {
        let mut w = v.get();
        w[1] += depth;
        v.set(w);
    });
}
#[cfg(test)]
pub(super) fn record_iteration() {
    WORK.with(|v| {
        let mut w = v.get();
        w[0] += 1;
        v.set(w);
    });
}

#[cfg(test)]
mod tests {
    use super::super::{Evaluator, bisect, portable};
    use super::*;

    fn fast(coefficients: &[f64], interval: [f64; 2], f_low: f64) -> (u64, [usize; 2]) {
        WORK.with(|v| v.set([0; 2]));
        let value = Evaluator::new().refine(coefficients, interval, f_low);
        (value.to_bits(), WORK.with(std::cell::Cell::get))
    }
    fn full(coefficients: &[f64], interval: [f64; 2], f_low: f64) -> (u64, [usize; 2]) {
        WORK.with(|v| v.set([0; 2]));
        let value = bisect::<false>(coefficients, interval, f_low);
        (value.to_bits(), WORK.with(std::cell::Cell::get))
    }

    #[test]
    fn certified_jump_preserves_root_bits_with_less_bisection_work() {
        for scale in [1e-300, 1e-120, 1.0, 1e120, 1e300] {
            for sign in [-1.0, 1.0] {
                let coefficients = [
                    -0.123456789 * scale * sign,
                    scale * sign,
                    0.002 * scale * sign,
                    -0.0003 * scale * sign,
                ];
                let f_low = portable(&coefficients, -0.5);
                let a = fast(&coefficients, [-0.5, 0.5], f_low);
                let b = full(&coefficients, [-0.5, 0.5], f_low);
                assert_eq!(a.0, b.0);
                assert!(
                    a.1[0] < b.1[0] && a.1[1] >= 16,
                    "certified interval still evaluated every bisection midpoint"
                );
            }
        }
    }

    #[test]
    fn canonical_budget_and_dyadic_stops_match_the_original_search() {
        for root in [
            0.0,
            0.125,
            -0.25,
            (-0.5_f64).next_up(),
            0.5_f64.next_down(),
            2.0_f64.powi(-70),
            -2.0_f64.powi(-70),
            0.000123456789,
            -0.000015729613,
        ] {
            for sign in [-1.0, 1.0] {
                let coefficients = [-root * sign, sign];
                let f_low = portable(&coefficients, -0.5);
                assert_eq!(
                    fast(&coefficients, [-0.5, 0.5], f_low).0,
                    full(&coefficients, [-0.5, 0.5], f_low).0,
                    "root={root}, sign={sign}"
                );
            }
        }
    }

    #[test]
    fn uncertain_slopes_and_exceptional_inputs_keep_the_full_interval() {
        for coefficients in [
            vec![],
            vec![1.0],
            vec![-0.125, 1.0, 2.0],
            vec![0.1, 0.0, 1.0],
            vec![0.1, 1e-20, 0.0, 1.0],
            vec![0.1, 1.0, f64::NAN],
            vec![0.1, 1.0, f64::INFINITY],
            vec![0.1, f64::MAX / 2.0, 0.1],
            vec![f64::from_bits(1), f64::from_bits(64), f64::from_bits(1)],
            vec![1.0; 14],
        ] {
            let f_low = portable(&coefficients, -0.5);
            assert!(enclose(&coefficients, [-0.5, 0.5], f_low).is_none());
        }
        for invalid in [0.0, f64::NAN, f64::INFINITY, 1.0] {
            assert!(enclose(&[-0.125, 1.0, 0.001], [-0.5, 0.5], invalid).is_none());
        }
    }

    #[test]
    fn every_skipped_midpoint_matches_the_original_path_across_scales() {
        let mut random = 0x60_5eed_u64;
        let (mut checked, mut jumps) = (0, 0);
        for degree in 2..=12 {
            for scale in [1e-300, 1e-120, 1.0, 1e120, 1e300] {
                for sign in [-1.0, 1.0] {
                    for _ in 0..256 {
                        random = random.wrapping_mul(6364136223846793005).wrapping_add(1);
                        let root = (random >> 11) as f64 / (1_u64 << 53) as f64 * 0.9 - 0.45;
                        let mut coefficients = vec![-root * scale * sign, scale * sign];
                        for _ in 2..=degree {
                            random = random.wrapping_mul(6364136223846793005).wrapping_add(1);
                            coefficients.push(
                                ((random >> 11) as f64 / (1_u64 << 53) as f64 - 0.5)
                                    * 0.04
                                    * scale
                                    * sign,
                            );
                        }
                        let f_low = portable(&coefficients, -0.5);
                        if let Some(jump) = enclose(&coefficients, [-0.5, 0.5], f_low) {
                            let (mut low, mut high, mut fa) = (-0.5, 0.5, f_low);
                            for _ in 0..jump.depth {
                                let mid = (low + high) * 0.5;
                                let f = portable(&coefficients, mid);
                                assert!(f.is_finite() && f != 0.0);
                                if f.is_sign_positive() == fa.is_sign_positive() {
                                    low = mid;
                                    fa = f;
                                } else {
                                    high = mid;
                                }
                            }
                            assert_eq!(low.to_bits(), jump.low.to_bits());
                            assert_eq!(high.to_bits(), jump.high.to_bits());
                            jumps += 1;
                        }
                        let reference = full(&coefficients, [-0.5, 0.5], f_low).0;
                        assert_eq!(
                            fast(&coefficients, [-0.5, 0.5], f_low).0,
                            reference,
                            "case={checked}"
                        );
                        assert_eq!(
                            Evaluator::portable()
                                .refine(&coefficients, [-0.5, 0.5], f_low)
                                .to_bits(),
                            reference,
                            "portable case={checked}"
                        );
                        checked += 1;
                    }
                }
            }
        }
        assert_eq!(checked, 28160);
        assert!(jumps > 10000, "only {jumps} certified prefixes");
    }

    #[test]
    fn flat_mixed_exponent_coefficients_and_partial_intervals_preserve_bits() {
        for coefficients in [
            vec![-0.123456789, 1.0, f64::from_bits(1), f64::MIN_POSITIVE],
            vec![-0.123456789e300, 1e300, 1e-300, -f64::from_bits(1)],
        ] {
            let f_low = portable(&coefficients, -0.5);
            assert!(enclose(&coefficients, [-0.5, 0.5], f_low).is_some());
            let reference = full(&coefficients, [-0.5, 0.5], f_low).0;
            for evaluator in [Evaluator::portable(), Evaluator::new()] {
                assert_eq!(
                    evaluator
                        .refine(&coefficients, [-0.5, 0.5], f_low)
                        .to_bits(),
                    reference
                );
            }
        }
        for scale in [1e-300, 1.0, 1e300] {
            for quadratic in [1.0 - 1e-10, 1.0, 1.0 + 1e-10] {
                for root in [-0.49, -0.1, 0.05, 0.4] {
                    let coefficients = [
                        -(root + quadratic * root * root) * scale,
                        scale,
                        quadratic * scale,
                    ];
                    for interval in [[-0.5, 0.5], [-0.37, 0.29], [-0.499999, 0.499999]] {
                        let f_low = portable(&coefficients, interval[0]);
                        if interval != [-0.5, 0.5] {
                            assert!(enclose(&coefficients, interval, f_low).is_none());
                        }
                        assert_eq!(
                            fast(&coefficients, interval, f_low).0,
                            full(&coefficients, interval, f_low).0
                        );
                    }
                }
            }
        }
    }
}
