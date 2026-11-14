//! Declarative bank parsers: TOML spec in, transactions out.
//!
//! The engine is fixed; a new bank or a new transaction type is one data file,
//! `parsers/<id>.toml`, where the file stem must equal the spec `id`.  Nothing
//! here performs I/O beyond reading spec files, so the parsing path is a pure
//! function and easy to test.

mod date;
mod field;
mod number;
mod router;
mod spec;

pub use router::{Candidate, Route, RouteReason, RouterConfig};
pub use spec::SpecSummary;
pub use spec::{
    Defaults, ExtractSpec, Field, LabelsSpec, MatchSpec, Normalize, NumberStyle, OccurredSpec,
    ParserSpec, StatusSource, StatusSpec, SubField,
};

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use abmx_tx::{currency_exponent, Block, Extraction, Money, NormalizedEmail, Status, Transaction};
use chrono::{NaiveDate, NaiveDateTime, TimeZone};
use regex::Regex;

use date::{parse_date, parse_datetime, parse_time};
use field::CompiledField;
use number::parse_decimal;

/// Version of the `parser.toml` format itself.
pub const SPEC_VERSION: u32 = 1;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("could not read {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("{path}: {message}")]
    Spec { path: PathBuf, message: String },
    #[error("no parser specs found under {0}")]
    NoParsers(PathBuf),
    #[error("{path}: spec id `{id}` does not match the file name `{expected}.toml`")]
    IdMismatch {
        path: PathBuf,
        id: String,
        expected: String,
    },
    #[error("{path}: duplicate parser id `{id}` (also in {first})")]
    DuplicateId {
        path: PathBuf,
        id: String,
        first: PathBuf,
    },
}

/// A parser found at lower precedence than another with the same id.
#[derive(Debug, Clone, serde::Serialize)]
pub struct Shadow {
    pub id: String,
    pub skipped: PathBuf,
    pub winner: PathBuf,
}

/// A spec with every regex compiled once at load time.
#[derive(Debug)]
pub struct Parser {
    pub spec: ParserSpec,
    /// The `.toml` file this parser was compiled from.
    pub source_file: PathBuf,
    subject: Regex,
    requires: Vec<String>,
    rejects: Vec<String>,
    contains: Vec<Regex>,
    excludes: Vec<Regex>,
    labels: Vec<String>,
    amount: CompiledField,
    occurred_at: CompiledOccurred,
    counterparty: Option<CompiledField>,
    counterparty_account: Option<CompiledField>,
    channel: Option<CompiledField>,
    account: Option<CompiledField>,
    reference_id: Option<CompiledField>,
    description: Option<CompiledField>,
    extra: Vec<(String, CompiledField)>,
    status: Option<CompiledStatus>,
}

#[derive(Debug)]
struct CompiledOccurred {
    single: Option<CompiledSingle>,
    split: Option<CompiledSplit>,
}

#[derive(Debug)]
struct CompiledSingle {
    field: CompiledField,
    formats: Vec<String>,
    locale: String,
}

#[derive(Debug)]
struct CompiledSplit {
    date_field: CompiledField,
    date_formats: Vec<String>,
    locale: String,
    time_field: CompiledField,
    time_formats: Vec<String>,
}

#[derive(Debug)]
struct CompiledStatus {
    source: StatusSource,
    field: Option<CompiledField>,
    map: HashMap<String, Status>,
    default: Option<Status>,
}

#[derive(Debug)]
pub struct Registry {
    pub parsers: Vec<Parser>,
    pub router: RouterConfig,
    /// Parsers skipped because a higher-precedence directory defines the same id.
    pub shadows: Vec<Shadow>,
}

#[derive(Debug)]
pub enum Outcome {
    /// A parser claimed the email and produced a transaction.
    Parsed(Box<Transaction>),
    /// No parser claimed it; the reason explains why.
    Unmatched(String),
    /// A parser claimed it but extraction failed.
    Failed { parser: String, error: String },
}

impl Registry {
    /// Load every `*.toml` under one directory.
    pub fn load(dir: impl AsRef<Path>, router_path: impl AsRef<Path>) -> Result<Self, Error> {
        Self::from_dirs(&[dir.as_ref().to_path_buf()], router_path)
    }

