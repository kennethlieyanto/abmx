//! The statement host: positioned lines, typed extraction, and the arithmetic.
//!
//! A script turns a document into observations — a period, sections, and rows
//! with an optional printed balance. The host proves the arithmetic and builds
//! transactions. A script cannot declare a document strict, and cannot invent a
//! balance: the running balance is recomputed here.

use std::collections::{HashMap, HashSet};

use abmx_doc::{Document, Line};
use abmx_tx::{Extraction, Kind, Money, Source, Status, Transaction};
use chrono::{DateTime, FixedOffset, NaiveDate, TimeZone};
use regex::Regex;
use rhai::{Array, Dynamic, Engine, EvalAltResult, Map, Position};
use serde::Deserialize;
use sha2::{Digest, Sha256};

use crate::engine::{describe_runtime_error, Executor, Script};
use crate::spec::Meta;
use crate::util::{
    capture, collapse, month_period, parse_date, parse_decimal, parse_row_date, NumberStyle,
};

/// One line of positioned text, owned so a script can keep it in an array.
#[derive(Clone)]
pub struct LineObj {
    page: i64,
    y: f64,
    text: String,
    runs: Vec<(f64, String)>,
    number: NumberStyle,
}

impl LineObj {
    fn new(page: usize, line: &Line, number: NumberStyle) -> Self {
        Self {
            page: page as i64,
            y: line.y as f64,
            text: line.text(),
            runs: line
                .runs
                .iter()
                .map(|run| (run.x as f64, run.text.clone()))
                .collect(),
            number,
        }
    }

    fn runs_in(&self, x0: f64, x1: f64) -> Vec<String> {
        self.runs
            .iter()
            .filter(|(x, _)| *x >= x0 && *x < x1)
            .map(|(_, text)| text.clone())
            .collect()
    }
}

/// The document a `parse_statement(doc)` script sees.
#[derive(Clone)]
pub struct DocCtx {
    lines: Vec<LineObj>,
    text: String,
    page_count: i64,
    number: NumberStyle,
}

impl DocCtx {
    fn build(document: &Document, meta: &Meta) -> Self {
        let mut lines = Vec::new();
        let mut text = String::new();

        for page in &document.pages {
            for line in &page.lines {
                text.push_str(&line.text());
                text.push('\n');
                lines.push(LineObj::new(page.number, line, meta.number));
            }
        }

        Self {
            lines,
            text,
            page_count: document.page_count() as i64,
            number: meta.number,
        }
    }
}

fn fail(message: impl Into<String>) -> Box<EvalAltResult> {
    Box::new(EvalAltResult::ErrorRuntime(
        message.into().into(),
        Position::NONE,
    ))
}

fn regex(pattern: &str) -> Result<Regex, Box<EvalAltResult>> {
    Regex::new(pattern).map_err(|error| fail(format!("invalid regex `{pattern}`: {error}")))
}

fn format_vec(formats: Array) -> Vec<String> {
    formats
        .into_iter()
        .filter_map(|value| value.into_string().ok())
        .collect()
}

