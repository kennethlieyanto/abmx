//! Live inventory used by `abmx status --strict`.
//!
//! Routing is the label: `receipt/<parser-id>`. gmailctl creates the label
//! and the filter; abmx only reports whether that label has a parser, an
//! account, and samples that parse strictly. There is no sender clustering and
//! no amount heuristic. Sample downloads for parser authoring live in
//! `abmx receipt samples`, not here.

use std::collections::BTreeSet;
use std::path::Path;

use abmx_gmail::GmailFetcher;
use anyhow::{Context as _, Result};

use crate::config::ReceiptsSection;

use super::samples::{self, Sample};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LabelStatus {
    /// Parser exists, account mapping exists, last samples all strict.
    Ok,
    /// The label's remainder is not a valid parser id.
    BadLabel,
    /// The parser id has no `[receipts.accounts]` entry.
    MissingAccount,
    /// The parser directory does not exist.
    MissingParser,
    /// The parser exists but at least one of the last samples failed.
    ParseFailed,
    /// The samples do not share a label skeleton: the label mixes templates.
    MixedShapes,
    /// Fewer than three messages: a parser cannot be trusted yet.
    InsufficientSamples,
    /// The label has no mail.
    Empty,
    /// The user declined this label in `abmx setup`.
    Ignored,
    /// A `[receipts.accounts]` entry has no matching Gmail label.
    OrphanAccount,
}

impl LabelStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::BadLabel => "bad_label",
            Self::MissingAccount => "missing_account",
            Self::MissingParser => "missing_parser",
            Self::ParseFailed => "parse_failed",
            Self::MixedShapes => "mixed_shapes",
            Self::InsufficientSamples => "insufficient_samples",
            Self::Empty => "empty",
            Self::Ignored => "ignored",
            Self::OrphanAccount => "orphan_account",
        }
    }

    pub fn needs_work(self) -> bool {
        !matches!(self, Self::Ok | Self::Ignored)
    }
}

#[derive(Debug, Clone)]
pub struct LabelReport {
    pub label: String,
    pub parser: Option<String>,
    pub account: Option<String>,
    pub status: LabelStatus,
    /// Messages under the label, not just the sampled ones.
    pub messages: usize,
    pub samples: Vec<Sample>,
    pub notes: Vec<String>,
}

#[derive(Debug, Clone, Default)]
pub struct Inventory {
    pub labels: Vec<LabelReport>,
    pub orphans: Vec<LabelReport>,
}

impl Inventory {
    pub fn needs_work(&self) -> usize {
        self.labels
            .iter()
            .chain(self.orphans.iter())
            .filter(|report| report.status.needs_work())
            .count()
    }
}

pub struct Options {
    /// How many of the newest messages to inspect. Defaults to 3.
    pub samples: usize,
}

impl Default for Options {
    fn default() -> Self {
        Self { samples: 3 }
    }
}

pub fn scan_gmail(
    fetcher: &GmailFetcher,
    parser: Option<&abmx_parser::Parser>,
    labels: &ReceiptsSection,
    options: &Options,
    work_dir: &Path,
) -> Result<Inventory> {
    let found = fetcher.routed_labels()?;

    let mut inventory = Inventory::default();
    for label in &found {
        let parser_id = labels.parser_id(&label.name);

        // A declined label is not sampled: authoring a parser for mail the user
        // does not want to import would only waste API calls and attention.
        if parser_id
            .as_deref()
            .is_some_and(|parser| labels.is_ignored(parser))
        {
            inventory.labels.push(LabelReport {
                label: label.name.clone(),
                parser: parser_id,
                account: None,
                status: LabelStatus::Ignored,
                messages: 0,
                samples: Vec::new(),
                notes: vec!["declined in `abmx setup`; `abmx setup --review` asks again".to_owned()],
            });
            continue;
        }

        let ids = fetcher.label_message_ids(&label.name)?;
        let mut samples = Vec::new();
        let mut notes = Vec::new();

        for (index, id) in ids.iter().take(options.samples.max(1)).enumerate() {
            match fetcher.raw(id) {
                Ok(bytes) => match save_sample(work_dir, &label.name, index + 1, &bytes) {
                    Ok(sample) => samples.push(sample),
                    Err(error) => notes.push(format!("gmail:{id}: {error}")),
                },
                Err(error) => notes.push(format!("gmail:{id}: {error}")),
            }
        }

        let status = evaluate(parser, labels, parser_id.as_deref(), &samples, &mut notes);
        inventory.labels.push(LabelReport {
            label: label.name.clone(),
            parser: parser_id.clone(),
            account: parser_id
                .as_deref()
                .and_then(|parser| labels.accounts.get(parser).cloned()),
            status,
            messages: ids.len(),
            samples,
            notes,
        });
    }

    let routed: BTreeSet<String> = found
        .iter()
        .filter_map(|label| labels.parser_id(&label.name))
        .collect();
    for parser_id in labels.accounts.keys() {
        if !routed.contains(parser_id) {
            inventory.orphans.push(LabelReport {
                label: format!("{}{}", labels.prefix, parser_id),
                parser: Some(parser_id.clone()),
                account: Some(labels.accounts[parser_id].clone()),
                status: LabelStatus::OrphanAccount,
                messages: 0,
                samples: Vec::new(),
                notes: vec!["[receipts.accounts] entry has no routed Gmail label".to_owned()],
            });
        }
    }

    Ok(inventory)
}

