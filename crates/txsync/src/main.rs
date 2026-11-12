use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};
use txsync::config::{Config, INIT_TEMPLATE};
use txsync::fetchers::DirFetcher;
use txsync::paths;
use txsync::state::State;
use txsync::Runner;
use txsync_core::{ApplyOptions, FetchRequest, Fetcher, Plan, Report};
use txsync_gmail::{authorize, authorize_manual, OAuthConfig, TokenStore};

/// Clean run.
const EXIT_OK: i32 = 0;
/// The tool itself failed (config, network, state).
const EXIT_TOOL: i32 = 1;
/// At least one message or row failed.
const EXIT_FAILED: i32 = 2;
/// Nothing failed, but rows are waiting for a human.
const EXIT_REVIEW: i32 = 3;

#[derive(Parser)]
#[command(
    name = "txsync",
    version,
    about = "Deliver parsed bank transactions to a budget ledger",
    long_about = "txsync fetches bank mail, parses it with txp, decides what to import, \
                  and writes it to a ledger. It never parses a bank template itself and \
                  never guesses an account mapping."
)]
struct Cli {
    /// Config file. Defaults to $TXSYNC_CONFIG, then the XDG config dir, then
    /// ./txsync.toml.
    #[arg(long, global = true, value_name = "FILE")]
    config: Option<PathBuf>,

    /// Print each planned row to stderr.
    #[arg(long, short, global = true)]
    verbose: bool,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Write a starter config file.
    Init {
        #[arg(long, value_name = "FILE")]
        path: Option<PathBuf>,
        #[arg(long)]
        force: bool,
    },

    /// Authorize a mail provider.
    Auth {
        #[command(subcommand)]
        provider: AuthProvider,
    },

    /// Check config, parser corpus, state, credentials, and the sink.
    Doctor {
        /// Make network calls (Gmail profile, sink accounts).
        #[arg(long)]
        online: bool,
    },

    /// Fetch, parse, and decide — then print the plan as JSON on stdout.
    Plan {
        /// Read `.eml` files instead of using the configured fetcher.
        #[arg(long, value_name = "DIR")]
        from_dir: Option<PathBuf>,
        /// Maximum messages to fetch in this run.
        #[arg(long, value_name = "N")]
        limit: Option<usize>,
        /// Write the plan to a file instead of stdout.
        #[arg(long, value_name = "FILE")]
        out: Option<PathBuf>,
    },

    /// Apply a plan (the current one, or a saved one).
    Apply {
        /// Apply a saved plan instead of building a new one.
        #[arg(long, value_name = "FILE")]
        plan: Option<PathBuf>,
        /// Ask the sink what it would do; write nothing.
        #[arg(long)]
        dry_run: bool,
        /// Read `.eml` files instead of using the configured fetcher.
        #[arg(long, value_name = "DIR")]
        from_dir: Option<PathBuf>,
        /// Maximum messages to fetch in this run.
        #[arg(long, value_name = "N")]
        limit: Option<usize>,
    },

    /// Plan and apply in one go. This is the timer entrypoint.
    Sync {
        /// Read `.eml` files instead of using the configured fetcher.
        #[arg(long, value_name = "DIR")]
        from_dir: Option<PathBuf>,
        /// Ask the sink what it would do; write nothing.
        #[arg(long)]
        dry_run: bool,
        /// Maximum messages to fetch in this run.
        #[arg(long, value_name = "N")]
        limit: Option<usize>,
    },

    /// Show rows waiting for a human.
    Review {
        /// Machine-readable output.
        #[arg(long)]
        json: bool,
    },

    /// Import specific rows from the review queue.
    Approve {
        /// Transaction ids from `txsync review`.
        ids: Vec<String>,
        /// Ask the sink what it would do; write nothing.
        #[arg(long)]
        dry_run: bool,
    },

    /// List the sink's accounts and how `[accounts]` maps onto them.
    Accounts {
        /// Machine-readable output.
        #[arg(long)]
        json: bool,
    },

