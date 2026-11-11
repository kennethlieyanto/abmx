use std::collections::HashSet;

use txsync_core::{
    decide, AccountMap, ApplyOptions, Decision, Error, FetchRequest, Fetcher, ImportRow, Plan,
    PlannedRow, PolicyConfig, Report, Result, Sink, SourceReport,
};

use crate::state::{ReviewEntry, State};

/// The pipeline: fetch → parse with `txp` → decide → plan → apply.
///
/// `plan` reads state but never writes it; `apply` is the only writer. That split
/// is what makes `--dry-run` free and plans replayable.
pub struct Runner {
    pub parser: txp::Parser,
    pub policy: PolicyConfig,
    pub accounts: AccountMap,
    pub state: State,
}

impl Runner {
    pub fn new(
        parser: txp::Parser,
        policy: PolicyConfig,
        accounts: AccountMap,
        state: State,
    ) -> Self {
        Self {
            parser,
            policy,
            accounts,
            state,
        }
    }

    /// Produce a plan without touching the mailbox twice or writing anything.
    pub fn plan(&self, fetcher: &dyn Fetcher, request: FetchRequest<'_>) -> Result<Plan> {
        self.state.check_schema()?;

        // An explicit cursor wins, so a caller can replay a specific window.
        // Otherwise the checkpoint stored for this fetcher is used.
        let stored = self.state.cursor(fetcher.name())?;
        let cursor = request.cursor.cloned().or(stored);
        let request = FetchRequest {
            cursor: cursor.as_ref(),
            limit: request.limit,
        };

        let batch = fetcher.fetch(request)?;

        let mut plan = Plan::new();
        plan.stats.fetched = batch.messages.len();

        let mut skipped = batch.skipped;
        let mut seen_transactions: HashSet<String> = HashSet::new();

        for message in &batch.messages {
            // Already delivered on a previous run. Cheap and, for an idempotent
            // sink, merely an optimisation.
            if self.state.is_seen(message.key.as_str())? {
                skipped += 1;
                continue;
            }

            match self
                .parser
                .parse_bytes(&message.bytes, Some(message.origin.clone()))
            {
                Ok(txp::Outcome::Parsed(transaction)) => {
                    plan.stats.parsed += 1;
                    let transaction = *transaction;

                    // Two byte-identical emails share a `txp` id by design.
                    if !seen_transactions.insert(transaction.id.clone()) {
                        continue;
                    }

                    let decision = decide(&transaction, &self.policy, &self.accounts);
                    let account = decision
                        .is_import()
                        .then(|| self.accounts.get(&transaction.account))
                        .flatten()
                        .map(str::to_owned);

                    match &decision {
                        Decision::Import => plan.stats.import += 1,
                        Decision::Review(_) => plan.stats.review += 1,
                        Decision::Skip(_) => plan.stats.skip += 1,
                    }

                    plan.rows.push(PlannedRow {
                        id: transaction.id.clone(),
                        message: message.key.clone(),
                        account_key: transaction.account.clone(),
                        account,
                        decision,
                        transaction,
                    });
                }
                Ok(txp::Outcome::Unmatched(reason)) => {
                    plan.stats.unmatched += 1;
                    // A parser-corpus gap, not a delivery failure: the mailbox
                    // query should already have excluded unrelated mail.
                    plan.stats
                        .warnings
                        .push(format!("{}: {reason}", message.key));
                }
                Ok(txp::Outcome::Failed { parser, error }) => {
                    plan.stats.failed += 1;
                    plan.stats
                        .errors
                        .push(format!("{}: {parser}: {error}", message.key));
                }
                Err(error) => {
                    plan.stats.failed += 1;
                    plan.stats.errors.push(format!("{}: {error}", message.key));
                }
            }
        }

        plan.sources.push(SourceReport {
            fetcher: fetcher.name().to_owned(),
            fetched: batch.messages.len(),
            skipped,
            next_cursor: batch.next_cursor,
        });

        Ok(plan)
    }

    /// Write the plan's import rows, then record what happened.
    ///
    /// The cursor only advances when every row succeeded. A partial failure
    /// leaves the cursor in place, so the next run replays the batch and the
    /// state database skips what already landed.
    pub fn apply(&self, sink: &dyn Sink, plan: &Plan, options: &ApplyOptions) -> Result<Report> {
        plan.check_compatible().map_err(Error::Config)?;
        self.state.check_schema()?;

        let rows = plan.import_rows();
        let report = sink.apply(&rows, options)?;

        if options.dry_run {
            return Ok(report);
        }

        let failed: HashSet<&str> = report
            .errors
            .iter()
            .filter_map(|error| error.tx_id.as_deref())
            .collect();

        for row in &plan.rows {
            match &row.decision {
                Decision::Import => {
                    if failed.contains(row.id.as_str()) {
                        continue;
                    }
                    self.state
                        .mark_seen(row.message.as_str(), sink.name(), &row.id, "imported")?;
                    self.state.clear_review(&row.id)?;
                }
                // Review and skip rows are deliberately *not* marked seen. They
                // are decisions, not deliveries: a policy change must be able to
                // pick them up on the next run.
                Decision::Review(reason) => {
                    self.state.upsert_review(&ReviewEntry {
                        tx_id: row.id.clone(),
                        message: row.message.as_str().to_owned(),
                        reason: reason.as_str().to_owned(),
                        transaction: row.transaction.clone(),
                        created_at: chrono::Utc::now().to_rfc3339(),
                    })?;
                }
                Decision::Skip(_) => {}
            }
        }

        if report.errors.is_empty() {
            for source in &plan.sources {
                if let Some(cursor) = &source.next_cursor {
                    self.state.set_cursor(&source.fetcher, cursor)?;
                }
            }
        }

        Ok(report)
    }

    /// Import specific rows that a human approved from the review queue.
    pub fn approve(
        &self,
        sink: &dyn Sink,
        ids: &[String],
        options: &ApplyOptions,
    ) -> Result<Report> {
        self.state.check_schema()?;

        let mut rows = Vec::new();
        let mut entries = Vec::new();

        for id in ids {
            let entry = self
                .state
                .review_entry(id)?
                .ok_or_else(|| Error::Config(format!("no review row for {id}")))?;

            let account = self
                .accounts
                .get(&entry.transaction.account)
                .ok_or_else(|| {
                    Error::Config(format!(
                        "account `{}` is not mapped in [accounts]",
                        entry.transaction.account
                    ))
                })?
                .to_owned();

            rows.push(ImportRow {
                tx_id: entry.tx_id.clone(),
                account,
                transaction: entry.transaction.clone(),
            });
            entries.push(entry);
        }

        let report = sink.apply(&rows, options)?;

        if options.dry_run {
            return Ok(report);
        }

        let failed: HashSet<&str> = report
            .errors
            .iter()
            .filter_map(|error| error.tx_id.as_deref())
            .collect();

        for entry in &entries {
            if failed.contains(entry.tx_id.as_str()) {
                continue;
            }
            self.state
                .mark_seen(&entry.message, sink.name(), &entry.tx_id, "approved")?;
            self.state.clear_review(&entry.tx_id)?;
        }

        Ok(report)
    }
}
