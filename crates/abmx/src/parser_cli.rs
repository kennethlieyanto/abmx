//! The `abmx parser` command line interface.
//!
//! Contract: transactions go to stdout, diagnostics go to stderr, and the exit
//! code tells a pipeline whether to trust the result.
//!
//!   0  every input produced a strict transaction
//!   2  at least one email failed to parse
//!   3  at least one transaction was heuristic (unless --allow-heuristic)
//!   1  the tool itself failed

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use abmx_parser::Outcome;
use anyhow::{bail, Context, Result};
use clap::{Args, Subcommand, ValueEnum};

const EXIT_FAILED: i32 = 2;
const EXIT_HEURISTIC: i32 = 3;

#[derive(Args)]
pub struct ParserCli {
    /// Parser directory. Repeat to layer a local override over a base corpus.
    #[arg(long, global = true, action = clap::ArgAction::Append)]
    parsers: Vec<PathBuf>,

    #[command(subcommand)]
    command: ParserCommand,
}

#[derive(Subcommand)]
pub enum ParserCommand {
    /// Parse a `.eml` file or every `.eml` under a directory.
    Parse {
        path: PathBuf,

        /// Parser id. Labels choose this in `abmx receipt sync`; here it is explicit.
        #[arg(long, value_name = "ID")]
        parser: String,

        #[arg(long, value_enum, default_value_t = Format::Json)]
        format: Format,

        /// Exit 0 even when heuristic transactions are present.
        #[arg(long)]
        allow_heuristic: bool,
    },

    /// Print the JSON Schema of the transaction contract.
    Schema,

    /// Show the parser search order and which directory won.
    Paths {
        #[arg(long)]
        json: bool,
    },

    /// List the loaded parsers and the file each came from.
    Specs {
        #[arg(long, value_enum, default_value_t = Format::Table)]
        format: Format,
    },

    /// Verify every mail parser against its fixtures.
    Check {
        #[arg(long)]
        parser: Option<String>,

        #[arg(long, short)]
        verbose: bool,

        #[arg(long)]
        json: bool,
    },

    /// Show the block regions a mail parser will see, without guessing.
    Explain {
        path: PathBuf,

        #[arg(long, value_name = "ID")]
        parser: String,
    },

    /// Static checks: labels covered by fixtures, fixtures present.
    Lint {
        #[arg(long, value_name = "ID")]
        parser: Option<String>,
    },