    /// Show the loaded txp parsers and the senders they claim.
    Parsers,
}

#[derive(Subcommand)]
enum AuthProvider {
    /// OAuth2 with PKCE: a loopback listener, or `--manual` on a headless box.
    Gmail {
        /// Print the URL instead of trying to open a browser.
        #[arg(long)]
        no_browser: bool,
        /// Loopback port for the OAuth redirect; 0 picks a free one.
        #[arg(long, value_name = "PORT")]
        port: Option<u16>,
        /// Approve in any browser and paste the callback URL back.
        ///
        /// Use this when the CLI and the browser are on different machines:
        /// the redirect's 127.0.0.1 belongs to the browser's machine, so a
        /// local listener can never receive it.
        #[arg(long)]
        manual: bool,
    },
}

fn main() {
    let cli = Cli::parse();
    match run(cli) {
        Ok(code) => std::process::exit(code),
        Err(error) => {
            eprintln!("txsync: {error:#}");
            std::process::exit(EXIT_TOOL);
        }
    }
}

/// The global flags, separated from the subcommand so the subcommand can be
/// moved without partially moving the whole CLI struct.
struct Settings {
    config: Option<PathBuf>,
    verbose: bool,
}

fn run(cli: Cli) -> Result<i32> {
    let settings = Settings {
        config: cli.config,
        verbose: cli.verbose,
    };

    match cli.command {
        Command::Init { path, force } => init(path, force),
        Command::Auth { provider } => match provider {
            AuthProvider::Gmail {
                no_browser,
                port,
                manual,
            } => auth_gmail(&settings, no_browser, port, manual),
        },
        Command::Doctor { online } => doctor(&settings, online),
        Command::Plan {
            from_dir,
            limit,
            out,
        } => plan(&settings, from_dir, limit, out),
        Command::Apply {
            plan,
            dry_run,
            from_dir,
            limit,
        } => apply(&settings, plan, dry_run, from_dir, limit),
        Command::Sync {
            from_dir,
            dry_run,
            limit,
        } => sync(&settings, from_dir, dry_run, limit),
        Command::Review { json } => review(&settings, json),
        Command::Approve { ids, dry_run } => approve(&settings, &ids, dry_run),
        Command::Accounts { json } => accounts(&settings, json),
        Command::Parsers => parsers(&settings),
    }
}

// ---------------------------------------------------------------- plumbing

fn load_config(settings: &Settings) -> Result<(Config, PathBuf, PathBuf)> {
    let path = paths::config_path(settings.config.as_deref())?;
    let config = Config::load(&path)?;
    let base = Config::base_dir(&path);
    Ok((config, base, path))
}

fn build_runner(config: &Config, base: &Path) -> Result<Runner> {
    let options = config.txp_options(base);
    let (parser, _discovery) = txp::Parser::load(&options)
        .map_err(|error| anyhow::anyhow!("loading the txp parser corpus: {error}"))
        .context("hint: run `txp paths` to see where txsync looked for parsers")?;

    let state = State::open(&config.state_path(base))?;
    state.check_schema()?;

    Ok(Runner::new(
        parser,
        config.policy.clone(),
        config.account_map(),
        state,
    ))
}

fn senders(parser: &txp::Parser) -> Vec<String> {
    let mut set = BTreeSet::new();
    for summary in parser.registry().summaries() {
        set.extend(summary.senders);
    }
    set.into_iter().collect()
}

fn build_fetcher(
    config: &Config,
    base: &Path,
    from_dir: Option<PathBuf>,
    parser: &txp::Parser,
) -> Result<Box<dyn Fetcher>> {
    if let Some(dir) = from_dir {
        return Ok(Box::new(DirFetcher::new(dir)));
    }

    let senders = senders(parser);
    Ok(Box::new(config.build_gmail(base, &senders)?))
}

fn apply_options(config: &Config, dry_run: bool) -> ApplyOptions {
    ApplyOptions {
        dry_run,
        chunk_size: config.actual_chunk_size(),
    }
}

