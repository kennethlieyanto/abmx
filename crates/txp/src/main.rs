//! The `txp` command line interface.
//!
//! Contract: transactions go to stdout, diagnostics go to stderr, and the exit
//! code tells a pipeline whether to trust the result.
//!
//!   0  every input produced a strict transaction
//!   2  at least one email was unmatched or failed to parse
//!   3  at least one transaction was heuristic (unless --allow-heuristic)
//!   1  the tool itself failed

use std::collections::{BTreeMap, BTreeSet};
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use clap::{Parser as ClapParser, Subcommand, ValueEnum};
use txp_core::{Money, Transaction};
use txp_parse::Outcome;

const EXIT_UNMATCHED: i32 = 2;
const EXIT_HEURISTIC: i32 = 3;

#[derive(ClapParser)]
#[command(
    name = "txp",
    version,
    about = "Parse Indonesian bank transaction emails into structured JSON"
)]
struct Cli {
    /// Parser directory. Repeat to layer a local override over a base corpus.
    #[arg(long, global = true, action = clap::ArgAction::Append)]
    parsers: Vec<PathBuf>,

    /// Router policy file. Defaults to `router.toml` inside the first parser directory.
    #[arg(long, global = true)]
    router: Option<PathBuf>,

    /// Fixture root used by `check` and `init`. Defaults to ./fixtures.
    #[arg(long, global = true)]
    fixtures: Option<PathBuf>,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Parse a `.eml` file or every `.eml` under a directory.
    Parse {
        path: PathBuf,

        #[arg(long, value_enum, default_value_t = Format::Json)]
        format: Format,

        /// Only emit transactions for this account key (e.g. example-account).
        #[arg(long)]
        account: Option<String>,

        /// Exit 0 even when heuristic transactions are present.
        #[arg(long)]
        allow_heuristic: bool,
    },

    /// Print the normalized block list of an email (used to author parsers).
    Dump { path: PathBuf },

    /// Explain which parser claims an email, and why.
    Route { path: PathBuf },

    /// Print the JSON Schema of the transaction contract.
    Schema,

    /// Show the parser search order and which directory won.
    Paths {
        #[arg(long)]
        json: bool,
    },

    /// List the loaded parsers and the file each came from.
    Specs {
        #[arg(long, value_enum, default_value_t = Format::Json)]
        format: Format,
    },

    /// List the sender addresses the resolved corpus covers.
    Senders {
        #[arg(long, value_enum, default_value_t = Format::Plain)]
        format: Format,
    },

    /// Verify every parser against its fixtures.
    Check {
        #[arg(long)]
        parser: Option<String>,

        #[arg(long, short)]
        verbose: bool,

        #[arg(long)]
        json: bool,
    },

    /// Summarise how a directory of emails is parsed.
    Corpus {
        #[arg(long, default_value = "sample")]
        dir: PathBuf,
    },

    /// Scaffold a new parser file and its fixture directory.
    Init {
        /// Parser id, e.g. example.card_debit. The file becomes parsers/<id>.toml.
        id: String,
    },

