//! Mapping `abmx-parser`'s contract onto Actual's transaction shape.
//!
//! All of Actual's quirks live here, not in `abmx-core`:
//!
//! * `amount` is always value × 100, for every currency. IDR 27,500 must be
//!   sent as `2_750_000`. This is the single easiest way to corrupt a ledger.
//! * `imported_id` is the dedupe key, so it gets `abmx-parser`'s stable `id`.
//! * `payee_name` creates a payee on demand; `imported_payee` keeps the raw
//!   string visible.

use abmx_core::Transaction;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// Whether imported rows are marked cleared in Actual.
///
/// `abmx-parser`'s `status: settled` means "the bank says the transaction completed".
/// Actual's `cleared` means "I have reconciled this against my bank statement".
/// Those are different claims, so the default does not make the second one for
/// you.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ClearedPolicy {
    /// Import uncleared; clearing is the user's reconciliation step.
    #[default]
    Never,
    /// Clear rows the bank reported as settled.
    Settled,
    /// Clear everything.
    Always,
}

impl ClearedPolicy {
    pub fn is_cleared(self, status: abmx_parser::Status) -> bool {
        match self {
            Self::Never => false,
            Self::Settled => status == abmx_parser::Status::Settled,
            Self::Always => true,
        }
    }
}

/// Which `abmx-parser` fields end up in Actual's `notes`.
///
/// Order is preserved and duplicates collapse, so a parser that sets both
/// `description` and `reference_id` to the same string does not repeat itself.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NotesConfig {
    pub fields: Vec<String>,
}

impl Default for NotesConfig {
    fn default() -> Self {
        Self {
            fields: vec![
                "description".to_owned(),
                "reference_id".to_owned(),
                "channel".to_owned(),
                "parser".to_owned(),
            ],
        }
    }
}

/// Convert minor units to Actual's "value × 100".
///
/// `None` means the multiplication overflowed, which is a data error worth
/// surfacing rather than wrapping.
pub fn to_cents(amount_minor: i64, currency_exponent: u32) -> Option<i64> {
    // Shared with the ledger comparison, so `abmx diff` and the importer can
    // never disagree about what a row is worth.
    abmx_core::cents(amount_minor, currency_exponent)
}

pub fn notes(tx: &Transaction, config: &NotesConfig) -> Option<String> {
    let mut parts: Vec<String> = Vec::new();
    for name in &config.fields {
        let Some(value) = field(tx, name) else {
            continue;
        };
        let value = value.trim();
        if value.is_empty() {
            continue;
        }
        if parts.iter().any(|part| part == value) {
            continue;
        }
        parts.push(value.to_owned());
    }

    (!parts.is_empty()).then(|| parts.join(" | "))
}

fn field(tx: &Transaction, name: &str) -> Option<String> {
    match name {
        "description" => tx.description.clone(),
        "reference_id" => tx.reference_id.clone(),
        "channel" => tx.channel.clone(),
        "parser" => Some(tx.parser.clone()),
        "bank" => Some(tx.bank.clone()),
        "kind" => serde_json::to_value(tx.kind)
            .ok()
            .and_then(|value| value.as_str().map(str::to_owned)),
        "status" => serde_json::to_value(tx.status)
            .ok()
            .and_then(|value| value.as_str().map(str::to_owned)),
        "account" => Some(tx.account.clone()),
        "account_last4" => tx.account_last4.clone(),
        "counterparty" => tx.counterparty.clone(),
        "counterparty_account" => tx.counterparty_account.clone(),
        "message_id" => tx.source.message_id.clone(),
        // Anything else is a parser-specific `extra` field.
        other => tx.extra.get(other).cloned(),
    }
}

/// One Actual transaction, as JSON.
///
/// `None` fields are omitted rather than sent as `null`, because a null
/// `payee_name` is not the same as an absent one to Actual's importer.
pub fn to_actual(
    tx: &Transaction,
    account: &str,
    notes_config: &NotesConfig,
    cleared: ClearedPolicy,
) -> Result<Value, String> {
    let amount = to_cents(tx.amount.amount_minor, tx.currency_exponent).ok_or_else(|| {
        format!(
            "amount {} overflows when converted to cents",
            tx.amount.amount_minor
        )
    })?;

    let mut object = Map::new();
    object.insert("account".to_owned(), Value::String(account.to_owned()));
    object.insert("date".to_owned(), Value::String(tx.occurred_on.to_string()));
    object.insert("amount".to_owned(), Value::from(amount));
    object.insert("imported_id".to_owned(), Value::String(tx.id.clone()));

    if let Some(payee) = tx
        .counterparty
        .as_ref()
        .filter(|value| !value.trim().is_empty())
    {
        object.insert("payee_name".to_owned(), Value::String(payee.clone()));
    }

    let imported_payee = tx
        .counterparty
        .as_ref()
        .filter(|value| !value.trim().is_empty())
        .or_else(|| {
            tx.description
                .as_ref()
                .filter(|value| !value.trim().is_empty())
        });
    if let Some(payee) = imported_payee {
        object.insert("imported_payee".to_owned(), Value::String(payee.clone()));
    }

    if let Some(notes) = notes(tx, notes_config) {
        object.insert("notes".to_owned(), Value::String(notes));
    }

    object.insert(
        "cleared".to_owned(),
        Value::Bool(cleared.is_cleared(tx.status)),
    );

    Ok(Value::Object(object))
}

