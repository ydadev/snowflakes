//! Fused Horner evaluation; CPU-specific instructions stay behind detection.
mod jump;
type Evaluate = unsafe fn(&[f64], f64) -> f64;
type Refine = unsafe fn(&[f64], [f64; 2], f64) -> f64;

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ContactBackend {
    Portable,
    Fma,
}

#[derive(Clone, Copy)]
pub(in crate::snow) struct Evaluator {
    evaluate: Evaluate,
    refine: Refine,
    backend: ContactBackend,
}

impl std::fmt::Debug for Evaluator {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("Evaluator").field(&self.backend).finish()
    }
}

#[cfg(test)]
thread_local! { static USED: std::cell::Cell<[usize; 4]> = const { std::cell::Cell::new([0; 4]) }; }
#[cfg(test)]
fn used(slot: usize) {
    USED.with(|v| {
        let mut counts = v.get();
        counts[slot] += 1;
        v.set(counts);
    });
}

impl Evaluator {
    pub fn new() -> Self {
        Self::choose(true)
    }

    pub fn portable() -> Self {
        Self {
            evaluate: portable,
            refine: portable_refine,
            backend: ContactBackend::Portable,
        }
    }

    fn choose(allow_acceleration: bool) -> Self {
        #[cfg(target_arch = "x86_64")]
        if allow_acceleration && std::is_x86_feature_detected!("fma") {
            return Self {
                evaluate: accelerated,
                refine: accelerated_refine,
                backend: ContactBackend::Fma,
            };
        }
        let _ = allow_acceleration;
        Self::portable()
    }

    pub fn backend(self) -> ContactBackend {
        self.backend
    }

    #[cfg(test)]
    pub fn reset_test_usage() {
        USED.with(|v| v.set([0; 4]));
    }
    #[cfg(test)]
    pub fn test_usage() -> [usize; 4] {
        USED.with(std::cell::Cell::get)
    }

    pub fn value(self, coefficients: &[f64], x: f64) -> f64 {
        // SAFETY: constructors only select the portable function or a function
        // whose CPU/OS feature requirements have been checked. Fields are private.
        unsafe { (self.evaluate)(coefficients, x) }
    }

    pub fn refine(self, coefficients: &[f64], interval: [f64; 2], f_low: f64) -> f64 {
        // SAFETY: the same private constructors guard the refinement function's
        // CPU/OS requirements. The selected backend lasts for this root search.
        unsafe { (self.refine)(coefficients, interval, f_low) }
    }
}

fn portable(coefficients: &[f64], x: f64) -> f64 {
    #[cfg(test)]
    used(0);
    coefficients.iter().rev().fold(0.0, |a, b| a.mul_add(x, *b))
}

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "fma")]
unsafe fn accelerated(coefficients: &[f64], x: f64) -> f64 {
    #[cfg(test)]
    used(1);
    horner(coefficients, x)
}

#[inline(always)]
fn horner(coefficients: &[f64], x: f64) -> f64 {
    let mut value = 0.0_f64;
    for &coefficient in coefficients.iter().rev() {
        value = value.mul_add(x, coefficient);
    }
    value
}

fn portable_refine(coefficients: &[f64], interval: [f64; 2], f_low: f64) -> f64 {
    #[cfg(test)]
    used(2);
    bisect::<true>(coefficients, interval, f_low)
}

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "fma")]
unsafe fn accelerated_refine(coefficients: &[f64], interval: [f64; 2], f_low: f64) -> f64 {
    #[cfg(test)]
    used(3);
    bisect::<true>(coefficients, interval, f_low)
}

#[inline(always)]
fn bisect<const JUMP: bool>(
    coefficients: &[f64],
    [mut low, mut high]: [f64; 2],
    mut f_low: f64,
) -> f64 {
    let mut remaining = 56;
    if JUMP && let Some(jump) = jump::enclose(coefficients, [low, high], f_low) {
        low = jump.low;
        high = jump.high;
        remaining -= jump.depth;
        #[cfg(test)]
        jump::record_jump(jump.depth);
    }
    for _ in 0..remaining {
        #[cfg(test)]
        jump::record_iteration();
        let mid = (low + high) * 0.5;
        if mid == low || mid == high {
            break;
        }
        let f = horner(coefficients, mid);
        if f == 0.0 {
            low = mid;
            high = mid;
            break;
        }
        if f.is_sign_positive() == f_low.is_sign_positive() {
            low = mid;
            f_low = f;
        } else {
            high = mid;
        }
    }
    (low + high) * 0.5
}

#[cfg(test)]
mod tests {
    use super::*;

    fn equal_bits_or_nan(actual: f64, expected: f64) {
        if expected.is_nan() {
            assert!(actual.is_nan());
        } else {
            assert_eq!(actual.to_bits(), expected.to_bits());
        }
    }

