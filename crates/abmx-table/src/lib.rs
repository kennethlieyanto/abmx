//! Declarative bank statement parsing: a PDF in, transactions out.
//!
//! The engine is fixed; a new bank is one data file,
//! `<corpus>/<id>/config.toml`, where the directory name is the parser id. The
//! hard part (PDF text extraction) lives in `abmx-doc`; this crate only turns
//! positioned text into rows and proves the arithmetic.
//!
//! A statement is only `strict` when its own numbers agree: the running balance
//! must follow every row, the closing balance must equal the last row's
//! balance, and the printed credit/debit totals must equal the parsed sums.
//! There is no partial import: a mismatch rejects the whole document.

pub mod spec;

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use abmx_doc::{Document, Line};
use abmx_parse::date::parse_date;
use abmx_parse::number::parse_decimal;
use abmx_tx::{currency_exponent, Extraction, Kind, Money, Source, Status, Transaction};
use chrono::{DateTime, Datelike, FixedOffset, NaiveDate, TimeZone};
use regex::Regex;
use sha2::{Digest, Sha256};

pub use spec::{
    Column, Columns, DateRule, Defaults, DirectionRule, DocField, DocumentSpec, PeriodField,
    RowWhen, SectionSpec, TableSpec,
};
pub use spec::{ParserSpec, SPEC_VERSION};

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
    #[error("no statement parser specs found under {path}")]
    NoParsers { path: PathBuf },
    #[error("{path}: directory `{found}` is not a valid parser id")]
    InvalidId { path: PathBuf, found: String },
}

/// A spec with every regex compiled once at load time.
#[derive(Debug)]
pub struct Parser {
    pub id: String,
    pub spec: ParserSpec,
    /// The `config.toml` this parser was compiled from.
    pub source_file: PathBuf,
    period: Regex,
    account: Option<Regex>,
    pages: Option<Regex>,
    sections: Vec<CompiledSection>,
}

#[derive(Debug)]
struct CompiledSection {
    spec: SectionSpec,
    start: Regex,
    opening: Option<Regex>,
    closing: Regex,
    total_credit: Option<Regex>,
    total_debit: Option<Regex>,
    row_when: Regex,
    skip: Vec<Regex>,
}

#[derive(Debug)]
pub struct Registry {
    pub parsers: Vec<Parser>,
}

impl Registry {
    pub fn from_dirs(dirs: &[PathBuf]) -> Result<Self, Error> {
        let mut parsers = Vec::new();
        let mut seen: HashSet<String> = HashSet::new();

        for dir in dirs {
            let mut paths = Vec::new();
            collect_specs(dir, &mut paths)?;
            paths.sort();

            for path in paths {
                let text = std::fs::read_to_string(&path).map_err(|source| Error::Io {
                    path: path.clone(),
                    source,
                })?;

                // Only this engine's specs are claimed. A spec with another
                // `input` belongs to another engine and is not an error here.
                let claimed = toml::from_str::<toml::Value>(&text)
                    .ok()
                    .and_then(|value| {
                        value
                            .get("input")
                            .and_then(|input| input.as_str())
                            .map(str::to_owned)
                    })
                    .is_some_and(|input| input == "pdf");
                if !claimed {
                    continue;
                }

                let id = path
                    .parent()
                    .and_then(Path::file_name)
                    .and_then(|name| name.to_str())
                    .unwrap_or_default()
                    .to_owned();

                if !abmx_parse::is_parser_id(&id) {
                    return Err(Error::InvalidId { path, found: id });
                }

                if !seen.insert(id.clone()) {
                    continue;
                }

                parsers.push(compile(text, path, id)?);
            }
        }

        if parsers.is_empty() {
            return Err(Error::NoParsers {
                path: dirs.first().cloned().unwrap_or_default(),
            });
        }

        Ok(Self { parsers })
    }

    pub fn find(&self, id: &str) -> Option<&Parser> {
        self.parsers.iter().find(|parser| parser.id == id)
    }
}

