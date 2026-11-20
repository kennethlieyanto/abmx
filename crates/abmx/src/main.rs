use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use abmx::authoring::{self, author};
use abmx::config::{Config, INIT_TEMPLATE};
use abmx::fetchers::DirFetcher;
use abmx::paths;
use abmx::receipts;
use abmx::state::State;
use abmx::Runner;
use abmx_core::{ApplyOptions, FetchRequest, Fetcher, LedgerMatch, Plan, Report, Sink};
use abmx_gmail::{revoke_refresh_token, Credentials, TokenStore, REVOKE_URL};
use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};

mod parser_cli;
mod statement_cli;

/// Clean run.
const EXIT_OK: i32 = 0;
/// The tool itself failed (config, network, state).
const EXIT_TOOL: i32 = 1;
/// One or more routed labels need a parser or account fix.
const EXIT_FAILED: i32 = 2;

#[derive(Parser)]
#[command(
    name = "abmx",
    version,
    about = "Sync bank transaction emails and statements into an Actual Budget ledger",
    long_about = "abmx fetches bank mail, parses it with a runtime parser corpus, decides \
                  what to import, and writes it to a ledger. It never parses a bank template \
                  in code and never guesses an account mapping."
)]
struct Cli {
    /// Config file. Defaults to $ABMX_CONFIG, then the XDG config dir, then
    /// ./abmx.toml.
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

    /// Interactive quickstart: config, Gmail, labels, and account mapping.
    Setup {
        /// Approve in any browser and paste the callback URL back.
        #[arg(long)]
        manual: bool,
        /// Print the Gmail authorization URL instead of opening a browser.
        #[arg(long)]
        no_browser: bool,
        /// Re-offer labels you previously declined.
        #[arg(long)]
        review: bool,
        /// Re-ask account mappings that already exist.
        #[arg(long)]
        remap: bool,
    },

    /// Authorize or deauthorize a mail provider.
    Auth {
        #[command(subcommand)]
        command: AuthCommand,
    },

    /// Show config, corpus, labels, accounts, state, sink, and auth health.
    Status {
        /// Skip network checks (Gmail and sink accounts).
        #[arg(long)]
        offline: bool,
        /// Verify the newest messages under every routed label.
        #[arg(long, conflicts_with = "offline")]
        strict: bool,
        /// Exit nonzero when anything needs attention.
        #[arg(long)]
        check: bool,
        /// Machine-readable output.
        #[arg(long)]
        json: bool,
    },

    /// Receipts: sync labelled mail and sample parser evidence.
    Receipt {
        #[command(subcommand)]
        command: ReceiptCommand,
    },

    /// Sync every enabled source: receipts first, then statements.
    Sync {
        /// Print the plans and write nothing.
        #[arg(long)]
        dry_run: bool,
        /// Print the plans as JSON on stdout.
        #[arg(long)]
        json: bool,
        /// Maximum unseen messages to fetch per source.
        #[arg(long, value_name = "N")]
        limit: Option<usize>,
    },

    /// Deprecated: use `abmx receipt samples`.
    Samples {
        /// Restrict to one Gmail label name or parser id.
        #[arg(long, value_name = "LABEL|PARSER")]
        label: Option<String>,
        /// How many of the newest messages to download per label.
        #[arg(long, default_value_t = 3, value_name = "N")]
        samples: usize,
        /// Output directory. Defaults to $XDG_CONFIG_HOME/abmx/samples.
        #[arg(long, value_name = "DIR")]
        out: Option<PathBuf>,
        /// Machine-readable manifest.
        #[arg(long)]
        json: bool,
    },

    /// Forget local state so the next sync re-fetches receipts and statements.
    ///
    /// No ledger is modified. Rows deleted in a ledger come back on the next
    /// sync, because the ledger is a projection of the mailbox.
    Reset {
        /// Delete the state database entirely, including the schema marker.
        #[arg(long)]
        purge: bool,
        /// Skip the confirmation prompt.
        #[arg(long)]
        yes: bool,
    },

    /// List the sink's accounts and how parsers map onto them.
    Accounts {
        /// Machine-readable output.
        #[arg(long)]
        json: bool,
    },

    /// Parser-corpus commands: parse, explain, inspect, verify fixtures.
    Parser(parser_cli::ParserCli),

    /// Bank statements: dump, verify, and import PDF statements.
    Statement(statement_cli::StatementCli),

    /// Print the parser-authoring skill to stdout.
    Skill,

    /// Print the normalized block list of an email (an authoring aid).
    Dump {
        /// A `.eml` file or a directory of them.
        path: PathBuf,
        /// Machine-readable output.
        #[arg(long)]
        json: bool,
    },
}

#[derive(Subcommand)]
enum ReceiptCommand {
    /// Fetch, parse, and write receipt rows.
    Sync {
        /// Read `.eml` files instead of using the configured fetcher.
        #[arg(long, value_name = "DIR")]
        from_dir: Option<PathBuf>,
        /// With --from-dir, the label to tag every file with.
        #[arg(long, value_name = "LABEL")]
        label: Option<String>,
        /// Print the plan and write nothing.
        #[arg(long)]
        dry_run: bool,
        /// Print the plan as JSON on stdout.
        #[arg(long)]
        json: bool,
        /// With --dry-run, also compare against the ledger.
        #[arg(long, requires = "dry_run")]
        diff: bool,
        /// Maximum unseen messages to fetch in this run.
        #[arg(long, value_name = "N")]
        limit: Option<usize>,
    },

    /// Download the newest messages for each managed label (parser evidence).
    Samples {
        /// Restrict to one Gmail label name or parser id.
        #[arg(long, value_name = "LABEL|PARSER")]
        label: Option<String>,
        /// How many of the newest messages to download per label.
        #[arg(long, default_value_t = 3, value_name = "N")]
        samples: usize,
        /// Output directory. Defaults to $XDG_CONFIG_HOME/abmx/samples.
        #[arg(long, value_name = "DIR")]
        out: Option<PathBuf>,
        /// Machine-readable manifest.
        #[arg(long)]
        json: bool,
    },
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

    /// Export a self-contained credential bundle for a server or container.
    ///
    /// The bundle adds the OAuth client secret to the refresh token, so one
    /// mounted file is enough to authenticate. Treat it like a password.
    Export {
        /// Write the bundle here. `-` writes to stdout.
        #[arg(long, value_name = "FILE", default_value = "abmx-gmail-auth.json")]
        out: PathBuf,
        /// Write the JSON to stdout instead of a file.
        #[arg(long)]
        stdout: bool,
        /// Overwrite an existing file.
        #[arg(long)]
        force: bool,
        /// Allow a bundle without a client secret (public OAuth clients).
        #[arg(long)]
        allow_missing_client_secret: bool,
    },