/// Register every `doc.*` and line function.
pub fn register(engine: &mut Engine) {
    engine.register_type_with_name::<DocCtx>("DocCtx");
    engine.register_type_with_name::<LineObj>("Line");

    engine.register_get("page_count", |doc: &mut DocCtx| doc.page_count);
    engine.register_get("text", |doc: &mut DocCtx| doc.text.clone());

    engine.register_get("text", |line: &mut LineObj| line.text.clone());
    engine.register_get("page", |line: &mut LineObj| line.page);
    engine.register_get("y", |line: &mut LineObj| line.y);

    engine.register_fn("lines", |doc: &mut DocCtx| {
        doc.lines
            .iter()
            .cloned()
            .map(Dynamic::from)
            .collect::<Array>()
    });

    engine.register_fn(
        "regex",
        |doc: &mut DocCtx, pattern: &str| -> Result<String, Box<EvalAltResult>> {
            Ok(capture(&regex(pattern)?, &doc.text).unwrap_or_default())
        },
    );

    engine.register_fn(
        "regex_group",
        |doc: &mut DocCtx, pattern: &str, group: i64| -> Result<String, Box<EvalAltResult>> {
            let regex = regex(pattern)?;
            let captures = regex
                .captures(&doc.text)
                .ok_or_else(|| fail(format!("no match for `{pattern}`")))?;
            let value = usize::try_from(group)
                .ok()
                .and_then(|index| captures.get(index))
                .ok_or_else(|| fail(format!("`{pattern}` has no group `{group}`")))?;
            Ok(value.as_str().trim().to_owned())
        },
    );

    engine.register_fn(
        "regex_group",
        |doc: &mut DocCtx, pattern: &str, group: &str| -> Result<String, Box<EvalAltResult>> {
            let regex = regex(pattern)?;
            let captures = regex
                .captures(&doc.text)
                .ok_or_else(|| fail(format!("no match for `{pattern}`")))?;
            let value = captures
                .name(group)
                .or_else(|| group.parse::<usize>().ok().and_then(|i| captures.get(i)))
                .ok_or_else(|| fail(format!("`{pattern}` has no group `{group}`")))?;
            Ok(value.as_str().trim().to_owned())
        },
    );

    engine.register_fn(
        "has",
        |doc: &mut DocCtx, pattern: &str| -> Result<bool, Box<EvalAltResult>> {
            Ok(regex(pattern)?.is_match(&doc.text))
        },
    );

    engine.register_fn(
        "money",
        |doc: &mut DocCtx, pattern: &str| -> Result<i64, Box<EvalAltResult>> {
            let raw = capture(&regex(pattern)?, &doc.text)
                .ok_or_else(|| fail(format!("money `{pattern}`: no match")))?;
            parse_decimal(&raw, doc.number, 2).map_err(fail)
        },
    );

    engine.register_fn(
        "int",
        |doc: &mut DocCtx, pattern: &str| -> Result<i64, Box<EvalAltResult>> {
            let raw = capture(&regex(pattern)?, &doc.text)
                .ok_or_else(|| fail(format!("int `{pattern}`: no match")))?;
            raw.replace(',', ".")
                .parse::<i64>()
                .map_err(|error| fail(format!("int `{pattern}`: `{raw}`: {error}")))
        },
    );

    engine.register_fn(
        "money_text",
        |doc: &mut DocCtx, raw: &str| -> Result<i64, Box<EvalAltResult>> {
            if raw.trim().is_empty() {
                return Err(fail("money_text(): the value is empty"));
            }
            parse_decimal(raw, doc.number, 2).map_err(fail)
        },
    );

    engine.register_fn(
        "date_text",
        |_doc: &mut DocCtx,
         raw: &str,
         formats: Array,
         locale: &str|
         -> Result<String, Box<EvalAltResult>> {
            parse_date(raw, &format_vec(formats), locale)
                .map(|date| date.format("%Y-%m-%d").to_string())
                .map_err(fail)
        },
    );

    engine.register_fn(
        "month",
        |doc: &mut DocCtx, label: &str| -> Result<Map, Box<EvalAltResult>> {
            let (start, end) = month_period(&doc.text, label).map_err(fail)?;
            let mut map = Map::new();
            map.insert(
                "start".into(),
                Dynamic::from(start.format("%Y-%m-%d").to_string()),
            );
            map.insert(
                "end".into(),
                Dynamic::from(end.format("%Y-%m-%d").to_string()),
            );
            Ok(map)
        },
    );

    // Line helpers.

    engine.register_fn("cell", |line: &mut LineObj, x0: f64, x1: f64| {
        collapse(&line.runs_in(x0, x1).join(""))
    });

    engine.register_fn("words", |line: &mut LineObj, x0: f64, x1: f64| {
        collapse(&line.runs_in(x0, x1).join(" "))
    });

    engine.register_fn(
        "matches",
        |line: &mut LineObj, pattern: &str, x0: f64, x1: f64| -> Result<bool, Box<EvalAltResult>> {
            let cell = collapse(&line.runs_in(x0, x1).join(""));
            Ok(regex(pattern)?.is_match(&cell))
        },
    );

    engine.register_fn(
        "contains",
        |line: &mut LineObj, needle: &str, x0: f64, x1: f64| {
            collapse(&line.runs_in(x0, x1).join("")).contains(needle)
        },
    );

    engine.register_fn(
        "money",
        |line: &mut LineObj, x0: f64, x1: f64| -> Result<i64, Box<EvalAltResult>> {
            let cell = collapse(&line.runs_in(x0, x1).join(""));
            if cell.is_empty() {
                return Err(fail("money(): the cell is empty"));
            }
            parse_decimal(&cell, line.number, 2).map_err(fail)
        },
    );

    engine.register_fn(
        "money_any",
        |line: &mut LineObj, x0: f64, x1: f64| -> Dynamic {
            let cell = collapse(&line.runs_in(x0, x1).join(""));
            if cell.is_empty() {
                return Dynamic::UNIT;
            }
            match parse_decimal(&cell, line.number, 2) {
                Ok(value) => Dynamic::from(value),
                Err(_) => Dynamic::UNIT,
            }
        },
    );
}