impl Parser {
    pub fn parse(
        &self,
        document: &Document,
        origin: Option<String>,
    ) -> Result<Vec<Transaction>, String> {
        let text = document_text(document);
        let (start, end) = self.period(&text)?;
        self.check_page_count(document, &text)?;
        let account = self
            .account
            .as_ref()
            .and_then(|regex| capture(regex, &text, "value"))
            .unwrap_or_else(|| self.id.clone());
        let document_id = document_id(&self.spec.defaults.bank, &account, start, end);

        let mut states: Vec<SectionState> = (0..self.sections.len())
            .map(|_| SectionState::default())
            .collect();
        let mut active: Option<usize> = None;

        for page in &document.pages {
            for line in &page.lines {
                if let Some(index) = self
                    .sections
                    .iter()
                    .position(|section| section.start.is_match(&line.text()))
                {
                    active = Some(index);
                }

                let Some(index) = active else {
                    continue;
                };
                let section = &self.sections[index];
                let state = &mut states[index];

                let text = line.text();
                if let Some(regex) = &section.opening {
                    if let Some(value) = capture_decimal(regex, &text, self.spec.defaults.number) {
                        state.opening = Some(value);
                    }
                }
                if let Some(value) =
                    capture_decimal(&section.closing, &text, self.spec.defaults.number)
                {
                    state.closing = Some(value);
                }
                if let Some(regex) = &section.total_credit {
                    if let Some(value) = capture_decimal(regex, &text, self.spec.defaults.number) {
                        state.total_credit = Some(value);
                    }
                }
                if let Some(regex) = &section.total_debit {
                    if let Some(value) = capture_decimal(regex, &text, self.spec.defaults.number) {
                        state.total_debit = Some(value);
                    }
                }

                let cells = extract_cells(&section.spec.columns, line);
                if !section
                    .row_when
                    .is_match(cells.row_key(&section.spec.table.row_when.column))
                {
                    continue;
                }

                if section
                    .skip
                    .iter()
                    .any(|regex| regex.is_match(&cells.description) || regex.is_match(&text))
                {
                    state.skipped += 1;
                    continue;
                }

                let row = row_from(
                    &cells,
                    &section.spec,
                    self.spec.defaults.number,
                    page.number,
                    start,
                    end,
                )?;
                state.rows.push(row);
            }
        }

        let mut transactions = Vec::new();
        let mut ordinals: HashMap<String, u32> = HashMap::new();

        for (index, state) in states.iter().enumerate() {
            let section = &self.sections[index];
            // A section that only appeared in the summary page (no rows, no
            // footer) is not part of this document.
            if state.rows.is_empty() && state.closing.is_none() && state.opening.is_none() {
                continue;
            }
            verify(section, state)?;

            for (row_index, row) in state.rows.iter().enumerate() {
                let base = content_key(&document_id, &section.spec.key, row);
                let ordinal = ordinals.entry(base.clone()).or_insert(0);
                let id = row_id(&base, *ordinal);
                *ordinal += 1;

                transactions.push(self.transaction(
                    section,
                    row,
                    id,
                    row_index as u32,
                    &document_id,
                    origin.clone(),
                )?);
            }
        }

        Ok(transactions)
    }

    fn period(&self, text: &str) -> Result<(NaiveDate, NaiveDate), String> {
        let captures = self
            .period
            .captures(text)
            .ok_or_else(|| "the statement period was not found".to_owned())?;
        let start = captures
            .name("start")
            .ok_or_else(|| "period regex has no `start` group".to_owned())?
            .as_str();
        let end = captures
            .name("end")
            .ok_or_else(|| "period regex has no `end` group".to_owned())?
            .as_str();

        let formats = &self.spec.document.period.formats;
        let start = parse_date(start, formats, "en")
            .map_err(|error| format!("period start `{start}`: {error}"))?;
        let end = parse_date(end, formats, "en")
            .map_err(|error| format!("period end `{end}`: {error}"))?;

        if start > end {
            return Err(format!("period starts after it ends: {start} > {end}"));
        }
        Ok((start, end))
    }

