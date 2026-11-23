//! Shared parsing helpers: money, dates, labels.
//!
//! These are bank-agnostic. Everything bank-specific lives in `parser.rhai`.

use chrono::{Datelike, NaiveDate, NaiveDateTime, NaiveTime};
use regex::Regex;
use serde::Deserialize;

/// How a bank writes decimals: `en` is `1,234.56`, `id` is `1.234,56`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum NumberStyle {
    #[default]
    En,
    Id,
}

impl NumberStyle {
    pub fn parse(value: &str) -> Result<Self, String> {
        match value.trim().to_ascii_lowercase().as_str() {
            "en" => Ok(Self::En),
            "id" => Ok(Self::Id),
            other => Err(format!("number style `{other}` is not `en` or `id`")),
        }
    }
}

/// Parse a money string into integer minor units.
///
/// `exponent` is the currency's ISO-4217 minor-unit exponent (IDR = 0).
pub fn parse_decimal(raw: &str, style: NumberStyle, exponent: u32) -> Result<i64, String> {
    let (thousands, decimal) = match style {
        NumberStyle::En => (',', '.'),
        NumberStyle::Id => ('.', ','),
    };

    // Keep only characters that can be part of the number; this tolerates
    // "IDR 11.000" when a regex was not precise.
    let cleaned: String = raw
        .chars()
        .filter(|ch| ch.is_ascii_digit() || *ch == thousands || *ch == decimal || *ch == '-')
        .collect();

    let negative = cleaned.starts_with('-');
    let cleaned = cleaned.trim_start_matches('-');
    if cleaned.is_empty() {
        return Err(format!("`{raw}` contains no digits"));
    }

    let (integer_part, fraction_part) = match cleaned.rsplit_once(decimal) {
        Some((integer, fraction)) => (integer, fraction),
        None => (cleaned, ""),
    };

    let integer_digits: String = integer_part.chars().filter(|ch| *ch != thousands).collect();

    if integer_digits.is_empty() || !integer_digits.chars().all(|ch| ch.is_ascii_digit()) {
        return Err(format!("`{raw}` is not a valid integer part"));
    }

    let mut fraction: String = fraction_part
        .chars()
        .filter(|ch| ch.is_ascii_digit())
        .collect();

    let exponent_usize = exponent as usize;
    if fraction.len() > exponent_usize {
        fraction.truncate(exponent_usize);
    }
    while fraction.len() < exponent_usize {
        fraction.push('0');
    }

    let integer: i128 = integer_digits
        .parse()
        .map_err(|_| format!("`{raw}` overflows"))?;
    let fraction: i128 = if exponent_usize == 0 {
        0
    } else {
        fraction
            .parse()
            .map_err(|_| format!("`{raw}` has a bad fraction"))?
    };

    let scale = 10i128.pow(exponent);
    let mut minor = integer
        .checked_mul(scale)
        .and_then(|value| value.checked_add(fraction))
        .ok_or_else(|| format!("`{raw}` overflows"))?;

    if negative {
        minor = -minor;
    }

    i64::try_from(minor).map_err(|_| format!("`{raw}` overflows"))
}

/// Indonesian and English month names, full and abbreviated.
fn english_month(token: &str) -> Option<&'static str> {
    match token.to_ascii_lowercase().as_str() {
        // Full names
        "januari" | "january" => Some("January"),
        "februari" | "pebruari" | "february" => Some("February"),
        "maret" | "march" => Some("March"),
        "april" => Some("April"),
        "mei" | "may" => Some("May"),
        "juni" | "june" => Some("June"),
        "juli" | "july" => Some("July"),
        "agustus" | "august" => Some("August"),
        "september" => Some("September"),
        "oktober" | "october" => Some("October"),
        "november" => Some("November"),
        "desember" | "december" => Some("December"),
        // Abbreviations
        "jan" => Some("Jan"),
        "feb" | "peb" => Some("Feb"),
        "mar" => Some("Mar"),
        "apr" => Some("Apr"),
        "jun" => Some("Jun"),
        "jul" => Some("Jul"),
        "agu" | "aug" => Some("Aug"),
        "sep" | "sept" => Some("Sep"),
        "okt" | "oct" => Some("Oct"),
        "nov" => Some("Nov"),
        "des" | "dec" => Some("Dec"),
        _ => None,
    }
}

/// Translate Indonesian month tokens into English so `chrono` can parse them.
///
/// Both full names and abbreviations are supported. `locale` is `en` or `id`;
/// `en` passes the value through untouched.
fn localize(value: &str, locale: &str) -> String {
    if locale != "id" {
        return value.to_string();
    }

    value
        .split(' ')
        .map(|token| english_month(token).unwrap_or(token))
        .collect::<Vec<_>>()
        .join(" ")
}

