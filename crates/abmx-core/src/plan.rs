use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::fetcher::MessageKey;
use crate::policy::BrokenReason;
use crate::sink::ImportRow;
use crate::ABMX_SCHEMA_VERSION;

/// Version of the plan document. Bumped when the shape changes incompatibly.
pub const PLAN_VERSION: u32 = 4;

/// The inspectable artifact between parsing and writing.
///
/// A plan carries the full transaction, not a reference, so `sync --dry-run`
/// can print it and a test can assert on it without touching the mailbox again.
/// Everything in `rows` is meant to be written; `broken` is the list a human
/// must fix at the parser or Gmail-rule level.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Plan {
    pub version: u32,
    /// `abmx-parser`'s output contract at the time the plan was made.
    pub abmx_schema_version: u32,
    pub generated_at: DateTime<Utc>,
    #[serde(default)]
    pub sources: Vec<SourceReport>,
    pub rows: Vec<PlannedRow>,
    /// Labeled messages that could not be imported. Each one names the label
    /// and the parser/rule defect that needs fixing.
    #[serde(default)]
    pub broken: Vec<BrokenMessage>,
    /// Messages no configured label claimed. Validation should make this
    /// unreachable, but a plan must never silently drop mail.
    #[serde(default)]
    pub unmatched: Vec<UnmatchedMessage>,
    pub stats: Stats,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SourceReport {
    pub fetcher: String,
    pub fetched: usize,
    pub skipped: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UnmatchedMessage {
    pub message: MessageKey,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subject: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from: Option<String>,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BrokenMessage {
    pub message: MessageKey,
    /// The routing label, e.g. `receipt/example.card_debit`.
    pub label: String,
    /// The parser id, when the label named one that exists.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parser: Option<String>,
    pub reason: BrokenReason,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlannedRow {
    /// `abmx-parser` transaction id. Also the sink's `imported_id`, which is why
    /// re-applying is safe.
    pub id: String,
    /// Source message identity, for state bookkeeping.
    pub message: MessageKey,
    /// The label that routed this message.
    pub account_key: String,
    /// Resolved sink account.
    pub account: String,
    pub transaction: abmx_tx::Transaction,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Stats {
    pub fetched: usize,
    pub parsed: usize,
    /// Rows that will be written.
    pub import: usize,
    /// Labeled messages that could not be imported.
    pub broken: usize,
    /// Messages skipped because they were already delivered.
    pub skipped: usize,
    /// Labeled messages no configured parser claimed.
    pub unmatched: usize,
    /// Parse-level problems that did not stop the run.
    pub failed: usize,
    #[serde(default)]
    pub errors: Vec<String>,
}

impl Plan {
    pub fn new() -> Self {
        Self {
            version: PLAN_VERSION,
            abmx_schema_version: ABMX_SCHEMA_VERSION,
            generated_at: Utc::now(),
            sources: Vec::new(),
            rows: Vec::new(),
            broken: Vec::new(),
            unmatched: Vec::new(),
            stats: Stats::default(),
        }
    }

    /// Rows that should be written, with their resolved sink account.
    pub fn import_rows(&self) -> Vec<ImportRow> {
        self.rows
            .iter()
            .map(|row| ImportRow {
                tx_id: row.id.clone(),
                account: row.account.clone(),
                transaction: row.transaction.clone(),
            })
            .collect()
    }

    /// Check that this plan can be applied by this build.
    pub fn check_compatible(&self) -> Result<(), String> {
        if self.version != PLAN_VERSION {
            return Err(format!(
                "plan version {} is not supported (this build writes {PLAN_VERSION})",
                self.version
            ));
        }
        if self.abmx_schema_version != ABMX_SCHEMA_VERSION {
            return Err(format!(
                "plan was built with parser schema {} but this build expects {}",
                self.abmx_schema_version, ABMX_SCHEMA_VERSION
            ));
        }
        Ok(())
    }
}

impl Default for Plan {
    fn default() -> Self {
        Self::new()
    }
}