    fn check_page_count(&self, document: &Document, text: &str) -> Result<(), String> {
        let Some(regex) = &self.pages else {
            return Ok(());
        };
        let Some(value) = capture(regex, text, "value") else {
            return Ok(());
        };
        let printed: usize = value
            .parse()
            .map_err(|_| format!("page count `{value}` is not a number"))?;
        if printed != document.page_count() {
            return Err(format!(
                "the PDF says it has {printed} pages but {} were read; the download may be truncated",
                document.page_count()
            ));
        }
        Ok(())
    }

    fn transaction(
        &self,
        section: &CompiledSection,
        row: &Row,
        id: String,
        index: u32,
        document_id: &str,
        origin: Option<String>,
    ) -> Result<Transaction, String> {
        let currency = &self.spec.defaults.currency;
        let exponent = currency_exponent(currency);
        let minor = scale_to_minor(row.amount, exponent)?;
        let signed = if row.out { -minor } else { minor };

        let offset = self
            .spec
            .defaults
            .offset
            .parse::<FixedOffset>()
            .map_err(|error| format!("defaults.offset `{}`: {error}", self.spec.defaults.offset))?;
        let naive = row
            .date
            .and_hms_opt(0, 0, 0)
            .ok_or_else(|| format!("invalid transaction date {}", row.date))?;
        let occurred_at: DateTime<FixedOffset> = offset
            .from_local_datetime(&naive)
            .single()
            .ok_or_else(|| format!("ambiguous local time on {}", row.date))?;

        Ok(Transaction {
            source: Source {
                message_id: None,
                file: Some(origin.unwrap_or_else(|| format!("stmt:{document_id}"))),
                tx_index: index,
            },
            id,
            parser: self.id.clone(),
            parser_version: self.spec.version,
            bank: self.spec.defaults.bank.clone(),
            account: section.spec.key.clone(),
            kind: Kind::Unknown,
            status: Status::Settled,
            amount: Money::new(signed, currency.clone()),
            currency_exponent: exponent,
            amount_decimal: Money::new(signed, currency.clone()).decimal(),
            occurred_at,
            occurred_on: row.date,
            timezone: self.spec.defaults.tz.clone(),
            counterparty: (!row.description.is_empty()).then(|| row.description.clone()),
            counterparty_account: None,
            account_last4: None,
            reference_id: None,
            channel: None,
            description: None,
            extra: Default::default(),
            extraction: Extraction::Strict,
            warnings: Vec::new(),
        })
    }
}

#[derive(Debug, Default)]
struct SectionState {
    opening: Option<i64>,
    closing: Option<i64>,
    total_credit: Option<i64>,
    total_debit: Option<i64>,
    rows: Vec<Row>,
    skipped: usize,
}

#[derive(Debug)]
struct Row {
    page: usize,
    date: NaiveDate,
    description: String,
    /// Unsigned, scale 2 (hundredths of the printed currency).
    amount: i64,
    out: bool,
    /// Scale 2.
    balance: i64,
}

#[derive(Debug, Default)]
struct Cells {
    date: String,
    value_date: String,
    description: String,
    amount: String,
    marker: String,
    balance: String,
}

impl Cells {
    fn row_key(&self, column: &str) -> &str {
        match column {
            "date" => &self.date,
            "value_date" => &self.value_date,
            "description" => &self.description,
            "amount" => &self.amount,
            "balance" => &self.balance,
            other => {
                // `column` is validated at load time; this is unreachable.
                debug_assert!(false, "unknown row_when column `{other}`");
                ""
            }
        }
    }
}

fn extract_cells(columns: &Columns, line: &Line) -> Cells {
    Cells {
        date: cell(line, &columns.date),
        value_date: columns
            .value_date
            .as_ref()
            .map(|column| cell(line, column))
            .unwrap_or_default(),
        description: cell(line, &columns.description),
        amount: cell(line, &columns.amount),
        marker: columns
            .marker
            .as_ref()
            .map(|column| cell(line, column))
            .unwrap_or_default(),
        balance: cell(line, &columns.balance),
    }
}

fn cell(line: &Line, column: &Column) -> String {
    let mut text = String::new();
    for run in &line.runs {
        if run.x >= column.x[0] && run.x < column.x[1] {
            text.push_str(&run.text);
        }
    }
    collapse(&text)
}