// ---------------------------------------------------------------- commands

fn init(path: Option<PathBuf>, force: bool) -> Result<i32> {
    let path = path.unwrap_or_else(|| paths::xdg_config_dir().join("txsync/config.toml"));

    if path.exists() && !force {
        bail!(
            "{} already exists; pass --force to overwrite",
            path.display()
        );
    }

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
    }

    std::fs::write(&path, INIT_TEMPLATE).with_context(|| format!("writing {}", path.display()))?;

    println!("wrote {}", path.display());
    println!();
    println!("next:");
    println!(
        "  1. edit {} and set [fetch.gmail] + [sink.actual]",
        path.display()
    );
    println!("  2. txsync auth gmail            (headless: --manual)");
    println!("  3. txsync doctor --online");
    println!("  4. txsync sync --dry-run");

    Ok(EXIT_OK)
}

fn auth_gmail(
    settings: &Settings,
    no_browser: bool,
    port: Option<u16>,
    manual: bool,
) -> Result<i32> {
    let (config, base, _) = load_config(settings)?;

    let Some(gmail) = &config.fetch.gmail else {
        bail!("no [fetch.gmail] section in the config");
    };

    let client_secret = config.gmail_client_secret(&base)?;
    let mut oauth = OAuthConfig::new(&gmail.client_id);
    oauth.client_secret = client_secret;
    oauth.port = port.unwrap_or_else(|| config.gmail_oauth_port());
    oauth.open_browser = !no_browser && !manual;

    let credentials = if manual {
        authorize_manual(&oauth)?
    } else {
        authorize(&oauth)?
    };
    let store = TokenStore::new(config.gmail_token_path(&base));
    store.save(&credentials)?;

    println!(
        "authorized; refresh token stored at {}",
        store.path().display()
    );
    Ok(EXIT_OK)
}

fn doctor(settings: &Settings, online: bool) -> Result<i32> {
    let mut failures = 0usize;
    let mut warnings = 0usize;

    let (config, base, config_path) = load_config(settings)?;
    println!("ok    config  {}", config_path.display());

    // Parser corpus.
    let parser = match txp::Parser::load(&config.txp_options(&base)) {
        Ok((parser, discovery)) => {
            println!(
                "ok    corpus  {} parsers from {}",
                parser.registry().summaries().len(),
                discovery
                    .dirs
                    .first()
                    .map(|dir| dir.display().to_string())
                    .unwrap_or_else(|| "(built-in)".to_owned())
            );
            Some(parser)
        }
        Err(error) => {
            println!("fail  corpus  {error}");
            failures += 1;
            None
        }
    };

    // State.
    match State::open(&config.state_path(&base)) {
        Ok(state) => match state.check_schema() {
            Ok(()) => println!("ok    state   {}", config.state_path(&base).display()),
            Err(error) => {
                println!("fail  state   {error}");
                failures += 1;
            }
        },
        Err(error) => {
            println!("fail  state   {error}");
            failures += 1;
        }
    }

    // Account mapping, checked against what the corpus actually declares.
    if let Some(parser) = &parser {
        let declared: BTreeSet<String> = parser
            .registry()
            .summaries()
            .into_iter()
            .map(|summary| summary.account)
            .collect();
        let mapped = config.account_map();
        for account in &declared {
            if mapped.contains(account) {
                println!("ok    map     {account}");
            } else {
                println!("warn  map     {account} is not in [accounts]");
                warnings += 1;
            }
        }
    }

    // Sink.
    match config.build_sink(&base) {
        Ok(sink) => {
            if !online {
                println!(
                    "ok    sink    {} configured (skipped, use --online)",
                    sink.name()
                );
            } else {
                match sink.accounts() {
                    Ok(accounts) if accounts.is_empty() => {
                        println!("ok    sink    {} has no accounts to verify", sink.name());
                    }
                    Ok(accounts) => {
                        let ids: BTreeSet<&str> =
                            accounts.iter().map(|account| account.id.as_str()).collect();
                        println!("ok    sink    {} accounts: {}", sink.name(), accounts.len());
                        for (key, account_id) in config.account_map().iter() {
                            if !ids.contains(account_id) {
                                println!(
                                    "fail  map     {key} -> {account_id} is not an account in {}",
                                    sink.name()
                                );
                                failures += 1;
                            }
                        }
                    }
                    Err(error) => {
                        println!("fail  sink    {error}");
                        failures += 1;
                    }
                }
            }
        }
        Err(error) => {
            println!("fail  sink    {error}");
            failures += 1;
        }
    }

    // Gmail credentials.
    match &config.fetch.gmail {
        None => println!("warn  gmail   no [fetch.gmail] section"),
        Some(_) => {
            let store = TokenStore::new(config.gmail_token_path(&base));
            match store.load() {
                Ok(Some(_)) => {
                    if online {
                        match config.build_gmail(&base, &[]) {
                            Ok(fetcher) => match fetcher.client().profile() {
                                Ok(profile) => println!(
                                    "ok    gmail   {} (history {})",
                                    profile.email_address, profile.history_id
                                ),
                                Err(error) => {
                                    println!("fail  gmail   {error}");
                                    failures += 1;
                                }
                            },
                            Err(error) => {
                                println!("fail  gmail   {error}");
                                failures += 1;
                            }
                        }
                    } else {
                        println!("ok    gmail   token at {}", store.path().display());
                    }
                }
                Ok(None) => {
                    println!("fail  gmail   no token; run `txsync auth gmail`");
                    failures += 1;
                }
                Err(error) => {
                    println!("fail  gmail   {error}");
                    failures += 1;
                }
            }
        }
    }

    println!();
    if failures > 0 {
        println!("{failures} failure(s), {warnings} warning(s)");
        return Ok(EXIT_TOOL);
    }
    if warnings > 0 {
        println!("0 failures, {warnings} warning(s)");
    } else {
        println!("all checks passed");
    }

    Ok(EXIT_OK)
}