    /// Load specs from several directories, highest precedence first.
    ///
    /// A later directory may not redefine an id already seen: the first one
    /// wins and the shadowed file is reported in [`Registry::shadows`]. Two
    /// files with the same id inside one directory is an error.
    pub fn from_dirs(dirs: &[PathBuf], router_path: impl AsRef<Path>) -> Result<Self, Error> {
        let mut parsers: Vec<Parser> = Vec::new();
        let mut shadows: Vec<Shadow> = Vec::new();
        let mut seen: HashMap<String, PathBuf> = HashMap::new();

        for dir in dirs {
            let mut paths = Vec::new();
            collect_specs(dir, &mut paths)?;
            paths.sort();

            let mut in_dir: HashMap<String, PathBuf> = HashMap::new();

            for path in paths {
                let parser = load_one(&path)?;
                let id = parser.spec.id.clone();

                if let Some(first) = in_dir.insert(id.clone(), path.clone()) {
                    return Err(Error::DuplicateId { path, id, first });
                }

                if let Some(winner) = seen.get(&id) {
                    shadows.push(Shadow {
                        id,
                        skipped: path,
                        winner: winner.clone(),
                    });
                    continue;
                }

                seen.insert(id, path);
                parsers.push(parser);
            }
        }

        if parsers.is_empty() {
            return Err(Error::NoParsers(dirs.first().cloned().unwrap_or_default()));
        }

        let router_path = router_path.as_ref();
        let router_text = std::fs::read_to_string(router_path).ok();
        let router = match &router_text {
            Some(text) => toml::from_str(text).map_err(|err| Error::Spec {
                path: router_path.to_path_buf(),
                message: err.to_string(),
            })?,
            None => RouterConfig::default(),
        };

        Ok(Self {
            parsers,
            router,
            shadows,
        })
    }

    pub fn route(&self, email: &NormalizedEmail) -> Route {
        router::route(&self.parsers, &self.router, email)
    }

    /// Route and parse in one step.
    pub fn run(&self, email: &NormalizedEmail) -> Outcome {
        let route = self.route(email);
        let Some(index) = route.parser else {
            return Outcome::Unmatched(route.explain());
        };

        let parser = &self.parsers[index];
        match parser.parse(email) {
            Ok(transaction) => Outcome::Parsed(Box::new(transaction)),
            Err(error) => Outcome::Failed {
                parser: parser.spec.id.clone(),
                error,
            },
        }
    }

    pub fn find(&self, id: &str) -> Option<&Parser> {
        self.parsers.iter().find(|parser| parser.spec.id == id)
    }

    /// A machine-readable description of what the registry covers.
    pub fn summaries(&self) -> Vec<SpecSummary> {
        self.parsers.iter().map(|parser| parser.summary()).collect()
    }
}

impl Parser {
    pub fn summary(&self) -> SpecSummary {
        SpecSummary {
            id: self.spec.id.clone(),
            version: self.spec.version,
            bank: self.spec.defaults.bank.clone(),
            account: self.spec.defaults.account.clone(),
            senders: self.spec.matcher.senders.clone(),
            subject: self.spec.matcher.subject.clone(),
            file: self.source_file.clone(),
        }
    }

    pub fn parse(&self, email: &NormalizedEmail) -> Result<Transaction, String> {
        let regions = Regions::build(&email.blocks, &self.labels);

        let defaults = &self.spec.defaults;
        let currency = defaults.currency.clone();
        let exponent = currency_exponent(&currency);

        let amount_raw = self
            .amount
            .get(&regions)
            .map_err(|error| format!("amount: {error}"))?
            .ok_or_else(|| "amount: no value matched".to_string())?;

        let magnitude = parse_decimal(
            &amount_raw,
            self.amount.spec.number.unwrap_or(NumberStyle::Id),
            exponent,
        )
        .map_err(|error| format!("amount: {error}"))?;

        // Body times are already local to the account; attach the offset the
        // email itself claims so the instant is unambiguous downstream.
        let occurred_at_local = self.extract_occurred(&regions)?;
        let offset = *email.date.offset();
        let occurred_at = offset
            .from_local_datetime(&occurred_at_local)
            .single()
            .unwrap_or_else(|| offset.from_utc_datetime(&occurred_at_local));

        let status = self.extract_status(email, &regions)?;

        let mut warnings = Vec::new();
        if magnitude == 0 {
            warnings.push("amount is zero".to_string());
        }

        let kind = defaults.kind;
        let minor = if kind.is_outflow() {
            -magnitude
        } else {
            magnitude
        };

        let counterparty = self.get(&self.counterparty, &regions);
        let counterparty_account = self.get(&self.counterparty_account, &regions);
        let channel = self.get(&self.channel, &regions);
        let reference_id = self.get(&self.reference_id, &regions);
        let description = self.get(&self.description, &regions);

        let mut extra = std::collections::BTreeMap::new();
        for (name, field) in &self.extra {
            if let Some(value) = self.get(&Some(field.clone()), &regions) {
                extra.insert(name.clone(), value);
            }
        }

        let account_last4 = self
            .account
            .as_ref()
            .and_then(|field| field.get(&regions).ok().flatten())
            .map(|value| value.trim().to_string());

        let extraction = if warnings.is_empty() {
            Extraction::Strict
        } else {
            Extraction::Heuristic
        };

        Ok(Transaction {
            id: email.source.stable_id(),
            source: email.source.clone(),
            parser: self.spec.id.clone(),
            parser_version: self.spec.version,
            bank: defaults.bank.clone(),
            account: defaults.account.clone(),
            kind,
            status,
            amount: Money::new(minor, currency.clone()),
            currency_exponent: exponent,
            amount_decimal: Money::new(minor, currency).decimal(),
            occurred_at,
            occurred_on: occurred_at.date_naive(),
            timezone: defaults.tz.clone(),
            counterparty: counterparty.map(clean),
            counterparty_account: counterparty_account.map(clean),
            account_last4,
            reference_id: reference_id.map(clean),
            channel: channel.map(clean),
            description: description.map(clean),
            extra,
            extraction,
            warnings,
        })
    }

