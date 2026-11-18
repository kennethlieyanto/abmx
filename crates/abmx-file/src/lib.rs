//! The smallest possible sink: append one JSON object per row to a file.
//!
//! It exists for three reasons:
//!
//! * It is the reference implementation of [`abmx_core::Sink`] — about sixty
//!   lines with no network, no auth, and no vendor quirks.
//! * It is the "pull" case: `abmx` writes, something else reads.
//! * It is deliberately **not** idempotent, which exercises the runner's
//!   applied-state bookkeeping that an idempotent sink does not need.
//!
//! The output is JSONL of `{ "tx_id", "account", "transaction" }`, where
//! `transaction` is `abmx-parser`'s contract verbatim.

use std::io::Write;
use std::path::PathBuf;

use abmx_core::{ApplyOptions, Error, ImportRow, LedgerRow, Report, Result, Sink, SinkAccount};
use chrono::NaiveDate;
use serde_json::json;

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
        // A file has no accounts. Returning an empty list is honest; `status`
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
                report.errors.push(abmx_core::RowError {
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

    /// Re-read the JSONL this sink wrote.
    ///
    /// A missing file is an empty ledger, not an error: that is the state
    /// before the first sync.
    fn snapshot(
        &self,
        accounts: &[String],
        since: NaiveDate,
        until: NaiveDate,
    ) -> Result<Vec<LedgerRow>> {
        let text = match std::fs::read_to_string(&self.path) {
            Ok(text) => text,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(Error::io(&self.path, error)),
        };

        let mut rows = Vec::new();
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }

            let value: serde_json::Value = serde_json::from_str(line)?;

            let Some(account) = value.get("account").and_then(|value| value.as_str()) else {
                continue;
            };
            if !accounts.iter().any(|wanted| wanted == account) {
                continue;
            }

            let transaction = &value["transaction"];
            let Some(date) = transaction
                .get("occurred_on")
                .and_then(|value| value.as_str())
                .and_then(|date| NaiveDate::parse_from_str(date, "%Y-%m-%d").ok())
            else {
                continue;
            };
            if date < since || date > until {
                continue;
            }

            rows.push(LedgerRow {
                account: account.to_owned(),
                date,
                amount_minor: transaction
                    .get("amount_minor")
                    .and_then(|value| value.as_i64())
                    .unwrap_or(0),
                currency_exponent: transaction
                    .get("currency_exponent")
                    .and_then(|value| value.as_u64())
                    .unwrap_or(0) as u32,
                payee: transaction
                    .get("counterparty")
                    .and_then(|value| value.as_str())
                    .map(str::to_owned),
                imported_id: value
                    .get("tx_id")
                    .and_then(|value| value.as_str())
                    .map(str::to_owned),
            });
        }

        Ok(rows)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_row(path: &std::path::Path, tx_id: &str, account: &str, date: &str, amount: i64) {
        let line = json!({
            "tx_id": tx_id,
            "account": account,
            "transaction": {
                "id": tx_id,
                "occurred_on": date,
                "amount_minor": amount,
                "currency_exponent": 0,
                "currency": "IDR",
                "counterparty": "ExamplePay"
            }
        });
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .unwrap();
        writeln!(file, "{line}").unwrap();
    }

    #[test]
    fn a_missing_file_is_an_empty_ledger() {
        let dir = tempfile::tempdir().unwrap();
        let sink = FileSink::new(dir.path().join("missing.jsonl"));
        let rows = sink
            .snapshot(
                &["acc-1".to_owned()],
                NaiveDate::from_ymd_opt(2026, 11, 1).unwrap(),
                NaiveDate::from_ymd_opt(2026, 11, 30).unwrap(),
            )
            .unwrap();
        assert!(rows.is_empty());
    }

    #[test]
    fn snapshot_filters_by_account_and_window() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ledger.jsonl");
        write_row(&path, "mail_1", "acc-1", "2026-11-06", -2_001_500);
        write_row(&path, "mail_2", "acc-1", "2026-10-01", -1_000);
        write_row(&path, "mail_3", "acc-2", "2026-11-06", -500);

        let sink = FileSink::new(&path);
        let rows = sink
            .snapshot(
                &["acc-1".to_owned()],
                NaiveDate::from_ymd_opt(2026, 11, 1).unwrap(),
                NaiveDate::from_ymd_opt(2026, 11, 30).unwrap(),
            )
            .unwrap();

        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].imported_id.as_deref(), Some("mail_1"));
        assert_eq!(rows[0].amount_minor, -2_001_500);
        assert_eq!(rows[0].payee.as_deref(), Some("ExamplePay"));
    }
}
