//! Money scaling shared by the ledger comparison.
//!
//! `txp` reports minor units plus an exponent (IDR = 0, USD = 2). Actual stores
//! "value × 100" for every currency. To compare the two, both sides are scaled
//! to the same unit: value × 100.

/// Scale `amount_minor` by `10^(2 - exponent)`.
///
/// `None` means the multiplication overflowed, which is a data error worth
/// surfacing rather than wrapping.
pub fn cents(amount_minor: i64, exponent: u32) -> Option<i64> {
    if exponent <= 2 {
        let factor = 10i64.checked_pow(2 - exponent)?;
        amount_minor.checked_mul(factor)
    } else {
        // No target has finer than two decimals, so the extra digit is lost.
        // That is a property of the target, not of the conversion.
        let factor = 10i64.checked_pow(exponent - 2)?;
        Some(amount_minor / factor)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn idr_is_multiplied_to_cents() {
        assert_eq!(cents(-27_500, 0), Some(-2_750_000));
    }

    #[test]
    fn two_decimal_currencies_are_unchanged() {
        assert_eq!(cents(-1234, 2), Some(-1234));
    }

    #[test]
    fn three_decimal_currencies_are_truncated() {
        assert_eq!(cents(1234, 3), Some(123));
    }

    #[test]
    fn overflow_is_reported() {
        assert_eq!(cents(i64::MAX, 0), None);
    }
}
