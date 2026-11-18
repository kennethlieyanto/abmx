use std::collections::HashSet;

use abmx_core::{
    check_transaction, AccountMap, ApplyOptions, BrokenMessage, Error, FetchRequest, Fetcher, Plan,
    PlannedRow, Report, Result, Sink, SourceReport, UnmatchedMessage,
};

use crate::state::State;

/// The pipeline: fetch labeled mail → parse with the label's parser → classify
/// → plan → apply.
///
/// `plan` reads state but never writes it; `apply` is the only writer. There is
/// no human queue: a row is either imported, or it is `broken` and the sync
/// says which parser or account to fix.
pub struct Runner {
    pub parser: abmx_parser::Parser,
    /// The Gmail label prefix. The remainder is the parser id.
    pub prefix: String,
    /// Parser id → sink account.
    pub accounts: AccountMap,
    pub state: State,
}

impl Runner {
    pub fn new(
        parser: abmx_parser::Parser,
        prefix: impl Into<String>,
        accounts: AccountMap,
        state: State,
    ) -> Self {
        Self {
            parser,
            prefix: prefix.into(),
            accounts,
            state,
        }
    }

    fn parser_id(&self, label: &str) -> Option<String> {
        let prefix = self.prefix.to_ascii_lowercase();
        let lower = label.to_ascii_lowercase();
        let rest = lower.strip_prefix(&prefix)?;
        abmx_parse::is_parser_id(rest).then(|| rest.to_owned())
    }

    /// Produce a plan without touching the mailbox twice or writing anything.
    pub fn plan(&self, fetcher: &dyn Fetcher, request: FetchRequest<'_>) -> Result<Plan> {
        self.state.check_schema()?;

        let seen = self.state.seen_keys()?;
        let batch = fetcher.fetch(request.with_skip(&seen))?;

        let mut plan = Plan::new();
        plan.stats.fetched = batch.messages.len();

        let mut already_delivered = batch.already_seen;
        let mut seen_transactions: HashSet<String> = HashSet::new();

        for message in &batch.messages {
            // Already delivered on a previous run. Fetchers normally drop these
            // before downloading, but a custom fetcher may ignore the skip set.
            if seen.contains(message.key.as_str()) {
                already_delivered += 1;
                continue;
            }

            let Some(parser_id) = self.parser_id(&message.label) else {
                // Validation makes this unreachable in a normal run, but a plan
                // must never silently drop labeled mail.
                plan.stats.unmatched += 1;
                let details =
                    abmx_parser::parse_eml(&message.bytes, Some(message.origin.clone())).ok();
                plan.unmatched.push(UnmatchedMessage {
                    message: message.key.clone(),
                    subject: details.as_ref().map(|email| email.subject.clone()),
                    from: details.as_ref().map(|email| email.from.email.clone()),
                    reason: format!(
                        "label `{}` does not encode a parser id after `{}`",
                        message.label, self.prefix
                    ),
                });
                continue;
            };

            match self.parser.parse_bytes_with(
                &parser_id,
                &message.bytes,
                Some(message.origin.clone()),
                &parser_id,
            ) {
                Ok(abmx_parser::Outcome::Parsed(transaction)) => {
                    plan.stats.parsed += 1;
                    let transaction = *transaction;

                    // Two byte-identical emails share an id by design.
                    if !seen_transactions.insert(transaction.id.clone()) {
                        continue;
                    }

                    match check_transaction(&transaction, &self.accounts) {
                        Ok(()) => {
                            let account = self
                                .accounts
                                .get(&parser_id)
                                .expect("check_transaction validated the mapping")
                                .to_owned();
                            plan.stats.import += 1;
                            plan.rows.push(PlannedRow {
                                id: transaction.id.clone(),
                                message: message.key.clone(),
                                account_key: message.label.clone(),
                                account,
                                transaction,
                            });
                        }
                        Err(reason) => {
                            plan.stats.broken += 1;
                            plan.broken.push(BrokenMessage {
                                message: message.key.clone(),
                                label: message.label.clone(),
                                parser: Some(parser_id),
                                reason,
                            });
                        }
                    }
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
            skipped: batch.skipped + already_delivered,
        });
        plan.stats.skipped = already_delivered;

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
            if failed.contains(row.id.as_str()) {
                continue;
            }
            self.state
                .mark_seen(row.message.as_str(), sink.name(), &row.id, "imported")?;
        }

        Ok(report)
    }
}