pub fn parse_datetime(
    value: &str,
    formats: &[String],
    locale: &str,
) -> Result<NaiveDateTime, String> {
    let value = localize(value.trim(), locale);

    for format in formats {
        if let Ok(parsed) = NaiveDateTime::parse_from_str(&value, format) {
            return Ok(parsed);
        }
    }

    Err(format!("`{value}` does not match any of {formats:?}"))
}

pub fn parse_date(value: &str, formats: &[String], locale: &str) -> Result<NaiveDate, String> {
    let value = localize(value.trim(), locale);

    for format in formats {
        if let Ok(parsed) = NaiveDate::parse_from_str(&value, format) {
            return Ok(parsed);
        }
    }

    Err(format!("`{value}` does not match any of {formats:?}"))
}

pub fn parse_time(value: &str, formats: &[String]) -> Result<NaiveTime, String> {
    let value = value.trim();

    for format in formats {
        if let Ok(parsed) = NaiveTime::parse_from_str(value, format) {
            return Ok(parsed);
        }
    }

    Err(format!("`{value}` does not match any of {formats:?}"))
}

/// Parse the datetime inside a prose region.
///
/// Banks append a zone token (`13 Sep 2026 12:44:09 WIB`) or surrounding text, so
/// the whole region rarely matches a format. Try the whole value, then every
/// contiguous token window a datetime can occupy.
pub fn parse_datetime_loose(
    value: &str,
    formats: &[String],
    locale: &str,
) -> Result<NaiveDateTime, String> {
    if let Ok(parsed) = parse_datetime(value, formats, locale) {
        return Ok(parsed);
    }

    for window in token_windows(value, 2, 8) {
        if let Ok(parsed) = parse_datetime(&window, formats, locale) {
            return Ok(parsed);
        }
    }

    Err(format!(
        "`{}` contains no datetime matching {formats:?}",
        value.trim()
    ))
}

pub fn parse_date_loose(
    value: &str,
    formats: &[String],
    locale: &str,
) -> Result<NaiveDate, String> {
    if let Ok(parsed) = parse_date(value, formats, locale) {
        return Ok(parsed);
    }

    for window in token_windows(value, 1, 5) {
        if let Ok(parsed) = parse_date(&window, formats, locale) {
            return Ok(parsed);
        }
    }

    Err(format!(
        "`{}` contains no date matching {formats:?}",
        value.trim()
    ))
}

pub fn parse_time_loose(value: &str, formats: &[String]) -> Result<NaiveTime, String> {
    if let Ok(parsed) = parse_time(value, formats) {
        return Ok(parsed);
    }

    for window in token_windows(value, 1, 3) {
        if let Ok(parsed) = parse_time(&window, formats) {
            return Ok(parsed);
        }
    }

    Err(format!(
        "`{}` contains no time matching {formats:?}",
        value.trim()
    ))
}

fn token_windows(value: &str, min: usize, max: usize) -> Vec<String> {
    let tokens: Vec<&str> = value.split_whitespace().collect();
    let mut out = Vec::new();

    for size in min..=max {
        if size > tokens.len() {
            break;
        }
        for start in 0..=(tokens.len() - size) {
            out.push(tokens[start..start + size].join(" "));
        }
    }

    out
}

