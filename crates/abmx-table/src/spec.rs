//! The TOML format for a statement parser.
//!
//! Same philosophy as the email spec: one directory per parser, the directory
//! name is the id, and every regex is compiled at load time. The difference is
//! that a statement is a table: columns are geometric, rows repeat, and the
//! document carries the arithmetic needed to prove the parse.

use abmx_parse::NumberStyle;
use serde::Deserialize;

/// Version of the statement spec format itself.
pub const SPEC_VERSION: u32 = 1;

#[derive(Debug, Clone, Deserialize)]
pub struct ParserSpec {
    pub spec_version: u32,
    #[serde(default = "one")]
    pub version: u32,
    #[serde(default = "yes")]
    pub enabled: bool,
    /// `pdf` today. The registry only claims specs whose input this engine
    /// understands, so another format can be added without touching email
    /// parsers.
    pub input: String,
    #[serde(default)]
    pub defaults: Defaults,
    pub document: DocumentSpec,
    pub sections: Vec<SectionSpec>,
}

fn one() -> u32 {
    1
}

fn yes() -> bool {
    true
}

#[derive(Debug, Clone, Deserialize)]
pub struct Defaults {
    pub bank: String,
    pub currency: String,
    #[serde(default)]
    pub tz: String,
    /// Fixed UTC offset used for `occurred_at`, e.g. `+07:00`. The IANA name in
    /// `tz` is informational; abmx carries a fixed offset for statements the
    /// same way it carries the email's offset for receipts.
    #[serde(default = "default_offset")]
    pub offset: String,
    #[serde(default = "default_number")]
    pub number: NumberStyle,
}

impl Default for Defaults {
    fn default() -> Self {
        Self {
            bank: String::new(),
            currency: "IDR".to_owned(),
            tz: String::new(),
            offset: default_offset(),
            number: default_number(),
        }
    }
}

fn default_offset() -> String {
    "+00:00".to_owned()
}

fn default_number() -> NumberStyle {
    NumberStyle::En
}

#[derive(Debug, Clone, Deserialize)]
pub struct DocumentSpec {
    /// Must capture `start` and `end`.
    pub period: PeriodField,
    /// Must capture `value`. Used only for document identity, never emitted.
    #[serde(default)]
    pub account: Option<DocField>,
    /// Must capture `value`, the total page count printed on every page.
    #[serde(default)]
    pub pages: Option<DocField>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct PeriodField {
    pub regex: String,
    pub formats: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct DocField {
    pub regex: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SectionSpec {
    /// Becomes `Transaction.account`; the config maps `<parser>:<key>` to a
    /// ledger account.
    pub key: String,
    /// A line matching this opens (or re-opens) the section.
    pub start: String,
    #[serde(default)]
    pub opening: Option<DocField>,
    /// Required: without a closing balance the section cannot be verified.
    pub closing: DocField,
    #[serde(default)]
    pub total_credit: Option<DocField>,
    #[serde(default)]
    pub total_debit: Option<DocField>,
    pub columns: Columns,
    pub table: TableSpec,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Columns {
    pub date: Column,
    #[serde(default)]
    pub value_date: Option<Column>,
    pub description: Column,
    pub amount: Column,
    #[serde(default)]
    pub marker: Option<Column>,
    pub balance: Column,
}

/// A half-open horizontal range in PDF points: `x[0] <= run.x < x[1]`.
#[derive(Debug, Clone, Deserialize)]
pub struct Column {
    pub x: [f32; 2],
}

#[derive(Debug, Clone, Deserialize)]
pub struct TableSpec {
    pub row_when: RowWhen,
    /// Matched against the description cell and the whole line. Use it for the
    /// opening-balance row and for repeated page furniture.
    #[serde(default)]
    pub skip: Vec<String>,
    #[serde(default)]
    pub date: DateRule,
    #[serde(default)]
    pub direction: DirectionRule,
}

#[derive(Debug, Clone, Deserialize)]
pub struct RowWhen {
    pub column: String,
    pub regex: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct DateRule {
    /// The printed transaction-date format. It must not contain a year: the
    /// year is resolved against the statement period.
    #[serde(default = "default_date_format")]
    pub format: String,
}

impl Default for DateRule {
    fn default() -> Self {
        Self {
            format: default_date_format(),
        }
    }
}

fn default_date_format() -> String {
    "%d/%m".to_owned()
}

#[derive(Debug, Clone, Deserialize)]
pub struct DirectionRule {
    /// The marker that means money left the account. A row without the marker
    /// is a credit.
    #[serde(default = "default_out_marker")]
    pub out_marker: String,
}

impl Default for DirectionRule {
    fn default() -> Self {
        Self {
            out_marker: default_out_marker(),
        }
    }
}

fn default_out_marker() -> String {
    "D".to_owned()
}
