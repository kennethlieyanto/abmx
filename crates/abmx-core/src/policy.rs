use std::collections::BTreeMap;

use abmx_parser::{Extraction, Status};
use abmx_tx::Transaction;
use serde::{Deserialize, Serialize};

/// What to do with a row that `abmx-parser` marked `heuristic`.
///
/// The default is `Review`: a heuristic row has a plausible but unverified
/// amount, and writing a wrong amount into a ledger is worse than writing
/// nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum HeuristicPolicy {
    #[default]
    Review,
    Import,
    Skip,
}

/// What to do with a row the bank reported as failed.
///
/// The default is `Skip`, and `abmx-parser`'s own contract says failed rows must not be
/// imported. The other variants exist for ledgers that want the audit trail.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum FailedPolicy {
    #[default]
    Skip,
    Review,
    Import,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PolicyConfig {
    #[serde(default)]
    pub heuristic: HeuristicPolicy,
    #[serde(default)]
    pub failed: FailedPolicy,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "decision", content = "detail", rename_all = "snake_case")]
pub enum Decision {
    /// Write this row to the sink.
    Import,
    /// Hold this row for a human. Never written to the sink.
    Review(ReviewReason),
    /// Drop this row deliberately, with a reason.
    Skip(SkipReason),
}

impl Decision {
    pub fn is_import(&self) -> bool {
        matches!(self, Self::Import)
    }

    pub fn is_review(&self) -> bool {
        matches!(self, Self::Review(_))
    }

    pub fn is_skip(&self) -> bool {
        matches!(self, Self::Skip(_))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "reason", rename_all = "snake_case")]
pub enum ReviewReason {
    /// `abmx-parser` extracted a row but flagged it.
    Heuristic { warnings: Vec<String> },
    /// The parser's account key has no entry in `[accounts]`.
    UnmappedAccount { account: String },
    /// A zero amount almost always means the parser matched the wrong block.
    ZeroAmount,
}

impl ReviewReason {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Heuristic { .. } => "heuristic",
            Self::UnmappedAccount { .. } => "unmapped_account",
            Self::ZeroAmount => "zero_amount",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "reason", rename_all = "snake_case")]
pub enum SkipReason {
    /// The bank reported the transaction as failed.
    Failed,
    /// A configured policy dropped it.
    Policy { detail: String },
}

impl SkipReason {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Failed => "failed",
            Self::Policy { .. } => "policy",
        }
    }
}

/// Mapping from `abmx-parser`'s parser-declared account key (`example-account`) to a sink's
/// own account identity (an Actual account UUID).
///
/// Kept as its own type so it is impossible to pass an unmapped key by accident
/// and so `doctor` can enumerate what is missing.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(transparent)]
pub struct AccountMap(BTreeMap<String, String>);

impl AccountMap {
    pub fn new() -> Self {
        Self(BTreeMap::new())
    }

    pub fn insert(&mut self, key: impl Into<String>, account: impl Into<String>) {
        self.0.insert(key.into(), account.into());
    }

    pub fn get(&self, key: &str) -> Option<&str> {
        self.0.get(key).map(String::as_str)
    }

    pub fn contains(&self, key: &str) -> bool {
        self.0.contains_key(key)
    }

    pub fn keys(&self) -> impl Iterator<Item = &str> {
        self.0.keys().map(String::as_str)
    }

    pub fn iter(&self) -> impl Iterator<Item = (&str, &str)> {
        self.0.iter().map(|(k, v)| (k.as_str(), v.as_str()))
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }
}

impl From<BTreeMap<String, String>> for AccountMap {
    fn from(value: BTreeMap<String, String>) -> Self {
        Self(value)
    }
}

impl FromIterator<(String, String)> for AccountMap {
    fn from_iter<T: IntoIterator<Item = (String, String)>>(iter: T) -> Self {
        Self(iter.into_iter().collect())
    }
}

