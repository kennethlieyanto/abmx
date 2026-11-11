use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::fetcher::MessageKey;
use crate::policy::Decision;
use crate::sink::ImportRow;
use crate::TXP_SCHEMA_VERSION;

/// Version of the plan document. Bumped when the shape changes incompatibly.
pub const PLAN_VERSION: u32 = 1;

/// The inspectable artifact between deciding and writing.
///
/// A plan is produced by `txsync plan` and consumed by `txsync apply`. It
/// carries the full transaction, not a reference, so a plan can be reviewed,
/// archived, replayed after a sink outage, and used as a test fixture without
/// touching the mailbox again.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Plan {
    pub version: u32,
    /// `txp`'s output contract at the time the plan was made.
    pub txp_schema_version: u32,
    pub generated_at: DateTime<Utc>,
    #[serde(default)]
    pub sources: Vec<SourceReport>,
    pub rows: Vec<PlannedRow>,
    pub stats: Stats,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SourceReport {
    pub fetcher: String,
    pub fetched: usize,
    pub skipped: usize,
    /// Persist this only after the batch applies cleanly.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlannedRow {
    /// `txp` transaction id. Also the sink's `imported_id`, which is why the
    /// plan can be replayed safely.
    pub id: String,
    /// Source message identity, for state bookkeeping.
    pub message: MessageKey,
    /// Parser-declared account key (`example-account`).
    pub account_key: String,
    /// Resolved sink account, present exactly when the decision is `Import`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account: Option<String>,
    pub decision: Decision,
    pub transaction: txp_core::Transaction,
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
    /// Parse-level problems that did not stop the run (an email that claimed to
    /// be a transaction but could not be extracted).
    #[serde(default)]
    pub errors: Vec<String>,
    /// Messages that parsed but were not claimed by any parser. Usually a
    /// corpus gap rather than a failure.
    #[serde(default)]
    pub warnings: Vec<String>,
}

impl Plan {
    pub fn new() -> Self {
        Self {
            version: PLAN_VERSION,
            txp_schema_version: TXP_SCHEMA_VERSION,
            generated_at: Utc::now(),
            sources: Vec::new(),
            rows: Vec::new(),
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
        if self.txp_schema_version != TXP_SCHEMA_VERSION {
            return Err(format!(
                "plan was built with txp schema {} but this build expects {}",
                self.txp_schema_version, TXP_SCHEMA_VERSION
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