    fn get(&self, field: &Option<CompiledField>, regions: &Regions<'_>) -> Option<String> {
        field
            .as_ref()
            .and_then(|field| field.get(regions).ok().flatten())
    }

    fn extract_occurred(&self, regions: &Regions<'_>) -> Result<NaiveDateTime, String> {
        if let Some(single) = &self.occurred_at.single {
            let raw = single
                .field
                .get(regions)
                .map_err(|error| format!("occurred_at: {error}"))?
                .ok_or_else(|| "occurred_at: no value matched".to_string())?;
            return parse_datetime(&raw, &single.formats, &single.locale)
                .map_err(|error| format!("occurred_at: {error}"));
        }

        if let Some(split) = &self.occurred_at.split {
            let date_raw = split
                .date_field
                .get(regions)
                .map_err(|error| format!("occurred_at.date: {error}"))?
                .ok_or_else(|| "occurred_at.date: no value matched".to_string())?;
            let time_raw = split
                .time_field
                .get(regions)
                .map_err(|error| format!("occurred_at.time: {error}"))?
                .ok_or_else(|| "occurred_at.time: no value matched".to_string())?;

            let day: NaiveDate = parse_date(&date_raw, &split.date_formats, &split.locale)
                .map_err(|error| format!("occurred_at.date: {error}"))?;
            let time = parse_time(&time_raw, &split.time_formats)
                .map_err(|error| format!("occurred_at.time: {error}"))?;
            return Ok(day.and_time(time));
        }

        Err("occurred_at: spec has neither a label nor date/time fields".to_string())
    }

    fn extract_status(
        &self,
        email: &NormalizedEmail,
        regions: &Regions<'_>,
    ) -> Result<Status, String> {
        let Some(status) = &self.status else {
            return Ok(Status::Settled);
        };

        let raw = match status.source {
            StatusSource::Subject => self
                .subject
                .captures(&email.subject)
                .and_then(|captures| captures.name("status").map(|m| m.as_str().to_string()))
                .or_else(|| {
                    status
                        .field
                        .as_ref()
                        .and_then(|field| field.get(regions).ok().flatten())
                }),
            StatusSource::Field => status
                .field
                .as_ref()
                .and_then(|field| field.get(regions).ok().flatten()),
        };

        let Some(raw) = raw else {
            return Ok(status.default.unwrap_or(Status::Unknown));
        };

        Ok(status
            .map
            .get(raw.trim())
            .copied()
            .unwrap_or(status.default.unwrap_or(Status::Unknown)))
    }
}

fn clean(value: String) -> String {
    value.trim().to_string()
}

/// Blocks grouped by the label that precedes them.
///
/// A "region" is everything between one label and the next, which makes values
/// that span several blocks (`Rp` / `100.000` / `,00`) fall out naturally.
#[derive(Debug, Default)]
pub struct Regions<'a> {
    regions: HashMap<String, Vec<&'a str>>,
}

