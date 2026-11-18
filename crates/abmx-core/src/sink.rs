use chrono::NaiveDate;
use serde::{Deserialize, Serialize};

use crate::diff::LedgerRow;
use crate::error::{Error, Result};

/// One row handed to a sink. Everything the sink needs, and nothing about where
/// it came from.
#[derive(Debug, Clone)]
pub struct ImportRow {
    /// `abmx-parser` transaction id, used as the sink's idempotency key.
    pub tx_id: String,
    /// Sink-native account identity (an Actual account UUID).
    pub account: String,
    pub transaction: abmx_tx::Transaction,
}

/// An account as the sink sees it, used by `status` and `abmx accounts`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SinkAccount {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub off_budget: bool,
    #[serde(default)]
    pub closed: bool,
}

#[derive(Debug, Clone)]
pub struct ApplyOptions {
    /// Ask the sink what it *would* do. Sinks that support a native dry run
    /// should use it; sinks that do not must not write.
    pub dry_run: bool,
    /// Upper bound on rows per request. Large enough to be fast, small enough
    /// that one bad row does not poison a whole run.
    pub chunk_size: usize,
}

impl Default for ApplyOptions {
    fn default() -> Self {
        Self {
            dry_run: false,
            chunk_size: 50,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RowError {
    /// Present when the failure could be attributed to one row.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tx_id: Option<String>,
    pub message: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Report {
    pub added: usize,
    pub updated: usize,
    /// Rows the sink itself declined, with its reason.
    #[serde(default)]
    pub skipped: Vec<RowError>,
    /// Rows that failed. A non-empty list means the run is not clean.
    #[serde(default)]
    pub errors: Vec<RowError>,
    #[serde(default)]
    pub dry_run: bool,
}

impl Report {
    pub fn is_clean(&self) -> bool {
        self.errors.is_empty() && self.skipped.is_empty()
    }

    pub fn merge(&mut self, other: Report) {
        self.added += other.added;
        self.updated += other.updated;
        self.skipped.extend(other.skipped);
        self.errors.extend(other.errors);
    }
}

/// A destination for decided transactions.
///
/// Push and pull are not separate primitives: a pull sink (QIF file, JSON feed)
/// implements the same `apply` and writes an artifact instead of calling an API.
pub trait Sink {
    fn name(&self) -> &str;

    /// Does the sink deduplicate on [`ImportRow::tx_id`]?
    ///
    /// When `true`, the runner does not need to track what has been applied and
    /// the sink is the single source of truth. When `false`, the runner records
    /// applied rows locally — necessary, but a second source of truth that can
    /// drift, so prefer sinks that answer `true`.
    fn idempotent(&self) -> bool {
        false
    }

    /// Enumerate the sink's accounts. Used by `status` and `accounts`; should be
    /// cheap and side-effect free.
    fn accounts(&self) -> Result<Vec<SinkAccount>>;

    /// Write a batch. Partial success is expected and reported, not returned as
    /// an error: a sink that imported 49 of 50 rows should say so.
    fn apply(&self, rows: &[ImportRow], options: &ApplyOptions) -> Result<Report>;

    /// Sink-specific prose for `sync --dry-run`, such as Actual's `notes`.
    /// Must be cheap and side-effect free. `None` means the sink has no
    /// preview for this transaction.
    fn preview_notes(&self, _transaction: &crate::Transaction) -> Option<String> {
        None
    }

    /// Read back what the ledger already holds for these accounts and dates.
    ///
    /// Read-only by contract: `abmx diff` calls this and must never be able
    /// to mutate a budget. Sinks that cannot read their own ledger return an
    /// error here rather than an empty list, so "no matches" is never confused
    /// with "I cannot see".
    fn snapshot(
        &self,
        _accounts: &[String],
        _since: NaiveDate,
        _until: NaiveDate,
    ) -> Result<Vec<LedgerRow>> {
        Err(Error::Sink(format!(
            "sink `{}` cannot read its ledger, so `abmx diff` is unavailable",
            self.name()
        )))
    }
}
