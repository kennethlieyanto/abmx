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

    /// The row was previously imported by `abmx` itself.
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
    use abmx_parser::{Extraction, Kind, Money, Source, Status};

    fn row(tx_id: &str, amount_minor: i64, exponent: u32) -> ImportRow {
        ImportRow {
            tx_id: tx_id.into(),
            account: "acc-1".into(),
            transaction: abmx_tx::Transaction {
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
        let matched = classify(
            &row("mail_1", -27_500, 0),
            &ledger(Some("example-2026-10-30")),
        );
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

/// How a statement row relates to the ledger when the date may differ.
///
/// Email notifications carry the transaction date; a statement carries the
/// posting date. Matching on the exact date alone leaves duplicates behind, and
/// matching on amount alone can collapse two real purchases. The conservative
/// rule is: a unique candidate inside the window is the same event; anything
/// ambiguous is reported and left to the caller.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WindowMatch {
    /// The ledger already has this exact `imported_id`.
    SameId,
    /// Exactly one ledger row matches inside the window.
    Present,
    /// Nothing plausible matches; the row is new.
    New,
    /// More than one ledger row matches; matching would be a guess.
    Ambiguous(usize),
}

impl WindowMatch {
    pub fn is_present(&self) -> bool {
        matches!(self, Self::SameId | Self::Present)
    }
}

/// Classify one row against the ledger with a posting-date tolerance.
pub fn classify_window(row: &ImportRow, ledger: &[LedgerRow], window_days: i64) -> WindowMatch {
    if ledger.iter().any(|candidate| {
        candidate.account == row.account
            && candidate.imported_id.as_deref() == Some(row.tx_id.as_str())
    }) {
        return WindowMatch::SameId;
    }

    let Some(wanted) = cents(
        row.transaction.amount.amount_minor,
        row.transaction.currency_exponent,
    ) else {
        return WindowMatch::New;
    };

    let date = row.transaction.occurred_on;
    let window = window_days.unsigned_abs() as i64;

    let mut matches = 0usize;
    for candidate in ledger {
        if candidate.account != row.account || candidate.cents() != Some(wanted) {
            continue;
        }
        let delta = candidate.date.signed_duration_since(date).num_days().abs();
        if delta <= window {
            matches += 1;
        }
    }

    match matches {
        0 => WindowMatch::New,
        1 => WindowMatch::Present,
        count => WindowMatch::Ambiguous(count),
    }
}

#[cfg(test)]
mod window_tests {
    use super::*;
    use abmx_parser::{Extraction, Kind, Money, Source, Status};

    fn row(tx_id: &str, date: &str, amount_minor: i64) -> ImportRow {
        ImportRow {
            tx_id: tx_id.into(),
            account: "acc-1".into(),
            transaction: abmx_tx::Transaction {
                source: Source::default(),
                id: tx_id.into(),
                parser: "test.bank".into(),
                parser_version: 1,
                bank: "test".into(),
                account: "test".into(),
                kind: Kind::Unknown,
                status: Status::Settled,
                amount: Money {
                    amount_minor,
                    currency: "IDR".into(),
                },
                currency_exponent: 0,
                amount_decimal: amount_minor.to_string(),
                occurred_at: chrono::DateTime::parse_from_rfc3339("2026-01-05T00:00:00+07:00")
                    .unwrap(),
                occurred_on: NaiveDate::parse_from_str(date, "%Y-%m-%d").unwrap(),
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

    fn ledger(date: &str, amount_minor: i64, imported_id: Option<&str>) -> LedgerRow {
        LedgerRow {
            account: "acc-1".into(),
            date: NaiveDate::parse_from_str(date, "%Y-%m-%d").unwrap(),
            amount_minor,
            currency_exponent: 2,
            payee: None,
            imported_id: imported_id.map(str::to_owned),
        }
    }

    #[test]
    fn the_same_id_wins_over_everything() {
        let ledger = vec![ledger("2026-01-07", -400_000, Some("stmt_1"))];
        assert_eq!(
            classify_window(&row("stmt_1", "2026-01-05", -4_000), &ledger, 3),
            WindowMatch::SameId
        );
    }

    #[test]
    fn a_posting_date_a_couple_of_days_later_still_matches() {
        let ledger = vec![ledger("2026-01-07", -400_000, Some("mail_1"))];
        assert_eq!(
            classify_window(&row("stmt_1", "2026-01-05", -4_000), &ledger, 3),
            WindowMatch::Present
        );
    }

    #[test]
    fn two_candidates_are_ambiguous_not_guessed() {
        let ledger = vec![
            ledger("2026-01-05", -400_000, None),
            ledger("2026-01-06", -400_000, None),
        ];
        assert_eq!(
            classify_window(&row("stmt_1", "2026-01-05", -4_000), &ledger, 3),
            WindowMatch::Ambiguous(2)
        );
    }

    #[test]
    fn a_different_amount_is_new() {
        let ledger = vec![ledger("2026-01-05", -400_001, None)];
        assert_eq!(
            classify_window(&row("stmt_1", "2026-01-05", -4_000), &ledger, 3),
            WindowMatch::New
        );
    }
}
