//! The `abmx statement` command line interface.
//!
//! Statements are a separate verb from `sync` because they are a separate
//! cadence and a different contract: a monthly document, many rows, and a
//! ledger that is read before anything is written.
//!
//!   0  clean
//!   2  at least one document failed to parse or verify
//!   1  the tool itself failed

use std::collections::{BTreeSet, HashSet};
use std::path::{Path, PathBuf};

use abmx::config::Config;
use abmx::paths;
use abmx::state::State;
use abmx_core::{
    classify_window, ApplyOptions, FetchRequest, Fetcher, ImportRow, MessageKey, Plan, PlannedRow,
    WindowMatch,
};
use anyhow::{bail, Context, Result};
use clap::{Args, Subcommand};

const EXIT_FAILED: i32 = 2;
const MATCH_WINDOW_DAYS: i64 = 3;

#[derive(Args)]
pub struct StatementCli {
    /// Parser directory. Repeat to layer a local override over a base corpus.
    #[arg(long, global = true, action = clap::ArgAction::Append)]
    parsers: Vec<PathBuf>,

    #[command(subcommand)]
    command: StatementCommand,
}

#[derive(Subcommand)]
enum StatementCommand {
    /// Print the positioned lines of a statement (the authoring evidence).
    Dump {
        /// A PDF statement.
        path: PathBuf,
        /// Password file. `$ABMX_STATEMENT_PASSWORD` is used when omitted.
        #[arg(long, value_name = "FILE")]
        password_file: Option<PathBuf>,
        /// Machine-readable output.
        #[arg(long)]
        json: bool,
    },

    /// Verify every statement parser against its `fixtures/` PDFs.
    Check {
        #[arg(long, value_name = "ID")]
        parser: Option<String>,
        /// Password file. The configured `[statements.passwords]` entry wins
        /// when this is omitted.
        #[arg(long, value_name = "FILE")]
        password_file: Option<PathBuf>,
        #[arg(long, short)]
        verbose: bool,
        #[arg(long)]
        json: bool,
    },

    /// Fetch statement PDFs, verify them, and import rows the ledger lacks.
    Sync {
        /// Print the plan and write nothing.
        #[arg(long)]
        dry_run: bool,
        /// Print the plan as JSON on stdout.
        #[arg(long)]
        json: bool,
        /// Maximum unseen messages to fetch in this run.
        #[arg(long, value_name = "N")]
        limit: Option<usize>,
    },
}

pub fn run(cli: StatementCli, config_path: Option<PathBuf>) -> Result<i32> {
    let StatementCli { parsers, command } = cli;

    match command {
        StatementCommand::Dump {
            path,
            password_file,
            json,
        } => dump(&path, password_file.as_deref(), json),
        StatementCommand::Check {
            parser,
            password_file,
            verbose,
            json,
        } => check(
            &parsers,
            config_path.as_deref(),
            parser.as_deref(),
            password_file.as_deref(),
            verbose,
            json,
        ),
        StatementCommand::Sync {
            dry_run,
            json,
            limit,
        } => match sync(
            config_path.as_deref(),
            &parsers,
            dry_run,
            json,
            false,
            limit,
        )? {
            Some((_, code)) => Ok(code),
            None => Ok(0),
        },
    }
}

fn dump(path: &Path, password_file: Option<&Path>, json: bool) -> Result<i32> {
    let bytes = std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;
    let password = password_from(password_file)?;
    let document = abmx_doc::load(&bytes, password.as_deref())
        .map_err(|error| anyhow::anyhow!("{}: {error}", path.display()))?;

    if json {
        println!("{}", serde_json::to_string_pretty(&document)?);
        return Ok(0);
    }

    println!("file: {}", path.display());
    println!("pages: {}", document.pages.len());
    for page in &document.pages {
        println!();
        println!("--- page {} ---", page.number);
        for line in &page.lines {
            print!("{:8.2} |", line.y);
            for run in &line.runs {
                print!(" [{:6.1}] {:?}", run.x, run.text);
            }
            println!();
        }
    }
    Ok(0)
}