#[cfg(test)]
mod tests {
    use super::*;
    use abmx_parser::{Extraction, Kind, Money, Source, Status};

    fn tx(amount_minor: i64, currency: &str, exponent: u32) -> Transaction {
        Transaction {
            source: Source {
                message_id: Some("abc@example.invalid".into()),
                file: Some("gmail:18f".into()),
                tx_index: 0,
            },
            id: "mail_a631e3d89a232b55".into(),
            parser: "example.card_debit".into(),
            parser_version: 1,
            bank: "example".into(),
            account: "example-account".into(),
            kind: Kind::Purchase,
            status: Status::Settled,
            amount: Money {
                amount_minor,
                currency: currency.into(),
            },
            currency_exponent: exponent,
            amount_decimal: amount_minor.to_string(),
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
            extraction: Extraction::Strict,
            warnings: vec![],
        }
    }

    #[test]
    fn idr_is_multiplied_to_cents() {
        assert_eq!(to_cents(-27_500, 0), Some(-2_750_000));
    }

    #[test]
    fn two_decimal_currencies_are_unchanged() {
        assert_eq!(to_cents(-1234, 2), Some(-1234));
    }

    #[test]
    fn three_decimal_currencies_are_truncated() {
        assert_eq!(to_cents(1234, 3), Some(123));
    }

    #[test]
    fn overflow_is_reported() {
        assert_eq!(to_cents(i64::MAX, 0), None);
    }

    #[test]
    fn mapping_sets_the_dedupe_key_and_sign() {
        let value = to_actual(
            &tx(-27_500, "IDR", 0),
            "uuid-example",
            &NotesConfig::default(),
            ClearedPolicy::Settled,
        )
        .unwrap();
        assert_eq!(value["amount"], -2_750_000);
        assert_eq!(value["imported_id"], "mail_a631e3d89a232b55");
        assert_eq!(value["date"], "2026-10-30");
        assert_eq!(value["account"], "uuid-example");
        assert_eq!(value["payee_name"], "ExamplePay");
        assert_eq!(value["cleared"], true);
        assert!(value["notes"]
            .as_str()
            .unwrap()
            .contains("DEBIT ONLINE | example.card_debit"));
    }

    #[test]
    fn missing_payee_is_omitted_not_null() {
        let mut transaction = tx(-1000, "IDR", 0);
        transaction.counterparty = None;
        transaction.description = Some("Transfer to savings".into());
        let value = to_actual(
            &transaction,
            "uuid",
            &NotesConfig::default(),
            ClearedPolicy::Never,
        )
        .unwrap();
        assert!(value.get("payee_name").is_none());
        assert_eq!(value["imported_payee"], "Transfer to savings");
    }

    #[test]
    fn the_cleared_policy_controls_the_flag() {
        let settled = tx(-1000, "IDR", 0);

        let mut pending = tx(-1000, "IDR", 0);
        pending.status = Status::Pending;

        assert!(!ClearedPolicy::Never.is_cleared(Status::Settled));
        assert!(!ClearedPolicy::Never.is_cleared(Status::Pending));

        assert!(ClearedPolicy::Settled.is_cleared(Status::Settled));
        assert!(!ClearedPolicy::Settled.is_cleared(Status::Pending));

        assert!(ClearedPolicy::Always.is_cleared(Status::Pending));

        let value = to_actual(
            &settled,
            "uuid",
            &NotesConfig::default(),
            ClearedPolicy::Never,
        )
        .unwrap();
        assert_eq!(value["cleared"], false);

        let value = to_actual(
            &pending,
            "uuid",
            &NotesConfig::default(),
            ClearedPolicy::Settled,
        )
        .unwrap();
        assert_eq!(value["cleared"], false);

        let value = to_actual(
            &settled,
            "uuid",
            &NotesConfig::default(),
            ClearedPolicy::Always,
        )
        .unwrap();
        assert_eq!(value["cleared"], true);
    }

    #[test]
    fn notes_skip_empty_and_duplicate_parts() {
        let mut transaction = tx(-1000, "IDR", 0);
        transaction.description = Some("DEBIT ONLINE".into());
        let value = to_actual(
            &transaction,
            "uuid",
            &NotesConfig::default(),
            ClearedPolicy::Never,
        )
        .unwrap();
        // `description` duplicates `channel`, so it appears once.
        assert_eq!(value["notes"], "DEBIT ONLINE | example.card_debit");
    }
}