/// Resolve a row date that may omit the year against the statement period.
pub fn parse_row_date(
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

/// Exact decimal rendering for a given minor-unit exponent.
pub fn decimal_with(amount_minor: i64, exponent: u32) -> String {
    let sign = if amount_minor < 0 { "-" } else { "" };
    let absolute = amount_minor.unsigned_abs();

    if exponent == 0 {
        format!("{sign}{absolute}")
    } else {
        let factor = 10u64.pow(exponent);
        format!(
            "{sign}{}.{:0width$}",
            absolute / factor,
            absolute % factor,
            width = exponent as usize
        )
    }
}

/// First and last day of the month printed after `label`.
///
/// This is the escape hatch for statements that print `PERIODE : OKTOBER 2026`
/// instead of a numeric range: the parser asks for a month and gets a range.
pub fn month_period(text: &str, label: &str) -> Result<(NaiveDate, NaiveDate), String> {
    let pattern = format!(
        r"(?i){}\s*[:\-]?\s*[^\n]{{0,80}}?\b(januari|februari|pebruari|maret|april|mei|juni|juli|agustus|september|oktober|november|desember|january|february|march|april|may|june|july|august|september|october|november|december)\s+(\d{{4}})\b",
        regex::escape(label)
    );
    let regex = Regex::new(&pattern).map_err(|error| format!("month period regex: {error}"))?;
    let captures = regex
        .captures(text)
        .ok_or_else(|| format!("no month and year found after `{label}`"))?;

    let month_token = captures.get(1).map(|m| m.as_str().to_ascii_lowercase());
    let year: i32 = captures
        .get(2)
        .and_then(|m| m.as_str().parse().ok())
        .ok_or_else(|| "the year after the month is not a number".to_owned())?;

    let month = month_token
        .as_deref()
        .and_then(month_index)
        .ok_or_else(|| "unrecognised month name".to_owned())?;

    let start = NaiveDate::from_ymd_opt(year, month, 1)
        .ok_or_else(|| format!("invalid month {year}-{month}"))?;
    let next = if month == 12 {
        NaiveDate::from_ymd_opt(year + 1, 1, 1)
    } else {
        NaiveDate::from_ymd_opt(year, month + 1, 1)
    }
    .ok_or_else(|| format!("invalid month {year}-{month}"))?;
    let end = next
        .pred_opt()
        .ok_or_else(|| "invalid month end".to_owned())?;

    Ok((start, end))
}

fn month_index(token: &str) -> Option<u32> {
    match token {
        "januari" | "january" => Some(1),
        "februari" | "pebruari" | "february" => Some(2),
        "maret" | "march" => Some(3),
        "april" => Some(4),
        "mei" | "may" => Some(5),
        "juni" | "june" => Some(6),
        "juli" | "july" => Some(7),
        "agustus" | "august" => Some(8),
        "september" => Some(9),
        "oktober" | "october" => Some(10),
        "november" => Some(11),
        "desember" | "december" => Some(12),
        _ => None,
    }
}

/// Normalized label form: case-insensitive, whitespace collapsed, trailing
/// colon removed.
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

/// Collapse runs of whitespace to single spaces.
pub fn collapse(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// A parser id is a single directory name: no separators, not hidden.
pub fn is_parser_id(id: &str) -> bool {
    !id.is_empty()
        && !id.starts_with('.')
        && !id.contains('/')
        && !id.contains('\\')
        && id != "fixtures"
}

/// Capture `group`, or the first named group, or group 1, or the whole match.
pub fn capture(regex: &Regex, haystack: &str) -> Option<String> {
    let captures = regex.captures(haystack)?;

    for name in regex.capture_names().flatten() {
        if let Some(found) = captures.name(name) {
            let value = found.as_str().trim();
            if !value.is_empty() {
                return Some(value.to_owned());
            }
        }
    }

    captures
        .get(1)
        .or_else(|| captures.get(0))
        .map(|found| found.as_str().trim().to_owned())
        .filter(|value| !value.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn formats(list: &[&str]) -> Vec<String> {
        list.iter().map(|value| (*value).to_string()).collect()
    }

    #[test]
    fn indonesian_months_full_and_short() {
        let parsed = parse_datetime(
            "22 Okt 2025 22:02:51",
            &formats(&["%d %b %Y %H:%M:%S"]),
            "id",
        )
        .unwrap();
        assert_eq!(
            parsed.date(),
            NaiveDate::from_ymd_opt(2025, 10, 22).unwrap()
        );

        let parsed = parse_date("18 AGUSTUS 2026", &formats(&["%d %B %Y"]), "id").unwrap();
        assert_eq!(parsed, NaiveDate::from_ymd_opt(2026, 8, 18).unwrap());
    }

    #[test]
    fn month_period_understands_prose_periods() {
        let (start, end) = month_period("PERIODE : OKTOBER 2026", "PERIODE").unwrap();
        assert_eq!(start, NaiveDate::from_ymd_opt(2026, 10, 1).unwrap());
        assert_eq!(end, NaiveDate::from_ymd_opt(2026, 10, 31).unwrap());

        let (start, end) = month_period("Period : February 2027", "Period").unwrap();
        assert_eq!(start, NaiveDate::from_ymd_opt(2027, 2, 1).unwrap());
        assert_eq!(end, NaiveDate::from_ymd_opt(2027, 2, 28).unwrap());
    }

    #[test]
    fn row_dates_resolve_across_a_year_boundary() {
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
    }

    #[test]
    fn money_styles() {
        assert_eq!(parse_decimal("11.000", NumberStyle::Id, 0), Ok(11_000));
        assert_eq!(parse_decimal("372,000.00", NumberStyle::En, 0), Ok(372_000));
        assert_eq!(parse_decimal("1,234.56", NumberStyle::En, 2), Ok(123_456));
    }

    #[test]
    fn norm_is_forgiving() {
        assert_eq!(norm("Nominal Transfer:"), "nominal transfer");
        assert_eq!(norm("  Nominal   Transfer "), "nominal transfer");
    }
}
