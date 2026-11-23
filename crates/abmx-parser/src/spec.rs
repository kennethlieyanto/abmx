//! What a script declares about itself: `meta()`.

use std::path::PathBuf;

use abmx_tx::{currency_exponent, Kind};
use chrono::FixedOffset;
use rhai::Dynamic;
use serde::Deserialize;

use crate::util::NumberStyle;

/// Version of the host API a script is written against.
///
/// Bump this only when the functions a script may call change in a way that
/// breaks existing scripts. `spec_version` in the old TOML format is gone.
pub const API_VERSION: u32 = 1;

/// Which entry point a parser implements.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum InputKind {
    Mail,
    Statement,
}

impl InputKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Mail => "mail",
            Self::Statement => "statement",
        }
    }
}

/// The validated contents of `meta()`.
#[derive(Debug, Clone)]
pub struct Meta {
    pub bank: String,
    pub currency: String,
    /// IANA name, informational.
    pub tz: String,
    /// Fixed offset used to build `occurred_at`, e.g. `+07:00`.
    pub offset: FixedOffset,
    pub offset_text: String,
    pub number: NumberStyle,
    /// Overrides the ISO-4217 minor-unit exponent when the bank prints finer
    /// precision than the currency has (BCA savings prints sub-rupiah interest).
    pub exponent_override: Option<u32>,
    pub kind: Kind,
    pub version: u32,
    pub enabled: bool,
    /// Every constant label block in a mail template. Used to partition regions.
    pub labels: Vec<String>,
    pub input: InputKind,
}

#[derive(Debug, Deserialize)]
struct RawMeta {
    bank: String,
    #[serde(default = "default_currency")]
    currency: String,
    #[serde(default)]
    tz: String,
    #[serde(default = "default_offset")]
    offset: String,
    #[serde(default)]
    number: NumberStyle,
    #[serde(default)]
    exponent: Option<u32>,
    #[serde(default = "default_kind")]
    kind: Kind,
    #[serde(default = "one")]
    version: u32,
    #[serde(default = "one")]
    api: u32,
    #[serde(default = "yes")]
    enabled: bool,
    #[serde(default)]
    labels: Vec<String>,
}

fn default_currency() -> String {
    "IDR".to_owned()
}

fn default_offset() -> String {
    "+00:00".to_owned()
}

fn default_kind() -> Kind {
    Kind::Unknown
}

fn one() -> u32 {
    1
}

fn yes() -> bool {
    true
}

impl Meta {
    /// The exponent used for this parser's money.
    pub fn exponent(&self) -> u32 {
        self.exponent_override
            .unwrap_or_else(|| currency_exponent(&self.currency))
    }

    /// Deserialize and validate `meta()` for a parser with a known entry point.
    pub fn from_dynamic(value: &Dynamic, input: InputKind, id: &str) -> Result<Self, String> {
        let raw: RawMeta = rhai::serde::from_dynamic(value)
            .map_err(|error| format!("meta() must return a map: {error}"))?;

        if raw.bank.trim().is_empty() {
            return Err("meta.bank must not be empty".to_owned());
        }
        if raw.currency.trim().is_empty() {
            return Err("meta.currency must not be empty".to_owned());
        }
        if raw.api != API_VERSION {
            return Err(format!(
                "meta.api {} is not supported (this build speaks {API_VERSION})",
                raw.api
            ));
        }
        if raw.version == 0 {
            return Err("meta.version must be at least 1".to_owned());
        }
        if input == InputKind::Mail && raw.kind == Kind::Unknown {
            return Err(
                "meta.kind is required for mail parsers (`purchase`, `transfer_out`, ...)"
                    .to_owned(),
            );
        }
        if input == InputKind::Mail && raw.labels.is_empty() {
            return Err("meta.labels must list every constant label in the template".to_owned());
        }

        let mut labels: Vec<String> = Vec::new();
        let mut seen: Vec<String> = Vec::new();
        for label in raw.labels {
            let normalized = crate::util::norm(&label);
            if normalized.is_empty() {
                return Err("meta.labels contains an empty label".to_owned());
            }
            if !seen.contains(&normalized) {
                seen.push(normalized);
                labels.push(label);
            }
        }

        let offset = raw
            .offset
            .parse::<FixedOffset>()
            .map_err(|error| format!("meta.offset `{}`: {error}", raw.offset))?;

        if id.trim().is_empty() {
            return Err("parser id must not be empty".to_owned());
        }

        Ok(Self {
            bank: raw.bank.trim().to_owned(),
            currency: raw.currency.trim().to_owned(),
            tz: raw.tz,
            offset,
            offset_text: raw.offset,
            number: raw.number,
            exponent_override: raw.exponent,
            kind: raw.kind,
            version: raw.version,
            enabled: raw.enabled,
            labels,
            input,
        })
    }
}

/// What `abmx parser specs` reports.
#[derive(Debug, Clone, serde::Serialize)]
pub struct SpecSummary {
    pub id: String,
    pub version: u32,
    pub bank: String,
    pub kind: Kind,
    pub input: &'static str,
    /// The `parser.rhai` file this parser was loaded from.
    pub file: PathBuf,
}
