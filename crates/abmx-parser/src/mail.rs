//! The mail host: block regions, typed extraction, and transaction building.
//!
//! A script only produces observations. The host assigns identity, applies the
//! offset from the email, and decides `Strict` versus `Heuristic`.

use std::collections::{BTreeMap, HashMap};

use abmx_tx::{Block, Extraction, Kind, Money, NormalizedEmail, Status, Transaction};
use chrono::{FixedOffset, NaiveDate, NaiveDateTime, TimeZone};
use regex::Regex;
use rhai::{Array, Dynamic, Engine, EvalAltResult, Position};
use serde::Deserialize;

use crate::engine::{describe_runtime_error, Executor, Script};
use crate::spec::Meta;
use crate::util::{
    capture, collapse, norm, parse_date, parse_date_loose, parse_datetime_loose, parse_decimal,
    parse_time, parse_time_loose, NumberStyle,
};

/// Everything a script may see about one email.
#[derive(Clone)]
pub struct MailCtx {
    from_email: String,
    from_name: String,
    subject: String,
    sent_at: String,
    offset: String,
    blocks: Vec<String>,
    regions: HashMap<String, Vec<String>>,
    exponent: u32,
}

impl MailCtx {
    fn build(email: &NormalizedEmail, meta: &Meta) -> Self {
        let labels: Vec<String> = meta.labels.clone();
        let regions = build_regions(&email.blocks, &labels);

        Self {
            from_email: email.from.email.clone(),
            from_name: email.from.name.clone().unwrap_or_default(),
            subject: email.subject.clone(),
            sent_at: email.date.to_rfc3339(),
            offset: email.date.offset().to_string(),
            blocks: email
                .blocks
                .iter()
                .map(|block| block.text.clone())
                .collect(),
            regions,
            exponent: meta.exponent(),
        }
    }

    fn region(&self, label: &str) -> Vec<String> {
        self.regions.get(&norm(label)).cloned().unwrap_or_default()
    }
}