/// The whole decision layer, as one pure function.
///
/// No clock, no I/O, no randomness: given the same transaction and config it
/// always returns the same decision, which is what makes plans reproducible and
/// review queues meaningful.
pub fn decide(tx: &Transaction, policy: &PolicyConfig, accounts: &AccountMap) -> Decision {
    // 1. A failed transaction is not money. Decide before anything else so a
    //    failed heuristic row cannot be imported by accident.
    if tx.status == Status::Failed {
        return match policy.failed {
            FailedPolicy::Skip => Decision::Skip(SkipReason::Failed),
            FailedPolicy::Review => Decision::Review(ReviewReason::Heuristic {
                warnings: vec!["bank reported the transaction as failed".to_owned()],
            }),
            FailedPolicy::Import => Decision::Import,
        };
    }

    // 2. A zero amount is the one plausibility check `abmx-parser` does outside the spec.
    //    Treat it as suspicious even if a spec somehow produced `strict`.
    if tx.amount.amount_minor == 0 {
        return Decision::Review(ReviewReason::ZeroAmount);
    }

    // 3. Suspect extraction. `abmx-parser` already tells us why.
    if tx.extraction == Extraction::Heuristic {
        return match policy.heuristic {
            HeuristicPolicy::Review => Decision::Review(ReviewReason::Heuristic {
                warnings: tx.warnings.clone(),
            }),
            HeuristicPolicy::Skip => Decision::Skip(SkipReason::Policy {
                detail: "heuristic row".to_owned(),
            }),
            HeuristicPolicy::Import => Decision::Import,
        };
    }

    // 4. Never guess which ledger account a bank account maps to.
    if !accounts.contains(&tx.account) {
        return Decision::Review(ReviewReason::UnmappedAccount {
            account: tx.account.clone(),
        });
    }

    Decision::Import
}

#[cfg(test)]
mod tests {
    use super::*;
    use abmx_parser::{Money, Source};

    fn tx(account: &str, amount: i64, extraction: Extraction, status: Status) -> Transaction {
        Transaction {
            source: Source::default(),
            id: "mail_test".into(),
            parser: "example.card_debit".into(),
            parser_version: 1,
            bank: "example".into(),
            account: account.into(),
            kind: abmx_parser::Kind::Purchase,
            status,
            amount: Money {
                amount_minor: amount,
                currency: "IDR".into(),
            },
            currency_exponent: 0,
            amount_decimal: amount.to_string(),
            occurred_at: chrono::DateTime::parse_from_rfc3339("2026-10-30T17:28:17+07:00").unwrap(),
            occurred_on: chrono::NaiveDate::from_ymd_opt(2026, 10, 30).unwrap(),
            timezone: "Asia/Jakarta".into(),
            counterparty: Some("ExamplePay".into()),
            counterparty_account: None,
            account_last4: Some("4821".into()),
            reference_id: None,
            channel: Some("DEBIT ONLINE".into()),
            description: None,
            extra: Default::default(),
            extraction,
            warnings: vec![],
        }
    }

    fn accounts() -> AccountMap {
        let mut map = AccountMap::new();
        map.insert("example-account", "uuid-example");
        map
    }

    #[test]
    fn strict_settled_mapped_row_imports() {
        let d = decide(
            &tx("example-account", -27500, Extraction::Strict, Status::Settled),
            &PolicyConfig::default(),
            &accounts(),
        );
        assert_eq!(d, Decision::Import);
    }

    #[test]
    fn failed_rows_are_skipped_by_default() {
        let d = decide(
            &tx("example-account", -27500, Extraction::Strict, Status::Failed),
            &PolicyConfig::default(),
            &accounts(),
        );
        assert_eq!(d, Decision::Skip(SkipReason::Failed));
    }

    #[test]
    fn heuristic_rows_go_to_review_by_default() {
        let d = decide(
            &tx("example-account", -27500, Extraction::Heuristic, Status::Settled),
            &PolicyConfig::default(),
            &accounts(),
        );
        assert!(matches!(
            d,
            Decision::Review(ReviewReason::Heuristic { .. })
        ));
    }

    #[test]
    fn unmapped_accounts_are_never_guessed() {
        let d = decide(
            &tx("wallet-account", -1000, Extraction::Strict, Status::Settled),
            &PolicyConfig::default(),
            &accounts(),
        );
        assert_eq!(
            d,
            Decision::Review(ReviewReason::UnmappedAccount {
                account: "wallet-account".into()
            })
        );
    }

    #[test]
    fn zero_amount_is_reviewed_even_when_strict() {
        let d = decide(
            &tx("example-account", 0, Extraction::Strict, Status::Settled),
            &PolicyConfig::default(),
            &accounts(),
        );
        assert_eq!(d, Decision::Review(ReviewReason::ZeroAmount));
    }

    #[test]
    fn failed_beats_unmapped_account() {
        let d = decide(
            &tx("wallet-account", -1000, Extraction::Strict, Status::Failed),
            &PolicyConfig::default(),
            &accounts(),
        );
        assert_eq!(d, Decision::Skip(SkipReason::Failed));
    }

    #[test]
    fn policy_can_import_heuristic_rows() {
        let policy = PolicyConfig {
            heuristic: HeuristicPolicy::Import,
            ..Default::default()
        };
        let d = decide(
            &tx("example-account", -27500, Extraction::Heuristic, Status::Settled),
            &policy,
            &accounts(),
        );
        assert_eq!(d, Decision::Import);
    }
}
