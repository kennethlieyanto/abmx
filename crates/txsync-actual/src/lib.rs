//! Actual Budget as a [`txsync_core::Sink`].
//!
//! Actual has no REST API of its own — the official client is a Node library
//! that mutates a local copy of the budget and syncs it back. Rather than shell
//! out to Node, this talks to
//! [`actual-http-api`](https://github.com/jhonderson/actual-http-api), which
//! wraps that library and exposes the one endpoint that matters:
//! `POST /v1/budgets/{syncId}/accounts/{accountId}/transactions/import`.
//!
//! That endpoint calls `importTransactions`, which is the *reconciling* import:
//! it honours `imported_id`, so re-running `txsync` can never double-post.

pub mod convert;

use std::collections::BTreeMap;
use std::time::Duration;

use chrono::NaiveDate;
use serde_json::{json, Value};

use txsync_core::{
    ApplyOptions, Error, ImportRow, LedgerRow, Report, Result, RowError, Sink, SinkAccount,
};

pub use convert::{to_actual, to_cents, ClearedPolicy, NotesConfig};

#[derive(Debug, Clone)]
pub struct ActualConfig {
    /// Base URL of `actual-http-api`, e.g. `http://localhost:5007`.
    pub base_url: String,
    /// The wrapper's own API key (not the Actual server password).
    pub api_key: String,
    /// Actual's Sync ID: Settings → Show advanced settings → Sync ID.
    pub budget_sync_id: String,
    /// Only for end-to-end encrypted budgets.
    pub budget_password: Option<String>,
    /// Whether imported rows are marked cleared in Actual.
    pub cleared: ClearedPolicy,
    pub timeout: Duration,
}

impl ActualConfig {
    pub fn new(
        base_url: impl Into<String>,
        api_key: impl Into<String>,
        budget_sync_id: impl Into<String>,
    ) -> Self {
        Self {
            base_url: base_url.into(),
            api_key: api_key.into(),
            budget_sync_id: budget_sync_id.into(),
            budget_password: None,
            cleared: ClearedPolicy::Never,
            timeout: Duration::from_secs(120),
        }
    }
}

pub struct ActualSink {
    http: reqwest::blocking::Client,
    config: ActualConfig,
    notes: NotesConfig,
}

impl ActualSink {
    pub fn new(config: ActualConfig, notes: NotesConfig) -> Result<Self> {
        let http = reqwest::blocking::Client::builder()
            .timeout(config.timeout)
            .build()
            .map_err(|error| Error::Sink(format!("building http client: {error}")))?;

        Ok(Self {
            http,
            config,
            notes,
        })
    }

    fn url(&self, path: &str) -> String {
        format!(
            "{}/v1/budgets/{}{}",
            self.config.base_url.trim_end_matches('/'),
            self.config.budget_sync_id,
            path
        )
    }

    fn request(&self, method: reqwest::Method, url: &str) -> reqwest::blocking::RequestBuilder {
        let mut builder = self
            .http
            .request(method, url)
            .header("x-api-key", &self.config.api_key)
            .header("accept", "application/json");

        if let Some(password) = &self.config.budget_password {
            builder = builder.header("budget-encryption-password", password);
        }

        builder
    }

    fn import_chunk(
        &self,
        account: &str,
        rows: &[&ImportRow],
        dry_run: bool,
    ) -> std::result::Result<(usize, usize), String> {
        let mut transactions = Vec::with_capacity(rows.len());
        for row in rows {
            transactions.push(to_actual(
                &row.transaction,
                account,
                &self.notes,
                self.config.cleared,
            )?);
        }

        let body = json!({
            "transactions": transactions,
            // Only used for rows that omit `cleared`, which we never do; keep it
            // consistent with the policy instead of asserting the opposite.
            "defaultCleared": self.config.cleared == ClearedPolicy::Always,
            // Anything deleted in Actual must stay deleted.
            "reimportDeleted": false,
            // Preserve `ExamplePay`, not `Examplepay`.
            "payeeNameNormalization": "original",
            "dryRun": dry_run,
        });

        let url = self.url(&format!("/accounts/{account}/transactions/import"));
        let response = self
            .request(reqwest::Method::POST, &url)
            .json(&body)
            .send()
            .map_err(|error| error.to_string())?;

        let status = response.status();
        let text = response.text().map_err(|error| error.to_string())?;

        if !status.is_success() {
            return Err(format!("actual {status}: {}", error_message(&text)));
        }

        let value: Value = serde_json::from_str(&text)
            .map_err(|error| format!("actual response: {error}: {text}"))?;

        let data = value.get("data").cloned().unwrap_or(Value::Null);
        let added = data
            .get("added")
            .and_then(Value::as_array)
            .map(Vec::len)
            .unwrap_or(0);
        let updated = data
            .get("updated")
            .and_then(Value::as_array)
            .map(Vec::len)
            .unwrap_or(0);

        Ok((added, updated))
    }
}

impl Sink for ActualSink {
    fn name(&self) -> &str {
        "actual"
    }

    /// Actual deduplicates on `imported_id`, which is exactly `txp`'s `id`.
    fn idempotent(&self) -> bool {
        true
    }

