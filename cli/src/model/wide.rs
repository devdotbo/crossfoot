//! Exact 256-bit multiply-then-divide for u128 operands.
//!
//! `price()` computes `totalAssets * 1e18 / totalSupply`. With totalAssets
//! around 8e22 the product is about 8e40, which overflows u128 (max 3.4e38),
//! so the intermediate has to be held in 256 bits exactly as the EVM does.
//! Dividing first would lose the low-order digits that the wei-exact
//! comparison depends on.

/// The full 256-bit product of two u128 values, as (high, low).
fn mul_full(a: u128, b: u128) -> (u128, u128) {
    const MASK: u128 = u64::MAX as u128;
    let (a_lo, a_hi) = (a & MASK, a >> 64);
    let (b_lo, b_hi) = (b & MASK, b >> 64);

    let lo_lo = a_lo * b_lo;
    let lo_hi = a_lo * b_hi;
    let hi_lo = a_hi * b_lo;
    let hi_hi = a_hi * b_hi;

    // Combine the cross terms, carrying into the high word.
    let (mid, carry_a) = lo_hi.overflowing_add(hi_lo);
    let carry_a = if carry_a { 1u128 << 64 } else { 0 };

    let (low, carry_b) = lo_lo.overflowing_add(mid << 64);
    let high = hi_hi + (mid >> 64) + carry_a + if carry_b { 1 } else { 0 };
    (high, low)
}

/// floor(a * b / divisor), exact, or None when the divisor is zero or the
/// quotient does not fit in u128.
pub fn mul_div_floor(a: u128, b: u128, divisor: u128) -> Option<u128> {
    if divisor == 0 {
        return None;
    }
    let (hi, lo) = mul_full(a, b);
    if hi == 0 {
        return Some(lo / divisor);
    }
    // Binary long division of the 256-bit value (hi, lo) by divisor.
    //
    // The running remainder is always strictly below divisor, so doubling it
    // and adding a bit can reach 2*divisor - 1, which needs 129 bits. The
    // overflow bit is tracked explicitly rather than widened: when it is set,
    // the true value is 2^128 + doubled, and subtracting divisor once (which
    // is at most u128::MAX) brings it back inside u128, with the wrapping
    // subtraction giving the right low 128 bits.
    let mut quotient: u128 = 0;
    let mut remainder: u128 = 0;
    for bit in (0..256).rev() {
        let current = if bit >= 128 {
            (hi >> (bit - 128)) & 1
        } else {
            (lo >> bit) & 1
        };
        // overflowing_shl reports a shift-amount overflow, not a value
        // overflow, so the top bit is tested directly.
        let overflowed = remainder >> 127 == 1;
        let doubled = (remainder << 1) | current;
        if overflowed || doubled >= divisor {
            remainder = doubled.wrapping_sub(divisor);
            if bit >= 128 {
                // A set quotient bit at or above 128 means the result does
                // not fit in u128.
                return None;
            }
            quotient |= 1u128 << bit;
        } else {
            remainder = doubled;
        }
    }
    Some(quotient)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn small_values_match_plain_arithmetic() {
        assert_eq!(mul_div_floor(6, 7, 3), Some(14));
        assert_eq!(mul_div_floor(0, 12345, 7), Some(0));
        assert_eq!(mul_div_floor(10, 10, 3), Some(33));
    }

    #[test]
    fn division_by_zero_is_none() {
        assert_eq!(mul_div_floor(1, 1, 0), None);
    }

    /// The real case: the product overflows u128, the quotient does not.
    #[test]
    fn the_price_computation_is_exact() {
        let total_assets = 81_769_497_488_003_849_675_143u128;
        let total_supply = 80_027_751_992_300_676_663_517u128;
        assert_eq!(
            mul_div_floor(total_assets, 1_000_000_000_000_000_000, total_supply),
            Some(1_021_764_268_673_581_424)
        );
    }

    /// Cases whose expected quotients were computed independently, in Python
    /// arbitrary-precision integers, not by this code.
    #[test]
    fn large_cases_match_independently_computed_quotients() {
        let cases: [(u128, u128, u128, Option<u128>); 6] = [
            (
                865240335981968963824986131144330724,
                946864788125462323,
                1113789423298541461217551960568,
                Some(735565978873166166802079),
            ),
            (
                73780990327521976013231051388289971660,
                641012325617808238136512351119,
                21063811107720574817888547250064448950,
                Some(2245297584295886938858280304563),
            ),
            (
                1234612210971607389249450586936,
                675083301366334671,
                7925632354750747694481361917831144092,
                Some(105160831336),
            ),
            (
                71312812827046622383623859651114997451,
                135805201774437356,
                30749000482321121655951006340421059928,
                Some(314958235492820631),
            ),
            (
                288304427720193692598056238993,
                135647390000891346621901649362,
                831611633111880575610630138289,
                Some(47026450315039733869313233073),
            ),
            (
                79218052391791520434156712630519509118,
                51002711797876073028924562290430617479,
                1006117994675185531230227297319,
                None,
            ),
        ];
        for (a, b, d, expected) in cases {
            assert_eq!(mul_div_floor(a, b, d), expected, "a={a} b={b} d={d}");
        }
    }

    #[test]
    fn a_quotient_that_does_not_fit_is_none() {
        assert_eq!(mul_div_floor(u128::MAX, u128::MAX, 1), None);
    }

    #[test]
    fn maximum_operands_divide_exactly() {
        assert_eq!(
            mul_div_floor(u128::MAX, u128::MAX, u128::MAX),
            Some(u128::MAX)
        );
    }

    /// Cross-check the 256-bit path against plain u128 arithmetic wherever
    /// plain arithmetic is still valid.
    #[test]
    fn agrees_with_u128_arithmetic_where_that_is_valid() {
        let cases = [
            (1_000_000_007u128, 999_999_937u128, 65_537u128),
            (u64::MAX as u128, 3, 7),
            (123_456_789_012_345_678u128, 1_000_000_007, 1_000_003),
        ];
        for (a, b, d) in cases {
            let plain = a.checked_mul(b).map(|p| p / d);
            assert_eq!(mul_div_floor(a, b, d), plain, "a={a} b={b} d={d}");
        }
    }
}