impl<'a> Regions<'a> {
    pub fn build(blocks: &'a [Block], labels: &[String]) -> Self {
        let label_set: std::collections::HashSet<String> = labels.iter().map(|l| norm(l)).collect();
        let mut regions: HashMap<String, Vec<&'a str>> = HashMap::new();
        let mut current: Option<String> = None;

        for block in blocks {
            let key = norm(&block.text);
            if label_set.contains(&key) {
                current = Some(key.clone());
                regions.entry(key).or_default();
            } else if let Some(label) = &current {
                regions.entry(label.clone()).or_default().push(&block.text);
            }
        }

        Self { regions }
    }

    pub fn get(&self, label: &str) -> Option<&[&'a str]> {
        self.regions.get(&norm(label)).map(Vec::as_slice)
    }

    pub fn has(&self, label: &str) -> bool {
        self.regions.contains_key(&norm(label))
    }
}

/// Normalized form used for matching labels: case-insensitive, whitespace
/// collapsed, trailing colon removed.
pub fn norm(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut last_space = false;
    for ch in text.trim().chars() {
        if ch.is_whitespace() {
            if !last_space {
                out.push(' ');
                last_space = true;
            }
        } else {
            out.push(ch.to_ascii_lowercase());
            last_space = false;
        }
    }
    out.trim_end_matches(':').trim().to_string()
}

fn collect_specs(dir: &Path, out: &mut Vec<PathBuf>) -> Result<(), Error> {
    let entries = std::fs::read_dir(dir).map_err(|source| Error::Io {
        path: dir.to_path_buf(),
        source,
    })?;

    for entry in entries {
        let entry = entry.map_err(|source| Error::Io {
            path: dir.to_path_buf(),
            source,
        })?;
        let path = entry.path();
        // ConfigMap volumes keep their payload in `..data` and timestamped
        // `..2026_...` siblings; those are symlink bookkeeping, not parser dirs.
        if path
            .file_name()
            .and_then(|name| name.to_str())
            .map(|name| name.starts_with(".."))
            .unwrap_or(false)
        {
            continue;
        }
        if path.is_dir() {
            collect_specs(&path, out)?;
        } else if path.extension().and_then(|value| value.to_str()) == Some("toml")
            && path
                .file_stem()
                .and_then(|stem| stem.to_str())
                .map(|stem| stem.contains('.'))
                .unwrap_or(false)
        {
            out.push(path);
        }
    }

    Ok(())
}

fn load_one(path: &Path) -> Result<Parser, Error> {
    let text = std::fs::read_to_string(path).map_err(|source| Error::Io {
        path: path.to_path_buf(),
        source,
    })?;

    let spec: ParserSpec = toml::from_str(&text).map_err(|err| Error::Spec {
        path: path.to_path_buf(),
        message: err.to_string(),
    })?;

    // A flat corpus is the whole point: the file name *is* the parser id, so a
    // mounted ConfigMap key and the parser identity cannot disagree.
    let stem = path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or_default()
        .to_string();

    if stem != spec.id {
        return Err(Error::IdMismatch {
            path: path.to_path_buf(),
            id: spec.id.clone(),
            expected: stem,
        });
    }

    compile(spec, path).map_err(|message| Error::Spec {
        path: path.to_path_buf(),
        message,
    })
}