fn check(
    parsers: &[PathBuf],
    config_path: Option<&Path>,
    only: Option<&str>,
    password_file: Option<&Path>,
    verbose: bool,
    json: bool,
) -> Result<i32> {
    let config = load_optional(config_path);
    let dirs = parser_dirs(parsers, config.as_ref());
    let registry = abmx_table::Registry::from_dirs(&dirs)
        .map_err(|error| anyhow::anyhow!("loading statement parsers: {error}"))?;

    let ids: Vec<String> = registry
        .parsers
        .iter()
        .map(|parser| parser.id.clone())
        .filter(|id| only.is_none_or(|only| only == id))
        .collect();

    if ids.is_empty() {
        bail!("no statement parser matched --parser");
    }

    let mut rows = Vec::new();
    let mut parsed = 0usize;
    let mut failures = 0usize;

    for id in ids {
        let parser = registry.find(&id).expect("id came from the registry");
        let fixtures = parser
            .source_file
            .parent()
            .map(|dir| dir.join("fixtures"))
            .unwrap_or_default();

        if !fixtures.is_dir() {
            failures += 1;
            rows.push(serde_json::json!({
                "parser": id,
                "fixtures": fixtures.display().to_string(),
                "status": "missing_fixtures",
            }));
            continue;
        }

        let password = match password_file {
            Some(path) => Some(read_password_file(path)?),
            None => config
                .as_ref()
                .and_then(|(config, base)| config.statement_password(base, &id).ok().flatten())
                .or_else(|| std::env::var("ABMX_STATEMENT_PASSWORD").ok()),
        };

        for file in pdf_files(&fixtures) {
            let bytes =
                std::fs::read(&file).with_context(|| format!("reading {}", file.display()))?;

            let outcome = abmx_doc::load(&bytes, password.as_deref())
                .map_err(|error| error.to_string())
                .and_then(|document| {
                    parser
                        .parse(&document, Some(file.display().to_string()))
                        .map(|rows| (document, rows))
                });

            match outcome {
                Ok((document, transactions)) => {
                    parsed += 1;
                    let credits: i64 = transactions
                        .iter()
                        .filter(|tx| tx.amount.amount_minor > 0)
                        .map(|tx| tx.amount.amount_minor)
                        .sum();
                    let debits: i64 = transactions
                        .iter()
                        .filter(|tx| tx.amount.amount_minor < 0)
                        .map(|tx| -tx.amount.amount_minor)
                        .sum();
                    let detail = format!(
                        "{} pages, {} rows, credits {credits}, debits {debits}",
                        document.page_count(),
                        transactions.len()
                    );
                    if verbose {
                        eprintln!("{}: {id} -> strict: {detail}", file.display());
                    }
                    rows.push(serde_json::json!({
                        "parser": id,
                        "file": file.display().to_string(),
                        "status": "strict",
                        "rows": transactions.len(),
                        "pages": document.page_count(),
                        "detail": detail,
                    }));
                }
                Err(error) => {
                    failures += 1;
                    eprintln!("{}: {id} -> failed: {error}", file.display());
                    rows.push(serde_json::json!({
                        "parser": id,
                        "file": file.display().to_string(),
                        "status": "failed",
                        "detail": error,
                    }));
                }
            }
        }
    }

    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "strict": parsed,
                "failures": failures,
                "samples": rows,
            }))?
        );
    } else if failures == 0 {
        println!("ok: {parsed} statement(s) parsed and balanced");
    } else {
        println!("{parsed} strict, {failures} failure(s)");
    }

    Ok(if failures > 0 { EXIT_FAILED } else { 0 })
}