fn plan(
    settings: &Settings,
    from_dir: Option<PathBuf>,
    limit: Option<usize>,
    out: Option<PathBuf>,
) -> Result<i32> {
    let (config, base, _) = load_config(settings)?;
    let runner = build_runner(&config, &base)?;
    let fetcher = build_fetcher(&config, &base, from_dir, &runner.parser)?;

    let request = FetchRequest::new(None).with_limit(limit.unwrap_or(txsync_core::DEFAULT_LIMIT));
    let plan = runner.plan(fetcher.as_ref(), request)?;

    let json = serde_json::to_string_pretty(&plan)?;
    match out {
        Some(path) => {
            std::fs::write(&path, format!("{json}\n"))
                .with_context(|| format!("writing {}", path.display()))?;
            eprintln!("txsync: wrote plan to {}", path.display());
        }
        None => println!("{json}"),
    }

    summarize_plan(&plan, settings.verbose);
    Ok(exit_for_plan(&plan))
}

fn apply(
    settings: &Settings,
    plan_file: Option<PathBuf>,
    dry_run: bool,
    from_dir: Option<PathBuf>,
    limit: Option<usize>,
) -> Result<i32> {
    let (config, base, _) = load_config(settings)?;
    let runner = build_runner(&config, &base)?;

    let plan = match plan_file {
        Some(path) => {
            let text = std::fs::read_to_string(&path)
                .with_context(|| format!("reading {}", path.display()))?;
            let plan: Plan = serde_json::from_str(&text)
                .with_context(|| format!("parsing plan {}", path.display()))?;
            summarize_plan(&plan, settings.verbose);
            plan
        }
        None => {
            let fetcher = build_fetcher(&config, &base, from_dir, &runner.parser)?;
            let request =
                FetchRequest::new(None).with_limit(limit.unwrap_or(txsync_core::DEFAULT_LIMIT));
            let plan = runner.plan(fetcher.as_ref(), request)?;
            summarize_plan(&plan, settings.verbose);
            plan
        }
    };

    let sink = config.build_sink(&base)?;
    let options = apply_options(&config, dry_run);
    let report = runner.apply(sink.as_ref(), &plan, &options)?;

    summarize_report(&report);
    Ok(exit_for_report(&report, &plan))
}

