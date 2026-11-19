//! Locale-aware decimal parsing.
//!
//! Every bank writes money differently, and two of our banks disagree *within*
//! the same bank (Example's journal uses `372,000.00`, its card alert uses `11.000`).
//! The spec declares the convention per field.

use crate::spec::NumberStyle;

/// Parse a money string into integer minor units.
///
/// `exponent` is the currency's ISO-4217 minor-unit exponent (IDR = 0).
pub fn parse_decimal(raw: &str, style: NumberStyle, exponent: u32) -> Result<i64, String> {
    let (thousands, decimal) = match style {
        NumberStyle::En => (',', '.'),
        NumberStyle::Id => ('.', ','),
    };

    // Keep only characters that can be part of the number; this tolerates
    // "IDR 11.000" when a spec has no regex.
    let cleaned: String = raw
        .chars()
        .filter(|ch| ch.is_ascii_digit() || *ch == thousands || *ch == decimal || *ch == '-')
        .collect();

    let negative = cleaned.starts_with('-');
    let cleaned = cleaned.trim_start_matches('-');
    if cleaned.is_empty() {
        return Err(format!("`{raw}` contains no digits"));
    }

    let (integer_part, fraction_part) = match cleaned.rsplit_once(decimal) {
        Some((integer, fraction)) => (integer, fraction),
        None => (cleaned, ""),
    };

    let integer_digits: String = integer_part.chars().filter(|ch| *ch != thousands).collect();

    if integer_digits.is_empty() || !integer_digits.chars().all(|ch| ch.is_ascii_digit()) {
        return Err(format!("`{raw}` is not a valid integer part"));
    }

    let mut fraction: String = fraction_part
        .chars()
        .filter(|ch| ch.is_ascii_digit())
        .collect();

    let exponent_usize = exponent as usize;
    if fraction.len() > exponent_usize {
        fraction.truncate(exponent_usize);
    }
    while fraction.len() < exponent_usize {
        fraction.push('0');
    }

    let integer: i128 = integer_digits
        .parse()
        .map_err(|_| format!("`{raw}` overflows"))?;
    let fraction: i128 = if exponent_usize == 0 {
        0
    } else {
        fraction
            .parse()
            .map_err(|_| format!("`{raw}` has a bad fraction"))?
    };

    let scale = 10i128.pow(exponent);
    let mut minor = integer
        .checked_mul(scale)
        .and_then(|value| value.checked_add(fraction))
        .ok_or_else(|| format!("`{raw}` overflows"))?;

    if negative {
        minor = -minor;
    }

    i64::try_from(minor).map_err(|_| format!("`{raw}` overflows"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn indonesian_thousands_with_cents() {
        assert_eq!(parse_decimal("11.000", NumberStyle::Id, 0), Ok(11_000));
        assert_eq!(
            parse_decimal("10.001.288,00", NumberStyle::Id, 0),
            Ok(10_001_288)
        );
        assert_eq!(parse_decimal("18.600,00", NumberStyle::Id, 0), Ok(18_600));
    }

    #[test]
    fn english_thousands_with_cents() {
        assert_eq!(parse_decimal("372,000.00", NumberStyle::En, 0), Ok(372_000));
        assert_eq!(parse_decimal("72,000.00", NumberStyle::En, 0), Ok(72_000));
    }

    #[test]
    fn tolerates_split_blocks_and_currency() {
        assert_eq!(
            parse_decimal("Rp 100.000 ,00", NumberStyle::Id, 0),
            Ok(100_000)
        );
        assert_eq!(parse_decimal("IDR 11.000", NumberStyle::Id, 0), Ok(11_000));
    }

    #[test]
    fn two_decimal_currency() {
        assert_eq!(parse_decimal("1.234,56", NumberStyle::Id, 2), Ok(123_456));
        assert_eq!(parse_decimal("1,234.56", NumberStyle::En, 2), Ok(123_456));
    }
}