/// Decide whether a label's samples are healthy enough to trust.
fn evaluate(
    parser: Option<&abmx_parser::Parser>,
    labels: &ReceiptsSection,
    parser_id: Option<&str>,
    samples: &[Sample],
    notes: &mut Vec<String>,
) -> LabelStatus {
    if samples.is_empty() {
        return LabelStatus::Empty;
    }

    let Some(parser_id) = parser_id else {
        notes.push(format!(
            "the label does not encode a parser id after `{}`",
            labels.prefix
        ));
        return LabelStatus::BadLabel;
    };

    if !labels.accounts.contains_key(parser_id) {
        notes.push(format!(
            "parser `{parser_id}` has no [receipts.accounts] entry"
        ));
        return LabelStatus::MissingAccount;
    }

    let Some(parser) = parser else {
        return LabelStatus::MissingParser;
    };

    if parser.registry().find(parser_id).is_none() {
        notes.push(format!("parser `{parser_id}` is not in the corpus"));
        return LabelStatus::MissingParser;
    }

    if mixed_shapes(samples) {
        notes.push(
            "the samples do not share a label skeleton; the routed label likely mixes templates"
                .to_owned(),
        );
        return LabelStatus::MixedShapes;
    }

    let mut failed = false;
    for sample in samples {
        match parser.parse_bytes_with(
            parser_id,
            &std::fs::read(&sample.origin).unwrap_or_default(),
            Some(sample.origin.clone()),
            parser_id,
        ) {
            Ok(abmx_parser::Outcome::Parsed(transaction)) => {
                if transaction.status == abmx_parser::Status::Failed {
                    failed = true;
                    notes.push(format!(
                        "{}: bank reported the transaction as failed; gmailctl should not route it",
                        sample.file_name
                    ));
                } else if transaction.extraction != abmx_parser::Extraction::Strict
                    || transaction.amount.amount_minor == 0
                {
                    failed = true;
                    notes.push(format!("{}: not a strict, non-zero row", sample.file_name));
                }
            }
            Ok(abmx_parser::Outcome::Missing(parser)) => {
                failed = true;
                notes.push(format!("{}: parser `{parser}` missing", sample.file_name));
            }
            Ok(abmx_parser::Outcome::Failed { parser, error }) => {
                failed = true;
                notes.push(format!("{}: {parser}: {error}", sample.file_name));
            }
            Err(error) => {
                failed = true;
                notes.push(format!("{}: {error}", sample.file_name));
            }
        }
    }

    if failed {
        return LabelStatus::ParseFailed;
    }
    if samples.len() < 3 {
        return LabelStatus::InsufficientSamples;
    }
    LabelStatus::Ok
}

fn save_sample(work_dir: &Path, label: &str, index: usize, bytes: &[u8]) -> Result<Sample> {
    let dir = work_dir.join(safe_name(label));
    std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
    let path = dir.join(format!("{index:02}.eml"));
    std::fs::write(&path, bytes).with_context(|| format!("writing {}", path.display()))?;
    samples::sample_from_bytes(path.display().to_string(), bytes)
        .map_err(|error| anyhow::anyhow!("{error}"))
}

fn safe_name(label: &str) -> String {
    let mut out = String::with_capacity(label.len());
    for ch in label.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
        } else if !out.ends_with('-') {
            out.push('-');
        }
    }
    out.trim_matches('-').to_owned()
}

fn skeleton(sample: &Sample) -> BTreeSet<String> {
    sample
        .email
        .blocks
        .iter()
        .map(|block| abmx_parse::norm(&block.text))
        .filter(|text| super::samples::is_label_like(text))
        .collect()
}

fn mixed_shapes(samples: &[Sample]) -> bool {
    if samples.len() < 2 {
        return false;
    }
    let base = skeleton(&samples[0]);
    if base.is_empty() {
        return false;
    }
    samples.iter().skip(1).any(|sample| {
        let other = skeleton(sample);
        let union = base.union(&other).count();
        if union == 0 {
            return false;
        }
        let overlap = base.intersection(&other).count() as f64 / union as f64;
        overlap < 0.5
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn safe_names_are_filesystem_friendly() {
        assert_eq!(
            safe_name("receipt/example.card_debit"),
            "receipt-example-card-debit"
        );
    }

    #[test]
    fn statuses_sort_by_work() {
        assert!(!LabelStatus::Ok.needs_work());
        assert!(LabelStatus::MixedShapes.needs_work());
    }
}