fn collapse(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn row_from(
    cells: &Cells,
    section: &SectionSpec,
    number: abmx_parse::NumberStyle,
    page: usize,
    start: NaiveDate,
    end: NaiveDate,
) -> Result<Row, String> {
    if cells.date.is_empty() {
        return Err(format!("page {page}: row has no transaction date"));
    }
    if cells.amount.is_empty() {
        return Err(format!("page {page}: row {} has no amount", cells.date));
    }
    if cells.balance.is_empty() {
        return Err(format!("page {page}: row {} has no balance", cells.date));
    }

    let date = parse_row_date(&cells.date, &section.table.date.format, start, end)
        .map_err(|error| format!("page {page}: {error}"))?;
    let amount = parse_decimal(&cells.amount, number, 2)
        .map_err(|error| format!("page {page}: amount `{}`: {error}", cells.amount))?;
    let balance = parse_decimal(&cells.balance, number, 2)
        .map_err(|error| format!("page {page}: balance `{}`: {error}", cells.balance))?;

    if amount <= 0 {
        return Err(format!(
            "page {page}: row {date} has a zero amount; check the column ranges"
        ));
    }

    let out = !cells.marker.trim().is_empty()
        && cells.marker.trim() == section.table.direction.out_marker.trim();

    Ok(Row {
        page,
        date,
        description: cells.description.clone(),
        amount,
        out,
        balance,
    })
}

fn parse_row_date(
    raw: &str,
    format: &str,
    start: NaiveDate,
    end: NaiveDate,
) -> Result<NaiveDate, String> {
    if format.contains("%Y") || format.contains("%y") {
        return NaiveDate::parse_from_str(raw.trim(), format)
            .map_err(|error| format!("date `{raw}`: {error}"));
    }

    let mut years = vec![start.year()];
    if end.year() != start.year() {
        years.push(end.year());
    }

    let mut best = None;
    for year in years {
        let candidate = format!("{}/{}", raw.trim(), year);
        let parsed = NaiveDate::parse_from_str(&candidate, &format!("{format}/%Y"));
        if let Ok(date) = parsed {
            if date >= start && date <= end {
                return Ok(date);
            }
            best.get_or_insert(date);
        }
    }

    match best {
        Some(date) => Err(format!(
            "transaction date {date} is outside the statement period {start}..{end}"
        )),
        None => Err(format!("date `{raw}` does not match `{format}`")),
    }
}

fn verify(section: &CompiledSection, state: &SectionState) -> Result<(), String> {
    let key = &section.spec.key;
    let opening = state
        .opening
        .ok_or_else(|| format!("section `{key}`: opening balance was not found"))?;
    let closing = state
        .closing
        .ok_or_else(|| format!("section `{key}`: closing balance was not found"))?;

    let mut previous = opening;
    let mut credits = 0i64;
    let mut debits = 0i64;

    for (index, row) in state.rows.iter().enumerate() {
        let delta = if row.out { -row.amount } else { row.amount };
        let expected = previous
            .checked_add(delta)
            .ok_or_else(|| format!("section `{key}`: balance overflow at row {}", index + 1))?;
        if row.balance != expected {
            return Err(format!(
                "section `{key}`: row {} ({} on page {}) balance {} does not follow {} {:+.2} (expected {:.2})",
                index + 1,
                row.date,
                row.page,
                money(row.balance),
                money(previous),
                if row.out { -row.amount as f64 / 100.0 } else { row.amount as f64 / 100.0 },
                money(expected)
            ));
        }
        if row.out {
            debits += row.amount;
        } else {
            credits += row.amount;
        }
        previous = row.balance;
    }

    if previous != closing {
        return Err(format!(
            "section `{key}`: the running balance ends at {} but the statement closes at {}",
            money(previous),
            money(closing)
        ));
    }

    if let Some(total) = state.total_credit {
        if credits != total {
            return Err(format!(
                "section `{key}`: parsed credits {} but the statement prints {}",
                money(credits),
                money(total)
            ));
        }
    }
    if let Some(total) = state.total_debit {
        if debits != total {
            return Err(format!(
                "section `{key}`: parsed debits {} but the statement prints {}",
                money(debits),
                money(total)
            ));
        }
    }

    Ok(())
}

fn money(scaled: i64) -> String {
    let sign = if scaled < 0 { "-" } else { "" };
    let absolute = scaled.unsigned_abs();
    format!("{sign}{}.{:02}", absolute / 100, absolute % 100)
}

fn scale_to_minor(scaled: i64, exponent: u32) -> Result<i64, String> {
    if exponent <= 2 {
        let divisor = 10i64.pow(2 - exponent);
        if scaled % divisor != 0 {
            return Err(format!(
                "amount {} has more precision than a {} exponent allows",
                money(scaled),
                exponent
            ));
        }
        Ok(scaled / divisor)
    } else {
        scaled
            .checked_mul(10i64.pow(exponent - 2))
            .ok_or_else(|| format!("amount {} overflows", money(scaled)))
    }
}

fn document_text(document: &Document) -> String {
    let mut text = String::new();
    for page in &document.pages {
        for line in &page.lines {
            text.push_str(&line.text());
            text.push('\n');
        }
    }
    text
}

fn capture(regex: &Regex, text: &str, group: &str) -> Option<String> {
    regex
        .captures(text)
        .and_then(|captures| captures.name(group))
        .map(|value| value.as_str().trim().to_owned())
        .filter(|value| !value.is_empty())
}

fn capture_decimal(regex: &Regex, text: &str, style: abmx_parse::NumberStyle) -> Option<i64> {
    let raw = regex
        .captures(text)
        .and_then(|captures| captures.name("value"))
        .map(|value| value.as_str())?;
    parse_decimal(raw, style, 2).ok()
}

fn document_id(bank: &str, account: &str, start: NaiveDate, end: NaiveDate) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bank.as_bytes());
    hasher.update(b"|");
    hasher.update(account.as_bytes());
    hasher.update(b"|");
    hasher.update(start.to_string().as_bytes());
    hasher.update(b"|");
    hasher.update(end.to_string().as_bytes());
    format!("stmt_{:016x}", first_u64(&hasher.finalize()))
}

