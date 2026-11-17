use std::collections::HashSet;

use abmx_core::{
    decide, ApplyOptions, Decision, Error, FetchRequest, Fetcher, ImportRow, LabelRoutes,
    MessageKey, Plan, PlannedRow, PolicyConfig, Report, Result, Sink, SourceReport,
    UnmatchedMessage,
};

use crate::state::{ReviewEntry, State};

/// The pipeline: fetch labeled mail → parse with the label's parser → decide →
/// plan → apply.
///
/// `plan` reads state but never writes it; `apply` is the only writer. That split
/// is what makes `--dry-run` free and plans replayable.
pub struct Runner {
    pub parser: abmx_parser::Parser,
    pub policy: PolicyConfig,
    pub labels: LabelRoutes,
    pub state: State,
}

impl Runner {
    pub fn new(
        parser: abmx_parser::Parser,
        policy: PolicyConfig,
        labels: LabelRoutes,
        state: State,
    ) -> Self {
        Self {
            parser,
            policy,
            labels,
            state,
        }
    }

    fn accounts(&self) -> abmx_core::AccountMap {
        self.labels.accounts()
    }

    /// Produce a plan without touching the mailbox twice or writing anything.
    pub fn plan(&self, fetcher: &dyn Fetcher, request: FetchRequest<'_>) -> Result<Plan> {
        self.state.check_schema()?;

        let batch = fetcher.fetch(request)?;
        let accounts = self.accounts();

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

            let Some(route) = self.labels.get(&message.label) else {
                // Validation makes this unreachable in a normal run, but a plan
                // must never silently drop labeled mail.
                plan.stats.unmatched += 1;
                let details =
                    abmx_parser::parse_eml(&message.bytes, Some(message.origin.clone())).ok();
                plan.unmatched.push(UnmatchedMessage {
                    message: message.key.clone(),
                    date: details.as_ref().map(|email| email.date.date_naive()),
                    subject: details.as_ref().map(|email| email.subject.clone()),
                    from: details.as_ref().map(|email| email.from.email.clone()),
                    reason: format!("label `{}` has no [labels] entry", message.label),
                    code: Some("unmapped_label".to_owned()),
                });
                continue;
            };

            match self.parser.parse_bytes_with(
                &route.parser,
                &message.bytes,
                Some(message.origin.clone()),
                &message.label,
            ) {
                Ok(abmx_parser::Outcome::Parsed(transaction)) => {
                    plan.stats.parsed += 1;
                    let transaction = *transaction;

                    // Two byte-identical emails share an id by design.
                    if !seen_transactions.insert(transaction.id.clone()) {
                        continue;
                    }

                    let decision = decide(&transaction, &self.policy, &accounts);
                    let account = decision.is_import().then(|| route.account.clone());

                    match &decision {
                        Decision::Import => plan.stats.import += 1,
                        Decision::Review(_) => plan.stats.review += 1,
                        Decision::Skip(_) => plan.stats.skip += 1,
                    }

                    plan.rows.push(PlannedRow {
                        id: transaction.id.clone(),
                        message: message.key.clone(),
                        account_key: message.label.clone(),
                        account,
                        decision,
                        transaction,
                    });
                }
                Ok(abmx_parser::Outcome::Missing(parser_id)) => {
                    plan.stats.failed += 1;
                    plan.stats.errors.push(format!(
                        "{}: label `{}` names parser `{parser_id}`, which is not in the corpus",
                        message.key, message.label
                    ));
                }
                Ok(abmx_parser::Outcome::Failed { parser, error }) => {
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
        });

        // Re-decide what is already in the review queue. The stored transaction
        // carries the label in `account`, so a mapping or policy fix takes
        // effect without re-fetching. This is what makes "fix [labels], run
        // sync again" work without approve.
        for entry in self.state.review()? {
            if !seen_transactions.insert(entry.tx_id.clone()) {
                continue;
            }

            let decision = decide(&entry.transaction, &self.policy, &accounts);
            let account = decision
                .is_import()
                .then(|| self.labels.get(&entry.transaction.account))
                .flatten()
                .map(|route| route.account.clone());

            match &decision {
                Decision::Import => plan.stats.import += 1,
                Decision::Review(_) => plan.stats.review += 1,
                Decision::Skip(_) => plan.stats.skip += 1,
            }
            plan.stats.recovered += 1;

            plan.rows.push(PlannedRow {
                id: entry.tx_id,
                message: MessageKey::new(entry.message),
                account_key: entry.transaction.account.clone(),
                account,
                decision,
                transaction: entry.transaction,
            });
        }

        Ok(plan)
    }

    /// Write the plan's import rows, then record what happened.
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
                // A row that was in the queue and is now skipped is resolved:
                // drop it so it stops showing up.
                Decision::Skip(_) => {
                    self.state.clear_review(&row.id)?;
                }
                // Review rows are deliberately *not* marked seen. They are
                // decisions, not deliveries: a policy change must be able to
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

            let route = self.labels.get(&entry.transaction.account).ok_or_else(|| {
                Error::Config(format!(
                    "label `{}` is not configured in [labels]",
                    entry.transaction.account
                ))
            })?;

            rows.push(ImportRow {
                tx_id: entry.tx_id.clone(),
                account: route.account.clone(),
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