    #[test]
    fn runtime_selection_matches_fma_availability() {
        let evaluator = Evaluator::new();
        #[cfg(target_arch = "x86_64")]
        assert_eq!(
            evaluator.backend() == ContactBackend::Fma,
            std::is_x86_feature_detected!("fma")
        );
        #[cfg(not(target_arch = "x86_64"))]
        assert_eq!(evaluator.backend(), ContactBackend::Portable);
        assert_eq!(evaluator.value(&[1.0, 2.0, 3.0], 0.5), 2.75);
    }

    #[test]
    fn disabled_acceleration_always_keeps_portable_evaluation() {
        let evaluator = Evaluator::choose(false);
        assert_eq!(evaluator.backend(), ContactBackend::Portable);
        assert_eq!(evaluator.value(&[], 0.5), 0.0);
        assert_eq!(evaluator.value(&[1.0, 2.0, 3.0], 0.5), 2.75);
    }

    #[test]
    fn reported_backend_tracks_the_functions_that_actually_execute() {
        for evaluator in [Evaluator::portable(), Evaluator::new()] {
            Evaluator::reset_test_usage();
            assert_eq!(evaluator.value(&[-0.125, 1.0], 0.25), 0.125);
            assert_eq!(evaluator.refine(&[-0.125, 1.0], [-0.5, 0.5], -0.625), 0.125);
            assert_eq!(
                Evaluator::test_usage(),
                match evaluator.backend() {
                    ContactBackend::Portable => [1, 0, 1, 0],
                    ContactBackend::Fma => [0, 1, 0, 1],
                }
            );
        }
    }

    #[test]
    fn refinement_backends_keep_exact_roots_and_the_iteration_budget() {
        for evaluator in [Evaluator::choose(false), Evaluator::new()] {
            for root in [
                (-0.5_f64).next_up(),
                -0.25,
                -2.0_f64.powi(-50),
                0.0,
                2.0_f64.powi(-50),
                0.125,
                0.5_f64.next_down(),
            ] {
                for sign in [-1.0, 1.0] {
                    let coefficients = [-root * sign, sign];
                    let f_low = portable(&coefficients, -0.5);
                    assert_eq!(
                        evaluator
                            .refine(&coefficients, [-0.5, 0.5], f_low)
                            .to_bits(),
                        root.to_bits()
                    );
                }
            }
        }
    }

    #[test]
    fn fused_evaluation_preserves_single_rounding_during_cancellation() {
        let x = 1.0 - 2.0_f64.powi(-27);
        let coefficient = 1.0 + 2.0_f64.powi(-27);
        let expected = -2.0_f64.powi(-54);
        assert_eq!(coefficient * x - 1.0, 0.0);
        for evaluator in [Evaluator::portable(), Evaluator::new()] {
            assert_eq!(
                evaluator.value(&[-1.0, coefficient], x).to_bits(),
                expected.to_bits()
            );
        }
    }

    #[test]
    fn finite_and_subnormal_polynomials_match_portable_fused_bits() {
        let evaluator = Evaluator::new();
        let mut random = 0x58_5eed_u64;
        let mut checked = 0;
        for degree in 0..=12 {
            for _ in 0..256 {
                let mut coefficients = Vec::new();
                for _ in 0..=degree {
                    random = random.wrapping_mul(6364136223846793005).wrapping_add(1);
                    let exponent = ((random >> 32) % 2047) << 52;
                    coefficients.push(f64::from_bits((random & 0x800f_ffff_ffff_ffff) | exponent));
                }
                for x in [
                    -0.5,
                    -0.125,
                    -0.0,
                    0.0,
                    f64::from_bits(1),
                    f64::MIN_POSITIVE,
                    0.125,
                    0.5,
                ] {
                    equal_bits_or_nan(
                        evaluator.value(&coefficients, x),
                        portable(&coefficients, x),
                    );
                    checked += 1;
                }
            }
        }
        assert_eq!(checked, 26624);
        for coefficients in [
            vec![f64::from_bits(1), -f64::MIN_POSITIVE, f64::MIN_POSITIVE],
            vec![-0.0, 0.0, -0.0],
            vec![f64::MAX, -f64::MAX, f64::from_bits(1)],
        ] {
            for x in [-0.5, -0.0, 0.0, f64::from_bits(1), 0.5] {
                equal_bits_or_nan(
                    evaluator.value(&coefficients, x),
                    portable(&coefficients, x),
                );
            }
        }
    }

    #[test]
    fn nonfinite_inputs_preserve_classification_and_zero_signs() {
        let evaluator = Evaluator::new();
        for coefficients in [
            vec![f64::NAN],
            vec![f64::INFINITY, 1.0],
            vec![f64::NEG_INFINITY, f64::INFINITY],
            vec![-0.0, 0.0],
        ] {
            for x in [
                f64::NAN,
                f64::NEG_INFINITY,
                -0.5,
                -0.0,
                0.0,
                0.5,
                f64::INFINITY,
            ] {
                equal_bits_or_nan(
                    evaluator.value(&coefficients, x),
                    portable(&coefficients, x),
                );
            }
        }
    }
}
