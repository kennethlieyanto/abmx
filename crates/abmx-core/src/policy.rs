use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use abmx_parser::{Extraction, Status};
use abmx_tx::Transaction;

/// Parser id → sink account id.
///
/// The key is the parser id (the label's remainder), not the Gmail label, so a
/// label rename never touches the account mapping.
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

/// Why a labeled message was **not** imported.
///
/// These are defects in the parser or the Gmail rule, never per-row judgement
/// calls: the fix is always `abmx samples --label …`, a `[labels.accounts]`
/// entry, or a gmailctl rule. Nothing is ever "approved" by a human.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "reason", rename_all = "snake_case")]
pub enum BrokenReason {
    /// The parser flagged the extraction: template drift or an incomplete spec.
    Heuristic { warnings: Vec<String> },
    /// A zero amount almost always means the wrong block was matched.
    ZeroAmount,
    /// No `[labels.accounts]` entry for this parser id.
    UnmappedAccount { parser: String },
    /// The Gmail rule routed a transaction the bank reported as failed.
    BankDeclaredFailed,
}

impl BrokenReason {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Heuristic { .. } => "heuristic",
            Self::ZeroAmount => "zero_amount",
            Self::UnmappedAccount { .. } => "unmapped_account",
            Self::BankDeclaredFailed => "bank_declared_failed",
        }
    }

    /// One line for `sync` and `status`.
    pub fn detail(&self) -> String {
        match self {
            Self::Heuristic { warnings } if warnings.is_empty() => {
                "the parser did not produce a strict row".to_owned()
            }
            Self::Heuristic { warnings } => warnings.join("; "),
            Self::ZeroAmount => "amount is zero".to_owned(),
            Self::UnmappedAccount { parser } => {
                format!("parser `{parser}` has no [labels.accounts] entry")
            }
            Self::BankDeclaredFailed => {
                "the bank reported the transaction as failed; the Gmail rule should not route it"
                    .to_owned()
            }
        }
    }
}

/// The whole decision: import, or record why not.
///
/// No clock, no I/O, no randomness. The order is deliberate: a failed row is
/// judged before anything else so it can never be imported by accident.
pub fn check_transaction(tx: &Transaction, accounts: &AccountMap) -> Result<(), BrokenReason> {
    if tx.status == Status::Failed {
        return Err(BrokenReason::BankDeclaredFailed);
    }

    if tx.amount.amount_minor == 0 {
        return Err(BrokenReason::ZeroAmount);
    }

    if tx.extraction == Extraction::Heuristic {
        return Err(BrokenReason::Heuristic {
            warnings: tx.warnings.clone(),
        });
    }

    if !accounts.contains(&tx.parser) {
        return Err(BrokenReason::UnmappedAccount {
            parser: tx.parser.clone(),
        });
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use abmx_parser::{Kind, Money, Source};

    fn tx(parser: &str, amount: i64, extraction: Extraction, status: Status) -> Transaction {
        Transaction {
            source: Source::default(),
            id: "mail_test".into(),
            parser: parser.into(),
            parser_version: 1,
            bank: "example".into(),
            account: parser.into(),
            kind: Kind::Purchase,
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
        map.insert("example.card_debit", "uuid-example");
        map
    }

    #[test]
    fn strict_settled_mapped_row_imports() {
        assert_eq!(
            check_transaction(
                &tx(
                    "example.card_debit",
                    -27500,
                    Extraction::Strict,
                    Status::Settled
                ),
                &accounts()
            ),
            Ok(())
        );
    }

    #[test]
    fn failed_rows_are_never_imported() {
        assert_eq!(
            check_transaction(
                &tx("example.card_debit", -27500, Extraction::Strict, Status::Failed),
                &accounts()
            ),
            Err(BrokenReason::BankDeclaredFailed)
        );
    }

    #[test]
    fn heuristic_rows_are_broken() {
        assert!(matches!(
            check_transaction(
                &tx(
                    "example.card_debit",
                    -27500,
                    Extraction::Heuristic,
                    Status::Settled
                ),
                &accounts()
            ),
            Err(BrokenReason::Heuristic { .. })
        ));
    }

    #[test]
    fn zero_amount_is_broken() {
        assert_eq!(
            check_transaction(
                &tx("example.card_debit", 0, Extraction::Strict, Status::Settled),
                &accounts()
            ),
            Err(BrokenReason::ZeroAmount)
        );
    }

    #[test]
    fn unmapped_parser_is_broken() {
        assert_eq!(
            check_transaction(
                &tx(
                    "wallet.qris_payment",
                    -1000,
                    Extraction::Strict,
                    Status::Settled
                ),
                &accounts()
            ),
            Err(BrokenReason::UnmappedAccount {
                parser: "wallet.qris_payment".into()
            })
        );
    }
}