fn content_key(document_id: &str, section: &str, row: &Row) -> String {
    format!(
        "{document_id}|{section}|{}|{}|{}|{}|{}",
        row.date, row.amount, row.out, row.balance, row.description
    )
}

fn row_id(base: &str, ordinal: u32) -> String {
    let mut hasher = Sha256::new();
    hasher.update(base.as_bytes());
    hasher.update(b"|");
    hasher.update(ordinal.to_le_bytes());
    format!("stmt_{:016x}", first_u64(&hasher.finalize()))
}

fn first_u64(bytes: &[u8]) -> u64 {
    let mut buffer = [0u8; 8];
    buffer.copy_from_slice(&bytes[..8]);
    u64::from_be_bytes(buffer)
}

fn compile(text: String, path: PathBuf, id: String) -> Result<Parser, Error> {
    let spec: ParserSpec = toml::from_str(&text).map_err(|error| Error::Spec {
        path: path.clone(),
        message: error.to_string(),
    })?;

    compile_spec(spec, path.clone(), id).map_err(|message| Error::Spec { path, message })
}

fn compile_spec(spec: ParserSpec, path: PathBuf, id: String) -> Result<Parser, String> {
    if spec.spec_version != SPEC_VERSION {
        return Err(format!(
            "spec_version {} is not supported (this build understands {SPEC_VERSION})",
            spec.spec_version
        ));
    }
    if spec.input != "pdf" {
        return Err(format!(
            "input `{}` is not supported by the statement engine (expected `pdf`)",
            spec.input
        ));
    }
    if spec.sections.is_empty() {
        return Err("at least one [[sections]] entry is required".to_owned());
    }
    if spec.defaults.bank.trim().is_empty() {
        return Err("defaults.bank must not be empty".to_owned());
    }
    if spec.defaults.currency.trim().is_empty() {
        return Err("defaults.currency must not be empty".to_owned());
    }
    spec.defaults
        .offset
        .parse::<FixedOffset>()
        .map_err(|error| format!("defaults.offset `{}`: {error}", spec.defaults.offset))?;

    let regex = |name: &str, pattern: &str| -> Result<Regex, String> {
        Regex::new(pattern).map_err(|error| format!("{name}: invalid regex: {error}"))
    };

    let period = regex("document.period.regex", &spec.document.period.regex)?;
    require_group(&period, "document.period.regex", "start")?;
    require_group(&period, "document.period.regex", "end")?;

    let account = spec
        .document
        .account
        .as_ref()
        .map(|field| regex("document.account.regex", &field.regex))
        .transpose()?;
    if let Some(regex) = &account {
        require_group(regex, "document.account.regex", "value")?;
    }
    let pages = spec
        .document
        .pages
        .as_ref()
        .map(|field| regex("document.pages.regex", &field.regex))
        .transpose()?;
    if let Some(regex) = &pages {
        require_group(regex, "document.pages.regex", "value")?;
    }

    let mut sections = Vec::with_capacity(spec.sections.len());
    let mut keys = HashSet::new();
    for section in &spec.sections {
        if !keys.insert(section.key.clone()) {
            return Err(format!("duplicate section key `{}`", section.key));
        }
        if section.key.trim().is_empty() {
            return Err("section key must not be empty".to_owned());
        }
        if section.table.row_when.column != "date" && section.table.row_when.column != "value_date"
        {
            return Err(format!(
                "section `{}`: row_when.column must be `date` or `value_date`",
                section.key
            ));
        }
        if section.table.direction.out_marker.trim().is_empty() {
            return Err(format!(
                "section `{}`: table.direction.out_marker must not be empty",
                section.key
            ));
        }
        validate_column(&section.key, "date", &section.columns.date)?;
        if let Some(column) = &section.columns.value_date {
            validate_column(&section.key, "value_date", column)?;
        }
        validate_column(&section.key, "description", &section.columns.description)?;
        validate_column(&section.key, "amount", &section.columns.amount)?;
        if let Some(column) = &section.columns.marker {
            validate_column(&section.key, "marker", column)?;
        }
        validate_column(&section.key, "balance", &section.columns.balance)?;

        let opening = section
            .opening
            .as_ref()
            .map(|field| regex("opening.regex", &field.regex))
            .transpose()?;
        let closing = regex("closing.regex", &section.closing.regex)?;
        let total_credit = section
            .total_credit
            .as_ref()
            .map(|field| regex("total_credit.regex", &field.regex))
            .transpose()?;
        let total_debit = section
            .total_debit
            .as_ref()
            .map(|field| regex("total_debit.regex", &field.regex))
            .transpose()?;
        for (name, regex) in [
            ("opening.regex", opening.as_ref()),
            ("closing.regex", Some(&closing)),
            ("total_credit.regex", total_credit.as_ref()),
            ("total_debit.regex", total_debit.as_ref()),
        ] {
            if let Some(regex) = regex {
                require_group(regex, name, "value")?;
            }
        }

        sections.push(CompiledSection {
            start: regex(&format!("sections.{}", section.key), &section.start)?,
            opening,
            closing,
            total_credit,
            total_debit,
            row_when: regex("row_when.regex", &section.table.row_when.regex)?,
            skip: section
                .table
                .skip
                .iter()
                .map(|pattern| regex("table.skip", pattern))
                .collect::<Result<_, _>>()?,
            spec: section.clone(),
        });
    }

    Ok(Parser {
        id,
        spec,
        source_file: path,
        period,
        account,
        pages,
        sections,
    })
}