fn sync(
    settings: &Settings,
    from_dir: Option<PathBuf>,
    dry_run: bool,
    limit: Option<usize>,
) -> Result<i32> {
    let (config, base, _) = load_config(settings)?;
    let runner = build_runner(&config, &base)?;
    let fetcher = build_fetcher(&config, &base, from_dir, &runner.parser)?;

    let request = FetchRequest::new(None).with_limit(limit.unwrap_or(txsync_core::DEFAULT_LIMIT));
    let plan = runner.plan(fetcher.as_ref(), request)?;
    summarize_plan(&plan, settings.verbose);

    let sink = config.build_sink(&base)?;
    let options = apply_options(&config, dry_run);
    let report = runner.apply(sink.as_ref(), &plan, &options)?;

    summarize_report(&report);
    Ok(exit_for_report(&report, &plan))
}

fn review(settings: &Settings, json: bool) -> Result<i32> {
    let (config, base, _) = load_config(settings)?;
    let state = State::open(&config.state_path(&base))?;
    state.check_schema()?;

    let entries = state.review()?;

    if json {
        println!("{}", serde_json::to_string_pretty(&entries)?);
    } else if entries.is_empty() {
        println!("no rows waiting for review");
    } else {
        println!(
            "{:<20} {:<20} {:>12} {:<10} counterparty",
            "id", "reason", "amount", "date"
        );
        for entry in &entries {
            let transaction = &entry.transaction;
            println!(
                "{:<20} {:<20} {:>12} {:<10} {}",
                entry.tx_id,
                entry.reason,
                format!(
                    "{} {}",
                    transaction.amount_decimal, transaction.amount.currency
                ),
                transaction.occurred_on,
                transaction.counterparty.as_deref().unwrap_or("-"),
            );
        }
        println!();
        println!("approve with: txsync approve {}", entries[0].tx_id);
    }

    if entries.is_empty() {
        Ok(EXIT_OK)
    } else {
        Ok(EXIT_REVIEW)
    }
}

fn approve(settings: &Settings, ids: &[String], dry_run: bool) -> Result<i32> {
    if ids.is_empty() {
        bail!("pass at least one review id; see `txsync review`");
    }

    let (config, base, _) = load_config(settings)?;
    let runner = build_runner(&config, &base)?;
    let sink = config.build_sink(&base)?;
    let options = apply_options(&config, dry_run);

    let report = runner.approve(sink.as_ref(), ids, &options)?;
    summarize_report(&report);

    if report.errors.is_empty() {
        Ok(EXIT_OK)
    } else {
        Ok(EXIT_FAILED)
    }
}

fn accounts(settings: &Settings, json: bool) -> Result<i32> {
    let (config, base, _) = load_config(settings)?;
    let sink = config.build_sink(&base)?;
    let accounts = sink.accounts()?;

    if json {
        println!("{}", serde_json::to_string_pretty(&accounts)?);
        return Ok(EXIT_OK);
    }

    if accounts.is_empty() {
        println!("{} exposes no accounts", sink.name());
        return Ok(EXIT_OK);
    }

    let mapped: BTreeSet<&str> = config.accounts.values().map(String::as_str).collect();

    println!("{:<38} {:<24} {:<8} flags", "id", "name", "mapped");
    for account in &accounts {
        let mut flags = Vec::new();
        if account.off_budget {
            flags.push("off-budget");
        }
        if account.closed {
            flags.push("closed");
        }
        println!(
            "{:<38} {:<24} {:<8} {}",
            account.id,
            account.name,
            if mapped.contains(account.id.as_str()) {
                "yes"
            } else {
                "no"
            },
            flags.join(",")
        );
    }

    println!();
    if config.accounts.is_empty() {
        println!("[accounts] is empty; map txp account keys to these ids.");
    } else {
        for (key, id) in &config.accounts {
            let exists = accounts.iter().any(|account| &account.id == id);
            println!("{key} -> {id} {}", if exists { "" } else { "(missing!)" });
        }
    }

    Ok(EXIT_OK)
}

