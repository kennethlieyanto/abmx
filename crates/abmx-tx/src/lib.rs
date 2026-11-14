//! Core data types shared by every crate.
//!
//! This crate has no I/O and no opinion about email formats.  Everything here is
//! plain data so that parsers can stay pure functions.

use chrono::{DateTime, FixedOffset, NaiveDate};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub mod money;

pub use money::{currency_exponent, Money};

/// Where a transaction came from.  Identity is `message_id` (or file name for
/// local fixtures) plus the index of the transaction inside that email.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Source {
    pub message_id: Option<String>,
    pub file: Option<String>,
    #[serde(default)]
    pub tx_index: u32,
}

impl Source {
    /// Stable identity, independent of parser version.
    ///
    /// Re-parsing an email after a spec change must never create a second row,
    /// so this deliberately excludes anything the parser produces.
    pub fn stable_id(&self) -> String {
        let key = self
            .message_id
            .as_deref()
            .or(self.file.as_deref())
            .unwrap_or("unknown");
        format!("mail_{:016x}", fnv1a(&format!("{key}#{}", self.tx_index)))
    }
}

fn fnv1a(s: &str) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in s.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Address {
    pub name: Option<String>,
    pub email: String,
}

impl Address {
    pub fn domain(&self) -> &str {
        self.email
            .rsplit_once('@')
            .map(|(_, domain)| domain)
            .unwrap_or("")
    }
}

/// One unit of visible text from the email body.
///
/// The normalizer splits the body on block-level HTML elements so that
/// "label then value" survives layout changes.  Parsers only ever see blocks.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Block {
    pub text: String,
}

impl Block {
    pub fn new(text: impl Into<String>) -> Self {
        Self { text: text.into() }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NormalizedEmail {
    pub source: Source,
    pub from: Address,
    pub subject: String,
    pub date: DateTime<FixedOffset>,
    pub blocks: Vec<Block>,
}

impl NormalizedEmail {
    pub fn block_texts(&self) -> Vec<&str> {
        self.blocks
            .iter()
            .map(|block| block.text.as_str())
            .collect()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
#[schemars(rename_all = "snake_case")]
pub enum Kind {
    Purchase,
    Payment,
    TransferIn,
    TransferOut,
    Fee,
    Refund,
    CashWithdrawal,
    Unknown,
}

impl Kind {
    /// Money leaves the account.  Used to derive the sign so that sign is never
    /// stored twice.
    pub fn is_outflow(self) -> bool {
        matches!(
            self,
            Kind::Purchase | Kind::Payment | Kind::TransferOut | Kind::Fee | Kind::CashWithdrawal
        )
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Purchase => "purchase",
            Kind::Payment => "payment",
            Kind::TransferIn => "transfer_in",
            Kind::TransferOut => "transfer_out",
            Kind::Fee => "fee",
            Kind::Refund => "refund",
            Kind::CashWithdrawal => "cash_withdrawal",
            Kind::Unknown => "unknown",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
#[schemars(rename_all = "snake_case")]
pub enum Status {
    Settled,
    Pending,
    Failed,
    Unknown,
}

impl Status {
    pub fn as_str(self) -> &'static str {
        match self {
            Status::Settled => "settled",
            Status::Pending => "pending",
            Status::Failed => "failed",
            Status::Unknown => "unknown",
        }
    }
}

/// How much trust to put in a parsed row.
///
/// With no LLM in the pipeline this is the only confidence signal, and it is
/// deliberately coarse so that it can be tested.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
#[schemars(rename_all = "snake_case")]
pub enum Extraction {
    Strict,
    Heuristic,
    Failed,
}

impl Extraction {
    pub fn as_str(self) -> &'static str {
        match self {
            Extraction::Strict => "strict",
            Extraction::Heuristic => "heuristic",
            Extraction::Failed => "failed",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct Transaction {
    pub source: Source,
    /// Deterministic identity derived from email metadata only. Consumers use
    /// it to deduplicate imports, whatever their storage is.
    pub id: String,

    pub parser: String,
    pub parser_version: u32,

    pub bank: String,
    pub account: String,

    pub kind: Kind,
    pub status: Status,

    /// Flattened so the wire format reads `amount_minor` / `currency`.
    #[serde(flatten)]
    pub amount: Money,
    /// ISO-4217 minor-unit exponent, repeated for consumers that do not keep a
    /// currency table.
    pub currency_exponent: u32,
    /// Exact decimal rendering of `amount_minor`.
    #[serde(rename = "amount")]
    pub amount_decimal: String,

    pub occurred_at: DateTime<FixedOffset>,
    pub occurred_on: NaiveDate,
    pub timezone: String,

    pub counterparty: Option<String>,
    pub counterparty_account: Option<String>,
    pub account_last4: Option<String>,
    pub reference_id: Option<String>,
    pub channel: Option<String>,
    pub description: Option<String>,
    pub extra: BTreeMap<String, String>,

    pub extraction: Extraction,
    pub warnings: Vec<String>,
}

/// JSON Schema for the transaction contract, generated from the Rust types so
/// it cannot drift from the documented output.
pub fn json_schema() -> serde_json::Value {
    serde_json::to_value(schemars::schema_for!(Transaction)).expect("schema is serializable")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stable_id_is_stable_and_index_aware() {
        let mut source = Source {
            message_id: Some("<abc@example.com>".into()),
            ..Source::default()
        };
        let first = source.stable_id();
        assert_eq!(first, source.stable_id());

        source.tx_index = 1;
        assert_ne!(first, source.stable_id());
    }

    #[test]
    fn kind_direction() {
        assert!(Kind::Purchase.is_outflow());
        assert!(Kind::TransferOut.is_outflow());
        assert!(!Kind::TransferIn.is_outflow());
        assert!(!Kind::Refund.is_outflow());
    }
}