    /// Draft a parser spec from a directory of emails.
    Infer {
        dir: PathBuf,

        /// Parser id to put in the draft.
        #[arg(long)]
        id: Option<String>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
enum Format {
    Json,
    Ndjson,
    Table,
    Plain,
}

fn main() {
    // Behave like a normal Unix filter when the reader goes away (e.g. `| head`).
    #[cfg(unix)]
    unsafe {
        libc::signal(libc::SIGPIPE, libc::SIG_DFL);
    }

    let code = match run() {
        Ok(code) => code,
        Err(error) => {
            eprintln!("txp: {error:#}");
            1
        }
    };

    std::process::exit(code);
}

fn run() -> Result<i32> {
    let Cli {
        parsers,
        router,
        fixtures,
        command,
    } = Cli::parse();

    let options = txp::Options { parsers, router };

    match command {
        Command::Dump { path } => {
            let email = read_email(&path)?;
            dump(&email);
            Ok(0)
        }
        Command::Route { path } => {
            let (parser, _) = load(&options)?;
            let email = read_email(&path)?;
            route(&parser, &email);
            Ok(0)
        }
        Command::Parse {
            path,
            format,
            account,
            allow_heuristic,
        } => {
            let (parser, _) = load(&options)?;
            parse(&parser, &path, format, account.as_deref(), allow_heuristic)
        }
        Command::Check {
            parser,
            verbose,
            json,
        } => check(
            &options,
            fixtures.as_deref(),
            parser.as_deref(),
            verbose,
            json,
        ),
        Command::Corpus { dir } => {
            let (parser, _) = load(&options)?;
            corpus(&parser, &dir)
        }
        Command::Senders { format } => {
            let (parser, _) = load(&options)?;
            senders(&parser, format)
        }
        Command::Paths { json } => paths(&options, json),
        Command::Specs { format } => {
            let (parser, _) = load(&options)?;
            specs(&parser, format)
        }
        Command::Schema => {
            println!("{}", serde_json::to_string_pretty(&txp::schema())?);
            Ok(0)
        }
        Command::Init { id } => init(
            &id,
            options.parsers.first().map(|path| path.as_path()),
            fixtures.as_deref(),
        ),
        Command::Infer { dir, id } => infer(&dir, id.as_deref()),
    }
}

fn load(options: &txp::Options) -> Result<(txp::Parser, txp::Discovery)> {
    let (parser, discovery) = txp::Parser::load(options).map_err(|error| match error {
        txp::Error::NoCorpus { tried } => {
            let tried = if tried.is_empty() {
                format!(
                    "{}/parsers, ~/.config/txp/parsers, /etc/txp/parsers",
                    std::env::var("PWD").unwrap_or_else(|_| ".".into())
                )
            } else {
                tried
                    .iter()
                    .map(|path| path.display().to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            };
            anyhow::anyhow!(
                "no parser specs found; tried {tried}\n  pass --parsers <dir> or set {}=<dir>",
                txp::ENV_PARSERS
            )
        }
        other => anyhow::Error::new(other),
    })?;

    for shadow in &parser.registry().shadows {
        eprintln!(
            "txp: {} is shadowed by {}",
            shadow.skipped.display(),
            shadow.winner.display()
        );
    }

    Ok((parser, discovery))
}

fn read_email(path: &Path) -> Result<txp_core::NormalizedEmail> {
    let bytes = std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;
    txp::parse_eml(&bytes, Some(path.display().to_string()))
        .map_err(|error| anyhow::anyhow!("{}: {error}", path.display()))
}

fn dump(email: &txp_core::NormalizedEmail) {
    println!(
        "from:    {} <{}>",
        email.from.name.as_deref().unwrap_or(""),
        email.from.email
    );
    println!("subject: {}", email.subject);
    println!("date:    {}", email.date);
    println!(
        "source:  {}",
        email.source.message_id.as_deref().unwrap_or("-")
    );
    println!("blocks ({}):", email.blocks.len());
    for (index, block) in email.blocks.iter().enumerate() {
        println!("{index:>3}  {}", block.text);
    }
}

fn route(parser: &txp::Parser, email: &txp_core::NormalizedEmail) {
    let result = parser.registry().route(email);

    println!("email:   {} / {}", email.from.email, email.subject);
    println!("candidates:");
    for candidate in &result.candidates {
        let marker = if candidate.eligible() { "*" } else { " " };
        println!(
            " {marker} {:<26} sender={:<5} subject={:<5} requires={}/{} rejects={} contains={} excludes={}",
            candidate.id,
            candidate.sender,
            candidate.subject,
            candidate.requires_matched,
            candidate.requires_total,
            candidate.rejects_clear,
            candidate.contains_ok,
            candidate.excludes_ok
        );
    }

    match result.parser {
        Some(index) => println!(
            "=> {} ({})",
            parser.registry().parsers[index].spec.id,
            result.reason.as_str()
        ),
        None => println!("=> none ({})", result.reason.as_str()),
    }
}

fn parse(
    parser: &txp::Parser,
    path: &Path,
    format: Format,
    account: Option<&str>,
    allow_heuristic: bool,
) -> Result<i32> {
    let results = parser
        .parse_path(path)
        .with_context(|| format!("parsing {}", path.display()))?;

    let mut transactions = Vec::new();
    let mut unmatched = 0usize;
    let mut failed = 0usize;
    let mut heuristic = 0usize;

    for (file, outcome) in &results {
        match outcome {
            Outcome::Parsed(transaction) => {
                if let Some(account) = account {
                    if transaction.account != account {
                        continue;
                    }
                }
                if transaction.extraction == txp_core::Extraction::Heuristic {
                    heuristic += 1;
                    eprintln!(
                        "txp: heuristic: {} ({})",
                        file.display(),
                        transaction.warnings.join("; ")
                    );
                }
                transactions.push(transaction.as_ref().clone());
            }
            Outcome::Unmatched(reason) => {
                unmatched += 1;
                eprintln!("txp: unmatched: {} ({reason})", file.display());
            }
            Outcome::Failed { parser, error } => {
                failed += 1;
                eprintln!("txp: failed: {} ({parser}: {error})", file.display());
            }
        }
    }

    emit(&transactions, format)?;

    let code = if unmatched + failed > 0 {
        EXIT_UNMATCHED
    } else if heuristic > 0 && !allow_heuristic {
        EXIT_HEURISTIC
    } else {
        0
    };

    Ok(code)
}

fn emit(transactions: &[Transaction], format: Format) -> Result<()> {
    let stdout = std::io::stdout();
    let mut out = stdout.lock();

    match format {
        Format::Json => {
            serde_json::to_writer_pretty(&mut out, transactions)?;
            writeln!(out)?;
        }
        Format::Ndjson => {
            for transaction in transactions {
                serde_json::to_writer(&mut out, transaction)?;
                writeln!(out)?;
            }
        }
        Format::Table => {
            writeln!(
                out,
                "{:<24} {:<11} {:<13} {:>16} {:<10} {:<28} {:<10} REF",
                "PARSER", "DATE", "KIND", "AMOUNT", "ACCOUNT", "COUNTERPARTY", "STATUS"
            )?;
            for transaction in transactions {
                writeln!(
                    out,
                    "{:<24} {:<11} {:<13} {:>16} {:<10} {:<28} {:<10} {}",
                    transaction.parser,
                    transaction.occurred_on,
                    transaction.kind.as_str(),
                    money(&transaction.amount),
                    truncate(&transaction.account, 10),
                    truncate(transaction.counterparty.as_deref().unwrap_or("-"), 28),
                    transaction.status.as_str(),
                    transaction.reference_id.as_deref().unwrap_or("-"),
                )?;
            }
        }
        Format::Plain => {
            for transaction in transactions {
                writeln!(out, "{}", transaction.id)?;
            }
        }
    }

    Ok(())
}

fn check(
    options: &txp::Options,
    fixtures: Option<&Path>,
    only: Option<&str>,
    verbose: bool,
    json: bool,
) -> Result<i32> {
    let (parser, discovery) = load(options)?;
    let fixtures_root = fixtures.unwrap_or(Path::new("fixtures"));

    let mut failures: Vec<String> = Vec::new();
    let mut checked = 0usize;

    for (index, entry) in parser.registry().parsers.iter().enumerate() {
        let id = &entry.spec.id;
        if let Some(only) = only {
            if only != id {
                continue;
            }
        }

        let fixtures = fixtures_root.join(id);
        let files = files_with_extension(&fixtures, "eml");

        if files.is_empty() {
            failures.push(format!(
                "{id}: no fixtures in {} (spec: {})",
                fixtures.display(),
                entry.source_file.display()
            ));
            continue;
        }

        for file in files {
            checked += 1;
            let bytes = std::fs::read(&file)?;
            let email = txp::parse_eml(&bytes, Some(file.display().to_string()))?;

            let result = parser.registry().route(&email);
            if result.parser != Some(index) {
                let actual = result
                    .parser
                    .map(|found| parser.registry().parsers[found].spec.id.as_str())
                    .unwrap_or("none");
                failures.push(format!(
                    "{id}: {} routed to {actual} ({})",
                    file.display(),
                    result.reason.as_str()
                ));
                continue;
            }

            match parser.registry().run(&email) {
                Outcome::Parsed(transaction) => {
                    if !transaction.warnings.is_empty() {
                        failures.push(format!(
                            "{id}: {}: {}",
                            file.display(),
                            transaction.warnings.join("; ")
                        ));
                    } else if verbose {
                        println!(
                            "ok   {id}: {:<14} {} {} {} {}",
                            file.file_name()
                                .and_then(|name| name.to_str())
                                .unwrap_or("?"),
                            transaction.occurred_on,
                            money(&transaction.amount),
                            transaction.kind.as_str(),
                            transaction.counterparty.as_deref().unwrap_or("-")
                        );
                    }
                }
                Outcome::Failed { error, .. } => {
                    failures.push(format!("{id}: {}: {error}", file.display()))
                }
                Outcome::Unmatched(reason) => {
                    failures.push(format!("{id}: {}: {reason}", file.display()))
                }
            }
        }
    }

    // A fixture directory with no parser means the spec was removed or renamed.
    for entry in std::fs::read_dir(fixtures_root)
        .into_iter()
        .flatten()
        .flatten()
    {
        if !entry.path().is_dir() {
            continue;
        }
        let id = entry.file_name().to_string_lossy().to_string();
        if parser.registry().find(&id).is_none() {
            failures.push(format!("{id}: fixture directory has no parser"));
        }
    }

    if json {
        let report = serde_json::json!({
            "schema_version": txp::SCHEMA_VERSION,
            "checked": checked,
            "parsers": parser.registry().parsers.len(),
            "dirs": discovery.dirs,
            "failures": failures,
            "ok": failures.is_empty(),
        });
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else if failures.is_empty() {
        println!("ok: {checked} fixtures parsed and routed correctly");
    } else {
        for failure in &failures {
            println!("FAIL {failure}");
        }
    }

    Ok(if failures.is_empty() {
        0
    } else {
        EXIT_UNMATCHED
    })
}

fn corpus(parser: &txp::Parser, dir: &Path) -> Result<i32> {
    let results = parser
        .parse_path(dir)
        .with_context(|| format!("parsing {}", dir.display()))?;

    let mut by_parser: BTreeMap<String, (usize, i64)> = BTreeMap::new();
    let mut unmatched = Vec::new();
    let mut failed = Vec::new();
    let mut warnings = Vec::new();
    let mut ids: BTreeMap<String, PathBuf> = BTreeMap::new();
    let mut duplicates = 0usize;

    for (file, outcome) in &results {
        if let Outcome::Parsed(transaction) = outcome {
            if let Some(message_id) = &transaction.source.message_id {
                if ids.insert(message_id.clone(), file.clone()).is_some() {
                    duplicates += 1;
                }
            }
            let entry = by_parser
                .entry(transaction.parser.clone())
                .or_insert((0, 0));
            entry.0 += 1;
            entry.1 += transaction.amount.amount_minor;
            for warning in &transaction.warnings {
                warnings.push(format!("{}: {warning}", file.display()));
            }
        }
    }

    for (file, outcome) in &results {
        match outcome {
            Outcome::Unmatched(reason) => unmatched.push(format!("{}: {reason}", file.display())),
            Outcome::Failed { parser, error } => {
                failed.push(format!("{}: {parser}: {error}", file.display()))
            }
            Outcome::Parsed(transaction) => {
                if let Some(route) = parser
                    .registry()
                    .route(&read_email(file)?)
                    .parser
                    .map(|index| parser.registry().parsers[index].spec.id.clone())
                {
                    debug_assert_eq!(route, transaction.parser);
                }
            }
        }
    }

    println!("corpus: {} emails", results.len());
    println!();
    println!("{:<26} {:>6} {:>18}", "PARSER", "COUNT", "TOTAL (minor)");
    for (id, (count, total)) in &by_parser {
        println!("{id:<26} {count:>6} {total:>18}");
    }
    for (title, lines) in [
        ("unmatched", &unmatched),
        ("failed", &failed),
        ("warnings", &warnings),
    ] {
        if lines.is_empty() {
            continue;
        }
        println!("\n{title} ({}):", lines.len());
        for line in lines {
            println!("  {line}");
        }
    }
    if duplicates > 0 {
        println!("\nduplicate Message-IDs: {duplicates}");
    }

    Ok(if unmatched.is_empty() && failed.is_empty() {
        0
    } else {
        EXIT_UNMATCHED
    })
}

fn paths(options: &txp::Options, json: bool) -> Result<i32> {
    let discovery = txp::discover(options);
    let env = std::env::var(txp::ENV_PARSERS)
        .ok()
        .filter(|value| !value.is_empty());

    if json {
        let candidates: Vec<_> = discovery
            .candidates
            .iter()
            .map(|candidate| {
                serde_json::json!({
                    "path": candidate.path,
                    "source": candidate.source,
                    "exists": candidate.exists,
                    "has_specs": candidate.has_specs,
                    "used": discovery.dirs.contains(&candidate.path),
                    "status": discovery.status(candidate),
                })
            })
            .collect();

        let report = serde_json::json!({
            "candidates": candidates,
            "dirs": discovery.dirs,
            "router": discovery.router,
            "router_source": discovery.router_source,
            "env_parsers": txp::ENV_PARSERS,
            "env_parsers_value": env,
            "env_router": txp::ENV_ROUTER,
            "note": discovery.note(),
        });
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        println!("{}", discovery.describe());

        match &env {
            Some(value) => println!(
                "\nenv: {}={value} ({})",
                txp::ENV_PARSERS,
                if discovery.dirs.len() > 1 {
                    "merged"
                } else {
                    "used"
                }
            ),
            None => println!("\nenv: {} unset", txp::ENV_PARSERS),
        }

        match &discovery.router {
            Some(router) => println!(
                "router: {} ({})",
                router.display(),
                discovery.router_source.unwrap_or("?")
            ),
            None => println!("router: built-in defaults"),
        }

        if let Some(note) = discovery.note() {
            println!("{note}");
        }
    }

    Ok(if discovery.dirs.is_empty() {
        EXIT_UNMATCHED
    } else {
        0
    })
}

fn specs(parser: &txp::Parser, format: Format) -> Result<i32> {
    let summaries = parser.registry().summaries();

    match format {
        Format::Json => println!("{}", serde_json::to_string_pretty(&summaries)?),
        Format::Table => {
            println!("{:<26} {:>3}  {:<16} FILE", "PARSER", "VER", "ACCOUNT");
            for summary in &summaries {
                println!(
                    "{:<26} {:>3}  {:<16} {}",
                    summary.id,
                    summary.version,
                    summary.account,
                    summary.file.display()
                );
            }
        }
        _ => {
            for summary in &summaries {
                println!("{}\t{}", summary.id, summary.file.display());
            }
        }
    }

    Ok(0)
}

fn senders(parser: &txp::Parser, format: Format) -> Result<i32> {
    let mut senders: BTreeSet<String> = BTreeSet::new();
    for summary in parser.registry().summaries() {
        senders.extend(summary.senders);
    }

    match format {
        Format::Json => println!("{}", serde_json::to_string_pretty(&senders)?),
        _ => {
            for sender in &senders {
                println!("{sender}");
            }
        }
    }

    Ok(0)
}

fn init(id: &str, parsers_root: Option<&Path>, fixtures_root: Option<&Path>) -> Result<i32> {
    let (bank, _kind) = id
        .split_once('.')
        .with_context(|| format!("parser id must look like `bank.type`, got `{id}`"))?;

    let parsers_dir = parsers_root.unwrap_or(Path::new("parsers"));
    let fixture_dir = fixtures_root.unwrap_or(Path::new("fixtures")).join(id);

    let spec_path = parsers_dir.join(format!("{id}.toml"));
    if spec_path.exists() {
        bail!("{} already exists", spec_path.display());
    }

    std::fs::create_dir_all(parsers_dir)?;
    std::fs::create_dir_all(&fixture_dir)?;

    let template = format!(
        r#"spec_version = 1
id = "{id}"
version = 1

[match]
senders  = ["TODO@example.com"]
subject  = '^TODO subject$'
requires = []
rejects  = []

[labels]
# Every constant label block in this template, in document order.
# Run `txp dump <fixture>` to see the blocks.
list = []

[defaults]
bank     = "{bank}"
account  = "{bank}-todo"
currency = "IDR"
tz       = "Asia/Jakarta"
kind     = "purchase"

[extract.amount]
label  = "TODO"
regex  = '(?P<value>[0-9][0-9.,]*(?:\s*,\d{{2}})?)'
number = "id"

[extract.occurred_at]
label   = "TODO"
regex   = '(?P<value>\d{{1,2}} \w{{3}} \d{{4}} \d{{2}}:\d{{2}}:\d{{2}})'
formats = ["%d %b %Y %H:%M:%S"]
locale  = "id"

[extract.counterparty]
label = "TODO"
"#
    );

    std::fs::write(&spec_path, template)?;
    std::fs::write(
        fixture_dir.join("README.md"),
        "Drop redacted `.eml` files here. Run `txp check --parser <id>` afterwards.\n",
    )?;

    println!("created {}", spec_path.display());
    println!("created {}", fixture_dir.display());
    println!("next: txp dump <fixture>.eml, then fill in the TODOs");
    Ok(0)
}

fn infer(dir: &Path, id: Option<&str>) -> Result<i32> {
    let files = files_with_extension(dir, "eml");
    if files.is_empty() {
        bail!("no .eml files under {}", dir.display());
    }

    let mut emails = Vec::new();
    for file in &files {
        let bytes = std::fs::read(file)?;
        emails.push(txp::parse_eml(&bytes, Some(file.display().to_string()))?);
    }

    let id = id.unwrap_or("bank.type").to_string();
    let (bank, _kind) = id.split_once('.').unwrap_or((id.as_str(), "type"));

    let senders: BTreeSet<&str> = emails
        .iter()
        .map(|email| email.from.email.as_str())
        .collect();
    let subjects: BTreeSet<&str> = emails.iter().map(|email| email.subject.as_str()).collect();

    let counts: BTreeSet<usize> = emails.iter().map(|email| email.blocks.len()).collect();
    if counts.len() > 1 {
        eprintln!(
            "warning: samples have different block counts {:?}; using the first file's shape",
            counts
        );
    }

    let width = emails[0].blocks.len().min(
        emails
            .iter()
            .map(|email| email.blocks.len())
            .min()
            .unwrap_or(0),
    );

    let mut labels = Vec::new();
    let mut prose = Vec::new();
    let mut varying: Vec<usize> = Vec::new();
    for index in 0..width {
        let first = &emails[0].blocks[index].text;
        if emails
            .iter()
            .all(|email| email.blocks.get(index).map(|block| &block.text) == Some(first))
        {
            if is_label_like(first) {
                labels.push(first.clone());
            } else {
                prose.push(first.clone());
            }
        } else {
            varying.push(index);
        }
    }

    let amount_label = guess_label(&labels, &emails, is_money);
    let date_label = guess_label(&labels, &emails, is_datetime);
    let split_date = find_label(&labels, &["date"]);
    let split_time = find_label(&labels, &["time"]);
    let kind = if labels
        .iter()
        .any(|label| label.to_ascii_lowercase().contains("transfer"))
    {
        "transfer_out"
    } else {
        "purchase"
    };
    let subject = if subjects.len() == 1 {
        format!(
            "'^{}'",
            regex_escape(subjects.iter().next().copied().unwrap_or_default())
        )
    } else {
        "'^TODO'  # subjects differ; anchor on a common prefix".to_string()
    };

    println!(
        "# draft generated from {} email(s) in {}",
        emails.len(),
        dir.display()
    );
    println!("spec_version = 1");
    println!("id = \"{id}\"");
    println!("version = 1");
    println!();
    println!("[match]");
    println!(
        "senders  = [{}]",
        senders
            .iter()
            .map(|sender| format!("\"{sender}\""))
            .collect::<Vec<_>>()
            .join(", ")
    );
    println!("subject  = {subject}");
    println!("requires = []  # add labels that distinguish this from sibling templates");
    println!("rejects  = []");
    println!();
    println!("[labels]");
    println!("list = [");
    for label in &labels {
        println!("  \"{}\",", label.replace('"', "\\\""));
    }
    println!("]");
    println!();
    println!("[defaults]");
    println!("bank     = \"{bank}\"");
    println!("account  = \"{bank}-todo\"");
    println!("currency = \"IDR\"");
    println!("tz       = \"Asia/Jakarta\"");
    println!("kind     = \"{kind}\"");
    println!();
    println!("[extract.amount]");
    println!("label  = \"{}\"", amount_label.as_deref().unwrap_or("TODO"));
    println!("regex  = '(?P<value>[0-9][0-9.,]*(?:\\s*,\\d{{2}})?)'");
    println!(
        "number = \"{}\"",
        detect_number_style(&emails, amount_label.as_deref())
    );
    println!();
    println!("[extract.occurred_at]");
    if let (Some(date), Some(time)) = (split_date, split_time) {
        println!(
            "date = {{ label = \"{date}\", regex = '(?P<value>\\d{{1,2}} \\w{{3}} \\d{{4}})', formats = [\"%d %b %Y\"], locale = \"id\" }}"
        );
        println!(
            "time = {{ label = \"{time}\", regex = '(?P<value>\\d{{2}}:\\d{{2}}:\\d{{2}})', formats = [\"%H:%M:%S\"] }}"
        );
    } else {
        println!("label   = \"{}\"", date_label.as_deref().unwrap_or("TODO"));
        print!("{}", detect_date_block(&emails, date_label.as_deref()));
    }
    println!();
    println!("[extract.counterparty]");
    println!(
        "label = \"{}\"",
        labels
            .iter()
            .find(|label| {
                matches!(
                    label.to_ascii_lowercase().as_str(),
                    "merchant"
                        | "payment to"
                        | "recipient"
                        | "beneficiary name"
                        | "company/product name"
                        | "description"
                )
            })
            .cloned()
            .unwrap_or_else(|| "TODO".to_string())
    );
    println!();
    println!(
        "# {} block(s) vary and were not mapped; inspect `txp dump`: {}",
        varying.len(),
        varying
            .iter()
            .take(12)
            .map(|index| index.to_string())
            .collect::<Vec<_>>()
            .join(", ")
    );
    if !prose.is_empty() {
        println!("# constant non-label blocks (left out of labels.list):");
        for line in prose.iter().take(4) {
            println!("#   {}", line.chars().take(70).collect::<String>());
        }
    }
    println!("# NOTE: this is a draft. Run `txp check --parser {id}` after adding fixtures.");

    Ok(0)
}

fn guess_label(
    labels: &[String],
    emails: &[txp_core::NormalizedEmail],
    predicate: fn(&str) -> bool,
) -> Option<String> {
    for label in labels {
        let mut found = 0usize;
        for email in emails {
            let Some(index) = email
                .blocks
                .iter()
                .position(|block| block.text.eq_ignore_ascii_case(label))
            else {
                continue;
            };
            let next_is_label = email
                .blocks
                .get(index + 1)
                .map(|block| labels.iter().any(|l| l.eq_ignore_ascii_case(&block.text)))
                .unwrap_or(true);
            if next_is_label {
                continue;
            }
            if let Some(value) = email.blocks.get(index + 1) {
                if predicate(&value.text) {
                    found += 1;
                }
            }
        }
        if found == emails.len() {
            return Some(label.clone());
        }
    }
    None
}

fn is_label_like(text: &str) -> bool {
    let trimmed = text.trim();
    let words = trimmed.split_whitespace().count();

    if trimmed.chars().count() > 48 || words > 6 {
        return false;
    }
    if words >= 4 && matches!(trimmed.chars().last(), Some(':' | ',' | '.')) {
        return false;
    }
    trimmed.chars().any(|ch| ch.is_ascii_alphabetic())
}

fn find_label(labels: &[String], candidates: &[&str]) -> Option<String> {
    labels
        .iter()
        .find(|label| {
            let lower = label.to_ascii_lowercase();
            candidates.iter().any(|candidate| lower == *candidate)
        })
        .cloned()
}

fn is_money(text: &str) -> bool {
    let trimmed = text.trim();
    let digits = trimmed.chars().filter(|ch| ch.is_ascii_digit()).count();
    digits >= 3
        && trimmed.chars().all(|ch| {
            ch.is_ascii_digit() || matches!(ch, '.' | ',' | ' ' | '-' | 'R' | 'p' | 'I' | 'D' | 'P')
        })
}

fn is_datetime(text: &str) -> bool {
    let has_time = text.matches(':').count() >= 2;
    let has_date = text.chars().filter(|ch| ch.is_ascii_digit()).count() >= 6
        && (text.contains('-') || text.contains('/') || text.chars().any(|ch| ch.is_alphabetic()));
    has_time && has_date
}

fn detect_number_style(emails: &[txp_core::NormalizedEmail], label: Option<&str>) -> &'static str {
    let Some(label) = label else {
        return "id";
    };

    for email in emails {
        for (index, block) in email.blocks.iter().enumerate() {
            if block.text.eq_ignore_ascii_case(label) {
                if let Some(value) = email.blocks.get(index + 1) {
                    let text = &value.text;
                    if let (Some(comma), Some(dot)) = (text.rfind(','), text.rfind('.')) {
                        return if comma > dot { "id" } else { "en" };
                    }
                }
            }
        }
    }

    "id"
}

fn detect_date_block(emails: &[txp_core::NormalizedEmail], label: Option<&str>) -> String {
    let mut format = "%d %b %Y %H:%M:%S".to_string();

    if let Some(label) = label {
        for email in emails {
            for (index, block) in email.blocks.iter().enumerate() {
                if block.text.eq_ignore_ascii_case(label) {
                    if let Some(value) = email.blocks.get(index + 1) {
                        format = guess_format(&value.text);
                    }
                }
            }
        }
    }

    format!(
        "regex   = '(?P<value>\\d{{1,2}} \\w{{3}} \\d{{4}} \\d{{2}}:\\d{{2}}:\\d{{2}})'\nformats = [\"{format}\"]\nlocale  = \"id\""
    )
}

fn guess_format(value: &str) -> String {
    if value.contains('-') && value.chars().filter(|ch| *ch == '-').count() >= 4 {
        "%d-%m-%Y %H:%M:%S".to_string()
    } else if value.contains('/') {
        "%d/%m/%Y %H:%M:%S".to_string()
    } else {
        "%d %b %Y %H:%M:%S".to_string()
    }
}

fn regex_escape(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for ch in value.chars() {
        if "\\^$.*+?()[]{}|".contains(ch) {
            escaped.push('\\');
        }
        escaped.push(ch);
    }
    escaped
}

fn money(money: &Money) -> String {
    let exponent = money.exponent();
    let sign = if money.amount_minor < 0 { "-" } else { "" };
    let absolute = money.amount_minor.unsigned_abs();

    if exponent == 0 {
        format!("{sign}{absolute} {}", money.currency)
    } else {
        let factor = 10u64.pow(exponent);
        format!(
            "{sign}{}.{:0width$} {}",
            absolute / factor,
            absolute % factor,
            money.currency,
            width = exponent as usize
        )
    }
}

fn truncate(value: &str, width: usize) -> String {
    if value.chars().count() <= width {
        value.to_string()
    } else {
        let shortened: String = value.chars().take(width.saturating_sub(1)).collect();
        format!("{shortened}…")
    }
}

fn files_with_extension(dir: &Path, extension: &str) -> Vec<PathBuf> {
    let mut files = Vec::new();
    collect_files(dir, extension, &mut files);
    files.sort();
    files
}

fn collect_files(dir: &Path, extension: &str, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_files(&path, extension, out);
        } else if path.extension().and_then(|value| value.to_str()) == Some(extension) {
            out.push(path);
        }
    }
}