fn validate_column(section: &str, name: &str, column: &Column) -> Result<(), String> {
    if !(column.x[0].is_finite() && column.x[1].is_finite()) || column.x[0] >= column.x[1] {
        return Err(format!(
            "section `{section}`: column `{name}` must be a range [min, max) with min < max"
        ));
    }
    Ok(())
}

fn require_group(regex: &Regex, name: &str, group: &str) -> Result<(), String> {
    if regex.capture_names().flatten().any(|found| found == group) {
        Ok(())
    } else {
        Err(format!("{name} must capture `{group}`"))
    }
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
        if path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.starts_with(".."))
        {
            continue;
        }
        if path.is_dir() {
            if path.file_name().and_then(|name| name.to_str()) == Some("fixtures") {
                continue;
            }
            collect_specs(&path, out)?;
        } else if path.file_name().and_then(|name| name.to_str()) == Some("config.toml") {
            out.push(path);
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use abmx_doc::{Line, Page, Run};

    fn line(y: f32, runs: &[(f32, &str)]) -> Line {
        Line {
            y,
            runs: runs
                .iter()
                .map(|(x, text)| Run {
                    x: *x,
                    y,
                    size: 9.0,
                    text: (*text).to_owned(),
                })
                .collect(),
        }
    }

    fn page(number: usize, lines: Vec<Line>) -> Page {
        Page { number, lines }
    }

    const SPEC: &str = r#"
spec_version = 1
input = "pdf"

[defaults]
bank = "test"
currency = "IDR"
offset = "+07:00"
number = "en"

[document]
period = { regex = 'Period: (?P<start>\d{2}/\d{2}/\d{4}) - (?P<end>\d{2}/\d{2}/\d{4})', formats = ["%d/%m/%Y"] }

[[sections]]
key = "savings"
start = 'Savings'
opening = { regex = 'Opening\s*:\s*(?P<value>[\d.,]+)' }
closing = { regex = 'Closing\s*:\s*(?P<value>[\d.,]+)' }
total_credit = { regex = 'Credits\s*:\s*(?P<value>[\d.,]+)' }
total_debit = { regex = 'Debits\s*:\s*(?P<value>[\d.,]+)' }

[sections.columns]
date = { x = [0, 10] }
description = { x = [10, 40] }
amount = { x = [40, 60] }
marker = { x = [60, 70] }
balance = { x = [70, 100] }

[sections.table]
row_when = { column = "date", regex = '^\d{2}/\d{2}$' }
skip = ['^Opening$']
date = { format = "%d/%m" }
direction = { out_marker = "D" }
"#;

    fn parser() -> Parser {
        compile_spec(
            toml::from_str(SPEC).unwrap(),
            PathBuf::from("test/config.toml"),
            "test.statement".into(),
        )
        .unwrap()
    }

    fn statement() -> Document {
        Document {
            pages: vec![
                page(
                    1,
                    vec![line(100.0, &[(0.0, "Period: 01/01/2026 - 31/01/2026")])],
                ),
                page(
                    2,
                    vec![
                        line(100.0, &[(0.0, "Savings")]),
                        line(
                            90.0,
                            &[(0.0, "01/01"), (10.0, "Opening"), (70.0, "500,000.00")],
                        ),
                        line(
                            80.0,
                            &[
                                (0.0, "05/01"),
                                (10.0, "Coffee"),
                                (40.0, "4,000.00"),
                                (60.0, "D"),
                                (70.0, "496,000.00"),
                            ],
                        ),
                        line(
                            70.0,
                            &[
                                (0.0, "10/01"),
                                (10.0, "Salary"),
                                (40.0, "5,000,000.00"),
                                (70.0, "5,496,000.00"),
                            ],
                        ),
                    ],
                ),
                page(
                    3,
                    vec![
                        line(100.0, &[(0.0, "Savings")]),
                        line(
                            90.0,
                            &[
                                (0.0, "20/01"),
                                (10.0, "Rent"),
                                (40.0, "1,000,000.00"),
                                (60.0, "D"),
                                (70.0, "4,496,000.00"),
                            ],
                        ),
                        line(40.0, &[(0.0, "Opening : 500,000.00")]),
                        line(30.0, &[(0.0, "Credits : 5,000,000.00")]),
                        line(20.0, &[(0.0, "Debits : 1,004,000.00")]),
                        line(10.0, &[(0.0, "Closing : 4,496,000.00")]),
                    ],
                ),
            ],
        }
    }

    #[test]
    fn parses_rows_across_pages_and_proves_the_balance() {
        let transactions = parser().parse(&statement(), None).unwrap();
        assert_eq!(transactions.len(), 3);
        assert_eq!(transactions[0].amount.amount_minor, -4_000);
        assert_eq!(transactions[1].amount.amount_minor, 5_000_000);
        assert_eq!(transactions[2].amount.amount_minor, -1_000_000);
        assert_eq!(
            transactions[0].occurred_on,
            NaiveDate::from_ymd_opt(2026, 1, 5).unwrap()
        );
        assert_eq!(
            transactions[2].occurred_on,
            NaiveDate::from_ymd_opt(2026, 1, 20).unwrap()
        );
        assert_eq!(transactions[0].account, "savings");
        assert_eq!(transactions[0].source.tx_index, 0);
    }

    #[test]
    fn ids_are_stable_and_content_derived() {
        let first = parser().parse(&statement(), None).unwrap();
        let second = parser().parse(&statement(), None).unwrap();
        assert_eq!(
            first.iter().map(|tx| tx.id.clone()).collect::<Vec<_>>(),
            second.iter().map(|tx| tx.id.clone()).collect::<Vec<_>>()
        );

        let third = parser()
            .parse(&statement(), Some("gmail:abc#att:0".into()))
            .unwrap();
        assert_eq!(first[0].id, third[0].id, "origin must not change identity");
    }

    #[test]
    fn a_broken_balance_rejects_the_document() {
        let mut document = statement();
        document.pages[2].lines[1].runs[4].text = "4,500,000.00".into();
        let error = parser().parse(&document, None).unwrap_err();
        assert!(error.contains("does not follow"), "{error}");
    }

    #[test]
    fn a_wrong_credit_total_rejects_the_document() {
        let mut document = statement();
        document.pages[2].lines[3].runs[0].text = "Credits : 5,000,001.00".into();
        let error = parser().parse(&document, None).unwrap_err();
        assert!(error.contains("parsed credits"), "{error}");
    }

    #[test]
    fn row_dates_resolve_against_a_period_that_crosses_a_year() {
        let mut document = statement();
        document.pages[0].lines[0].runs[0].text = "Period: 26/12/2026 - 25/01/2027".into();
        document.pages[1].lines[2].runs[0].text = "30/12".into();
        document.pages[1].lines[3].runs[0].text = "02/01".into();
        // The arithmetic is no longer meaningful in this fixture; check the
        // dates directly through the internal row parser.
        let start = NaiveDate::from_ymd_opt(2026, 12, 26).unwrap();
        let end = NaiveDate::from_ymd_opt(2027, 1, 25).unwrap();
        assert_eq!(
            parse_row_date("30/12", "%d/%m", start, end).unwrap(),
            NaiveDate::from_ymd_opt(2026, 12, 30).unwrap()
        );
        assert_eq!(
            parse_row_date("02/01", "%d/%m", start, end).unwrap(),
            NaiveDate::from_ymd_opt(2027, 1, 2).unwrap()
        );
        assert!(parse_row_date("26/02", "%d/%m", start, end).is_err());
        let _ = document;
    }

    #[test]
    fn a_statement_with_no_transactions_must_still_balance() {
        let document = Document {
            pages: vec![
                page(
                    1,
                    vec![line(100.0, &[(0.0, "Period: 01/01/2026 - 31/01/2026")])],
                ),
                page(
                    2,
                    vec![
                        line(100.0, &[(0.0, "Savings")]),
                        line(40.0, &[(0.0, "Opening : 500,000.00")]),
                        line(30.0, &[(0.0, "Credits : 0.00")]),
                        line(20.0, &[(0.0, "Debits : 0.00")]),
                        line(10.0, &[(0.0, "Closing : 500,000.00")]),
                    ],
                ),
            ],
        };
        assert!(parser().parse(&document, None).unwrap().is_empty());

        let mut broken = document;
        broken.pages[1].lines[4].runs[0].text = "Closing : 500,001.00".into();
        assert!(parser().parse(&broken, None).is_err());
    }
}