/// Blocks grouped by the label that precedes them.
///
/// A "region" is everything between one label and the next, which makes values
/// that span several blocks (`Rp` / `100.000` / `,00`) fall out naturally.
pub fn build_regions(blocks: &[Block], labels: &[String]) -> HashMap<String, Vec<String>> {
    let label_set: std::collections::HashSet<String> =
        labels.iter().map(|label| norm(label)).collect();
    let mut regions: HashMap<String, Vec<String>> = HashMap::new();
    let mut current: Option<String> = None;

    for block in blocks {
        let key = norm(&block.text);
        if label_set.contains(&key) {
            current = Some(key.clone());
            regions.entry(key).or_default();
        } else if let Some(label) = &current {
            regions
                .entry(label.clone())
                .or_default()
                .push(block.text.clone());
        }
    }

    regions
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

fn number_style(value: &str) -> Result<NumberStyle, Box<EvalAltResult>> {
    NumberStyle::parse(value).map_err(fail)
}

fn format_vec(formats: Array) -> Vec<String> {
    formats
        .into_iter()
        .filter_map(|value| value.into_string().ok())
        .collect()
}

/// Register every `ctx.*` function and getter.
pub fn register(engine: &mut Engine) {
    engine.register_type_with_name::<MailCtx>("MailCtx");

    engine.register_get("from", |ctx: &mut MailCtx| ctx.from_email.clone());
    engine.register_get("from_name", |ctx: &mut MailCtx| ctx.from_name.clone());
    engine.register_get("subject", |ctx: &mut MailCtx| ctx.subject.clone());
    engine.register_get("sent_at", |ctx: &mut MailCtx| ctx.sent_at.clone());
    engine.register_get("offset", |ctx: &mut MailCtx| ctx.offset.clone());
    engine.register_get("blocks", |ctx: &mut MailCtx| {
        ctx.blocks
            .iter()
            .map(|block| Dynamic::from(block.clone()))
            .collect::<Array>()
    });
    engine.register_get("text", |ctx: &mut MailCtx| ctx.blocks.join(" "));

    engine.register_fn("has_label", |ctx: &mut MailCtx, label: &str| {
        ctx.regions.contains_key(&norm(label))
    });

    engine.register_fn("region", |ctx: &mut MailCtx, label: &str| ctx.region(label));

    engine.register_fn("text_in", |ctx: &mut MailCtx, label: &str| {
        collapse(&ctx.region(label).join(" "))
    });

    engine.register_fn("block_in", |ctx: &mut MailCtx, label: &str, index: i64| {
        if index < 0 {
            return String::new();
        }
        ctx.region(label)
            .get(index as usize)
            .cloned()
            .unwrap_or_default()
    });

    engine.register_fn(
        "regex_in",
        |ctx: &mut MailCtx, label: &str, pattern: &str| -> Result<String, Box<EvalAltResult>> {
            let regex = regex(pattern)?;
            let joined = ctx.region(label).join(" ");
            Ok(capture(&regex, &joined).unwrap_or_default())
        },
    );

    engine.register_fn(
        "per_block",
        |ctx: &mut MailCtx, label: &str, pattern: &str| -> Result<String, Box<EvalAltResult>> {
            let regex = regex(pattern)?;
            for block in ctx.region(label) {
                if let Some(value) = capture(&regex, &block) {
                    return Ok(value);
                }
            }
            Ok(String::new())
        },
    );

    engine.register_fn(
        "matches_in",
        |ctx: &mut MailCtx, label: &str, pattern: &str| -> Result<bool, Box<EvalAltResult>> {
            Ok(regex(pattern)?.is_match(&ctx.region(label).join(" ")))
        },
    );

    engine.register_fn("strip_spaces", |value: &str| {
        value
            .chars()
            .filter(|ch| !ch.is_whitespace())
            .collect::<String>()
    });

    register_money(engine);
    register_dates(engine);
}

fn register_money(engine: &mut Engine) {
    engine.register_fn(
        "money_in",
        |ctx: &mut MailCtx, label: &str, style: &str| -> Result<i64, Box<EvalAltResult>> {
            let raw = ctx.region(label).join(" ");
            if raw.trim().is_empty() {
                return Err(fail(format!("label `{label}` is not present")));
            }
            parse_decimal(&raw, number_style(style)?, ctx.exponent).map_err(fail)
        },
    );

    engine.register_fn(
        "money_regex_in",
        |ctx: &mut MailCtx,
         label: &str,
         pattern: &str,
         style: &str|
         -> Result<i64, Box<EvalAltResult>> {
            let regex = regex(pattern)?;
            let raw = capture(&regex, &ctx.region(label).join(" "))
                .ok_or_else(|| fail(format!("label `{label}`: no match for `{pattern}`")))?;
            parse_decimal(&raw, number_style(style)?, ctx.exponent).map_err(fail)
        },
    );

    engine.register_fn(
        "money",
        |ctx: &mut MailCtx, raw: &str, style: &str| -> Result<i64, Box<EvalAltResult>> {
            if raw.trim().is_empty() {
                return Err(fail("money(): the value is empty"));
            }
            parse_decimal(raw, number_style(style)?, ctx.exponent).map_err(fail)
        },
    );
}

fn register_dates(engine: &mut Engine) {
    engine.register_fn(
        "datetime_in",
        |ctx: &mut MailCtx,
         label: &str,
         formats: Array,
         locale: &str|
         -> Result<String, Box<EvalAltResult>> {
            let raw = ctx.region(label).join(" ");
            if raw.trim().is_empty() {
                return Err(fail(format!("label `{label}` is not present")));
            }
            let parsed = parse_datetime_loose(&raw, &format_vec(formats), locale).map_err(fail)?;
            Ok(parsed.format("%Y-%m-%dT%H:%M:%S").to_string())
        },
    );

    engine.register_fn(
        "date_in",
        |ctx: &mut MailCtx,
         label: &str,
         formats: Array,
         locale: &str|
         -> Result<String, Box<EvalAltResult>> {
            let raw = ctx.region(label).join(" ");
            if raw.trim().is_empty() {
                return Err(fail(format!("label `{label}` is not present")));
            }
            parse_date_loose(&raw, &format_vec(formats), locale)
                .map(|date| date.format("%Y-%m-%d").to_string())
                .map_err(fail)
        },
    );

    engine.register_fn(
        "time_in",
        |ctx: &mut MailCtx, label: &str, formats: Array| -> Result<String, Box<EvalAltResult>> {
            let raw = ctx.region(label).join(" ");
            if raw.trim().is_empty() {
                return Err(fail(format!("label `{label}` is not present")));
            }
            parse_time_loose(&raw, &format_vec(formats))
                .map(|time| time.format("%H:%M:%S").to_string())
                .map_err(fail)
        },
    );

    engine.register_fn(
        "date_text",
        |raw: &str, formats: Array, locale: &str| -> Result<String, Box<EvalAltResult>> {
            parse_date(raw, &format_vec(formats), locale)
                .map(|date| date.format("%Y-%m-%d").to_string())
                .map_err(fail)
        },
    );

    engine.register_fn(
        "time_text",
        |raw: &str, formats: Array| -> Result<String, Box<EvalAltResult>> {
            parse_time(raw, &format_vec(formats))
                .map(|time| time.format("%H:%M:%S").to_string())
                .map_err(fail)
        },
    );
}

/// What `parse_mail` returns: a map, or an array of maps.
#[derive(Debug, Deserialize)]
pub struct ObservedMail {
    /// Unsigned magnitude in currency minor units.
    pub amount: i64,
    #[serde(default)]
    pub kind: Option<Kind>,
    #[serde(default)]
    pub status: Option<Status>,
    /// `YYYY-MM-DDTHH:MM:SS`, or a date only for all-day rows.
    pub occurred_at: String,
    #[serde(default)]
    pub counterparty: Option<String>,
    #[serde(default)]
    pub counterparty_account: Option<String>,
    #[serde(default)]
    pub account_last4: Option<String>,
    #[serde(default)]
    pub reference_id: Option<String>,
    #[serde(default)]
    pub channel: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub extra: BTreeMap<String, String>,
    #[serde(default)]
    pub warnings: Vec<String>,
}

/// Run `parse_mail(ctx)` and build zero or more transactions.
pub fn parse(
    script: &Script,
    executor: &Executor,
    email: &NormalizedEmail,
    account: &str,
) -> Result<Vec<Transaction>, String> {
    let ctx = MailCtx::build(email, &script.meta);
    let value: Dynamic = executor
        .call(&script.ast, "parse_mail", (ctx,))
        .map_err(|error| describe_runtime_error("parse_mail", &error))?;

    let observed = observed_list(&value)?;
    if observed.is_empty() {
        return Err("parse_mail returned no transactions".to_owned());
    }

    observed
        .into_iter()
        .enumerate()
        .map(|(index, row)| build(script, email, account, row, index as u32))
        .collect()
}

fn observed_list(value: &Dynamic) -> Result<Vec<ObservedMail>, String> {
    if value.is::<rhai::Map>() {
        return rhai::serde::from_dynamic::<ObservedMail>(value)
            .map(|row| vec![row])
            .map_err(|error| format!("parse_mail must return a transaction map: {error}"));
    }

    rhai::serde::from_dynamic::<Vec<ObservedMail>>(value)
        .map_err(|error| format!("parse_mail must return a map or an array of maps: {error}"))
}

fn build(
    script: &Script,
    email: &NormalizedEmail,
    account: &str,
    row: ObservedMail,
    tx_index: u32,
) -> Result<Transaction, String> {
    if row.amount < 0 {
        return Err("amount must be the unsigned magnitude; `kind` decides the sign".to_owned());
    }

    let mut source = email.source.clone();
    source.tx_index = tx_index;

    let kind = row.kind.unwrap_or(script.meta.kind);
    let currency = script.meta.currency.clone();
    let exponent = script.meta.exponent();

    let naive = parse_occurred(&row.occurred_at)?;
    let offset = *email.date.offset();
    let occurred_at = offset
        .from_local_datetime(&naive)
        .single()
        .unwrap_or_else(|| offset.from_utc_datetime(&naive));

    let mut warnings = row.warnings;
    if row.amount == 0 {
        warnings.push("amount is zero".to_owned());
    }
    let extraction = if warnings.is_empty() {
        Extraction::Strict
    } else {
        Extraction::Heuristic
    };

    let signed = if kind.is_outflow() {
        -row.amount
    } else {
        row.amount
    };

    Ok(Transaction {
        id: source.stable_id(),
        source,
        parser: script.id.clone(),
        parser_version: script.meta.version,
        bank: script.meta.bank.clone(),
        account: account.to_owned(),
        kind,
        status: row.status.unwrap_or(Status::Settled),
        amount: Money::new(signed, currency.clone()),
        currency_exponent: exponent,
        amount_decimal: crate::util::decimal_with(signed, exponent),
        occurred_at,
        occurred_on: occurred_at.date_naive(),
        timezone: script.meta.tz.clone(),
        counterparty: row.counterparty.and_then(clean),
        counterparty_account: row.counterparty_account.and_then(clean),
        account_last4: row.account_last4.and_then(clean),
        reference_id: row.reference_id.and_then(clean),
        channel: row.channel.and_then(clean),
        description: row.description.and_then(clean),
        extra: row
            .extra
            .into_iter()
            .filter_map(|(name, value)| clean(value).map(|value| (name, value)))
            .collect(),
        extraction,
        warnings,
    })
}

fn parse_occurred(value: &str) -> Result<NaiveDateTime, String> {
    let formats = [
        "%Y-%m-%dT%H:%M:%S",
        "%Y-%m-%d %H:%M:%S",
        "%Y-%m-%dT%H:%M",
        "%Y-%m-%d %H:%M",
    ];

    for format in formats {
        if let Ok(parsed) = NaiveDateTime::parse_from_str(value.trim(), format) {
            return Ok(parsed);
        }
    }

    if let Ok(date) = NaiveDate::parse_from_str(value.trim(), "%Y-%m-%d") {
        return date
            .and_hms_opt(0, 0, 0)
            .ok_or_else(|| format!("invalid date `{value}`"));
    }

    Err(format!(
        "occurred_at `{value}` is not `YYYY-MM-DDTHH:MM:SS` or `YYYY-MM-DD`"
    ))
}

fn clean(value: String) -> Option<String> {
    let value = value.trim().to_owned();
    (!value.is_empty()).then_some(value)
}

/// Parse the fixed offset a script returned, if any (unused today, kept for a
/// future `occurred_at` that carries its own zone).
#[allow(dead_code)]
fn parse_offset(value: &str) -> Result<FixedOffset, String> {
    value
        .parse::<FixedOffset>()
        .map_err(|error| format!("offset `{value}`: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn regions_group_by_label() {
        let blocks: Vec<Block> = ["Total", "Rp", "5.500", ",00", "walletAccount", "Warung"]
            .into_iter()
            .map(Block::new)
            .collect();
        let labels = vec!["Total".to_string(), "walletAccount".to_string()];
        let regions = build_regions(&blocks, &labels);

        assert_eq!(regions.get("total").unwrap(), &["Rp", "5.500", ",00"]);
        assert_eq!(regions.get("walletaccount").unwrap(), &["Warung"]);
    }

    #[test]
    fn observed_list_accepts_one_map_or_an_array() {
        let engine = crate::engine::engine();
        let single: Dynamic = engine
            .eval(r#"#{ amount: 1, occurred_at: "2026-01-01" }"#)
            .unwrap();
        assert_eq!(observed_list(&single).unwrap().len(), 1);

        let many: Dynamic = engine
            .eval(r#"[#{ amount: 1, occurred_at: "2026-01-01" }, #{ amount: 2, occurred_at: "2026-01-02" }]"#)
            .unwrap();
        assert_eq!(observed_list(&many).unwrap().len(), 2);
    }
}