    /// Install an exported bundle at the configured token path.
    ///
    /// No browser or OAuth flow is needed on the target machine.
    Import {
        /// Bundle to read. `-` reads stdin.
        #[arg(long, value_name = "FILE")]
        file: PathBuf,
        /// Overwrite an existing token file.
        #[arg(long)]
        force: bool,
    },
}

fn main() {
    // Behave like a normal Unix filter when the reader goes away (e.g. `| head`).
    #[cfg(unix)]
    unsafe {
        libc::signal(libc::SIGPIPE, libc::SIG_DFL);
    }

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
        Command::Setup {
            manual,
            no_browser,
            review,
            remap,
        } => setup(&settings, manual, no_browser, review, remap),
        Command::Auth { command } => match command {
            AuthCommand::Gmail {
                no_browser,
                port,
                manual,
            } => auth_gmail(&settings, no_browser, port, manual),
            AuthCommand::Logout { no_revoke } => auth_logout(&settings, no_revoke),
            AuthCommand::Export {
                out,
                stdout,
                force,
                allow_missing_client_secret,
            } => auth_export(&settings, &out, stdout, force, allow_missing_client_secret),
            AuthCommand::Import { file, force } => auth_import(&settings, &file, force),
        },
        Command::Status {
            offline,
            strict,
            check,
            json,
        } => status(&settings, offline, strict, check, json),
        Command::Receipt { command } => match command {
            ReceiptCommand::Sync {
                from_dir,
                label,
                dry_run,
                json,
                diff,
                limit,
            } => match receipt_sync(
                &settings,
                ReceiptSyncOptions {
                    from_dir,
                    label,
                    dry_run,
                    json,
                    diff,
                    limit,
                    silent: false,
                },
            )? {
                Some((_, code)) => Ok(code),
                None => Ok(EXIT_OK),
            },
            ReceiptCommand::Samples {
                label,
                samples,
                out,
                json,
            } => run_samples(&settings, label, samples, out, json),
        },
        Command::Sync {
            dry_run,
            json,
            limit,
        } => sync_all(&settings, dry_run, json, limit),
        Command::Samples {
            label,
            samples,
            out,
            json,
        } => {
            eprintln!("note: `abmx samples` is now `abmx receipt samples`");
            run_samples(&settings, label, samples, out, json)
        }
        Command::Reset { purge, yes } => reset(&settings, purge, yes),
        Command::Accounts { json } => accounts(&settings, json),
        Command::Parser(cli) => parser_cli::run(cli),
        Command::Statement(cli) => statement_cli::run(cli, settings.config.clone()),
        Command::Skill => skill(),
        Command::Dump { path, json } => dump(&path, json),
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
    config.validate_receipts()?;

    let options = config.parser_options(base);
    let (parser, _discovery) =
        abmx_parser::Parser::load(&options).map_err(|error| match &error {
            abmx_parser::Error::NoCorpus { .. }
            | abmx_parser::Error::EmptyCorpus { .. }
            | abmx_parser::Error::Parse(abmx_parse::Error::NoParsers { .. }) => anyhow::anyhow!(
                "no parsers installed — nothing can be parsed yet.\n\n\
             download samples and ask an agent to author one:\n  \
             abmx receipt samples\n  \
             mkdir -p ~/.agents/skills/abmx && abmx skill > ~/.agents/skills/abmx/SKILL.md\n  \
             then: \"help me create parser for all my transaction email, use abmx parser authoring skill\"\n\n\
             the repository's synthetic example is under examples/parsers/"
            ),
            other => anyhow::anyhow!(
                "loading the parser corpus: {other}\n  \
             hint: run `abmx parser paths` to see where abmx looked"
            ),
        })?;

    for parser_id in config.receipts.accounts.keys() {
        if parser.registry().find(parser_id).is_none() {
            bail!("[receipts.accounts] names parser `{parser_id}`, which is not in the corpus");
        }
    }

    let state = State::open(&config.state_path(base))?;
    state.check_schema()?;

    Ok(Runner::new(
        parser,
        config.receipts.prefix.clone(),
        config.account_map(),
        state,
    ))
}

/// Check the Gmail label inventory against `[receipts.accounts]`.
///
/// Only mapped labels are fetched. A discovered label with no account entry is
/// unmanaged, not an error: the user may have declined it, or gmailctl may have
/// created it ahead of time. A *configured* account with no Gmail label is
/// different — that is drift and fails loudly.
fn resolve_label_inventory(
    fetcher: &abmx_gmail::GmailFetcher,
    config: &Config,
) -> Result<Vec<String>> {
    let discovered: Vec<String> = fetcher
        .routed_labels()?
        .into_iter()
        .map(|label| label.name)
        .collect();
    let plan = receipts::plan(&discovered, &config.receipts);

    if !plan.orphans.is_empty() {
        bail!(
            "[receipts.accounts] maps parser(s) with no Gmail label:\n  {}\n\
             add the gmailctl rule or remove the [receipts.accounts] entry",
            plan.orphans.join("\n  ")
        );
    }

    if !plan.invalid.is_empty() {
        eprintln!(
            "note: ignoring {} label(s) whose remainder is not a parser id: {}",
            plan.invalid.len(),
            plan.invalid.join(", ")
        );
    }

    if !plan.unmanaged.is_empty() {
        eprintln!(
            "note: {} routed label(s) are not managed by abmx; run `abmx setup` to review",
            plan.unmanaged.len()
        );
    }

    if plan.managed.is_empty() {
        bail!(
            "no routed Gmail label is mapped in [receipts.accounts]\n  \
             run `abmx setup` to choose a label and an account"
        );
    }

    Ok(plan.managed_names())
}

