use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};
use txsync::config::{Config, INIT_TEMPLATE};
use txsync::fetchers::DirFetcher;
use txsync::paths;
use txsync::state::State;
use txsync::Runner;
use txsync_core::{
    classify, ApplyOptions, FetchRequest, Fetcher, ImportRow, LedgerMatch, Plan, Report,
};
use txsync_gmail::{
    authorize, authorize_manual, revoke_refresh_token, OAuthConfig, TokenStore, REVOKE_URL,
};

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

    /// Print parser and message ids next to each row.
    #[arg(long, short, global = true)]
    verbose: bool,

    /// Only print the summary, not each row.
    #[arg(long, short, global = true, conflicts_with = "verbose")]
    quiet: bool,

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

    /// Authorize or deauthorize a mail provider.
    Auth {
        #[command(subcommand)]
        command: AuthCommand,
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

    /// Compare the current plan against what the ledger already holds.
    Diff {
        /// Read `.eml` files instead of using the configured fetcher.
        #[arg(long, value_name = "DIR")]
        from_dir: Option<PathBuf>,
        /// Restrict the ledger lookup to this date (YYYY-MM-DD). The plan is unchanged.
        #[arg(long, value_name = "DATE")]
        since: Option<String>,
        /// Restrict the ledger lookup up to this date (YYYY-MM-DD).
        #[arg(long, value_name = "DATE")]
        until: Option<String>,
        /// Machine-readable output.
        #[arg(long)]
        json: bool,
        /// Maximum messages to fetch in this run.
        #[arg(long, value_name = "N")]
        limit: Option<usize>,
    },

    /// Forget the local state so the next sync re-fetches everything.
    ///
    /// No ledger is modified. Rows deleted in a ledger come back on the next
    /// sync, because the ledger is a projection of the mailbox.
    Reset {
        /// Also clear the review queue.
        #[arg(long)]
        review: bool,
        /// Delete the state database entirely, including the schema marker.
        /// Implies --review.
        #[arg(long)]
        purge: bool,
        /// Skip the confirmation prompt.
        #[arg(long)]
        yes: bool,
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
enum AuthCommand {
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

    /// Forget the stored Gmail credentials, revoking them at Google.
    ///
    /// Deletes the refresh token file. By default the token is also revoked at
    /// Google, so it cannot be used from a copy of the file.
    Logout {
        /// Keep the refresh token valid at Google; delete it locally only.
        #[arg(long)]
        no_revoke: bool,
    },
}

fn main() {
    let cli = Cli::parse();
    match run(cli) {
        Ok(code) => std::process::exit(code),
        Err(error) => {
            eprintln!("error: {error:#}");
            std::process::exit(EXIT_TOOL);
        }
    }
}

/// The global flags, separated from the subcommand so the subcommand can be
/// moved without partially moving the whole CLI struct.
struct Settings {
    config: Option<PathBuf>,
    verbose: bool,
    quiet: bool,
}

fn run(cli: Cli) -> Result<i32> {
    let settings = Settings {
        config: cli.config,
        verbose: cli.verbose,
        quiet: cli.quiet,
    };

    match cli.command {
        Command::Init { path, force } => init(path, force),
        Command::Auth { command } => match command {
            AuthCommand::Gmail {
                no_browser,
                port,
                manual,
            } => auth_gmail(&settings, no_browser, port, manual),
            AuthCommand::Logout { no_revoke } => auth_logout(&settings, no_revoke),
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
        Command::Diff {
            from_dir,
            since,
            until,
            json,
            limit,
        } => diff(&settings, from_dir, since, until, json, limit),
        Command::Reset { review, purge, yes } => reset(&settings, review, purge, yes),
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

fn auth_logout(settings: &Settings, no_revoke: bool) -> Result<i32> {
    let (config, base, _) = load_config(settings)?;
    let store = TokenStore::new(config.gmail_token_path(&base));

    let Some(credentials) = store.load()? else {
        println!("no stored Gmail credentials at {}", store.path().display());
        return Ok(EXIT_OK);
    };

    if !no_revoke {
        match revoke_refresh_token(REVOKE_URL, &credentials.refresh_token) {
            Ok(()) => println!("revoked the refresh token with Google"),
            Err(error) => {
                eprintln!("warning: could not revoke the token ({error})");
                eprintln!("revoke it manually at https://myaccount.google.com/permissions");
            }
        }
    }

    store.clear()?;
    println!("removed {}", store.path().display());

    Ok(EXIT_OK)
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Status {
    Ok,
    Warn,
    Fail,
    Skip,
}

impl Status {
    fn label(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::Warn => "warn",
            Self::Fail => "fail",
            Self::Skip => "skip",
        }
    }
}

struct Line {
    status: Status,
    message: String,
}

struct Section {
    name: &'static str,
    lines: Vec<Line>,
}

impl Section {
    fn new(name: &'static str) -> Self {
        Self {
            name,
            lines: Vec::new(),
        }
    }

    fn push(&mut self, status: Status, message: impl Into<String>) {
        self.lines.push(Line {
            status,
            message: message.into(),
        });
    }
}

fn render_sections(sections: &[Section]) -> (usize, usize, usize) {
    let mut failures = 0usize;
    let mut warnings = 0usize;
    let mut skipped = 0usize;

    for (index, section) in sections.iter().enumerate() {
        if index > 0 {
            println!();
        }
        println!("{}", section.name);
        for line in &section.lines {
            println!("  {:<5} {}", line.status.label(), line.message);
            match line.status {
                Status::Fail => failures += 1,
                Status::Warn => warnings += 1,
                Status::Skip => skipped += 1,
                Status::Ok => {}
            }
        }
    }

    (failures, warnings, skipped)
}

fn doctor(settings: &Settings, online: bool) -> Result<i32> {
    let (config, base, config_path) = load_config(settings)?;

    let mut config_section = Section::new("config");
    config_section.push(Status::Ok, config_path.display().to_string());

    let mut parsers_section = Section::new("parsers");
    let parser = match txp::Parser::load(&config.txp_options(&base)) {
        Ok((parser, discovery)) => {
            let summaries = parser.registry().summaries();
            let accounts: BTreeSet<&str> = summaries
                .iter()
                .map(|summary| summary.account.as_str())
                .collect();
            let dir = discovery
                .dirs
                .first()
                .map(|dir| dir.display().to_string())
                .unwrap_or_else(|| "(built-in)".to_owned());
            parsers_section.push(
                Status::Ok,
                format!(
                    "{} parsers, {} account keys, from {dir}",
                    summaries.len(),
                    accounts.len()
                ),
            );
            Some(parser)
        }
        Err(error) => {
            parsers_section.push(Status::Fail, error.to_string());
            None
        }
    };

    let mut state_section = Section::new("state");
    let state_path = config.state_path(&base);
    match State::open(&state_path) {
        Ok(state) => match state.check_schema() {
            Ok(()) => state_section.push(
                Status::Ok,
                format!(
                    "{} (txp schema {})",
                    state_path.display(),
                    txsync_core::TXP_SCHEMA_VERSION
                ),
            ),
            Err(error) => state_section.push(Status::Fail, error.to_string()),
        },
        Err(error) => state_section.push(Status::Fail, error.to_string()),
    }

    // The sink is resolved first so the account section can show names, not
    // just UUIDs, when --online is set.
    let mut sink_section = Section::new("sink");
    let mut sink_accounts: Option<Vec<txsync_core::SinkAccount>> = None;
    match config.build_sink(&base) {
        Err(error) => sink_section.push(Status::Fail, error.to_string()),
        Ok(sink) => {
            if !online {
                let location = config
                    .sink
                    .actual
                    .as_ref()
                    .map(|actual| actual.url.clone())
                    .or_else(|| {
                        config
                            .sink
                            .file
                            .as_ref()
                            .map(|file| file.path.display().to_string())
                    });
                let message = match location {
                    Some(location) => {
                        format!("{} at {location} (offline; use --online)", sink.name())
                    }
                    None => format!("{} (offline; use --online)", sink.name()),
                };
                sink_section.push(Status::Skip, message);
            } else {
                match sink.accounts() {
                    Ok(accounts) => {
                        sink_section.push(
                            Status::Ok,
                            format!("{}: {} accounts", sink.name(), accounts.len()),
                        );
                        sink_accounts = Some(accounts);
                    }
                    Err(error) => sink_section.push(Status::Fail, error.to_string()),
                }
            }
        }
    }

    let mut accounts_section = Section::new("accounts");
    if let Some(parser) = &parser {
        let declared: BTreeSet<String> = parser
            .registry()
            .summaries()
            .into_iter()
            .map(|summary| summary.account)
            .collect();
        let mapped = config.account_map();
        let width = declared.iter().map(String::len).max().unwrap_or(0);

        for account in &declared {
            let Some(id) = mapped.get(account) else {
                accounts_section.push(Status::Warn, format!("{account} is not in [accounts]"));
                continue;
            };

            let resolved = sink_accounts
                .as_ref()
                .and_then(|accounts| accounts.iter().find(|candidate| candidate.id == id));

            match (online, resolved, sink_accounts.is_some()) {
                (true, Some(sink_account), _) => accounts_section.push(
                    Status::Ok,
                    format!("{:<width$} -> {} ({id})", account, sink_account.name),
                ),
                (true, None, true) => accounts_section.push(
                    Status::Fail,
                    format!("{:<width$} -> {id} is not an account in the sink", account),
                ),
                _ => accounts_section.push(Status::Ok, format!("{:<width$} -> {id}", account)),
            }
        }
    }

    let mut fetchers_section = Section::new("fetchers");
    match &config.fetch.gmail {
        Some(gmail) => fetchers_section.push(
            Status::Ok,
            format!(
                "gmail (lookback {}d, max {})",
                gmail.lookback_days.unwrap_or(7),
                gmail.max_messages.unwrap_or(txsync_core::DEFAULT_LIMIT)
            ),
        ),
        None => fetchers_section.push(Status::Warn, "no [fetch.gmail] section"),
    }

    let mut auth_section = Section::new("auth");
    match &config.fetch.gmail {
        None => auth_section.push(Status::Skip, "no fetcher configured"),
        Some(_) => {
            let store = TokenStore::new(config.gmail_token_path(&base));
            match store.load() {
                Ok(Some(_)) if !online => auth_section.push(
                    Status::Ok,
                    format!("gmail token at {}", store.path().display()),
                ),
                Ok(Some(_)) => match config.build_gmail(&base, &[]) {
                    Ok(fetcher) => match fetcher.client().profile() {
                        Ok(profile) => auth_section.push(
                            Status::Ok,
                            format!(
                                "gmail: {} (history {})",
                                profile.email_address, profile.history_id
                            ),
                        ),
                        Err(error) => auth_section.push(Status::Fail, error.to_string()),
                    },
                    Err(error) => auth_section.push(Status::Fail, error.to_string()),
                },
                Ok(None) => {
                    auth_section.push(Status::Fail, "gmail: no token; run `txsync auth gmail`")
                }
                Err(error) => auth_section.push(Status::Fail, error.to_string()),
            }
        }
    }

    let sections = [
        config_section,
        parsers_section,
        state_section,
        accounts_section,
        sink_section,
        fetchers_section,
        auth_section,
    ];
    let (failures, warnings, skipped) = render_sections(&sections);

    println!();
    if failures > 0 {
        println!("{failures} failure(s), {warnings} warning(s)");
        return Ok(EXIT_TOOL);
    }
    if warnings > 0 {
        println!("0 failures, {warnings} warning(s)");
    } else if skipped > 0 {
        println!("all checks passed ({skipped} skipped)");
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
            eprintln!("wrote plan to {}", path.display());
        }
        None => println!("{json}"),
    }

    summarize_plan(&plan, settings.quiet, settings.verbose);
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
            summarize_plan(&plan, settings.quiet, settings.verbose);
            plan
        }
        None => {
            let fetcher = build_fetcher(&config, &base, from_dir, &runner.parser)?;
            let request =
                FetchRequest::new(None).with_limit(limit.unwrap_or(txsync_core::DEFAULT_LIMIT));
            let plan = runner.plan(fetcher.as_ref(), request)?;
            summarize_plan(&plan, settings.quiet, settings.verbose);
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
    summarize_plan(&plan, settings.quiet, settings.verbose);

    let sink = config.build_sink(&base)?;
    let options = apply_options(&config, dry_run);
    let report = runner.apply(sink.as_ref(), &plan, &options)?;

    summarize_report(&report);
    Ok(exit_for_report(&report, &plan))
}

fn diff(
    settings: &Settings,
    from_dir: Option<PathBuf>,
    since: Option<String>,
    until: Option<String>,
    json: bool,
    limit: Option<usize>,
) -> Result<i32> {
    let (config, base, _) = load_config(settings)?;
    let runner = build_runner(&config, &base)?;
    let fetcher = build_fetcher(&config, &base, from_dir, &runner.parser)?;

    let request = FetchRequest::new(None).with_limit(limit.unwrap_or(txsync_core::DEFAULT_LIMIT));
    let plan = runner.plan(fetcher.as_ref(), request)?;
    summarize_plan(&plan, true, false);

    let rows = plan.import_rows();
    if rows.is_empty() {
        eprintln!("nothing to import, so there is nothing to diff");
        return Ok(EXIT_OK);
    }

    let mut accounts: Vec<String> = rows.iter().map(|row| row.account.clone()).collect();
    accounts.sort();
    accounts.dedup();

    let dates: Vec<chrono::NaiveDate> =
        rows.iter().map(|row| row.transaction.occurred_on).collect();
    let earliest = *dates.iter().min().expect("at least one row");
    let latest = *dates.iter().max().expect("at least one row");

    let since = match since {
        Some(value) => parse_date(&value, "--since")?,
        None => earliest - chrono::Duration::days(1),
    };
    let until = match until {
        Some(value) => parse_date(&value, "--until")?,
        None => latest + chrono::Duration::days(1),
    };

    let sink = config.build_sink(&base)?;
    let ledger = sink.snapshot(&accounts, since, until)?;

    let mut entries: Vec<(&ImportRow, LedgerMatch)> = Vec::with_capacity(rows.len());
    let mut foreign = 0usize;
    let mut matched = 0usize;

    for row in &rows {
        let decision = classify(row, &ledger);
        if !decision.is_new() {
            matched += 1;
        }
        if decision.is_foreign() {
            foreign += 1;
        }
        entries.push((row, decision));
    }

    if json {
        let rows: Vec<serde_json::Value> = entries
            .iter()
            .map(|(row, decision)| {
                serde_json::json!({
                    "id": row.tx_id,
                    "date": row.transaction.occurred_on.to_string(),
                    "amount": row.transaction.amount_decimal,
                    "currency": row.transaction.amount.currency,
                    "account_key": row.transaction.account,
                    "account": row.account,
                    "counterparty": row.transaction.counterparty,
                    "ledger": decision,
                })
            })
            .collect();

        let report = serde_json::json!({
            "window": { "since": since.to_string(), "until": until.to_string() },
            "rows": rows,
            "summary": {
                "planned": entries.len(),
                "new": entries.len() - matched,
                "match": matched,
                "foreign": foreign,
            },
        });
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        println!(
            "{:<10} {:>14} {:<14} {:<30} ledger",
            "date", "amount", "account", "counterparty"
        );
        for (row, decision) in &entries {
            println!(
                "{:<10} {:>14} {:<14} {:<30} {}",
                row.transaction.occurred_on,
                format_amount(&row.transaction),
                row.transaction.account,
                truncate(row.transaction.counterparty.as_deref().unwrap_or("-"), 30),
                ledger_label(decision, &row.tx_id),
            );
        }
    }

    eprintln!(
        "window {since}..{until}: {} planned, {} new, {} match existing, {} with a foreign imported_id",
        entries.len(),
        entries.len() - matched,
        matched,
        foreign,
    );

    if foreign > 0 {
        eprintln!(
            "warning: a foreign imported_id means the ledger already has this row from another \
             importer; expect a duplicate rather than a reconcile"
        );
        Ok(EXIT_REVIEW)
    } else {
        Ok(EXIT_OK)
    }
}

fn parse_date(value: &str, flag: &str) -> Result<chrono::NaiveDate> {
    chrono::NaiveDate::parse_from_str(value, "%Y-%m-%d")
        .with_context(|| format!("{flag}: expected YYYY-MM-DD, got `{value}`"))
}

fn ledger_label(decision: &LedgerMatch, tx_id: &str) -> String {
    match decision {
        LedgerMatch::New => "new".to_owned(),
        LedgerMatch::Existing {
            imported_id: None, ..
        } => "match (manual entry)".to_owned(),
        LedgerMatch::Existing {
            imported_id: Some(id),
            ..
        } if id == tx_id => "already imported".to_owned(),
        LedgerMatch::Existing {
            imported_id: Some(id),
            ..
        } => format!("match (foreign id {})", truncate(id, 32)),
    }
}

fn truncate(value: &str, limit: usize) -> String {
    if value.chars().count() <= limit {
        return value.to_owned();
    }
    let shortened: String = value.chars().take(limit.saturating_sub(1)).collect();
    format!("{shortened}…")
}

fn reset(settings: &Settings, review: bool, purge: bool, yes: bool) -> Result<i32> {
    let (config, base, _) = load_config(settings)?;
    let path = config.state_path(&base);

    if purge {
        if !path.exists() {
            println!("no local state at {}", path.display());
            return Ok(EXIT_OK);
        }
        let question = format!(
            "This removes the local state database only. No ledger is modified.\n\
             delete {} entirely?",
            path.display()
        );
        if !yes && !confirm(&question)? {
            println!("aborted");
            return Ok(EXIT_OK);
        }
        for suffix in ["", "-wal", "-shm"] {
            let candidate = PathBuf::from(format!("{}{suffix}", path.display()));
            match std::fs::remove_file(&candidate) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => {
                    return Err(error).with_context(|| format!("removing {}", candidate.display()))
                }
            }
        }
        println!("removed {}", path.display());
        println!("No ledger was modified.");
        return Ok(EXIT_OK);
    }

    let state = State::open(&path)?;
    let (cursors, messages, reviews) = state.counts()?;

    if cursors == 0 && messages == 0 && (!review || reviews == 0) {
        println!("local state is already empty");
        return Ok(EXIT_OK);
    }

    let mut what = format!("{cursors} cursor(s) and {messages} delivered message(s)");
    if review {
        what.push_str(&format!(" and {reviews} review row(s)"));
    }

    let question =
        format!("This clears txsync's local state only. No ledger is modified.\nclear {what}?");
    if !yes && !confirm(&question)? {
        println!("aborted");
        return Ok(EXIT_OK);
    }

    state.reset_progress()?;
    if review {
        state.clear_reviews()?;
    }

    println!("cleared {what}");
    println!(
        "No ledger was modified. The next `txsync sync` re-fetches and re-applies; \
         rows deleted in a ledger are re-imported."
    );

    Ok(EXIT_OK)
}

fn confirm(question: &str) -> Result<bool> {
    use std::io::{IsTerminal, Write};

    if !std::io::stdin().is_terminal() {
        bail!("refusing to reset without --yes (no terminal to confirm)");
    }

    eprint!("{question} [y/N] ");
    std::io::stderr().flush().ok();

    let mut line = String::new();
    std::io::stdin().read_line(&mut line)?;
    let answer = line.trim().to_ascii_lowercase();

    Ok(answer == "y" || answer == "yes")
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

const UNMATCHED_PREVIEW: usize = 10;

fn summarize_plan(plan: &Plan, quiet: bool, verbose: bool) {
    eprintln!(
        "fetched {}, parsed {} — import {}, review {}, skip {}, unmatched {}, failed {}",
        plan.stats.fetched,
        plan.stats.parsed,
        plan.stats.import,
        plan.stats.review,
        plan.stats.skip,
        plan.stats.unmatched,
        plan.stats.failed,
    );

    for error in &plan.stats.errors {
        eprintln!("error: {error}");
    }

    if !quiet && !plan.rows.is_empty() {
        eprintln!();
        eprintln!(
            "{:<10} {:>16} {:<14} {:<8} counterparty",
            "date", "amount", "account", "decision"
        );
        for row in &plan.rows {
            let transaction = &row.transaction;
            let mut line = format!(
                "{:<10} {:>16} {:<14} {:<8} {}",
                transaction.occurred_on,
                format_amount(transaction),
                row.account_key,
                decision_label(&row.decision),
                truncate(transaction.counterparty.as_deref().unwrap_or("-"), 40),
            );
            if verbose {
                line.push_str(&format!("  · {} · {}", transaction.parser, row.message));
            }
            eprintln!("{line}");
        }
    }

    if !quiet && !plan.unmatched.is_empty() {
        eprintln!();
        eprintln!(
            "unmatched ({}), no parser claimed them:",
            plan.unmatched.len()
        );
        for (index, entry) in plan.unmatched.iter().enumerate() {
            if index == UNMATCHED_PREVIEW {
                eprintln!(
                    "  … and {} more (see `txsync plan --out`)",
                    plan.unmatched.len() - UNMATCHED_PREVIEW
                );
                break;
            }

            let date = entry
                .date
                .map(|date| date.to_string())
                .unwrap_or_else(|| "----------".to_owned());
            let subject = entry
                .subject
                .as_deref()
                .map(|subject| format!("\"{}\"", truncate(subject, 50)))
                .unwrap_or_else(|| "(no subject)".to_owned());
            let from = entry.from.as_deref().unwrap_or("-");

            eprintln!("  {date}  {subject:<54} {from}");
            if verbose {
                eprintln!(
                    "             {} · {}",
                    entry.message,
                    entry.code.as_deref().unwrap_or(&entry.reason)
                );
            }
        }
    }

    if plan.stats.review > 0 {
        if !quiet {
            eprintln!();
        }
        eprintln!(
            "{} row(s) waiting for review; run `txsync review`",
            plan.stats.review
        );
    }

    if !quiet && (!plan.rows.is_empty() || !plan.unmatched.is_empty()) {
        eprintln!();
    }
}

fn decision_label(decision: &txsync_core::Decision) -> &'static str {
    match decision {
        txsync_core::Decision::Import => "import",
        txsync_core::Decision::Review(_) => "review",
        txsync_core::Decision::Skip(_) => "skip",
    }
}

/// Group the integer part for readability. Display only: `amount_decimal`
/// stays exact and `amount_minor` stays authoritative.
fn format_amount(transaction: &txsync_core::Transaction) -> String {
    let raw = transaction.amount_decimal.as_str();
    let (sign, rest) = match raw.strip_prefix('-') {
        Some(rest) => ("-", rest),
        None => ("", raw),
    };
    let (integer, fraction) = match rest.split_once('.') {
        Some((integer, fraction)) => (integer, Some(fraction)),
        None => (rest, None),
    };

    let mut grouped = String::with_capacity(integer.len() + integer.len() / 3);
    for (index, digit) in integer.chars().enumerate() {
        if index > 0 && (integer.len() - index) % 3 == 0 {
            grouped.push(',');
        }
        grouped.push(digit);
    }

    match fraction {
        Some(fraction) => format!("{sign}{grouped}.{fraction} {}", transaction.amount.currency),
        None => format!("{sign}{grouped} {}", transaction.amount.currency),
    }
}

fn summarize_report(report: &Report) {
    let prefix = if report.dry_run { "dry run" } else { "applied" };
    eprintln!(
        "{prefix}: added {}, updated {}, skipped {}, errors {}",
        report.added,
        report.updated,
        report.skipped.len(),
        report.errors.len(),
    );

    for skipped in &report.skipped {
        eprintln!(
            "skipped: {}: {}",
            skipped.tx_id.as_deref().unwrap_or("(batch)"),
            skipped.message
        );
    }
    for error in &report.errors {
        eprintln!(
            "error: {}: {}",
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
