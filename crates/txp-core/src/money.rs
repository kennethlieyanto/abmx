//! Money is stored as integer minor units plus an ISO-4217 code.
//!
//! Never floating point: a budget transaction that is off by a cent is a bug
//! you cannot find three months later.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Money {
    /// Signed amount in the currency's minor units. Negative means outflow.
    pub amount_minor: i64,
    pub currency: String,
}

impl Money {
    pub fn new(amount_minor: i64, currency: impl Into<String>) -> Self {
        Self {
            amount_minor,
            currency: currency.into(),
        }
    }

    /// Exact decimal rendering, for consumers that do not want to deal with
    /// minor units. Never used for arithmetic.
    pub fn decimal(&self) -> String {
        let exponent = self.exponent();
        let sign = if self.amount_minor < 0 { "-" } else { "" };
        let absolute = self.amount_minor.unsigned_abs();

        if exponent == 0 {
            format!("{sign}{absolute}")
        } else {
            let factor = 10u64.pow(exponent);
            format!(
                "{sign}{}.{:0width$}",
                absolute / factor,
                absolute % factor,
                width = exponent as usize
            )
        }
    }

    pub fn exponent(&self) -> u32 {
        currency_exponent(&self.currency)
    }

    pub fn is_zero(&self) -> bool {
        self.amount_minor == 0
    }
}

/// ISO-4217 minor-unit exponent.
///
/// Only the currencies we actually meet matter; everything else is assumed to
/// have two decimals.  Add to this table rather than guessing downstream.
pub fn currency_exponent(code: &str) -> u32 {
    match code.to_ascii_uppercase().as_str() {
        "IDR" | "JPY" | "KRW" | "VND" | "CLP" | "ISK" | "UGX" => 0,
        "KWD" | "BHD" | "OMR" | "JOD" | "TND" | "IQD" | "LYD" => 3,
        _ => 2,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exponents_follow_the_currency() {
        assert_eq!(Money::new(11_000, "IDR").exponent(), 0);
        assert_eq!(Money::new(1234, "USD").exponent(), 2);
        assert_eq!(Money::new(1234, "KWD").exponent(), 3);
    }

    #[test]
    fn decimal_rendering_is_exact() {
        assert_eq!(Money::new(-11_000, "IDR").decimal(), "-11000");
        assert_eq!(Money::new(-1234, "USD").decimal(), "-12.34");
        assert_eq!(Money::new(5, "USD").decimal(), "0.05");
        assert_eq!(Money::new(-1234, "KWD").decimal(), "-1.234");
    }
}