    /// Scaffold <id>/parser.rhai and <id>/fixtures/ in the user corpus.
    Init {
        /// Parser id, e.g. example-qris. It becomes the directory name.
        id: String,

        /// Which entry point to scaffold.
        #[arg(long, value_enum, default_value_t = InitKind::Mail)]
        kind: InitKind,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum Format {
    Json,
    Ndjson,
    Table,
    Plain,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum InitKind {
    Mail,
    Statement,
}

pub fn run(cli: ParserCli) -> Result<i32> {
    let ParserCli { parsers, command } = cli;
    let options = abmx_parser::Options { parsers };

    match command {
        ParserCommand::Parse {
            path,
            parser,
            format,
            allow_heuristic,
        } => {
            let (corpus, _) = load(&options)?;
            parse(&corpus, &parser, &path, format, allow_heuristic)
        }
        ParserCommand::Schema => {
            println!("{}", serde_json::to_string_pretty(&abmx_parser::schema())?);
            Ok(0)
        }
        ParserCommand::Paths { json } => paths(&options, json),
        ParserCommand::Specs { format } => {
            let (corpus, _) = load(&options)?;
            specs(&corpus, format)
        }
        ParserCommand::Check {
            parser,
            verbose,
            json,
        } => check(&options, parser.as_deref(), verbose, json),
        ParserCommand::Explain { path, parser } => {
            let (corpus, _) = load(&options)?;
            explain(&corpus, &parser, &path)
        }
        ParserCommand::Lint { parser } => lint(&options, parser.as_deref()),
        ParserCommand::Init { id, kind } => {
            init(&id, kind, options.parsers.first().map(PathBuf::as_path))
        }
    }
}

fn load(options: &abmx_parser::Options) -> Result<(abmx_parser::Parser, abmx_parser::Discovery)> {
    let (parser, discovery) = abmx_parser::Parser::load(options).map_err(|error| match error {
        abmx_parser::Error::NoCorpus { tried } => {
            let tried = if tried.is_empty() {
                "~/.config/abmx/parsers, /etc/abmx/parsers".to_string()
            } else {
                tried
                    .iter()
                    .map(|path| path.display().to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            };
            anyhow::anyhow!("no parser scripts found; looked in {tried}")
        }
        other => anyhow::anyhow!("{other}"),
    })?;
    Ok((parser, discovery))
}

fn parse(
    corpus: &abmx_parser::Parser,
    id: &str,
    path: &Path,
    format: Format,
    allow_heuristic: bool,
) -> Result<i32> {
    let results = corpus
        .parse_path_with(id, path, "cli")
        .with_context(|| format!("parsing {}", path.display()))?;

    let mut heuristic = 0usize;
    let mut failed = 0usize;

    for (file, outcome) in &results {
        match outcome {
            Outcome::Parsed(transactions) => {
                for transaction in transactions {
                    if transaction.extraction == abmx_parser::Extraction::Heuristic {
                        heuristic += 1;
                    }
                    match format {
                        Format::Json => println!("{}", serde_json::to_string_pretty(transaction)?),
                        Format::Ndjson => println!("{}", serde_json::to_string(transaction)?),
                        Format::Table => println!(
                            "{:<8} {:<20} {:>12} {}",
                            transaction.occurred_on,
                            transaction.counterparty.as_deref().unwrap_or("-"),
                            transaction.amount.decimal(),
                            transaction.amount.currency
                        ),
                        Format::Plain => println!("{}", serde_json::to_string(transaction)?),
                    }
                }
            }
            Outcome::Missing(parser) => {
                failed += 1;
                eprintln!("{}: parser `{parser}` is not in the corpus", file.display());
            }
            Outcome::Failed { parser, error } => {
                failed += 1;
                eprintln!("{}: {parser}: {error}", file.display());
            }
        }
    }

    if failed > 0 {
        Ok(EXIT_FAILED)
    } else if heuristic > 0 && !allow_heuristic {
        Ok(EXIT_HEURISTIC)
    } else {
        Ok(0)
    }
}

fn check(
    options: &abmx_parser::Options,
    only: Option<&str>,
    verbose: bool,
    json: bool,
) -> Result<i32> {
    let (corpus, _) = load(options)?;
    let ids: Vec<&str> = corpus
        .registry()
        .mail_parsers()
        .map(|script| script.id.as_str())
        .filter(|id| only.is_none_or(|only| only == *id))
        .collect();

    if ids.is_empty() {
        bail!("no mail parser matched --parser");
    }

    let mut rows = Vec::new();
    let mut strict = 0usize;
    let mut heuristic = 0usize;
    let mut failures = 0usize;

    for id in ids {
        let Some(script) = corpus.registry().find(id) else {
            continue;
        };
        let fixtures = abmx_parser::engine::fixtures_dir(script);
        if !fixtures.is_dir() {
            failures += 1;
            rows.push(serde_json::json!({
                "parser": id,
                "fixtures": fixtures.display().to_string(),
                "status": "missing_fixtures",
            }));
            continue;
        }
        let results = corpus
            .parse_path_with(id, &fixtures, "check")
            .with_context(|| format!("parsing {}", fixtures.display()))?;
        for (file, outcome) in results {
            let (status, detail) = match outcome {
                Outcome::Parsed(transactions) => {
                    let mut detail = Vec::new();
                    let mut status = "strict";
                    for transaction in transactions {
                        match transaction.extraction {
                            abmx_parser::Extraction::Strict => strict += 1,
                            abmx_parser::Extraction::Heuristic => {
                                heuristic += 1;
                                status = "heuristic";
                            }
                            abmx_parser::Extraction::Failed => {
                                failures += 1;
                                status = "failed";
                            }
                        }
                        detail.extend(transaction.warnings);
                    }
                    (status, detail.join("; "))
                }
                Outcome::Missing(parser) => {
                    failures += 1;
                    ("failed", format!("parser `{parser}` missing"))
                }
                Outcome::Failed { error, .. } => {
                    failures += 1;
                    ("failed", error)
                }
            };
            if verbose && detail.is_empty() {
                eprintln!("{}: {id} -> {status}", file.display());
            } else if !detail.is_empty() {
                eprintln!("{}: {id} -> {status}: {detail}", file.display());
            }
            rows.push(serde_json::json!({
                "parser": id,
                "file": file.display().to_string(),
                "status": status,
                "detail": detail,
            }));
        }
    }

    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "strict": strict,
                "heuristic": heuristic,
                "failures": failures,
                "samples": rows,
            }))?
        );
    } else if failures == 0 && heuristic == 0 {
        println!("ok: {strict} fixtures parsed and routed correctly");
    } else {
        println!("{strict} strict, {heuristic} heuristic, {failures} failure(s)");
    }

    Ok(if failures > 0 {
        EXIT_FAILED
    } else if heuristic > 0 {
        EXIT_HEURISTIC
    } else {
        0
    })
}