/// What `parse_statement(doc)` returns.
#[derive(Debug, Deserialize)]
pub struct ObservedStatement {
    pub period: PeriodObserved,
    /// Used only for document identity, never emitted.
    #[serde(default)]
    pub account: Option<String>,
    /// The page count printed in the footer, when there is one.
    #[serde(default)]
    pub pages: Option<usize>,
    pub sections: Vec<ObservedSection>,
}

#[derive(Debug, Deserialize)]
pub struct PeriodObserved {
    pub start: String,
    pub end: String,
}

#[derive(Debug, Deserialize)]
pub struct ObservedSection {
    pub key: String,
    /// Scale-2 value, e.g. `278301099` for 2,783,010.99.
    pub opening: i64,
    pub closing: i64,
    #[serde(default)]
    pub total_credit: Option<i64>,
    #[serde(default)]
    pub total_debit: Option<i64>,
    #[serde(default = "default_date_format")]
    pub date_format: String,
    #[serde(default)]
    pub rows: Vec<ObservedRow>,
}

#[derive(Debug, Deserialize)]
pub struct ObservedRow {
    /// Raw printed date, resolved against the period with `date_format`.
    pub date: String,
    #[serde(default)]
    pub description: String,
    /// Unsigned scale-2 amount.
    pub amount: i64,
    #[serde(default)]
    pub out: bool,
    /// The printed balance, when the bank prints one on this row.
    #[serde(default)]
    pub balance: Option<i64>,
}

fn default_date_format() -> String {
    "%d/%m".to_owned()
}

struct VerifiedRow {
    date: NaiveDate,
    description: String,
    amount: i64,
    out: bool,
    balance: i64,
}

struct VerifiedSection<'a> {
    observed: &'a ObservedSection,
    rows: Vec<VerifiedRow>,
}

/// Run `parse_statement(doc)`, verify it, and build transactions.
pub fn parse(
    script: &Script,
    executor: &Executor,
    document: &Document,
    origin: Option<String>,
) -> Result<Vec<Transaction>, String> {
    let doc = DocCtx::build(document, &script.meta);
    let value: Dynamic = executor
        .call(&script.ast, "parse_statement", (doc,))
        .map_err(|error| describe_runtime_error("parse_statement", &error))?;

    let observed: ObservedStatement = rhai::serde::from_dynamic(&value)
        .map_err(|error| format!("parse_statement must return a statement map: {error}"))?;

    let (start, end) = period(&observed.period)?;
    check_page_count(observed.pages, document)?;

    let account = observed
        .account
        .clone()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| script.id.clone());
    let document_id = document_id(&script.meta.bank, &account, start, end);

    if observed.sections.is_empty() {
        return Err("parse_statement returned no sections".to_owned());
    }

    let mut keys = HashSet::new();
    let mut verified = Vec::new();
    for section in &observed.sections {
        if section.key.trim().is_empty() {
            return Err("a section has an empty key".to_owned());
        }
        if !keys.insert(section.key.clone()) {
            return Err(format!("duplicate section key `{}`", section.key));
        }
        verified.push(verify(section, start, end)?);
    }

    let mut transactions = Vec::new();
    let mut ordinals: HashMap<String, u32> = HashMap::new();

    for section in &verified {
        for (index, row) in section.rows.iter().enumerate() {
            let base = content_key(&document_id, &section.observed.key, row);
            let ordinal = ordinals.entry(base.clone()).or_insert(0);
            let id = row_id(&base, *ordinal);
            *ordinal += 1;

            transactions.push(transaction(
                script,
                section.observed,
                row,
                id,
                index as u32,
                &document_id,
                origin.clone(),
            )?);
        }
    }

    Ok(transactions)
}

