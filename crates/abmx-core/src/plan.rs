use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::fetcher::MessageKey;
use crate::policy::Decision;
use crate::sink::ImportRow;
use crate::ABMX_SCHEMA_VERSION;

/// Version of the plan document. Bumped when the shape changes incompatibly.
pub const PLAN_VERSION: u32 = 3;

/// The inspectable artifact between deciding and writing.
///
/// A plan is produced by `abmx plan` and consumed by `abmx apply`. It
/// carries the full transaction, not a reference, so a plan can be reviewed,
/// archived, replayed after a sink outage, and used as a test fixture without
/// touching the mailbox again.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Plan {
    pub version: u32,
    /// `abmx-parser`'s output contract at the time the plan was made.
    pub abmx_schema_version: u32,
    pub generated_at: DateTime<Utc>,
    #[serde(default)]
    pub sources: Vec<SourceReport>,
    pub rows: Vec<PlannedRow>,
    /// Messages the fetcher returned that no parser claimed. Structured so the
    /// CLI can say *what* was skipped, and so `plan --out` carries it.
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
    pub date: Option<chrono::NaiveDate>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subject: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from: Option<String>,
    /// Human-readable explanation, e.g. "no parser matched the sender or subject".
    pub reason: String,
    /// The raw router code (`no_match`, `disabled`, …), for debugging.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlannedRow {
    /// `abmx-parser` transaction id. Also the sink's `imported_id`, which is why the
    /// plan can be replayed safely.
    pub id: String,
    /// Source message identity, for state bookkeeping.
    pub message: MessageKey,
    /// The label that routed this message (`abmx:example-qris`).
    pub account_key: String,
    /// Resolved sink account, present exactly when the decision is `Import`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account: Option<String>,
    pub decision: Decision,
    pub transaction: abmx_tx::Transaction,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Stats {
    pub fetched: usize,
    pub parsed: usize,
    pub unmatched: usize,
    pub failed: usize,
    pub import: usize,
    pub review: usize,
    pub skip: usize,
    /// Rows re-decided from the review queue rather than freshly fetched.
    /// This is how a mapping or policy fix takes effect without a reset.
    #[serde(default)]
    pub recovered: usize,
    /// Parse-level problems that did not stop the run (an email that claimed to
    /// be a transaction but could not be extracted).
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
            unmatched: Vec::new(),
            stats: Stats::default(),
        }
    }

    /// Rows that should be written, with their resolved sink account.
    ///
    /// A `Decision::Import` row without an account would be a bug in the
    /// planner, so it is filtered rather than defaulted.
    pub fn import_rows(&self) -> Vec<ImportRow> {
        self.rows
            .iter()
            .filter_map(|row| {
                if !row.decision.is_import() {
                    return None;
                }
                let account = row.account.clone()?;
                Some(ImportRow {
                    tx_id: row.id.clone(),
                    account,
                    transaction: row.transaction.clone(),
                })
            })
            .collect()
    }

    pub fn review_rows(&self) -> impl Iterator<Item = &PlannedRow> {
        self.rows.iter().filter(|row| row.decision.is_review())
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