fn paths(options: &abmx_parser::Options, json: bool) -> Result<i32> {
    let discovery = abmx_parser::discover(options);
    if json {
        let rows: Vec<serde_json::Value> = discovery
            .candidates
            .iter()
            .map(|candidate| {
                serde_json::json!({
                    "path": candidate.path.display().to_string(),
                    "source": candidate.source,
                    "exists": candidate.exists,
                    "has_specs": candidate.has_specs,
                    "status": discovery.status(candidate),
                })
            })
            .collect();
        println!("{}", serde_json::to_string_pretty(&rows)?);
    } else {
        println!("{}", discovery.describe());
        if let Some(note) = discovery.note() {
            println!("{note}");
        }
    }
    Ok(0)
}

fn specs(corpus: &abmx_parser::Parser, format: Format) -> Result<i32> {
    match format {
        Format::Json => println!(
            "{}",
            serde_json::to_string_pretty(&corpus.registry().summaries())?
        ),
        Format::Ndjson => {
            for summary in corpus.registry().summaries() {
                println!("{}", serde_json::to_string(&summary)?);
            }
        }
        Format::Table => {
            println!(
                "{:<24} {:<4} {:<10} {:<10} kind",
                "parser", "ver", "bank", "input"
            );
            for summary in corpus.registry().summaries() {
                println!(
                    "{:<24} {:<4} {:<10} {:<10} {:?}",
                    summary.id, summary.version, summary.bank, summary.input, summary.kind
                );
            }
        }
        Format::Plain => {
            for summary in corpus.registry().summaries() {
                println!("{}", summary.id);
            }
        }
    }
    Ok(0)
}

fn explain(corpus: &abmx_parser::Parser, id: &str, path: &Path) -> Result<i32> {
    let Some(script) = corpus.registry().find(id) else {
        bail!("no parser `{id}` in the corpus");
    };
    if script.input() != abmx_parser::InputKind::Mail {
        bail!("`{id}` is a statement parser; explain works on mail parsers");
    }

    let bytes = std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;
    let email = abmx_parser::parse_eml(&bytes, Some(path.display().to_string()))?;
    let regions = abmx_parser::mail::build_regions(&email.blocks, &script.meta.labels);

    println!("file:    {}", path.display());
    println!("parser:  {id}");
    println!("bank:    {}", script.meta.bank);
    println!("kind:    {:?}", script.meta.kind);
    println!("from:    {}", email.from.email);
    println!("subject: {}", email.subject);
    println!("date:    {}", email.date.to_rfc3339());
    println!();

    for label in &script.meta.labels {
        match regions.get(&abmx_parser::norm(label)) {
            Some(blocks) => println!("{:<28} [{}]", label, blocks.join(" · ")),
            None => println!("{:<28} (label not present)", label),
        }
    }

    Ok(0)
}

fn lint(options: &abmx_parser::Options, only: Option<&str>) -> Result<i32> {
    let (corpus, _) = load(options)?;
    let mut problems = 0usize;

    for script in &corpus.registry().parsers {
        if let Some(only) = only {
            if script.id != only {
                continue;
            }
        }

        let fixtures = abmx_parser::engine::fixtures_dir(script);
        if !fixtures.is_dir() {
            problems += 1;
            eprintln!("{}: no fixtures directory", script.id);
            continue;
        }

        match script.input() {
            abmx_parser::InputKind::Mail => {
                let mut seen: BTreeSet<String> = BTreeSet::new();
                let mut files = Vec::new();
                collect_eml(&fixtures, &mut files);
                if files.is_empty() {
                    problems += 1;
                    eprintln!("{}: no .eml fixtures", script.id);
                    continue;
                }
                for file in files {
                    let Ok(bytes) = std::fs::read(&file) else {
                        continue;
                    };
                    let Ok(email) =
                        abmx_parser::parse_eml(&bytes, Some(file.display().to_string()))
                    else {
                        continue;
                    };
                    for block in &email.blocks {
                        seen.insert(abmx_parser::norm(&block.text));
                    }
                }
                for label in &script.meta.labels {
                    if !seen.contains(&abmx_parser::norm(label)) {
                        eprintln!(
                            "{}: meta.labels contains `{label}`, which never appears in fixtures",
                            script.id
                        );
                    }
                }
            }
            abmx_parser::InputKind::Statement => {
                let pdfs = std::fs::read_dir(&fixtures)
                    .map(|entries| {
                        entries.flatten().any(|entry| {
                            entry
                                .path()
                                .extension()
                                .and_then(|ext| ext.to_str())
                                .is_some_and(|ext| ext.eq_ignore_ascii_case("pdf"))
                        })
                    })
                    .unwrap_or(false);
                if !pdfs {
                    problems += 1;
                    eprintln!("{}: no .pdf fixtures", script.id);
                }
            }
        }
    }

    if problems == 0 {
        println!("ok: no problems");
    } else {
        println!("{problems} problem(s)");
    }
    Ok(if problems > 0 { EXIT_FAILED } else { 0 })
}