fn build_fetcher(
    config: &Config,
    base: &Path,
    from_dir: Option<PathBuf>,
    label: Option<String>,
) -> Result<(Box<dyn Fetcher>, Vec<String>)> {
    if let Some(dir) = from_dir {
        let label = match label {
            Some(label) => label,
            None => {
                if config.receipts.accounts.len() == 1 {
                    let parser = config.receipts.accounts.keys().next().expect("one entry");
                    format!("{}{}", config.receipts.prefix, parser)
                } else {
                    bail!("--from-dir needs --label unless exactly one parser is configured");
                }
            }
        };
        let Some(parser_id) = config.receipts.parser_id(&label) else {
            bail!(
                "`{label}` has no parser id after `{}`",
                config.receipts.prefix
            );
        };
        if !config.receipts.accounts.contains_key(&parser_id) {
            bail!("`{label}` has no [receipts.accounts] entry");
        }
        return Ok((
            Box::new(DirFetcher::labeled(dir, label.clone())),
            vec![label],
        ));
    }

    if label.is_some() {
        bail!("--label only applies with --from-dir");
    }

    let gmail = config.build_gmail(base)?;
    let labels = resolve_label_inventory(&gmail, config)?;
    Ok((Box::new(gmail), labels))
}

fn apply_options(config: &Config, dry_run: bool) -> ApplyOptions {
    ApplyOptions {
        dry_run,
        chunk_size: config.actual_chunk_size(),
    }
}

// ---------------------------------------------------------------- commands

fn init(path: Option<PathBuf>, force: bool) -> Result<i32> {
    let path = path.unwrap_or_else(|| paths::xdg_config_dir().join("abmx/config.toml"));

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
    println!("  abmx setup                   interactive config, Gmail, labels, accounts");
    println!();
    println!("or by hand:");
    println!(
        "  1. edit {} and set [fetch.gmail] + [sink.actual]",
        path.display()
    );
    println!("  2. abmx auth gmail            (headless: --manual)");
    println!("  3. abmx status");
    println!("  4. abmx receipt sync --dry-run");

    Ok(EXIT_OK)
}

fn setup(
    settings: &Settings,
    manual: bool,
    no_browser: bool,
    review: bool,
    remap: bool,
) -> Result<i32> {
    abmx::setup::run(&abmx::setup::Options {
        config: settings.config.clone(),
        no_browser,
        manual,
        review,
        remap,
    })
}