fn parsers(settings: &Settings) -> Result<i32> {
    let (config, base, _) = load_config(settings)?;
    let (parser, _) = txp::Parser::load(&config.txp_options(&base))
        .map_err(|error| anyhow::anyhow!("loading the txp parser corpus: {error}"))?;

    println!("{:<24} {:<4} {:<14} bank", "parser", "ver", "account");
    for summary in parser.registry().summaries() {
        println!(
            "{:<24} {:<4} {:<14} {}",
            summary.id, summary.version, summary.account, summary.bank
        );
    }

    println!();
    println!("senders:");
    for sender in senders(&parser) {
        println!("  {sender}");
    }

    Ok(EXIT_OK)
}

// ---------------------------------------------------------------- output

fn summarize_plan(plan: &Plan, verbose: bool) {
    eprintln!(
        "txsync: fetched {}  parsed {}  import {}  review {}  skip {}  unmatched {}  failed {}",
        plan.stats.fetched,
        plan.stats.parsed,
        plan.stats.import,
        plan.stats.review,
        plan.stats.skip,
        plan.stats.unmatched,
        plan.stats.failed,
    );

    for warning in &plan.stats.warnings {
        eprintln!("txsync: unmatched: {warning}");
    }
    for error in &plan.stats.errors {
        eprintln!("txsync: failed: {error}");
    }

    if plan.stats.review > 0 {
        eprintln!(
            "txsync: {} row(s) waiting for review; run `txsync review`",
            plan.stats.review
        );
    }

    if verbose {
        for row in &plan.rows {
            let transaction = &row.transaction;
            eprintln!(
                "txsync:   {:<10} {:<12} {:>14} {:<4} {:<10} {}",
                decision_label(&row.decision),
                row.account_key,
                transaction.amount_decimal,
                transaction.amount.currency,
                transaction.occurred_on,
                transaction.counterparty.as_deref().unwrap_or("-"),
            );
        }
    }
}

fn decision_label(decision: &txsync_core::Decision) -> &'static str {
    match decision {
        txsync_core::Decision::Import => "import",
        txsync_core::Decision::Review(_) => "review",
        txsync_core::Decision::Skip(_) => "skip",
    }
}

fn summarize_report(report: &Report) {
    let prefix = if report.dry_run { "dry-run" } else { "applied" };
    eprintln!(
        "txsync: {prefix}: added {}  updated {}  skipped {}  errors {}",
        report.added,
        report.updated,
        report.skipped.len(),
        report.errors.len(),
    );

    for skipped in &report.skipped {
        eprintln!(
            "txsync: skipped: {}: {}",
            skipped.tx_id.as_deref().unwrap_or("(batch)"),
            skipped.message
        );
    }
    for error in &report.errors {
        eprintln!(
            "txsync: error: {}: {}",
            error.tx_id.as_deref().unwrap_or("(batch)"),
            error.message
        );
    }
}

fn exit_for_plan(plan: &Plan) -> i32 {
    if !plan.stats.errors.is_empty() {
        EXIT_FAILED
    } else if plan.stats.review > 0 {
        EXIT_REVIEW
    } else {
        EXIT_OK
    }
}

/// `apply` and `sync` exit `0` when the only outstanding work is a review row.
///
/// Review rows are an expected state, not a failure: a timer that reports
/// failure every 15 minutes because one heuristic email is waiting is a timer
/// nobody reads. `txsync review` returns `3` so scripts can still detect them.
fn exit_for_report(report: &Report, plan: &Plan) -> i32 {
    if !report.errors.is_empty() || !plan.stats.errors.is_empty() {
        EXIT_FAILED
    } else {
        EXIT_OK
    }
}
