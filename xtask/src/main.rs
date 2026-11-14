//! Developer tasks that are not part of the published CLI.
//!
//! `redact` turns real bank emails into publishable fixtures: it decodes each
//! message, rewrites only the *text nodes* (never tags or attributes), and
//! re-serializes a minimal single-part HTML message. Structure is preserved, so
//! the parser still sees the same blocks.

use std::collections::BTreeSet;
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use base64::Engine;
use html5ever::parse_document;
use html5ever::serialize::{serialize, SerializeOpts};
use html5ever::tendril::{StrTendril, TendrilSink};
use markup5ever_rcdom::{Handle, NodeData, RcDom, SerializableHandle};
use regex::Regex;
use serde::Deserialize;
use std::sync::LazyLock;

static GROUPED_ACCOUNT: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\b\d{4} \d{4} \d{4}\b").expect("regex"));
static LONG_DIGITS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\d{8,}").expect("regex"));

fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    match args.next().as_deref() {
        Some("redact") => {
            let mut input: Option<PathBuf> = None;
            let mut output: Option<PathBuf> = None;
            let mut rules = PathBuf::from("redact.toml");

            while let Some(flag) = args.next() {
                match flag.as_str() {
                    "--in" => input = args.next().map(PathBuf::from),
                    "--out" => output = args.next().map(PathBuf::from),
                    "--rules" => {
                        rules = args.next().map(PathBuf::from).unwrap_or(rules);
                    }
                    other => bail!("unknown flag {other}"),
                }
            }

            redact(
                input.context("--in is required")?,
                output.context("--out is required")?,
                &rules,
            )
        }
        _ => {
            eprintln!("usage: xtask redact --in <dir> --out <dir> [--rules redact.toml]");
            Ok(())
        }
    }
}

#[derive(Debug, Deserialize)]
struct RulesFile {
    #[serde(default)]
    replace: Vec<Replacement>,
    #[serde(default)]
    rules: Options,
}

#[derive(Debug, Deserialize)]
struct Replacement {
    from: String,
    to: String,
}

#[derive(Debug, Deserialize)]
struct Options {
    #[serde(default = "default_digits_min")]
    digits_min: usize,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            digits_min: default_digits_min(),
        }
    }
}

fn default_digits_min() -> usize {
    8
}

struct Rules {
    replace: Vec<Replacement>,
    digits_min: usize,
}

impl Rules {
    fn load(path: &Path) -> Result<Self> {
        let text =
            std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
        let file: RulesFile =
            toml::from_str(&text).with_context(|| format!("parsing {}", path.display()))?;

        // Longest first so "FULL NAME" is replaced before "NAME".
        let mut replace = file.replace;
        replace.sort_by_key(|rule| std::cmp::Reverse(rule.from.len()));

        Ok(Self {
            replace,
            digits_min: file.rules.digits_min,
        })
    }

    /// Values that must not appear in the output.
    fn sensitive(&self, text: &str) -> BTreeSet<String> {
        let mut found = BTreeSet::new();

        for rule in &self.replace {
            if text.contains(&rule.from) {
                found.insert(rule.from.clone());
            }
        }
        for capture in GROUPED_ACCOUNT.find_iter(text) {
            found.insert(capture.as_str().to_string());
        }
        for capture in LONG_DIGITS.find_iter(text) {
            if capture.as_str().len() >= self.digits_min {
                found.insert(capture.as_str().to_string());
            }
        }

        found
    }

    fn apply(&self, text: &str) -> String {
        let mut out = text.to_string();

        for rule in &self.replace {
            if out.contains(&rule.from) {
                out = out.replace(&rule.from, &rule.to);
            }
        }

        out = GROUPED_ACCOUNT
            .replace_all(&out, |captures: &regex::Captures| fake_digits(&captures[0]))
            .into_owned();

        out = LONG_DIGITS
            .replace_all(&out, |captures: &regex::Captures| {
                if captures[0].len() >= self.digits_min {
                    fake_digits(&captures[0])
                } else {
                    captures[0].to_string()
                }
            })
            .into_owned();

        out
    }
}

/// Deterministic same-length digit replacement: the same input always maps to
/// the same output, so account numbers stay consistent across fixtures.
fn fake_digits(text: &str) -> String {
    let mut state = fnv1a(text);
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        if ch.is_ascii_digit() {
            state = state
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            out.push((b'0' + ((state >> 33) % 10) as u8) as char);
        } else {
            out.push(ch);
        }
    }
    out
}

fn fnv1a(text: &str) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in text.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