pub fn sync(
    config_path: Option<&Path>,
    parsers: &[PathBuf],
    dry_run: bool,
    json: bool,
    silent: bool,
    limit: Option<usize>,
) -> Result<Option<(Plan, i32)>> {
    let path = paths::config_path(config_path)?;
    let config = Config::load(&path)?;
    let base = Config::base_dir(&path);
    config.validate_statements()?;
    if !config.statements.enabled {
        if !silent {
            eprintln!("statements are disabled ([statements].enabled = false); nothing to do");
        }
        return Ok(None);
    }

    if config.statements.accounts.is_empty() {
        bail!(
            "no [statements.accounts] entries; route statements with `{prefix}<parser-id>` \
             and map them to an account",
            prefix = config.statements.prefix
        );
    }

    let dirs = parser_dirs(parsers, Some(&(config.clone(), base.clone())));
    let registry = abmx_table::Registry::from_dirs(&dirs)
        .map_err(|error| anyhow::anyhow!("loading statement parsers: {error}"))?;

    for key in config.statements.accounts.keys() {
        let parser_id = key.split(':').next().unwrap_or(key);
        if registry.find(parser_id).is_none() {
            bail!(
                "[statements.accounts] names parser `{parser_id}`, which is not a statement parser"
            );
        }
    }

    let gmail =
        config.build_gmail_with(&base, &config.statements.prefix, config.statements.since)?;
    let discovered: Vec<String> = gmail
        .routed_labels()?
        .into_iter()
        .map(|label| label.name)
        .collect();

    let mut labels = Vec::new();
    let mut mapped = BTreeSet::new();
    for label in &discovered {
        let Some(parser_id) = config.statements.parser_id(label) else {
            continue;
        };
        if config.statements.is_ignored(&parser_id) {
            continue;
        }
        if is_mapped(&config, &parser_id) {
            labels.push(label.clone());
            mapped.insert(parser_id);
        }
    }

    let orphans: BTreeSet<&str> = config
        .statements
        .accounts
        .keys()
        .map(|key| key.split(':').next().unwrap_or(key))
        .filter(|parser| !mapped.contains(*parser))
        .collect();
    if !orphans.is_empty() {
        bail!(
            "[statements.accounts] maps parser(s) with no matching Gmail label:\n  {}\n\
             add the gmailctl rule or remove the entry",
            orphans.into_iter().collect::<Vec<_>>().join("\n  ")
        );
    }

    if labels.is_empty() {
        bail!(
            "no routed statement label is mapped in [statements.accounts]\n  \
             run `abmx setup` to choose one"
        );
    }

    let state = State::open(&config.state_path(&base))?;
    state.check_schema()?;
    let seen = state.seen_keys()?;

    let mut request = FetchRequest::new(&labels);
    if let Some(limit) = limit {
        request = request.with_limit(limit);
    }
    let batch = gmail.fetch(request.with_skip(&seen))?;

    let mut plan = Plan::new();
    plan.stats.fetched = batch.messages.len();

    // document key -> row ids, so a document is only recorded as delivered
    // when every row it produced was written.
    let mut documents: Vec<(String, Vec<String>)> = Vec::new();

    for message in &batch.messages {
        let Some(parser_id) = config.statements.parser_id(&message.label) else {
            plan.stats.failed += 1;
            plan.stats.errors.push(format!(
                "{}: label `{}` has no parser id after `{}`",
                message.key, message.label, config.statements.prefix
            ));
            continue;
        };
        let Some(parser) = registry.find(&parser_id) else {
            plan.stats.failed += 1;
            plan.stats.errors.push(format!(
                "{}: label `{}` names parser `{parser_id}`, which is not in the statement corpus",
                message.key, message.label
            ));
            continue;
        };

        let password = config.statement_password(&base, &parser_id)?;
        let attachments = abmx_mail::attachments(&message.bytes)
            .map_err(|error| anyhow::anyhow!("{}: {error}", message.key))?;
        let pdfs: Vec<&abmx_mail::Attachment> = attachments
            .iter()
            .filter(|attachment| attachment.is_pdf())
            .collect();

        if pdfs.is_empty() {
            plan.stats.failed += 1;
            plan.stats.errors.push(format!(
                "{}: the message has no PDF attachment",
                message.key
            ));
            continue;
        }

        for (index, attachment) in pdfs.iter().enumerate() {
            let key = format!("{}#att:{index}", message.key);
            if seen.contains(&key) {
                plan.stats.skipped += 1;
                continue;
            }

            let document = match abmx_doc::load(&attachment.bytes, password.as_deref()) {
                Ok(document) => document,
                Err(error) => {
                    plan.stats.failed += 1;
                    plan.stats.errors.push(format!("{key}: {error}"));
                    continue;
                }
            };

            match parser.parse(&document, Some(key.clone())) {
                Ok(transactions) => {
                    plan.stats.parsed += transactions.len();
                    let ids: Vec<String> = transactions.iter().map(|tx| tx.id.clone()).collect();
                    let unmapped = plan_rows(&config, &mut plan, &key, &parser_id, transactions);
                    if unmapped == 0 {
                        documents.push((key, ids));
                    }
                }
                Err(error) => {
                    plan.stats.failed += 1;
                    plan.stats.errors.push(format!("{key}: {error}"));
                }
            }
        }
    }

    // Read the ledger once, over every affected account and date.
    let sink = config.build_statement_sink(&base)?;
    let accounts: BTreeSet<String> = plan.rows.iter().map(|row| row.account.clone()).collect();
    let accounts: Vec<String> = accounts.into_iter().collect();

    let mut present = 0usize;
    let mut ambiguous = 0usize;

    if !plan.rows.is_empty() {
        let since = plan
            .rows
            .iter()
            .map(|row| row.transaction.occurred_on)
            .min()
            .expect("non-empty rows")
            - chrono::Duration::days(MATCH_WINDOW_DAYS);
        let until = plan
            .rows
            .iter()
            .map(|row| row.transaction.occurred_on)
            .max()
            .expect("non-empty rows")
            + chrono::Duration::days(MATCH_WINDOW_DAYS);

        let ledger = sink.snapshot(&accounts, since, until)?;
        let mut kept = Vec::new();
        for row in plan.rows.drain(..) {
            let import = ImportRow {
                tx_id: row.id.clone(),
                account: row.account.clone(),
                transaction: row.transaction.clone(),
            };
            match classify_window(&import, &ledger, MATCH_WINDOW_DAYS) {
                WindowMatch::SameId | WindowMatch::Present => present += 1,
                WindowMatch::New => kept.push(row),
                WindowMatch::Ambiguous(_) => {
                    ambiguous += 1;
                    kept.push(row);
                }
            }
        }
        plan.rows = kept;
    }

    plan.stats.import = plan.rows.len();
    plan.stats.broken = 0;
    plan.stats.unmatched = 0;

    if !silent {
        if json {
            println!("{}", serde_json::to_string_pretty(&plan)?);
        } else {
            eprintln!(
                "fetched {}, parsed {} rows — new {}, already present {}, ambiguous {} (added), failed {}",
                plan.stats.fetched,
                plan.stats.parsed,
                plan.stats.import,
                present,
                ambiguous,
                plan.stats.failed,
            );
            for error in &plan.stats.errors {
                eprintln!("error: {error}");
            }
            if !plan.rows.is_empty() {
                eprintln!();
                eprintln!(
                    "{:<12} {:>18} {:<28} description",
                    "date", "amount", "account"
                );
                for row in &plan.rows {
                    let transaction = &row.transaction;
                    eprintln!(
                        "{:<12} {:>18} {:<28} {}",
                        transaction.occurred_on,
                        transaction.amount.decimal(),
                        row.account_key,
                        transaction.counterparty.as_deref().unwrap_or("-"),
                    );
                }
            }
        }
    } else {
        for error in &plan.stats.errors {
            eprintln!("error: {error}");
        }
    }

    if plan.stats.failed > 0 {
        eprintln!(
            "note: {} document(s) failed and were not imported; nothing else was blocked",
            plan.stats.failed
        );
    }

    let options = ApplyOptions {
        dry_run,
        chunk_size: config.actual_chunk_size(),
    };
    let rows = plan.import_rows();
    let report = if rows.is_empty() {
        abmx_core::Report {
            dry_run,
            ..abmx_core::Report::default()
        }
    } else {
        sink.apply(&rows, &options)?
    };

    if silent {
        for error in &report.errors {
            eprintln!(
                "error: {}: {}",
                error.tx_id.as_deref().unwrap_or("-"),
                error.message
            );
        }
    } else if !json {
        if report.dry_run {
            println!(
                "dry run: {} row(s) would be added, {} already present",
                report.added, present
            );
        } else {
            println!("wrote {} row(s); {} already present", report.added, present);
        }
        if !report.errors.is_empty() {
            eprintln!("{} row(s) failed", report.errors.len());
            for error in &report.errors {
                eprintln!(
                    "  {}: {}",
                    error.tx_id.as_deref().unwrap_or("-"),
                    error.message
                );
            }
        }
    }

    if !dry_run {
        let failed: HashSet<&str> = report
            .errors
            .iter()
            .filter_map(|error| error.tx_id.as_deref())
            .collect();
        for (key, ids) in documents {
            if ids.iter().any(|id| failed.contains(id.as_str())) {
                continue;
            }
            let tx_id = ids.first().map(String::as_str).unwrap_or("");
            state.mark_seen(&key, sink.name(), tx_id, "imported_statement")?;
        }
    }

    let code = if plan.stats.failed > 0 || !report.errors.is_empty() {
        EXIT_FAILED
    } else {
        0
    };
    Ok(Some((plan, code)))
}