fn compile(spec: ParserSpec, path: &Path) -> Result<Parser, String> {
    if spec.spec_version != SPEC_VERSION {
        return Err(format!(
            "spec_version {} is not supported (this build understands {SPEC_VERSION})",
            spec.spec_version
        ));
    }

    let subject = Regex::new(&spec.matcher.subject)
        .map_err(|err| format!("match.subject: invalid regex: {err}"))?;

    let compile_field = |name: &str, field: &Field| -> Result<CompiledField, String> {
        let regex = match &field.regex {
            Some(pattern) => Some(
                Regex::new(pattern)
                    .map_err(|err| format!("extract.{name}.regex: invalid regex: {err}"))?,
            ),
            None => None,
        };
        Ok(CompiledField {
            spec: field.clone(),
            regex,
        })
    };

    let amount = compile_field("amount", &spec.extract.amount)?;

    let occurred_at = {
        let single = match (
            &spec.extract.occurred_at.label,
            &spec.extract.occurred_at.date,
        ) {
            (Some(label), None) => {
                let field = Field {
                    label: label.clone(),
                    regex: spec.extract.occurred_at.regex.clone(),
                    block: None,
                    per_block: false,
                    number: None,
                    normalize: None,
                };
                let compiled = compile_field("occurred_at", &field)?;
                Some(CompiledSingle {
                    field: compiled,
                    formats: spec.extract.occurred_at.formats.clone(),
                    locale: spec.extract.occurred_at.locale.clone(),
                })
            }
            _ => None,
        };

        let split = match (
            &spec.extract.occurred_at.date,
            &spec.extract.occurred_at.time,
        ) {
            (Some(date), Some(time)) => {
                let date_field = compile_field(
                    "occurred_at.date",
                    &Field {
                        label: date.label.clone(),
                        regex: date.regex.clone(),
                        block: None,
                        per_block: false,
                        number: None,
                        normalize: None,
                    },
                )?;
                let time_field = compile_field(
                    "occurred_at.time",
                    &Field {
                        label: time.label.clone(),
                        regex: time.regex.clone(),
                        block: None,
                        per_block: false,
                        number: None,
                        normalize: None,
                    },
                )?;
                Some(CompiledSplit {
                    date_field,
                    date_formats: date.formats.clone(),
                    locale: date.locale.clone(),
                    time_field,
                    time_formats: time.formats.clone(),
                })
            }
            _ => None,
        };

        if single.is_none() && split.is_none() {
            return Err(
                "extract.occurred_at needs either `label` (with `formats`) or `date` and `time`"
                    .to_string(),
            );
        }

        CompiledOccurred { single, split }
    };

    let compile_opt =
        |name: &str, field: &Option<Field>| -> Result<Option<CompiledField>, String> {
            match field {
                Some(field) => Ok(Some(compile_field(name, field)?)),
                None => Ok(None),
            }
        };

    let counterparty = compile_opt("counterparty", &spec.extract.counterparty)?;
    let counterparty_account =
        compile_opt("counterparty_account", &spec.extract.counterparty_account)?;
    let channel = compile_opt("channel", &spec.extract.channel)?;
    let account = compile_opt("account", &spec.extract.account)?;
    let reference_id = compile_opt("reference_id", &spec.extract.reference_id)?;
    let description = compile_opt("description", &spec.extract.description)?;

    let mut extra = Vec::new();
    for (name, field) in &spec.extract.extra {
        extra.push((
            name.clone(),
            compile_field(&format!("extra.{name}"), field)?,
        ));
    }

    let status = match &spec.status {
        Some(status) => {
            let field = match &status.label {
                Some(label) => Some(compile_field(
                    "status",
                    &Field {
                        label: label.clone(),
                        regex: None,
                        block: None,
                        per_block: false,
                        number: None,
                        normalize: None,
                    },
                )?),
                None => None,
            };

            let map = status
                .map
                .iter()
                .map(|(key, value)| (key.clone(), *value))
                .collect();

            Some(CompiledStatus {
                source: status.source,
                field,
                map,
                default: status.default,
            })
        }
        None => None,
    };

    let compile_all = |name: &str, patterns: &[String]| -> Result<Vec<Regex>, String> {
        patterns
            .iter()
            .map(|pattern| {
                Regex::new(pattern)
                    .map_err(|err| format!("match.{name}: invalid regex `{pattern}`: {err}"))
            })
            .collect()
    };

    Ok(Parser {
        subject,
        requires: spec.matcher.requires.iter().map(|l| norm(l)).collect(),
        rejects: spec.matcher.rejects.iter().map(|l| norm(l)).collect(),
        contains: compile_all("contains", &spec.matcher.contains)?,
        excludes: compile_all("excludes", &spec.matcher.excludes)?,
        labels: spec.labels.list.clone(),
        amount,
        occurred_at,
        counterparty,
        counterparty_account,
        channel,
        account,
        reference_id,
        description,
        extra,
        status,
        source_file: path.to_path_buf(),
        spec,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn regions_group_by_label() {
        let blocks: Vec<Block> = ["Total", "Rp", "5.500", ",00", "walletaccount", "Warung"]
            .into_iter()
            .map(Block::new)
            .collect();
        let labels = vec!["Total".to_string(), "walletaccount".to_string()];
        let regions = Regions::build(&blocks, &labels);

        assert_eq!(regions.get("Total"), Some(&["Rp", "5.500", ",00"][..]));
        assert_eq!(regions.get("walletaccount"), Some(&["Warung"][..]));
    }

    #[test]
    fn norm_is_forgiving() {
        assert_eq!(norm("Nominal Transfer:"), "nominal transfer");
        assert_eq!(norm("  Nominal   Transfer "), "nominal transfer");
    }
}