fn redact(input: PathBuf, output: PathBuf, rules_path: &Path) -> Result<()> {
    let rules = Rules::load(rules_path)?;

    let mut files = Vec::new();
    collect(&input, &mut files)?;
    files.sort();

    let mut changed = 0usize;

    for path in &files {
        let relative = path.strip_prefix(&input).unwrap_or(path);
        let bytes = std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;

        let email = abmx_mail::parse_eml(&bytes, Some(path.display().to_string()))
            .map_err(|error| anyhow::anyhow!("{}: {error}", path.display()))?;

        let html =
            decoded_html(&bytes).with_context(|| format!("{}: no HTML body", path.display()))?;

        let mut sensitive = BTreeSet::new();
        let redacted_html = redact_html(&html, &rules, &mut sensitive)?;

        let name = email.from.name.as_deref().map(|name| rules.apply(name));
        let subject = rules.apply(&email.subject);
        let local_part = name
            .as_deref()
            .map(|name| {
                if name.is_empty() {
                    String::new()
                } else {
                    format!("{name} ")
                }
            })
            .unwrap_or_default();

        let message_id = format!(
            "txredact.{:016x}@example.invalid",
            fnv1a(email.source.message_id.as_deref().unwrap_or("none"))
        );

        let message = build_message(
            &format!("{local_part}<{}>", email.from.email),
            &subject,
            &email.date.to_rfc2822(),
            &message_id,
            &redacted_html,
        );

        // Safety net: nothing we classified as sensitive may survive.
        for value in &sensitive {
            if message.contains(value) {
                bail!("{}: redaction left `{value}` in the output", path.display());
            }
        }

        verify_structure(&bytes, message.as_bytes(), path, &rules)?;

        let destination = output.join(relative);
        if let Some(parent) = destination.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut file = std::fs::File::create(&destination)
            .with_context(|| format!("writing {}", destination.display()))?;
        file.write_all(message.as_bytes())?;

        changed += 1;
    }

    println!(
        "redacted {changed} emails from {} into {}",
        input.display(),
        output.display()
    );
    Ok(())
}

/// Re-serialize the DOM so tags, classes and nesting are untouched; only text
/// nodes are rewritten.
fn redact_html(html: &str, rules: &Rules, sensitive: &mut BTreeSet<String>) -> Result<String> {
    let dom = parse_document(RcDom::default(), Default::default())
        .from_utf8()
        .read_from(&mut html.as_bytes())
        .context("parsing HTML")?;

    walk_text(&dom.document, rules, sensitive);

    let mut out = Vec::new();
    let handle = SerializableHandle::from(dom.document.clone());
    serialize(&mut out, &handle, SerializeOpts::default()).context("serializing HTML")?;
    String::from_utf8(out).context("HTML was not UTF-8")
}

fn walk_text(node: &Handle, rules: &Rules, sensitive: &mut BTreeSet<String>) {
    if let NodeData::Text { contents } = &node.data {
        let current = contents.borrow().to_string();
        sensitive.extend(rules.sensitive(&current));
        let redacted = rules.apply(&current);
        if redacted != current {
            *contents.borrow_mut() = StrTendril::from(redacted);
        }
    }

    for child in node.children.borrow().iter() {
        walk_text(child, rules, sensitive);
    }
}

fn decoded_html(bytes: &[u8]) -> Option<String> {
    let message = mail_parser::MessageParser::new().parse(bytes)?;
    message.body_html(0).map(|body| body.into_owned())
}

fn build_message(from: &str, subject: &str, date: &str, message_id: &str, html: &str) -> String {
    let subject = encode_header(subject);

    let mut message = String::with_capacity(html.len() + 512);
    message.push_str(&format!("From: {from}\r\n"));
    message.push_str("To: user@example.invalid\r\n");
    message.push_str(&format!("Subject: {subject}\r\n"));
    message.push_str(&format!("Date: {date}\r\n"));
    message.push_str(&format!("Message-ID: <{message_id}>\r\n"));
    message.push_str("MIME-Version: 1.0\r\n");
    message.push_str("Content-Type: text/html; charset=utf-8\r\n");
    message.push_str("Content-Transfer-Encoding: 8bit\r\n");
    message.push_str("\r\n");
    message.push_str(html);
    message.push_str("\r\n");
    message
}

fn encode_header(value: &str) -> String {
    if value.is_ascii() {
        return value.to_string();
    }

    let encoded = base64::engine::general_purpose::STANDARD.encode(value.as_bytes());
    format!("=?UTF-8?B?{encoded}?=")
}

/// The redacted email must produce exactly the same block sequence, except for
/// the values we deliberately replaced.
fn verify_structure(original: &[u8], redacted: &[u8], path: &Path, rules: &Rules) -> Result<()> {
    let before =
        abmx_mail::parse_eml(original, None).map_err(|error| anyhow::anyhow!("{error}"))?;
    let after = abmx_mail::parse_eml(redacted, None).map_err(|error| anyhow::anyhow!("{error}"))?;

    if before.blocks.len() != after.blocks.len() {
        bail!(
            "{}: block count changed {} -> {}",
            path.display(),
            before.blocks.len(),
            after.blocks.len()
        );
    }

    for (index, (old, new)) in before.blocks.iter().zip(after.blocks.iter()).enumerate() {
        // Blocks with no sensitive content must survive the MIME round trip
        // byte for byte; anything else means serialization damaged the email.
        if rules.sensitive(&old.text).is_empty() && old.text != new.text {
            bail!(
                "{}: block {index} changed unexpectedly: {:?} -> {:?}",
                path.display(),
                old.text,
                new.text
            );
        }
    }

    Ok(())
}

fn collect(dir: &Path, out: &mut Vec<PathBuf>) -> Result<()> {
    for entry in std::fs::read_dir(dir).with_context(|| format!("reading {}", dir.display()))? {
        let path = entry?.path();
        if path.is_dir() {
            collect(&path, out)?;
        } else if path.extension().and_then(|value| value.to_str()) == Some("eml") {
            out.push(path);
        }
    }
    Ok(())
}