/// Turn parsed transactions into planned rows, resolving each section's account.
fn plan_rows(
    config: &Config,
    plan: &mut Plan,
    document: &str,
    parser_id: &str,
    transactions: Vec<abmx_core::Transaction>,
) -> usize {
    let mut unmapped = 0usize;
    for transaction in transactions {
        let account = config
            .statements
            .accounts
            .get(&format!("{}:{}", transaction.parser, transaction.account))
            .or_else(|| config.statements.accounts.get(&transaction.parser))
            .cloned();

        let Some(account) = account else {
            unmapped += 1;
            plan.stats.failed += 1;
            plan.stats.errors.push(format!(
                "{document}: `{}:{}` has no [statements.accounts] entry",
                transaction.parser, transaction.account
            ));
            continue;
        };

        plan.rows.push(PlannedRow {
            id: transaction.id.clone(),
            message: MessageKey::new(document),
            account_key: parser_id.to_owned(),
            account,
            transaction,
        });
    }
    unmapped
}

fn is_mapped(config: &Config, parser_id: &str) -> bool {
    config.statements.accounts.contains_key(parser_id)
        || config.statements.accounts.keys().any(|key| {
            key.split_once(':')
                .is_some_and(|(parser, _)| parser == parser_id)
        })
}

fn load_optional(config: Option<&Path>) -> Option<(Config, PathBuf)> {
    let path = config
        .map(Path::to_path_buf)
        .or_else(|| paths::config_path(None).ok())?;
    let config = Config::load(&path).ok()?;
    let base = Config::base_dir(&path);
    Some((config, base))
}

