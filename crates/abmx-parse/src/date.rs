//! Date parsing, including Indonesian month abbreviations.
//!
//! `chrono`'s `%b` only understands English, and Wallet writes `22 Okt 2025`, so a
//! translation pass is not optional.

use chrono::{NaiveDate, NaiveDateTime, NaiveTime};

/// Translate Indonesian month abbreviations into English before parsing.
///
/// Only the ones that differ need entries; `Jan`, `Feb`, `Mar`, `Apr`, `Jun`,
/// `Jul`, `Sep`, `Nov` are identical.
fn localize(value: &str, locale: &str) -> String {
    if locale != "id" {
        return value.to_string();
    }

    value
        .split(' ')
        .map(|token| match token.to_ascii_lowercase().as_str() {
            "mei" => "May",
            "agu" => "Aug",
            "okt" => "Oct",
            "des" => "Dec",
            "peb" => "Feb",
            _ => token,
        })
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

    Err(format!("`{value}` does not match any of {:?}", formats))
}

pub fn parse_date(value: &str, formats: &[String], locale: &str) -> Result<NaiveDate, String> {
    let value = localize(value.trim(), locale);

    for format in formats {
        if let Ok(parsed) = NaiveDate::parse_from_str(&value, format) {
            return Ok(parsed);
        }
    }

    Err(format!("`{value}` does not match any of {:?}", formats))
}

pub fn parse_time(value: &str, formats: &[String]) -> Result<NaiveTime, String> {
    let value = value.trim();

    for format in formats {
        if let Ok(parsed) = NaiveTime::parse_from_str(value, format) {
            return Ok(parsed);
        }
    }

    Err(format!("`{value}` does not match any of {:?}", formats))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn formats(list: &[&str]) -> Vec<String> {
        list.iter().map(|value| (*value).to_string()).collect()
    }

    #[test]
    fn indonesian_months() {
        let parsed = parse_datetime(
            "22 Okt 2025 22:02:51",
            &formats(&["%d %b %Y %H:%M:%S"]),
            "id",
        )
        .expect("parse");
        assert_eq!(
            parsed.date(),
            NaiveDate::from_ymd_opt(2025, 10, 22).unwrap()
        );

        let parsed = parse_date("18 Agu 2026", &formats(&["%d %b %Y"]), "id").expect("parse");
        assert_eq!(parsed, NaiveDate::from_ymd_opt(2026, 8, 18).unwrap());
    }

    #[test]
    fn english_months_and_unpadded_days() {
        let parsed = parse_datetime(
            "6 Sep 2026 15:24:15",
            &formats(&["%d %b %Y %H:%M:%S"]),
            "en",
        )
        .expect("parse");
        assert_eq!(parsed.date(), NaiveDate::from_ymd_opt(2026, 9, 6).unwrap());
    }

    #[test]
    fn split_date_and_time() {
        let day = parse_date("18 Sep 2026", &formats(&["%d %b %Y"]), "en").expect("date");
        let time = parse_time("12:54:46", &formats(&["%H:%M:%S"])).expect("time");
        assert_eq!(day.and_time(time).to_string(), "2026-09-18 12:54:46");
    }
}