fn period(observed: &PeriodObserved) -> Result<(NaiveDate, NaiveDate), String> {
    let start = NaiveDate::parse_from_str(observed.start.trim(), "%Y-%m-%d")
        .map_err(|error| format!("period start `{}`: {error}", observed.start))?;
    let end = NaiveDate::parse_from_str(observed.end.trim(), "%Y-%m-%d")
        .map_err(|error| format!("period end `{}`: {error}", observed.end))?;

    if start > end {
        return Err(format!("period starts after it ends: {start} > {end}"));
    }
    Ok((start, end))
}

fn check_page_count(printed: Option<usize>, document: &Document) -> Result<(), String> {
    let Some(printed) = printed else {
        return Ok(());
    };
    if printed != document.page_count() {
        return Err(format!(
            "the PDF says it has {printed} pages but {} were read; the download may be truncated",
            document.page_count()
        ));
    }
    Ok(())
}

fn verify(
    section: &ObservedSection,
    start: NaiveDate,
    end: NaiveDate,
) -> Result<VerifiedSection<'_>, String> {
    let key = &section.key;
    let mut previous = section.opening;
    let mut credits = 0i64;
    let mut debits = 0i64;
    let mut rows = Vec::with_capacity(section.rows.len());

    for (index, row) in section.rows.iter().enumerate() {
        if row.amount <= 0 {
            return Err(format!(
                "section `{key}`: row {} has a zero or negative amount; give the unsigned magnitude",
                index + 1
            ));
        }

        let date = parse_row_date(&row.date, &section.date_format, start, end)
            .map_err(|error| format!("section `{key}`: row {}: {error}", index + 1))?;

        let delta = if row.out { -row.amount } else { row.amount };
        let expected = previous
            .checked_add(delta)
            .ok_or_else(|| format!("section `{key}`: balance overflow at row {}", index + 1))?;

        if let Some(printed) = row.balance {
            if printed != expected {
                return Err(format!(
                    "section `{key}`: row {} ({date}) balance {} does not follow {} {:+.2} (expected {})",
                    index + 1,
                    scaled(printed),
                    scaled(previous),
                    if row.out {
                        -(row.amount as f64) / 100.0
                    } else {
                        row.amount as f64 / 100.0
                    },
                    scaled(expected)
                ));
            }
        }

        if row.out {
            debits += row.amount;
        } else {
            credits += row.amount;
        }

        rows.push(VerifiedRow {
            date,
            description: row.description.clone(),
            amount: row.amount,
            out: row.out,
            balance: expected,
        });
        previous = expected;
    }

    if previous != section.closing {
        return Err(format!(
            "section `{key}`: the running balance ends at {} but the statement closes at {}",
            scaled(previous),
            scaled(section.closing)
        ));
    }

    if let Some(total) = section.total_credit {
        if credits != total {
            return Err(format!(
                "section `{key}`: parsed credits {} but the statement prints {}",
                scaled(credits),
                scaled(total)
            ));
        }
    }
    if let Some(total) = section.total_debit {
        if debits != total {
            return Err(format!(
                "section `{key}`: parsed debits {} but the statement prints {}",
                scaled(debits),
                scaled(total)
            ));
        }
    }

    Ok(VerifiedSection {
        observed: section,
        rows,
    })
}