    fn accounts(&self) -> Result<Vec<SinkAccount>> {
        let url = self.url("/accounts");
        let response = self
            .request(reqwest::Method::GET, &url)
            .send()
            .map_err(|error| Error::Sink(format!("actual: {error}")))?;

        let status = response.status();
        let text = response
            .text()
            .map_err(|error| Error::Sink(format!("actual: {error}")))?;

        if !status.is_success() {
            return Err(Error::Sink(format!(
                "actual {status}: {}",
                error_message(&text)
            )));
        }

        let value: Value = serde_json::from_str(&text)?;
        let accounts = value
            .get("data")
            .and_then(Value::as_array)
            .ok_or_else(|| Error::Sink(format!("actual: accounts response had no data: {text}")))?;

        Ok(accounts
            .iter()
            .filter_map(|account| {
                Some(SinkAccount {
                    id: account.get("id")?.as_str()?.to_owned(),
                    name: account
                        .get("name")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_owned(),
                    off_budget: account
                        .get("offbudget")
                        .and_then(Value::as_bool)
                        .unwrap_or(false),
                    closed: account
                        .get("closed")
                        .and_then(Value::as_bool)
                        .unwrap_or(false),
                })
            })
            .collect())
    }

    /// Read the account's transactions for a date window.
    ///
    /// Amounts come back in Actual's "value × 100", which is why the exponent
    /// is reported as 2 regardless of the currency.
    fn snapshot(
        &self,
        accounts: &[String],
        since: NaiveDate,
        until: NaiveDate,
    ) -> Result<Vec<LedgerRow>> {
        let mut rows = Vec::new();

        for account in accounts {
            let url = format!(
                "{}?since_date={since}&until_date={until}",
                self.url(&format!("/accounts/{account}/transactions"))
            );

            let response = self
                .request(reqwest::Method::GET, &url)
                .send()
                .map_err(|error| Error::Sink(format!("actual: {error}")))?;

            let status = response.status();
            let text = response
                .text()
                .map_err(|error| Error::Sink(format!("actual: {error}")))?;

            if !status.is_success() {
                return Err(Error::Sink(format!(
                    "actual {status}: {}",
                    error_message(&text)
                )));
            }

            let value: Value = serde_json::from_str(&text)?;
            let transactions = value.get("data").and_then(Value::as_array).ok_or_else(|| {
                Error::Sink(format!("actual: transactions response had no data: {text}"))
            })?;

            for transaction in transactions {
                let Some(date) = transaction
                    .get("date")
                    .and_then(Value::as_str)
                    .and_then(|date| NaiveDate::parse_from_str(date, "%Y-%m-%d").ok())
                else {
                    continue;
                };

                rows.push(LedgerRow {
                    account: account.clone(),
                    date,
                    amount_minor: transaction
                        .get("amount")
                        .and_then(Value::as_i64)
                        .unwrap_or(0),
                    currency_exponent: 2,
                    payee: transaction
                        .get("imported_payee")
                        .and_then(Value::as_str)
                        .map(str::to_owned),
                    imported_id: transaction
                        .get("imported_id")
                        .and_then(Value::as_str)
                        .map(str::to_owned),
                });
            }
        }

        Ok(rows)
    }

    fn apply(&self, rows: &[ImportRow], options: &ApplyOptions) -> Result<Report> {
        let mut report = Report {
            dry_run: options.dry_run,
            ..Report::default()
        };

        let mut by_account: BTreeMap<&str, Vec<&ImportRow>> = BTreeMap::new();
        for row in rows {
            // Policy should have filtered this, but a ledger must never receive
            // a transaction the bank says failed.
            if row.transaction.status == txp::Status::Failed {
                report.skipped.push(RowError {
                    tx_id: Some(row.tx_id.clone()),
                    message: "bank reported the transaction as failed".to_owned(),
                });
                continue;
            }
            by_account
                .entry(row.account.as_str())
                .or_default()
                .push(row);
        }

        let chunk_size = options.chunk_size.max(1);

        for (account, account_rows) in by_account {
            for chunk in account_rows.chunks(chunk_size) {
                match self.import_chunk(account, chunk, options.dry_run) {
                    Ok((added, updated)) => {
                        report.added += added;
                        report.updated += updated;
                    }
                    // One bad row should not cost the whole chunk. Retry the
                    // rows individually so the failure is attributable.
                    Err(_) if chunk.len() > 1 => {
                        for row in chunk {
                            match self.import_chunk(account, &[*row], options.dry_run) {
                                Ok((added, updated)) => {
                                    report.added += added;
                                    report.updated += updated;
                                }
                                Err(message) => report.errors.push(RowError {
                                    tx_id: Some(row.tx_id.clone()),
                                    message,
                                }),
                            }
                        }
                    }
                    Err(message) => report.errors.push(RowError {
                        tx_id: Some(chunk[0].tx_id.clone()),
                        message,
                    }),
                }
            }
        }

        Ok(report)
    }
}

fn error_message(body: &str) -> String {
    if let Ok(value) = serde_json::from_str::<Value>(body) {
        if let Some(error) = value.get("error").and_then(Value::as_str) {
            return error.to_owned();
        }
    }

    let trimmed = body.trim();
    if trimmed.chars().count() > 300 {
        let shortened: String = trimmed.chars().take(300).collect();
        format!("{shortened}…")
    } else if trimmed.is_empty() {
        "(empty response)".to_owned()
    } else {
        trimmed.to_owned()
    }
}
