//! Comparing a plan against what the ledger already contains.
//!
//! The comparison is deliberately conservative: it matches on
//! **account + date + amount**, which is the only tuple every sink can report
//! without guessing. Payee similarity is Actual's job, not ours — this module
//! exists to answer "will this row land as a new transaction or reconcile with
//! an existing one, and is that existing one from a different importer?"

use chrono::NaiveDate;
use serde::{Deserialize, Serialize};

use crate::money::cents;
use crate::sink::ImportRow;

/// A row read back from a ledger. Amounts are in the sink's minor units plus an
/// exponent, so any sink can report them honestly.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LedgerRow {
    pub account: String,
    pub date: NaiveDate,
    pub amount_minor: i64,
    pub currency_exponent: u32,
    /// The raw description the ledger kept, when it kept one.
    pub payee: Option<String>,
    /// The id the row was imported under, if any. `None` means a manual entry.
    pub imported_id: Option<String>,
}

impl LedgerRow {
    /// Value × 100, the unit used for comparison.
    pub fn cents(&self) -> Option<i64> {
        cents(self.amount_minor, self.currency_exponent)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum LedgerMatch {
    /// Nothing in the ledger has this account, date and amount.
    New,
    /// A ledger row matches on account, date and amount.
    Existing {
        imported_id: Option<String>,
        /// The matching row was imported under a different id scheme, so the
        /// sink will probably add a duplicate instead of reconciling.
        foreign: bool,
    },
}

impl LedgerMatch {
    pub fn is_new(&self) -> bool {
        matches!(self, Self::New)
    }

    pub fn is_foreign(&self) -> bool {
        matches!(self, Self::Existing { foreign: true, .. })
    }

    /// The row was previously imported by `txsync` itself.
    pub fn is_same_id(&self, tx_id: &str) -> bool {
        matches!(
            self,
            Self::Existing {
                imported_id: Some(id),
                ..
            } if id == tx_id
        )
    }
}

/// Classify one planned row against a ledger snapshot.
pub fn classify(row: &ImportRow, ledger: &[LedgerRow]) -> LedgerMatch {
    let Some(wanted) = cents(
        row.transaction.amount.amount_minor,
        row.transaction.currency_exponent,
    ) else {
        // An amount that cannot be scaled cannot match anything safely.
        return LedgerMatch::New;
    };

    let hit = ledger.iter().find(|candidate| {
        candidate.account == row.account
            && candidate.date == row.transaction.occurred_on
            && candidate.cents() == Some(wanted)
    });

    match hit {
        None => LedgerMatch::New,
        Some(candidate) => LedgerMatch::Existing {
            imported_id: candidate.imported_id.clone(),
            foreign: matches!(
                &candidate.imported_id,
                Some(id) if id.as_str() != row.tx_id
            ),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use txp::{Extraction, Kind, Money, Source, Status};

    fn row(tx_id: &str, amount_minor: i64, exponent: u32) -> ImportRow {
        ImportRow {
            tx_id: tx_id.into(),
            account: "acc-1".into(),
            transaction: txp_core::Transaction {
                source: Source::default(),
                id: tx_id.into(),
                parser: "test.bank".into(),
                parser_version: 1,
                bank: "test".into(),
                account: "test-account".into(),
                kind: Kind::Purchase,
                status: Status::Settled,
                amount: Money {
                    amount_minor,
                    currency: "IDR".into(),
                },
                currency_exponent: exponent,
                amount_decimal: amount_minor.to_string(),
                occurred_at: chrono::DateTime::parse_from_rfc3339("2026-10-30T17:28:17+07:00")
                    .unwrap(),
                occurred_on: NaiveDate::from_ymd_opt(2026, 10, 30).unwrap(),
                timezone: "Asia/Jakarta".into(),
                counterparty: None,
                counterparty_account: None,
                account_last4: None,
                reference_id: None,
                channel: None,
                description: None,
                extra: Default::default(),
                extraction: Extraction::Strict,
                warnings: vec![],
            },
        }
    }

    fn ledger(imported_id: Option<&str>) -> Vec<LedgerRow> {
        vec![LedgerRow {
            account: "acc-1".into(),
            date: NaiveDate::from_ymd_opt(2026, 10, 30).unwrap(),
            amount_minor: -2_750_000,
            currency_exponent: 2,
            payee: Some("ExamplePay".into()),
            imported_id: imported_id.map(str::to_owned),
        }]
    }

    #[test]
    fn an_empty_ledger_means_new() {
        assert_eq!(classify(&row("mail_1", -27_500, 0), &[]), LedgerMatch::New);
    }

    #[test]
    fn a_manual_entry_matches() {
        assert_eq!(
            classify(&row("mail_1", -27_500, 0), &ledger(None)),
            LedgerMatch::Existing {
                imported_id: None,
                foreign: false
            }
        );
    }

    #[test]
    fn a_different_importer_is_flagged() {
        let matched = classify(&row("mail_1", -27_500, 0), &ledger(Some("example-2026-10-30")));
        assert!(matched.is_foreign());
        assert!(!matched.is_same_id("mail_1"));
    }

    #[test]
    fn the_same_id_is_not_foreign() {
        let matched = classify(&row("mail_1", -27_500, 0), &ledger(Some("mail_1")));
        assert!(!matched.is_foreign());
        assert!(matched.is_same_id("mail_1"));
    }

    #[test]
    fn a_different_amount_does_not_match() {
        let matched = classify(&row("mail_1", -27_501, 0), &ledger(None));
        assert!(matched.is_new());
    }

    #[test]
    fn idr_and_cents_compare_in_the_same_unit() {
        // IDR -27,500 (exponent 0) is -2,750,000 in Actual's cents.
        let matched = classify(&row("mail_1", -27_500, 0), &ledger(None));
        assert!(!matched.is_new());
    }
}