fn parser_dirs(explicit: &[PathBuf], config: Option<&(Config, PathBuf)>) -> Vec<PathBuf> {
    if !explicit.is_empty() {
        return explicit.to_vec();
    }
    if let Some((config, base)) = config {
        let dirs = config.parser_options(base).parsers;
        if !dirs.is_empty() {
            return dirs;
        }
    }
    abmx_parser::discover(&abmx_parser::Options::default()).dirs
}

fn password_from(password_file: Option<&Path>) -> Result<Option<String>> {
    if let Some(path) = password_file {
        return read_password_file(path).map(Some);
    }
    Ok(std::env::var("ABMX_STATEMENT_PASSWORD").ok())
}

fn read_password_file(path: &Path) -> Result<String> {
    let mut value = std::fs::read_to_string(path)
        .with_context(|| format!("reading the password from {}", path.display()))?;
    if value.ends_with('\n') {
        value.pop();
        if value.ends_with('\r') {
            value.pop();
        }
    }
    Ok(value)
}

fn pdf_files(dir: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    collect_pdfs(dir, &mut files);
    files.sort();
    files
}

fn collect_pdfs(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_pdfs(&path, out);
        } else if path.extension().and_then(|ext| ext.to_str()) == Some("pdf") {
            out.push(path);
        }
    }
}