fn auth_gmail(
    settings: &Settings,
    no_browser: bool,
    port: Option<u16>,
    manual: bool,
) -> Result<i32> {
    let (config, base, _) = load_config(settings)?;

    let path = abmx::gmail_auth::authorize_gmail(
        &config,
        &base,
        &abmx::gmail_auth::Options {
            no_browser,
            port,
            manual,
        },
    )?;

    println!("authorized; refresh token stored at {}", path.display());
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

fn auth_export(
    settings: &Settings,
    out: &Path,
    stdout: bool,
    force: bool,
    allow_missing_client_secret: bool,
) -> Result<i32> {
    let (config, base, _) = load_config(settings)?;
    let store = TokenStore::new(config.gmail_token_path(&base));

    let Some(token) = store.load()? else {
        bail!(
            "no stored Gmail credentials at {}; run `abmx auth gmail` first",
            store.path().display()
        );
    };

    let client = config.gmail_credentials(&base)?;
    if client.client_secret.is_none() && !allow_missing_client_secret {
        bail!(
            "no Gmail client secret found; set ABMX_GMAIL_CLIENT_SECRET or point \
             `[fetch.gmail].credentials_file` at Google's OAuth client JSON. \
             Pass --allow-missing-client-secret for a public OAuth client."
        );
    }

    let scopes = if token.scopes.is_empty() {
        vec![abmx_gmail::auth::SCOPE.to_owned()]
    } else {
        token.scopes
    };

    let bundle = Credentials {
        client_id: client.client_id,
        refresh_token: token.refresh_token,
        scopes,
        client_secret: client.client_secret,
    };

    let to_stdout = stdout || out == Path::new("-");
    if to_stdout {
        eprintln!(
            "warning: this bundle contains a refresh token and the OAuth client secret; \
             do not paste it into logs or commit it"
        );
        println!("{}", serde_json::to_string_pretty(&bundle)?);
    } else {
        if out.exists() && !force {
            bail!(
                "{} already exists; pass --force to overwrite",
                out.display()
            );
        }
        TokenStore::new(out).save(&bundle)?;
        println!("exported Gmail credentials to {}", out.display());
    }

    Ok(EXIT_OK)
}

fn auth_import(settings: &Settings, file: &Path, force: bool) -> Result<i32> {
    let (config, base, _) = load_config(settings)?;

    let text = if file == Path::new("-") {
        let mut text = String::new();
        std::io::Read::read_to_string(&mut std::io::stdin(), &mut text)
            .context("reading the credential bundle from stdin")?;
        text
    } else {
        std::fs::read_to_string(file)
            .with_context(|| format!("reading the credential bundle {}", file.display()))?
    };

    let bundle: Credentials =
        serde_json::from_str(&text).context("parsing the credential bundle")?;

    if bundle.client_id.trim().is_empty() {
        bail!("the bundle has no client_id");
    }
    if bundle.refresh_token.trim().is_empty() {
        bail!("the bundle has no refresh_token");
    }

    let store = TokenStore::new(config.gmail_token_path(&base));
    if store.path().exists() && !force {
        bail!(
            "{} already exists; pass --force to overwrite",
            store.path().display()
        );
    }
    store.save(&bundle)?;

    println!("imported Gmail credentials to {}", store.path().display());
    if bundle.client_secret.is_none() {
        eprintln!(
            "note: the bundle has no client secret; set ABMX_GMAIL_CLIENT_SECRET or \
             `[fetch.gmail].client_secret_file`"
        );
    }

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
    hint: Option<String>,
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
            hint: None,
        });
    }

    /// Like `push`, plus a "what to do about it" line.
    fn hint(&mut self, status: Status, message: impl Into<String>, hint: impl Into<String>) {
        self.lines.push(Line {
            status,
            message: message.into(),
            hint: Some(hint.into()),
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
            if let Some(hint) = &line.hint {
                println!("        {hint}");
            }
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

fn status(
    settings: &Settings,
    offline: bool,
    strict: bool,
    check: bool,
    json: bool,
) -> Result<i32> {
    let (config, base, config_path) = load_config(settings)?;
    config.validate_receipts()?;
    config.validate_statements()?;

    // Fetch network data once; the sections only present the result.
    let mut gmail: Option<abmx_gmail::GmailFetcher> = None;
    let mut gmail_build_error: Option<String> = None;
    if config.receipts.enabled && !offline && config.fetch.gmail.is_some() {
        match config.build_gmail(&base) {
            Ok(fetcher) => gmail = Some(fetcher),
            Err(error) => gmail_build_error = Some(format!("{error:#}")),
        }
    }

    let mut gmail_labels: Option<Vec<String>> = None;
    let mut gmail_labels_error: Option<String> = None;
    if let Some(fetcher) = &gmail {
        match fetcher.routed_labels() {
            Ok(found) => {
                gmail_labels = Some(found.into_iter().map(|label| label.name).collect());
            }
            Err(error) => gmail_labels_error = Some(error.to_string()),
        }
    }

    let mut sink: Option<Box<dyn abmx_core::Sink>> = None;
    let mut sink_build_error: Option<String> = None;
    match config.build_sink(&base) {
        Ok(value) => sink = Some(value),
        Err(error) => sink_build_error = Some(format!("{error:#}")),
    }

    let mut sink_accounts: Option<Vec<abmx_core::SinkAccount>> = None;
    let mut sink_accounts_error: Option<String> = None;
    if !offline {
        if let Some(value) = &sink {
            match value.accounts() {
                Ok(accounts) => sink_accounts = Some(accounts),
                Err(error) => sink_accounts_error = Some(error.to_string()),
            }
        }
    }

    let plan = gmail_labels
        .as_deref()
        .map(|found| receipts::plan(found, &config.receipts));
    let accounts_by_id: Option<BTreeMap<&str, &abmx_core::SinkAccount>> =
        sink_accounts.as_ref().map(|accounts| {
            accounts
                .iter()
                .map(|account| (account.id.as_str(), account))
                .collect()
        });
    let validate_accounts = config.sink.actual.is_some() && accounts_by_id.is_some();

    let mut sections: Vec<Section> = Vec::new();

    let mut config_section = Section::new("config");
    config_section.push(Status::Ok, config_path.display().to_string());
    config_section.push(
        Status::Ok,
        format!(
            "receipts `{}`{}{}",
            config.receipts.prefix,
            config
                .receipts
                .since
                .map(|date| format!(", since {date}"))
                .unwrap_or_else(|| ", all history".to_owned()),
            if config.receipts.enabled {
                ""
            } else {
                " (disabled)"
            }
        ),
    );
    sections.push(config_section);

    let mut corpus_section = Section::new("corpus");
    let parser = match abmx_parser::Parser::load(&config.parser_options(&base)) {
        Ok((parser, discovery)) => {
            let summaries = parser.registry().summaries();
            let dir = discovery
                .dirs
                .first()
                .map(|dir| dir.display().to_string())
                .unwrap_or_else(|| "(built-in)".to_owned());
            corpus_section.push(
                Status::Ok,
                format!("{} parser(s), from {dir}", summaries.len()),
            );
            Some(parser)
        }
        Err(error) => {
            corpus_section.hint(
                Status::Fail,
                error.to_string(),
                "run `abmx parser paths` to see where the corpus was looked for",
            );
            None
        }
    };
    sections.push(corpus_section);

    let mut labels_section = Section::new("receipts");
    if !config.receipts.enabled {
        labels_section.push(
            Status::Skip,
            "receipts disabled ([receipts].enabled = false)",
        );
    } else {
        if config.receipts.accounts.is_empty() {
            labels_section.hint(
                Status::Fail,
                "no [receipts.accounts] entries",
                "run `abmx setup` to choose which labels to import and where they go",
            );
        }
        for (parser_id, account) in &config.receipts.accounts {
            let managed = plan.as_ref().and_then(|plan| {
                plan.managed
                    .iter()
                    .find(|label| label.parser_id == *parser_id)
            });
            let label = managed
                .map(|managed| managed.label.clone())
                .unwrap_or_else(|| format!("{}{}", config.receipts.prefix, parser_id));

            let parser_missing = parser
                .as_ref()
                .is_some_and(|parser| parser.registry().find(parser_id).is_none());
            let account_missing = validate_accounts
                && !account.trim().is_empty()
                && accounts_by_id
                    .as_ref()
                    .is_some_and(|accounts| !accounts.contains_key(account.as_str()));
            let label_missing = plan
                .as_ref()
                .is_some_and(|plan| plan.orphans.iter().any(|orphan| orphan == parser_id));

            let account_display = accounts_by_id
                .as_ref()
                .and_then(|accounts| accounts.get(account.as_str()))
                .map(|found| format!("{} ({account})", found.name))
                .unwrap_or_else(|| account.clone());

            let mut problems = Vec::new();
            if account.trim().is_empty() {
                problems.push("no account id".to_owned());
            }
            if parser_missing {
                problems.push(format!("parser `{parser_id}` is missing"));
            }
            if account_missing {
                problems.push(format!("account `{account}` is not in the sink"));
            }
            if label_missing {
                problems.push("no Gmail label routes to it".to_owned());
            }

            if problems.is_empty() {
                labels_section.push(Status::Ok, format!("{label} -> {account_display}"));
            } else {
                let mut hints = Vec::new();
                if parser_missing {
                    hints.push(format!("author it: `abmx receipt samples --label {label}`"));
                }
                if account_missing {
                    hints.push("run `abmx accounts` to list valid ids".to_owned());
                }
                if label_missing {
                    hints.push("fix the gmailctl rule or remove the mapping".to_owned());
                }
                labels_section.hint(
                    Status::Fail,
                    format!("{label} -> {account_display}: {}", problems.join("; ")),
                    hints.join(" · "),
                );
            }
        }

        if settings.verbose {
            for parser_id in &config.receipts.ignore {
                labels_section.push(
                    Status::Skip,
                    format!("{}{} (ignored)", config.receipts.prefix, parser_id),
                );
            }
            if let Some(plan) = &plan {
                for label in &plan.unmanaged {
                    labels_section.hint(
                        Status::Skip,
                        format!("{} is not managed", label.label),
                        "run `abmx setup` to import it or decline it",
                    );
                }
                for label in &plan.invalid {
                    labels_section.hint(
                        Status::Warn,
                        format!("{label}: not a parser id after the prefix"),
                        "rename the gmailctl label",
                    );
                }
            }
        } else {
            if !config.receipts.ignore.is_empty() {
                labels_section.push(
                    Status::Skip,
                    format!(
                        "{} ignored label(s) (use -v to list)",
                        config.receipts.ignore.len()
                    ),
                );
            }
            if let Some(plan) = &plan {
                if !plan.unmanaged.is_empty() {
                    labels_section.push(
                        Status::Skip,
                        format!(
                            "{} unmanaged Gmail label(s) (use -v to list)",
                            plan.unmanaged.len()
                        ),
                    );
                }
                if !plan.invalid.is_empty() {
                    labels_section.push(
                        Status::Warn,
                        format!(
                            "{} Gmail label(s) with an invalid parser id (use -v to list)",
                            plan.invalid.len()
                        ),
                    );
                }
            }
        }

        if plan.is_none() {
            if offline {
                labels_section.push(Status::Skip, "Gmail label inventory skipped (offline)");
            } else if config.fetch.gmail.is_some() {
                if let Some(error) = &gmail_labels_error {
                    labels_section.hint(
                        Status::Warn,
                        "could not verify Gmail labels",
                        error.clone(),
                    );
                } else {
                    labels_section
                        .push(Status::Skip, "Gmail label inventory unavailable (see auth)");
                }
            }
        }

        if !config.receipts.accounts.is_empty() && config.receipts.since.is_none() {
            labels_section.hint(
                Status::Warn,
                "no [receipts].since; sync scans every labelled message",
                "set a date or run `abmx setup`",
            );
        }
    }
    sections.push(labels_section);

    let mut statements_section = Section::new("statements");
    if !config.statements.enabled {
        statements_section.push(
            Status::Skip,
            "statements disabled ([statements].enabled = false)",
        );
    } else {
        statements_section.push(
            Status::Ok,
            format!(
                "labels `{}`{}",
                config.statements.prefix,
                config
                    .statements
                    .since
                    .map(|date| format!(", since {date}"))
                    .unwrap_or_else(|| ", all history".to_owned())
            ),
        );
        if config.statements.accounts.is_empty() {
            statements_section.push(
                Status::Skip,
                "no statement parser is mapped (run `abmx setup`)",
            );
        } else {
            let explicit = config.parser_options(&base).parsers;
            let dirs = if explicit.is_empty() {
                abmx_parser::discover(&abmx_parser::Options::default()).dirs
            } else {
                explicit
            };
            match abmx_table::Registry::from_dirs(&dirs) {
                Ok(registry) => {
                    for key in config.statements.accounts.keys() {
                        let parser_id = key.split(':').next().unwrap_or(key);
                        if registry.find(parser_id).is_none() {
                            statements_section.hint(
                                Status::Fail,
                                format!("`{key}` has no statement parser"),
                                "author one, then run `abmx statement check`",
                            );
                        }
                    }
                }
                Err(error) => statements_section.hint(
                    Status::Warn,
                    "no statement parsers loaded",
                    error.to_string(),
                ),
            }

            for parser_id in config.statements.passwords.keys() {
                match config.statement_password(&base, parser_id) {
                    Ok(Some(_)) => statements_section
                        .push(Status::Ok, format!("{parser_id}: password available")),
                    Ok(None) => statements_section
                        .push(Status::Warn, format!("{parser_id}: no password configured")),
                    Err(error) => statements_section.hint(
                        Status::Fail,
                        format!("{parser_id}: {error}"),
                        "fix [statements.passwords] or mount the secret",
                    ),
                }
            }

            if settings.verbose {
                for parser_id in &config.statements.ignore {
                    statements_section.push(
                        Status::Skip,
                        format!("{}{} (ignored)", config.statements.prefix, parser_id),
                    );
                }
            } else if !config.statements.ignore.is_empty() {
                statements_section.push(
                    Status::Skip,
                    format!(
                        "{} ignored statement label(s) (use -v to list)",
                        config.statements.ignore.len()
                    ),
                );
            }

            if config.statements.since.is_none() {
                statements_section.hint(
                    Status::Warn,
                    "no [statements].since; every statement is fetched",
                    "set a date or run `abmx setup`",
                );
            }
        }
    }
    sections.push(statements_section);

    let mut state_section = Section::new("state");
    let state_path = config.state_path(&base);
    match State::open(&state_path) {
        Ok(state) => match state.check_schema() {
            Ok(()) => {
                let seen = state.seen_count().unwrap_or(0);
                state_section.push(
                    Status::Ok,
                    format!(
                        "{} ({seen} delivered, parser schema {})",
                        state_path.display(),
                        abmx_core::ABMX_SCHEMA_VERSION
                    ),
                );
            }
            Err(error) => state_section.hint(
                Status::Fail,
                error.to_string(),
                "the state was written against a different parser schema; `abmx reset --purge` starts fresh",
            ),
        },
        Err(error) => state_section.hint(
            Status::Fail,
            error.to_string(),
            "if the file is corrupt, `abmx reset --purge` removes it",
        ),
    }
    sections.push(state_section);

    let mut sink_section = Section::new("sink");
    let actual_config = config.actual_sink_config(&base).ok().flatten();
    if let Some(error) = &sink_build_error {
        sink_section.hint(
            Status::Fail,
            error.clone(),
            "run `abmx setup` to configure Actual or a file sink",
        );
    } else if let Some(value) = &sink {
        if offline {
            let location = actual_config
                .as_ref()
                .map(|actual| format!("Actual server {}", actual.server_url))
                .or_else(|| {
                    config
                        .sink
                        .file
                        .as_ref()
                        .map(|file| format!("file at {}", file.path.display()))
                })
                .unwrap_or_else(|| value.name().to_owned());
            sink_section.push(Status::Skip, format!("{location} (offline)"));
        } else if let Some(error) = &sink_accounts_error {
            sink_section.hint(
                Status::Fail,
                error.clone(),
                "is the Actual server reachable, and are `server_url`, the API key, \
                 and `budget_sync_id` correct?",
            );
        } else if let Some(accounts) = &sink_accounts {
            if config.sink.actual.is_some() {
                sink_section.push(
                    Status::Ok,
                    format!("{}: {} accounts", value.name(), accounts.len()),
                );
            } else {
                sink_section.push(Status::Skip, format!("{} (no account list)", value.name()));
            }
        }

        if !offline {
            if let Some(actual) = &actual_config {
                let versions = abmx_actual::version::report(actual);
                for line in versions.describe() {
                    sink_section.push(Status::Ok, line);
                }
                if let Some(concern) = versions.concern() {
                    sink_section.push(
                        if concern.fatal {
                            Status::Fail
                        } else {
                            Status::Warn
                        },
                        concern.message,
                    );
                }
            }
        }
    }
    sections.push(sink_section);

    let mut auth_section = Section::new("auth");
    match &config.fetch.gmail {
        None => auth_section.hint(
            Status::Warn,
            "no [fetch.gmail] section",
            "run `abmx setup`, or use `--from-dir` to sync saved .eml files",
        ),
        Some(_) => {
            let store = TokenStore::new(config.gmail_token_path(&base));
            match store.load() {
                Ok(Some(_)) if offline => auth_section.push(
                    Status::Ok,
                    format!("gmail token at {}", store.path().display()),
                ),
                Ok(Some(_)) => {
                    if let Some(fetcher) = &gmail {
                        match fetcher.client().profile() {
                            Ok(profile) => auth_section
                                .push(Status::Ok, format!("gmail: {}", profile.email_address)),
                            Err(error) => auth_section.hint(
                                Status::Fail,
                                error.to_string(),
                                "the refresh token may be revoked or expired; run `abmx auth gmail`",
                            ),
                        }
                    } else {
                        auth_section.hint(
                            Status::Fail,
                            gmail_build_error
                                .clone()
                                .unwrap_or_else(|| "could not build the Gmail fetcher".to_owned()),
                            "run `abmx setup` (or `abmx auth gmail`) to store credentials",
                        );
                    }
                }
                Ok(None) => auth_section.hint(
                    Status::Fail,
                    "gmail: no token",
                    "run `abmx setup` (or `abmx auth gmail`)",
                ),
                Err(error) => auth_section.hint(
                    Status::Fail,
                    error.to_string(),
                    "run `abmx setup` (or `abmx auth gmail`) to store credentials",
                ),
            }
        }
    }
    sections.push(auth_section);

    if strict && config.receipts.enabled {
        let mut strict_section = Section::new("strict receipts");
        match (&gmail, &parser) {
            (Some(fetcher), Some(parser)) => {
                let work = tempfile::tempdir().context("creating a temporary sample directory")?;
                let inventory = author::scan_gmail(
                    fetcher,
                    Some(parser),
                    &config.receipts,
                    &author::Options::default(),
                    work.path(),
                )?;
                if inventory.labels.is_empty() && inventory.orphans.is_empty() {
                    strict_section.hint(
                        Status::Fail,
                        format!("no `{}*` labels found", config.receipts.prefix),
                        "create a label and a gmailctl rule",
                    );
                }
                for report in inventory.labels.iter().chain(inventory.orphans.iter()) {
                    if report.status == author::LabelStatus::Ok {
                        strict_section.push(Status::Ok, report.label.clone());
                    } else if report.status == author::LabelStatus::Ignored {
                        strict_section.push(Status::Skip, format!("{}: ignored", report.label));
                    } else if report.status == author::LabelStatus::MissingAccount {
                        strict_section.hint(
                            Status::Skip,
                            format!("{}: not managed", report.label),
                            "run `abmx setup` to import it or decline it",
                        );
                    } else {
                        strict_section.hint(
                            Status::Fail,
                            format!("{}: {}", report.label, report.status.as_str()),
                            format!("run `abmx receipt samples --label {}`", report.label),
                        );
                    }
                }
            }
            (None, Some(_)) => strict_section.hint(
                Status::Skip,
                "Gmail is not available",
                gmail_build_error
                    .as_deref()
                    .unwrap_or("run `abmx setup` or `abmx auth gmail`"),
            ),
            (_, None) => strict_section.push(Status::Skip, "no parser corpus"),
        }
        sections.push(strict_section);
    }

    let (failures, warnings, skipped) = count_sections(&sections);

    if json {
        let value = serde_json::json!({
            "failures": failures,
            "warnings": warnings,
            "skipped": skipped,
            "sections": sections.iter().map(|section| serde_json::json!({
                "name": section.name,
                "lines": section.lines.iter().map(|line| serde_json::json!({
                    "status": line.status.label(),
                    "message": line.message,
                    "hint": line.hint,
                })).collect::<Vec<_>>(),
            })).collect::<Vec<_>>(),
        });
        println!("{}", serde_json::to_string_pretty(&value)?);
        return Ok(if check && failures > 0 {
            EXIT_FAILED
        } else {
            EXIT_OK
        });
    }

    render_sections(&sections);
    println!();
    if failures > 0 {
        println!("{failures} failure(s), {warnings} warning(s)");
    } else if warnings > 0 {
        println!("0 failures, {warnings} warning(s)");
    } else if skipped > 0 {
        println!("all checks passed ({skipped} skipped)");
    } else {
        println!("all checks passed");
    }

    Ok(if check && failures > 0 {
        EXIT_FAILED
    } else {
        EXIT_OK
    })
}

fn count_sections(sections: &[Section]) -> (usize, usize, usize) {
    let mut failures = 0usize;
    let mut warnings = 0usize;
    let mut skipped = 0usize;
    for section in sections {
        for line in &section.lines {
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

fn require_parsers(runner: &Runner) -> Result<()> {
    if runner.parser.registry().parsers.is_empty() {
        bail!(
            "no parsers installed — nothing can be parsed yet.\n\n\
             download samples and ask an agent to author one:\n  \
             abmx receipt samples\n  \
             mkdir -p ~/.agents/skills/abmx && abmx skill > ~/.agents/skills/abmx/SKILL.md\n  \
             then: \"help me create parser for all my transaction email, use abmx parser authoring skill\""
        );
    }
    Ok(())
}

fn sync_all(settings: &Settings, dry_run: bool, json: bool, limit: Option<usize>) -> Result<i32> {
    let mut exit = EXIT_OK;
    let mut tool_failed = false;
    let mut plans = serde_json::Map::new();
    // In JSON mode each source stays quiet and the combined plan is printed at
    // the end. In human mode the source summaries print in sequence.
    let silent = json;

    if !json {
        eprintln!("== receipts ==");
    }
    match receipt_sync(
        settings,
        ReceiptSyncOptions {
            from_dir: None,
            label: None,
            dry_run,
            json: false,
            diff: false,
            limit,
            silent,
        },
    ) {
        Ok(Some((plan, code))) => {
            exit = exit.max(code);
            if json {
                plans.insert("receipts".to_owned(), serde_json::to_value(&plan)?);
            }
        }
        Ok(None) => {}
        Err(error) => {
            eprintln!("error: receipts: {error:#}");
            tool_failed = true;
        }
    }

    if !json {
        eprintln!("\n== statements ==");
    }
    match statement_cli::sync(
        settings.config.as_deref(),
        &[],
        dry_run,
        false,
        silent,
        limit,
    ) {
        Ok(Some((plan, code))) => {
            exit = exit.max(code);
            if json {
                plans.insert("statements".to_owned(), serde_json::to_value(&plan)?);
            }
        }
        Ok(None) => {}
        Err(error) => {
            eprintln!("error: statements: {error:#}");
            tool_failed = true;
        }
    }

    if json {
        println!("{}", serde_json::to_string_pretty(&plans)?);
    }
    if exit == EXIT_OK && tool_failed {
        exit = EXIT_TOOL;
    }
    Ok(exit)
}

struct ReceiptSyncOptions {
    from_dir: Option<PathBuf>,
    label: Option<String>,
    dry_run: bool,
    json: bool,
    diff: bool,
    limit: Option<usize>,
    /// Suppress the plan and summary so the combined `abmx sync` can print one
    /// labelled report. Diagnostics still go to stderr.
    silent: bool,
}

/// Sync receipts.
fn receipt_sync(settings: &Settings, options: ReceiptSyncOptions) -> Result<Option<(Plan, i32)>> {
    let ReceiptSyncOptions {
        from_dir,
        label,
        dry_run,
        json,
        diff,
        limit,
        silent,
    } = options;
    let (config, base, _) = load_config(settings)?;
    config.validate_receipts()?;
    if !config.receipts.enabled {
        if !silent {
            eprintln!("receipts are disabled ([receipts].enabled = false); nothing to do");
        }
        return Ok(None);
    }
    if config.receipts.accounts.is_empty() {
        bail!(
            "no [receipts.accounts] entries; route mail with `{prefix}<parser-id>` and map it to an Actual account",
            prefix = config.receipts.prefix
        );
    }
    if from_dir.is_none() && config.receipts.since.is_none() {
        eprintln!(
            "warning: [receipts].since is not set; every labelled message in the mailbox \
             will be scanned. Set a date or run `abmx setup`."
        );
    }

    // Refuse to write with a client a year behind the server. A minor skew is
    // routine enough to warn about and continue.
    if !dry_run {
        if let Some(actual) = config.actual_sink_config(&base)? {
            let versions = abmx_actual::version::report(&actual);
            if let Some(concern) = versions.concern() {
                if concern.fatal {
                    bail!("{}", concern.message);
                }
                eprintln!("warning: {}", concern.message);
            }
        }
    }

    let runner = build_runner(&config, &base)?;
    require_parsers(&runner)?;
    let (fetcher, labels) = build_fetcher(&config, &base, from_dir, label)?;

    let request = match limit {
        Some(limit) => FetchRequest::new(&labels).with_limit(limit),
        None => FetchRequest::new(&labels),
    };
    let plan = runner.plan(fetcher.as_ref(), request)?;

    // A dry run previews the sink's prose (Actual notes).
    // Building a sink is side-effect free, but a broken sink must not turn a
    // plain dry run into an error; a real sync still requires it.
    let sink = if dry_run {
        if json || silent {
            None
        } else {
            config.build_sink(&base).ok()
        }
    } else {
        Some(config.build_sink(&base)?)
    };

    if !silent {
        if json {
            println!("{}", serde_json::to_string_pretty(&plan)?);
        } else {
            summarize_plan(
                &plan,
                settings.quiet,
                settings.verbose,
                !dry_run,
                sink.as_deref(),
            );
        }
    } else {
        for error in &plan.stats.errors {
            eprintln!("error: {error}");
        }
    }

    if dry_run {
        if diff && !silent {
            diff_against_ledger(&config, &base, &plan)?;
        }
        let code = if plan.stats.broken > 0 || plan.stats.failed > 0 {
            EXIT_FAILED
        } else {
            EXIT_OK
        };
        return Ok(Some((plan, code)));
    }

    if plan.stats.broken > 0 || plan.stats.failed > 0 {
        eprintln!(
            "refusing to apply: {} labeled message(s) need a parser or account fix; nothing was written",
            plan.stats.broken + plan.stats.failed
        );
        return Ok(Some((plan, EXIT_FAILED)));
    }

    let sink = sink.expect("a real sync builds the sink");
    let options = apply_options(&config, dry_run);
    let report = runner.apply(sink.as_ref(), &plan, &options)?;

    if !silent {
        summarize_report(&report);
    } else {
        for error in &report.errors {
            eprintln!(
                "error: {}: {}",
                error.tx_id.as_deref().unwrap_or("-"),
                error.message
            );
        }
    }
    let code = exit_for_report(&report);
    Ok(Some((plan, code)))
}

/// Compare the plan against what the ledger already holds.
fn diff_against_ledger(config: &Config, base: &Path, plan: &Plan) -> Result<()> {
    let rows = plan.import_rows();
    if rows.is_empty() {
        println!("ledger: nothing to compare (the plan is empty)");
        return Ok(());
    }

    let accounts: BTreeSet<String> = rows.iter().map(|row| row.account.clone()).collect();
    let accounts: Vec<String> = accounts.into_iter().collect();
    let since = rows
        .iter()
        .map(|row| row.transaction.occurred_on)
        .min()
        .expect("non-empty rows");
    let until = rows
        .iter()
        .map(|row| row.transaction.occurred_on)
        .max()
        .expect("non-empty rows");

    let sink = config.build_sink(base)?;
    let ledger = sink.snapshot(&accounts, since, until)?;

    let mut new = 0usize;
    let mut existing = 0usize;
    let mut foreign = Vec::new();
    for row in &rows {
        match abmx_core::classify(row, &ledger) {
            LedgerMatch::New => new += 1,
            LedgerMatch::Existing {
                imported_id,
                foreign: is_foreign,
            } => {
                existing += 1;
                if is_foreign {
                    foreign.push((row, imported_id));
                }
            }
        }
    }

    println!(
        "ledger: {new} new, {existing} already present, {} foreign imported_id",
        foreign.len()
    );
    for (row, imported_id) in foreign {
        println!(
            "  foreign  {}  {}  {}  imported as {}",
            row.transaction.occurred_on,
            format_amount(&row.transaction),
            truncate(row.transaction.counterparty.as_deref().unwrap_or("-"), 24),
            imported_id.as_deref().unwrap_or("-")
        );
    }
    Ok(())
}

fn truncate(value: &str, limit: usize) -> String {
    if value.chars().count() <= limit {
        return value.to_owned();
    }
    let shortened: String = value.chars().take(limit.saturating_sub(1)).collect();
    format!("{shortened}…")
}

fn reset(settings: &Settings, purge: bool, yes: bool) -> Result<i32> {
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
    let messages = state.seen_count()?;

    if messages == 0 {
        println!("local state is already empty");
        return Ok(EXIT_OK);
    }

    let what = format!("{messages} delivered source(s) (receipt labels and statement attachments)");
    let question =
        format!("This clears abmx's local state only. No ledger is modified.\nclear {what}?");
    if !yes && !confirm(&question)? {
        println!("aborted");
        return Ok(EXIT_OK);
    }

    state.reset_progress()?;
    println!("cleared {what}");
    println!(
        "No ledger was modified. The next sync re-fetches and re-parses. \
         Existing ledger rows are not deleted; already-imported statement rows stay \
         deduplicated by their ids."
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

    let mapped: BTreeSet<&str> = config
        .receipts
        .accounts
        .values()
        .map(String::as_str)
        .collect();

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
    if config.receipts.accounts.is_empty() {
        println!(
            "[receipts.accounts] is empty; route mail with `{}<parser-id>` and map it to one of these ids.",
            config.receipts.prefix
        );
    } else {
        for (parser_id, account) in &config.receipts.accounts {
            let exists = accounts.iter().any(|candidate| candidate.id == *account);
            println!(
                "{:<24} -> {} {}",
                parser_id,
                account,
                if exists { "" } else { "(missing!)" }
            );
        }
    }

    Ok(EXIT_OK)
}

fn run_samples(
    settings: &Settings,
    label: Option<String>,
    count: usize,
    out: Option<PathBuf>,
    json: bool,
) -> Result<i32> {
    let (config, base, _) = load_config(settings)?;
    config.validate_receipts()?;
    if !config.receipts.enabled {
        eprintln!("receipts are disabled ([receipts].enabled = false)");
        return Ok(EXIT_OK);
    }
    if config.receipts.accounts.is_empty() {
        bail!("no [receipts.accounts] entries; run `abmx setup` to choose which labels to import");
    }
    if config.receipts.since.is_none() {
        eprintln!(
            "warning: [receipts].since is not set; sync will scan every labelled message. \
             Set a date or run `abmx setup`."
        );
    }

    let fetcher = config
        .build_gmail(&base)
        .context("Gmail is not authorized; run `abmx setup` or `abmx auth gmail`")?;
    let report = abmx::samples::download(
        &config,
        &fetcher,
        &abmx::samples::Options {
            out,
            samples: count,
            label,
        },
    )?;

    if json {
        println!("{}", serde_json::to_string_pretty(&report)?);
        return Ok(EXIT_OK);
    }

    println!(
        "samples: {} label(s) -> {}",
        report.labels.len(),
        report.output.display()
    );
    for label in &report.labels {
        println!(
            "  {:<32} {:>2}/{:<2}",
            label.label, label.downloaded, label.requested
        );
    }

    for warning in &report.warnings {
        eprintln!("warning: {warning}");
    }
    for label in &report.labels {
        for warning in &label.warnings {
            eprintln!("warning: {}: {warning}", label.label);
        }
    }

    println!();
    println!("next: author the parsers from these samples; see `abmx skill`");
    Ok(EXIT_OK)
}

fn dump(path: &Path, json: bool) -> Result<i32> {
    let emails = authoring::dump::load(path)?;
    if json {
        println!("{}", authoring::dump::render_json(&emails)?);
    } else {
        print!("{}", authoring::dump::render_text(&emails));
    }
    Ok(EXIT_OK)
}

// ---------------------------------------------------------------- output

const UNMATCHED_PREVIEW: usize = 10;

fn summarize_plan(
    plan: &Plan,
    quiet: bool,
    verbose: bool,
    persisted: bool,
    sink: Option<&dyn Sink>,
) {
    let delivered = if plan.stats.skipped > 0 {
        format!(" ({} already delivered)", plan.stats.skipped)
    } else {
        String::new()
    };
    eprintln!(
        "fetched {}{delivered}, parsed {} — import {}, broken {}, unmatched {}, failed {}",
        plan.stats.fetched,
        plan.stats.parsed,
        plan.stats.import,
        plan.stats.broken,
        plan.stats.unmatched,
        plan.stats.failed,
    );

    for error in &plan.stats.errors {
        eprintln!("error: {error}");
    }

    if !quiet && !plan.rows.is_empty() {
        eprintln!();
        eprintln!(
            "{:<10} {:>16} {:<28} {:<40} notes",
            "date", "amount", "label", "counterparty"
        );
        for row in &plan.rows {
            let transaction = &row.transaction;
            let notes = sink
                .and_then(|sink| sink.preview_notes(transaction))
                .unwrap_or_else(|| "-".to_owned());
            let mut line = format!(
                "{:<10} {:>16} {:<28} {:<40} {}",
                transaction.occurred_on,
                format_amount(transaction),
                truncate(&row.account_key, 28),
                truncate(transaction.counterparty.as_deref().unwrap_or("-"), 40),
                truncate(&notes, 40),
            );
            if verbose {
                line.push_str(&format!("  · {} · {}", transaction.parser, row.message));
            }
            eprintln!("{line}");
        }
    }

    if !quiet && !plan.broken.is_empty() {
        eprintln!();
        eprintln!("broken ({}), not imported:", plan.broken.len());
        for entry in &plan.broken {
            eprintln!(
                "  {:<32} {:<20} {}",
                truncate(&entry.label, 32),
                entry.reason.as_str(),
                entry.reason.detail()
            );
        }

        let labels: BTreeSet<&str> = plan
            .broken
            .iter()
            .map(|entry| entry.label.as_str())
            .collect();
        for label in labels {
            eprintln!("  next: abmx receipt samples --label {label}");
        }

        if persisted {
            eprintln!("  nothing was written to the ledger");
        }
    }

    if !quiet && !plan.unmatched.is_empty() {
        eprintln!();
        eprintln!(
            "unmatched ({}), no configured label claimed them:",
            plan.unmatched.len()
        );
        for (index, entry) in plan.unmatched.iter().enumerate() {
            if index == UNMATCHED_PREVIEW {
                eprintln!("  … and {} more", plan.unmatched.len() - UNMATCHED_PREVIEW);
                break;
            }
            let subject = entry
                .subject
                .as_deref()
                .map(|subject| format!("\"{}\"", truncate(subject, 50)))
                .unwrap_or_else(|| "(no subject)".to_owned());
            let from = entry.from.as_deref().unwrap_or("-");
            eprintln!("  {subject:<54} {from}");
            if verbose {
                eprintln!("  {} · {}", entry.message, entry.reason);
            }
        }
    }

    if !quiet && (!plan.rows.is_empty() || !plan.broken.is_empty() || !plan.unmatched.is_empty()) {
        eprintln!();
    }
}

fn format_amount(transaction: &abmx_core::Transaction) -> String {
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

fn exit_for_report(report: &Report) -> i32 {
    if !report.errors.is_empty() {
        EXIT_FAILED
    } else {
        EXIT_OK
    }
}

fn skill() -> Result<i32> {
    print!("{}", authoring::skill::render_authoring()?);
    Ok(EXIT_OK)
}