fn transaction(
    script: &Script,
    section: &ObservedSection,
    row: &VerifiedRow,
    id: String,
    index: u32,
    document_id: &str,
    origin: Option<String>,
) -> Result<Transaction, String> {
    let currency = &script.meta.currency;
    let exponent = script.meta.exponent();
    let minor = scale_to_minor(row.amount, exponent)?;
    let signed = if row.out { -minor } else { minor };

    let naive = row
        .date
        .and_hms_opt(0, 0, 0)
        .ok_or_else(|| format!("invalid transaction date {}", row.date))?;
    let occurred_at: DateTime<FixedOffset> = script
        .meta
        .offset
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
        parser: script.id.clone(),
        parser_version: script.meta.version,
        bank: script.meta.bank.clone(),
        account: section.key.clone(),
        kind: Kind::Unknown,
        status: Status::Settled,
        amount: Money::new(signed, currency.clone()),
        currency_exponent: exponent,
        amount_decimal: crate::util::decimal_with(signed, exponent),
        occurred_at,
        occurred_on: row.date,
        timezone: script.meta.tz.clone(),
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

fn scaled(value: i64) -> String {
    let sign = if value < 0 { "-" } else { "" };
    let absolute = value.unsigned_abs();
    format!("{sign}{}.{:02}", absolute / 100, absolute % 100)
}

fn scale_to_minor(value: i64, exponent: u32) -> Result<i64, String> {
    if exponent <= 2 {
        let divisor = 10i64.pow(2 - exponent);
        if value % divisor != 0 {
            return Err(format!(
                "amount {} has more precision than a {exponent} exponent allows",
                scaled(value)
            ));
        }
        Ok(value / divisor)
    } else {
        value
            .checked_mul(10i64.pow(exponent - 2))
            .ok_or_else(|| format!("amount {} overflows", scaled(value)))
    }
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

fn content_key(document_id: &str, section: &str, row: &VerifiedRow) -> String {
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

    fn document() -> Document {
        Document {
            pages: vec![
                page(1, vec![line(100.0, &[(0.0, "Period: OCTOBER 2026")])]),
                page(
                    2,
                    vec![
                        line(100.0, &[(0.0, "Savings")]),
                        line(
                            90.0,
                            &[(0.0, "01/10"), (10.0, "Opening"), (70.0, "500,000.00")],
                        ),
                        line(
                            80.0,
                            &[
                                (0.0, "05/10"),
                                (10.0, "Coffee"),
                                (40.0, "4,000.00"),
                                (60.0, "D"),
                                (70.0, "496,000.00"),
                            ],
                        ),
                        line(
                            70.0,
                            &[
                                (0.0, "10/10"),
                                (10.0, "Salary"),
                                (40.0, "5,000,000.00"),
                                (70.0, "5,496,000.00"),
                            ],
                        ),
                        line(
                            60.0,
                            &[
                                (0.0, "20/10"),
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

    const SCRIPT: &str = r#"
fn meta() {
    #{ bank: "test", currency: "IDR", offset: "+07:00", number: "en", api: 1, version: 1 }
}

fn parse_statement(doc) {
    let period = doc.month("Period");
    let rows = [];
    for line in doc.lines() {
        if !line.matches(`^\d{2}/\d{2}$`, 0.0, 10.0) { continue; }
        let description = line.cell(10.0, 40.0);
        if description == "Opening" { continue; }
        rows.push(#{
            date: line.cell(0.0, 10.0),
            description: description,
            amount: line.money(40.0, 60.0),
            out: line.cell(60.0, 70.0) == "D",
            balance: line.money_any(70.0, 100.0),
        });
    }
    #{
        period: period,
        sections: [#{
            key: "savings",
            opening: doc.money(`Opening\s*:\s*([\d.,]+)`),
            closing: doc.money(`Closing\s*:\s*([\d.,]+)`),
            total_credit: doc.money(`Credits\s*:\s*([\d.,]+)`),
            total_debit: doc.money(`Debits\s*:\s*([\d.,]+)`),
            rows: rows,
        }],
    }
}
"#;

    fn script() -> (crate::engine::Script, Executor) {
        let engine = crate::engine::engine();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("parser.rhai");
        std::fs::write(&path, SCRIPT).unwrap();
        let script = crate::engine::Script::load(&engine, &path, "test.statement".into()).unwrap();
        drop(dir);
        (script, engine)
    }

    #[test]
    fn proves_the_balance_and_builds_rows() {
        let (script, engine) = script();
        let transactions = parse(&script, &engine, &document(), None).unwrap();
        assert_eq!(transactions.len(), 3);
        assert_eq!(transactions[0].amount.amount_minor, -4_000);
        assert_eq!(transactions[1].amount.amount_minor, 5_000_000);
        assert_eq!(transactions[2].amount.amount_minor, -1_000_000);
    }

    #[test]
    fn a_broken_balance_rejects_the_document() {
        let (script, engine) = script();
        let mut doc = document();
        doc.pages[1].lines[3].runs[3].text = "3,049,000.00".into();
        let error = parse(&script, &engine, &doc, None).unwrap_err();
        assert!(error.contains("does not follow"), "{error}");
    }

    #[test]
    fn rows_without_a_printed_balance_are_walked() {
        let (script, engine) = script();
        let mut doc = document();
        doc.pages[1].lines[2].runs.pop(); // Coffee row loses its balance
        let transactions = parse(&script, &engine, &doc, None).unwrap();
        assert_eq!(transactions.len(), 3);
        assert_eq!(transactions[0].amount.amount_minor, -4_000);
    }
}
