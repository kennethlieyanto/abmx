//! TOML shape of `config.toml`.
//!
//! This file is the user-facing contract.  Keep field names stable, and keep
//! the vocabulary small: `label`, `regex`, `block`, `number`, `formats`.

use abmx_tx::{Kind, Status};
use serde::Deserialize;
use std::collections::BTreeMap;

/// What a parser covers, for `abmx labels` and registry discovery.
#[derive(Debug, Clone, serde::Serialize)]
pub struct SpecSummary {
    pub id: String,
    pub version: u32,
    pub bank: String,
    pub kind: Kind,
    /// The `<corpus>/<id>/config.toml` file this came from.
    pub file: std::path::PathBuf,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ParserSpec {
    pub spec_version: u32,
    #[serde(default = "one")]
    pub version: u32,
    #[serde(default = "yes")]
    pub enabled: bool,
    pub labels: LabelsSpec,
    pub defaults: Defaults,
    #[serde(default)]
    pub status: Option<StatusSpec>,
    pub extract: ExtractSpec,
}

fn one() -> u32 {
    1
}

fn yes() -> bool {
    true
}

/// Every constant label block in this template.
///
/// The engine partitions the email's blocks on these, so this list must contain
/// all labels, not only the ones we extract.
#[derive(Debug, Clone, Deserialize)]
pub struct LabelsSpec {
    pub list: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Defaults {
    pub bank: String,
    pub currency: String,
    pub tz: String,
    pub kind: Kind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NumberStyle {
    /// 1,234.56
    En,
    /// 1.234,56
    Id,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Normalize {
    /// Remove all spaces, e.g. a reference printed as "8289 0071 5313".
    StripSpaces,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Field {
    pub label: String,
    /// Applied to the joined region, or to each block when `per_block` is set.
    #[serde(default)]
    pub regex: Option<String>,
    /// Use this index of the region instead of a regex (0-based).
    #[serde(default)]
    pub block: Option<usize>,
    /// Run the regex against each block and take the first match.
    #[serde(default)]
    pub per_block: bool,
    /// Only meaningful for `amount`: which decimal convention the bank uses.
    #[serde(default)]
    pub number: Option<NumberStyle>,
    #[serde(default)]
    pub normalize: Option<Normalize>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct OccurredSpec {
    /// Single-field datetime, e.g. "07 Nov 2026 12:17:25".
    #[serde(default)]
    pub label: Option<String>,
    #[serde(default)]
    pub regex: Option<String>,
    #[serde(default)]
    pub formats: Vec<String>,
    /// `en` or `id` (Indonesian month abbreviations).
    #[serde(default = "english")]
    pub locale: String,
    /// Split date and time fields, e.g. Banktwo's `Date` + `Time`.
    #[serde(default)]
    pub date: Option<SubField>,
    #[serde(default)]
    pub time: Option<SubField>,
}

fn english() -> String {
    "en".to_string()
}

#[derive(Debug, Clone, Deserialize)]
pub struct SubField {
    pub label: String,
    #[serde(default)]
    pub regex: Option<String>,
    #[serde(default)]
    pub formats: Vec<String>,
    #[serde(default = "english")]
    pub locale: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct StatusSpec {
    #[serde(default)]
    pub label: Option<String>,
    #[serde(default)]
    pub map: BTreeMap<String, Status>,
    #[serde(default)]
    pub default: Option<Status>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ExtractSpec {
    pub amount: Field,
    pub occurred_at: OccurredSpec,
    #[serde(default)]
    pub counterparty: Option<Field>,
    #[serde(default)]
    pub counterparty_account: Option<Field>,
    #[serde(default)]
    pub channel: Option<Field>,
    #[serde(default)]
    pub account: Option<Field>,
    #[serde(default)]
    pub reference_id: Option<Field>,
    #[serde(default)]
    pub description: Option<Field>,
    #[serde(default)]
    pub extra: BTreeMap<String, Field>,
}