fn collect_eml(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_eml(&path, out);
        } else if path.extension().and_then(|ext| ext.to_str()) == Some("eml") {
            out.push(path);
        }
    }
}

fn init(id: &str, kind: InitKind, first: Option<&Path>) -> Result<i32> {
    if !abmx_parser::is_parser_id(id) {
        bail!("`{id}` is not a valid parser id");
    }
    let corpus = first
        .map(Path::to_path_buf)
        .unwrap_or_else(crate::paths::default_parsers_dir);
    let dir = corpus.join(id);
    let fixtures = dir.join("fixtures");
    std::fs::create_dir_all(&fixtures)
        .with_context(|| format!("creating {}", fixtures.display()))?;

    let script = match kind {
        InitKind::Mail => MAIL_TEMPLATE,
        InitKind::Statement => STATEMENT_TEMPLATE,
    };

    let path = dir.join(abmx_parser::SCRIPT_FILE);
    std::fs::write(&path, script).with_context(|| format!("writing {}", path.display()))?;
    println!("wrote {}", path.display());
    Ok(0)
}

const MAIL_TEMPLATE: &str = r#"// A parser is a directory: <corpus>/<id>/parser.rhai.
// Replace the labels, then extract each field with a ctx helper.
// Run `abmx parser explain <file.eml> --parser <id>` to see the regions.

fn meta() {
    #{
        bank: "",                 // e.g. "example"
        currency: "IDR",
        tz: "Asia/Jakarta",
        offset: "+07:00",
        number: "id",             // id: 1.234,56   en: 1,234.56
        kind: "purchase",         // purchase | transfer_out | transfer_in | refund | fee
        labels: [],               // every constant label block in the template
        api: 1,
        version: 1,
    }
}

fn parse_mail(ctx) {
    #{
        amount: ctx.money_in("Total Payment", "id"),
        occurred_at: ctx.datetime_in("Transaction Date", ["%d %b %Y %H:%M:%S"], "id"),
        counterparty: ctx.text_in("Merchant"),
        // optional: counterparty_account, account_last4, reference_id,
        // channel, description, extra, status, kind, warnings
    }
}
"#;

const STATEMENT_TEMPLATE: &str = r#"// A statement parser receives positioned lines. `dump` shows them:
//   abmx statement dump <file.pdf>
// Rows are #{} maps; the host proves the arithmetic.

fn meta() {
    #{
        bank: "",
        currency: "IDR",
        tz: "Asia/Jakarta",
        offset: "+07:00",
        number: "en",
        api: 1,
        version: 1,
    }
}

fn parse_statement(doc) {
    let rows = [];
    for line in doc.lines() {
        if !line.matches(`^\d{2}/\d{2}$`, 35.0, 88.0) { continue; }
        rows.push(#{
            date: line.cell(35.0, 88.0),
            description: line.words(88.0, 380.0),
            amount: line.money(380.0, 435.0),
            out: line.cell(435.0, 460.0) == "DB",
            balance: line.money_any(500.0, 600.0),
        });
    }

    #{
        period: doc.month("PERIODE"),
        sections: [#{
            key: "savings",
            opening: doc.money(`SALDO AWAL\s*:\s*([\d.,]+)`),
            closing: doc.money(`SALDO AKHIR\s*:\s*([\d.,]+)`),
            rows: rows,
        }],
    }
}
"#;
