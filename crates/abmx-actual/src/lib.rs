//! Actual Budget as a [`abmx_core::Sink`].
//!
//! Actual has no HTTP API: the official client is [`@actual-app/api`], a Node
//! library that downloads a copy of the budget, applies changes locally, and
//! syncs them back. This crate therefore drives the official client through a
//! bundled helper script (`helper/helper.js`) that `abmx` spawns for one
//! operation at a time. Nothing stays running between operations.
//!
//! [`@actual-app/api`]: https://actualbudget.org/docs/api/

pub mod convert;
pub mod helper;
pub mod version;

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::Duration;

use chrono::NaiveDate;
use serde_json::{json, Map, Value};

use abmx_core::{
    ApplyOptions, Error, ImportRow, LedgerRow, Report, Result, RowError, Sink, SinkAccount,
};

pub use convert::{to_actual, to_cents, ClearedPolicy, NotesConfig};
pub use helper::{Helper, ProcessHelper};
pub use version::{Concern, VersionReport};

/// How `abmx` authenticates to the Actual server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ActualAuth {
    /// A budget-scoped API key, preferred: revocable and limited to one budget.
    ApiKey(String),
    /// The server password, for servers too old to have API keys.
    ServerPassword(String),
}

#[derive(Debug, Clone)]
pub struct ActualConfig {
    /// Base URL of the Actual server itself, e.g. `http://localhost:5006`.
    pub server_url: String,
    pub auth: ActualAuth,
    /// Actual's Sync ID: Settings → Show advanced settings → Sync ID.
    pub budget_sync_id: String,
    /// Only for end-to-end encrypted budgets.
    pub budget_password: Option<String>,
    /// Where `@actual-app/api` caches its copy of the budget. Persistent, or
    /// every run downloads the whole file.
    pub data_dir: PathBuf,
    /// Node executable. `node` resolves through `PATH`.
    pub node: PathBuf,
    /// The helper script (`helper/helper.js`).
    pub helper: PathBuf,
    /// Whether imported rows are marked cleared in Actual.
    pub cleared: ClearedPolicy,
    /// Re-import rows that were previously imported and then deleted.
    ///
    /// `true` treats the ledger as a projection of the mailbox: if a row is
    /// gone, a rebuild puts it back. `false` makes a ledger deletion permanent,
    /// which is what Actual's file-import UI does.
    pub reimport_deleted: bool,
    pub timeout: Duration,
}

impl ActualConfig {
    pub fn new(
        server_url: impl Into<String>,
        api_key: impl Into<String>,
        budget_sync_id: impl Into<String>,
    ) -> Self {
        Self {
            server_url: server_url.into(),
            auth: ActualAuth::ApiKey(api_key.into()),
            budget_sync_id: budget_sync_id.into(),
            budget_password: None,
            data_dir: std::env::temp_dir().join("abmx-actual"),
            node: PathBuf::from("node"),
            helper: PathBuf::from("abmx-actual-helper"),
            cleared: ClearedPolicy::Never,
            reimport_deleted: true,
            timeout: Duration::from_secs(120),
        }
    }

    /// Authenticate with the Actual server password instead of an API key.
    pub fn with_password(
        server_url: impl Into<String>,
        password: impl Into<String>,
        budget_sync_id: impl Into<String>,
    ) -> Self {
        let mut config = Self::new(server_url, "", budget_sync_id);
        config.auth = ActualAuth::ServerPassword(password.into());
        config
    }
}

pub struct ActualSink {
    helper: Box<dyn Helper>,
    config: ActualConfig,
    notes: NotesConfig,
}

impl ActualSink {
    pub fn new(config: ActualConfig, notes: NotesConfig) -> Result<Self> {
        if config.server_url.trim().is_empty() {
            return Err(Error::Sink(
                "[sink.actual].server_url must not be empty".to_owned(),
            ));
        }
        if config.budget_sync_id.trim().is_empty() {
            return Err(Error::Sink(
                "[sink.actual].budget_sync_id must not be empty".to_owned(),
            ));
        }
        let helper = Box::new(ProcessHelper::new(config.clone()));
        Ok(Self {
            helper,
            config,
            notes,
        })
    }

    /// Build a sink around a different transport. Used by tests.
    #[doc(hidden)]
    pub fn with_helper(config: ActualConfig, notes: NotesConfig, helper: Box<dyn Helper>) -> Self {
        Self {
            helper,
            config,
            notes,
        }
    }

    fn params(&self, extra: Map<String, Value>) -> Value {
        let mut params = extra;
        params.insert("budgetSyncId".to_owned(), json!(self.config.budget_sync_id));
        if let Some(password) = &self.config.budget_password {
            params.insert("budgetPassword".to_owned(), json!(password));
        }
        Value::Object(params)
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

        let mut params = Map::new();
        params.insert("account".to_owned(), json!(account));
        params.insert("transactions".to_owned(), json!(transactions));
        params.insert(
            "options".to_owned(),
            json!({
                // Only used for rows that omit `cleared`, which we never do;
                // keep it consistent instead of asserting the opposite.
                "defaultCleared": self.config.cleared == ClearedPolicy::Always,
                // The ledger is a projection of the mailbox: a rebuild
                // recreates rows that were deleted.
                "reimportDeleted": self.config.reimport_deleted,
                // Preserve `ExamplePay`, not `Examplepay`.
                "payeeNameNormalization": "original",
                "dryRun": dry_run,
            }),
        );

        let data = self
            .helper
            .call("import", self.params(params))
            .map_err(|error| error.to_string())?;

        let added = data.get("added").and_then(Value::as_u64).unwrap_or(0) as usize;
        let updated = data.get("updated").and_then(Value::as_u64).unwrap_or(0) as usize;

        Ok((added, updated))
    }
}

impl Sink for ActualSink {
    fn name(&self) -> &str {
        "actual"
    }

    /// Actual deduplicates on `imported_id`, which is exactly `abmx-parser`'s `id`.
    fn idempotent(&self) -> bool {
        true
    }

    fn preview_notes(&self, transaction: &abmx_core::Transaction) -> Option<String> {
        crate::convert::notes(transaction, &self.notes)
    }

    fn accounts(&self) -> Result<Vec<SinkAccount>> {
        let data = self.helper.call("accounts", self.params(Map::new()))?;
        let accounts = data.as_array().ok_or_else(|| {
            Error::Sink(format!(
                "actual: accounts response was not a list: {}",
                version::truncate(&data.to_string(), 300)
            ))
        })?;

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
        let mut params = Map::new();
        params.insert("accounts".to_owned(), json!(accounts));
        params.insert("since".to_owned(), json!(since.to_string()));
        params.insert("until".to_owned(), json!(until.to_string()));

        let data = self.helper.call("snapshot", self.params(params))?;
        let transactions = data.as_array().ok_or_else(|| {
            Error::Sink(format!(
                "actual: transactions response was not a list: {}",
                version::truncate(&data.to_string(), 300)
            ))
        })?;

        let mut rows = Vec::new();
        for transaction in transactions {
            let Some(date) = transaction
                .get("date")
                .and_then(Value::as_str)
                .and_then(|date| NaiveDate::parse_from_str(date, "%Y-%m-%d").ok())
            else {
                continue;
            };
            let Some(account) = transaction.get("account").and_then(Value::as_str) else {
                continue;
            };

            rows.push(LedgerRow {
                account: account.to_owned(),
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
            if row.transaction.status == abmx_parser::Status::Failed {
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
