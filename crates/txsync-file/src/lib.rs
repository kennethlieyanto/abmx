//! The smallest possible sink: append one JSON object per row to a file.
//!
//! It exists for three reasons:
//!
//! * It is the reference implementation of [`txsync_core::Sink`] — about sixty
//!   lines with no network, no auth, and no vendor quirks.
//! * It is the "pull" case: `txsync` writes, something else reads.
//! * It is deliberately **not** idempotent, which exercises the runner's
//!   applied-state bookkeeping that an idempotent sink does not need.
//!
//! The output is JSONL of `{ "tx_id", "account", "transaction" }`, where
//! `transaction` is `txp`'s contract verbatim.

use std::io::Write;
use std::path::PathBuf;

use serde_json::json;
use txsync_core::{ApplyOptions, Error, ImportRow, Report, Result, Sink, SinkAccount};

pub struct FileSink {
    path: PathBuf,
}

impl FileSink {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    pub fn path(&self) -> &std::path::Path {
        &self.path
    }
}

impl Sink for FileSink {
    fn name(&self) -> &str {
        "file"
    }

    fn idempotent(&self) -> bool {
        false
    }

    fn accounts(&self) -> Result<Vec<SinkAccount>> {
        // A file has no accounts. Returning an empty list is honest; `doctor`
        // treats that as "nothing to verify" rather than an error.
        Ok(Vec::new())
    }

    fn apply(&self, rows: &[ImportRow], options: &ApplyOptions) -> Result<Report> {
        let mut report = Report {
            dry_run: options.dry_run,
            ..Report::default()
        };

        if options.dry_run {
            report.added = rows.len();
            return Ok(report);
        }

        if let Some(parent) = self.path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent).map_err(|error| Error::io(parent, error))?;
            }
        }

        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
            .map_err(|error| Error::io(&self.path, error))?;

        for row in rows {
            let line = json!({
                "tx_id": row.tx_id,
                "account": row.account,
                "transaction": row.transaction,
            });
            let encoded = serde_json::to_string(&line)?;
            if let Err(error) = writeln!(file, "{encoded}") {
                report.errors.push(txsync_core::RowError {
                    tx_id: Some(row.tx_id.clone()),
                    message: error.to_string(),
                });
                continue;
            }
            report.added += 1;
        }

        file.flush().map_err(|error| Error::io(&self.path, error))?;
        Ok(report)
    }
}
