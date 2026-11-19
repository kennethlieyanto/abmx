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

use abmx_parse::Outcome;
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

        /// Parser id. Labels choose this in `abmx sync`; here it is explicit.
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

    /// Verify every parser against its fixtures.
    Check {
        #[arg(long)]
        parser: Option<String>,

        #[arg(long, short)]
        verbose: bool,

        #[arg(long)]
        json: bool,
    },

    /// Show the block regions a parser will see, without guessing.
    Explain {
        path: PathBuf,

        #[arg(long, value_name = "ID")]
        parser: String,
    },

    /// Static checks for the common spec mistakes.
    Lint {
        #[arg(long, value_name = "ID")]
        parser: Option<String>,
    },

    /// Scaffold <id>/config.toml and <id>/fixtures/ in the user corpus.
    Init {
        /// Parser id, e.g. example-qris. It becomes the directory name.
        id: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum Format {
    Json,
    Ndjson,
    Table,
    Plain,
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
        ParserCommand::Init { id } => init(&id, options.parsers.first().map(PathBuf::as_path)),
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
            anyhow::anyhow!("no parser specs found; looked in {tried}")
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
            Outcome::Parsed(transaction) => {
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
    let summaries = corpus.registry().summaries();
    let ids: Vec<&str> = summaries
        .iter()
        .map(|summary| summary.id.as_str())
        .filter(|id| only.is_none_or(|only| only == *id))
        .collect();

    if ids.is_empty() {
        bail!("no parser matched --parser");
    }

    let mut rows = Vec::new();
    let mut strict = 0usize;
    let mut heuristic = 0usize;
    let mut failures = 0usize;

    for id in ids {
        let Some(parser) = corpus.registry().find(id) else {
            continue;
        };
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
        let results = corpus
            .parse_path_with(id, &fixtures, "check")
            .with_context(|| format!("parsing {}", fixtures.display()))?;
        for (file, outcome) in results {
            let (status, detail) = match outcome {
                Outcome::Parsed(transaction) => {
                    let status = match transaction.extraction {
                        abmx_parser::Extraction::Strict => {
                            strict += 1;
                            "strict"
                        }
                        abmx_parser::Extraction::Heuristic => {
                            heuristic += 1;
                            "heuristic"
                        }
                        abmx_parser::Extraction::Failed => {
                            failures += 1;
                            "failed"
                        }
                    };
                    (status, transaction.warnings.join("; "))
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
            println!("{:<24} {:<4} {:<10} kind", "parser", "ver", "bank");
            for summary in corpus.registry().summaries() {
                println!(
                    "{:<24} {:<4} {:<10} {:?}",
                    summary.id, summary.version, summary.bank, summary.kind
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
    let Some(parser) = corpus.registry().find(id) else {
        bail!("no parser `{id}` in the corpus");
    };
    let bytes = std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;
    let email = abmx_parser::parse_eml(&bytes, Some(path.display().to_string()))?;
    let regions = abmx_parse::Regions::build(&email.blocks, &parser.spec.labels.list);

    println!("file:    {}", path.display());
    println!("parser:  {id}");
    println!("from:    {}", email.from.email);
    println!("subject: {}", email.subject);
    println!("date:    {}", email.date.to_rfc3339());
    println!();

    for label in &parser.spec.labels.list {
        match regions.get(label) {
            Some(blocks) => println!("{:<28} [{}]", label, blocks.join(" · ")),
            None => println!("{:<28} (label not present)", label),
        }
    }

    println!();
    println!("fields:");
    println!("  amount        <- {:?}", parser.spec.extract.amount.label);
    if let Some(label) = &parser.spec.extract.occurred_at.label {
        println!("  occurred_at   <- {label:?}");
    }
    if let Some(date) = &parser.spec.extract.occurred_at.date {
        println!("  occurred_at   <- date {:?}", date.label);
    }
    if let Some(counterparty) = &parser.spec.extract.counterparty {
        println!("  counterparty  <- {:?}", counterparty.label);
    }
    if let Some(list) = &parser.spec.status {
        println!("  status        <- {:?}", list.label);
    }

    Ok(0)
}

fn lint(options: &abmx_parser::Options, only: Option<&str>) -> Result<i32> {
    let (corpus, _) = load(options)?;
    let mut problems = 0usize;

    for parser in &corpus.registry().parsers {
        if let Some(only) = only {
            if parser.id != only {
                continue;
            }
        }

        let declared: BTreeSet<String> = parser
            .spec
            .labels
            .list
            .iter()
            .map(|label| abmx_parse::norm(label))
            .collect();

        let mut field_labels = vec![parser.spec.extract.amount.label.clone()];
        if let Some(label) = &parser.spec.extract.occurred_at.label {
            field_labels.push(label.clone());
        }
        if let Some(date) = &parser.spec.extract.occurred_at.date {
            field_labels.push(date.label.clone());
        }
        if let Some(time) = &parser.spec.extract.occurred_at.time {
            field_labels.push(time.label.clone());
        }
        for field in [
            &parser.spec.extract.counterparty,
            &parser.spec.extract.counterparty_account,
            &parser.spec.extract.channel,
            &parser.spec.extract.account,
            &parser.spec.extract.reference_id,
            &parser.spec.extract.description,
        ]
        .into_iter()
        .flatten()
        {
            field_labels.push(field.label.clone());
        }
        for field in parser.spec.extract.extra.values() {
            field_labels.push(field.label.clone());
        }
        if let Some(status) = &parser.spec.status {
            if let Some(label) = &status.label {
                field_labels.push(label.clone());
            }
        }

        for label in field_labels {
            if !declared.contains(&abmx_parse::norm(&label)) {
                problems += 1;
                eprintln!(
                    "{}: field label `{label}` is missing from labels.list",
                    parser.id
                );
            }
        }

        if parser
            .spec
            .extract
            .amount
            .regex
            .as_deref()
            .is_some_and(|regex| regex.trim_end().ends_with('$'))
        {
            problems += 1;
            eprintln!(
                "{}: amount regex is end-anchored; match the value shape instead",
                parser.id
            );
        }

        let fixtures = parser
            .source_file
            .parent()
            .map(|dir| dir.join("fixtures"))
            .unwrap_or_default();
        if !fixtures.is_dir() {
            problems += 1;
            eprintln!("{}: no fixtures directory", parser.id);
            continue;
        }

        let mut seen: BTreeSet<String> = BTreeSet::new();
        let mut files = Vec::new();
        collect_eml(&fixtures, &mut files);
        for file in files {
            let Ok(bytes) = std::fs::read(&file) else {
                continue;
            };
            let Ok(email) = abmx_parser::parse_eml(&bytes, Some(file.display().to_string())) else {
                continue;
            };
            for block in &email.blocks {
                seen.insert(abmx_parse::norm(&block.text));
            }
        }

        for label in &parser.spec.labels.list {
            if !seen.contains(&abmx_parse::norm(label)) {
                eprintln!(
                    "{}: labels.list contains `{label}`, which never appears in fixtures",
                    parser.id
                );
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

fn init(id: &str, first: Option<&Path>) -> Result<i32> {
    if !abmx_parse::is_parser_id(id) {
        bail!("`{id}` is not a valid parser id");
    }
    let corpus = first
        .map(Path::to_path_buf)
        .unwrap_or_else(crate::paths::default_parsers_dir);
    let dir = corpus.join(id);
    let fixtures = dir.join("fixtures");
    std::fs::create_dir_all(&fixtures)
        .with_context(|| format!("creating {}", fixtures.display()))?;

    let config = format!(
        "spec_version = {}\nversion = 1\nenabled = true\n\n\
         [labels]\nlist = []\n\n\
         [defaults]\nbank = \"\"\ncurrency = \"IDR\"\ntz = \"Asia/Jakarta\"\nkind = \"purchase\"\n\n\
         [extract.amount]\nlabel = \"\"\nregex = '(?P<value>[0-9][0-9.,]*)'\nnumber = \"id\"\n\n\
         [extract.occurred_at]\nlabel = \"\"\nregex = ''\nformats = []\n",
        abmx_parser::SPEC_VERSION
    );
    let path = dir.join("config.toml");
    std::fs::write(&path, config).with_context(|| format!("writing {}", path.display()))?;
    println!("wrote {}", path.display());
    Ok(0)
}
