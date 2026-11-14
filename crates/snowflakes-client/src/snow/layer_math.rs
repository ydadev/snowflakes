//! The same rounded layer total, without arithmetic on a provably irrelevant tail.

#[inline(always)]
pub(super) fn total(mass: f32, pending: f32) -> f32 {
    let mass_bits = mass.to_bits();
    let pending_bits = pending.to_bits();
    // Positive finite binary32 values only. A pending exponent E bounds the
    // value strictly below 2^(E-126), including subnormals at E=0. For mass
    // exponent M, half the next ULP is 2^(M-151). A gap of 25 therefore keeps
    // the exact sum strictly below the rounding midpoint. +0 is exact as well.
    const EXPONENT_GAP: u32 = 25;
    if mass_bits < f32::INFINITY.to_bits()
        && pending_bits < f32::INFINITY.to_bits()
        && (pending_bits == 0 || (mass_bits >> 23) >= (pending_bits >> 23) + EXPONENT_GAP)
    {
        mass
    } else {
        rounded_add(mass, pending)
    }
}

// Keep the fallback out of line: an inline conditional expression can compile
// to an unconditional add followed by selection, retaining the denormal cost.
#[inline(never)]
fn rounded_add(mass: f32, pending: f32) -> f32 {
    mass + pending
}

#[cfg(test)]
mod tests {
    use super::*;

    #[inline(never)]
    fn literal(mass: f32, pending: f32) -> f32 {
        mass + pending
    }
    fn same(mass: f32, pending: f32) {
        let expected = literal(mass, pending);
        let actual = total(mass, pending);
        if expected.is_nan() {
            assert!(actual.is_nan());
        } else {
            assert_eq!(actual.to_bits(), expected.to_bits());
        }
    }

    #[test]
    fn every_positive_subnormal_tail_keeps_literal_sum_bits() {
        for bits in 1..f32::MIN_POSITIVE.to_bits() {
            let exponent = 25 + bits % 230;
            let mass_bits = (exponent << 23) | (bits.wrapping_mul(0x9e37_79b9) & 0x007f_ffff);
            same(f32::from_bits(mass_bits), f32::from_bits(bits));
        }
    }

    #[test]
    fn normal_boundaries_and_halfway_ties_keep_real_rounding_changes() {
        let mut checked = 0;
        for exponent in 0..255_u32 {
            for gap in [0, 1, 23, 24, 25, 26, 64, 128, 254] {
                for mass_mantissa in [0, 1, 0x003f_ffff, 0x007f_ffff] {
                    for pending_mantissa in [0, 1, 0x003f_ffff, 0x007f_ffff] {
                        same(
                            f32::from_bits((exponent << 23) | mass_mantissa),
                            f32::from_bits((exponent.saturating_sub(gap) << 23) | pending_mantissa),
                        );
                        checked += 1;
                    }
                }
            }
        }
        assert_eq!(checked, 36_720);
        let odd = f32::from_bits((25 << 23) | 1);
        assert_eq!(
            total(odd, f32::MIN_POSITIVE).to_bits(),
            odd.next_up().to_bits()
        );
        let lower = f32::from_bits((24 << 23) | 1);
        assert_eq!(
            total(lower, f32::MIN_POSITIVE.next_down()).to_bits(),
            lower.next_up().to_bits()
        );
    }

    #[test]
    fn signed_zero_negative_and_exceptional_inputs_keep_the_original_addition() {
        let magnitudes = [
            0,
            1,
            0x007f_ffff,
            0x0080_0000,
            (24 << 23) | 1,
            25 << 23,
            (25 << 23) | 1,
            1.0_f32.to_bits(),
            f32::MAX.to_bits(),
            f32::INFINITY.to_bits(),
            0x7fc0_1234,
            0x7f80_0001,
        ];
        for a in magnitudes {
            for b in magnitudes {
                for sign_a in [0, 0x8000_0000] {
                    for sign_b in [0, 0x8000_0000] {
                        same(f32::from_bits(a | sign_a), f32::from_bits(b | sign_b));
                    }
                }
            }
        }
    }
}
