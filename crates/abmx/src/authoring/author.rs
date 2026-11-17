//! `abmx author`: inventory routed labels and package the last three samples
//! of each into a prompt.
//!
//! Routing is the label: `receipt/<parser-id>`. gmailctl creates the label
//! and the filter; abmx only reports whether that label has a parser, an
//! account, and samples that parse strictly. There is no sender clustering and
//! no amount heuristic.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use abmx_gmail::GmailFetcher;
use anyhow::{Context as _, Result};

use crate::config::LabelsSection;

use super::samples::{self, Sample};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LabelStatus {
    /// Parser exists, account mapping exists, last samples all strict.
    Ok,
    /// The label's remainder is not a valid parser id.
    BadLabel,
    /// The parser id has no `[labels.accounts]` entry.
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
    /// A `[labels.accounts]` entry has no matching Gmail label.
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

    pub fn sample_count(&self) -> usize {
        self.labels.iter().map(|report| report.samples.len()).sum()
    }
}

pub struct Options {
    /// How many of the newest messages to inspect. Defaults to 3.
    pub samples: usize,
    /// Restrict the scan to one label.
    pub label: Option<String>,
    /// Offline mode: a directory of `.eml` files (or per-label subdirectories).
    pub input: Option<PathBuf>,
    pub quiet: bool,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            samples: 3,
            label: None,
            input: None,
            quiet: false,
        }
    }
}

pub struct Context<'a> {
    pub config_path: Option<&'a Path>,
    pub corpus: &'a Path,
    pub work_dir: &'a Path,
    pub summaries: &'a [abmx_parser::SpecSummary],
    pub prefix: &'a str,
}

pub fn scan_gmail(
    fetcher: &GmailFetcher,
    parser: Option<&abmx_parser::Parser>,
    labels: &LabelsSection,
    options: &Options,
    work_dir: &Path,
) -> Result<Inventory> {
    let mut found = fetcher.routed_labels()?;
    if let Some(only) = &options.label {
        found.retain(|label| label.name.eq_ignore_ascii_case(only));
        if found.is_empty() {
            anyhow::bail!("no routed label named `{only}`");
        }
    }

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
                notes: vec!["[labels.accounts] entry has no routed Gmail label".to_owned()],
            });
        }
    }

    Ok(inventory)
}

/// Offline mode. Immediate subdirectories are labels; a flat directory needs
/// `--label`, or exactly one configured parser.
pub fn scan_input(
    input: &Path,
    parser: Option<&abmx_parser::Parser>,
    labels: &LabelsSection,
    options: &Options,
    work_dir: &Path,
) -> Result<Inventory> {
    let set = samples::load_path(input, options.samples.max(1))?;
    let default_label = options.label.clone().or_else(|| {
        if labels.accounts.len() == 1 {
            let parser = labels.accounts.keys().next().expect("one entry");
            Some(format!("{}{}", labels.prefix, parser))
        } else {
            None
        }
    });

    let mut inventory = Inventory::default();
    for group in &set.groups {
        let label = group
            .label
            .clone()
            .or_else(|| options.label.clone())
            .or_else(|| default_label.clone());
        let Some(label) = label else {
            anyhow::bail!("--input needs --label unless exactly one parser is configured");
        };
        let parser_id = labels.parser_id(&label);
        if parser_id
            .as_deref()
            .is_some_and(|parser| labels.is_ignored(parser))
        {
            inventory.labels.push(LabelReport {
                label,
                parser: parser_id,
                account: None,
                status: LabelStatus::Ignored,
                messages: 0,
                samples: Vec::new(),
                notes: vec![
                    "declined in `abmx setup`; `abmx setup --review` asks again".to_owned()
                ],
            });
            continue;
        }
        let mut notes = Vec::new();
        let mut samples = Vec::new();

        for (index, sample) in group
            .samples
            .iter()
            .take(options.samples.max(1))
            .enumerate()
        {
            match std::fs::read(&sample.origin) {
                Ok(bytes) => match save_sample(work_dir, &label, index + 1, &bytes) {
                    Ok(sample) => samples.push(sample),
                    Err(error) => notes.push(format!("{}: {error}", sample.origin)),
                },
                Err(error) => notes.push(format!("{}: {error}", sample.origin)),
            }
        }

        let status = evaluate(parser, labels, parser_id.as_deref(), &samples, &mut notes);
        inventory.labels.push(LabelReport {
            label,
            parser: parser_id.clone(),
            account: parser_id
                .as_deref()
                .and_then(|parser| labels.accounts.get(parser).cloned()),
            status,
            messages: group.samples.len(),
            samples,
            notes,
        });
    }

    if inventory.labels.is_empty() {
        anyhow::bail!("no .eml files under {}", input.display());
    }

    Ok(inventory)
}

fn evaluate(
    parser: Option<&abmx_parser::Parser>,
    labels: &LabelsSection,
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
            "parser `{parser_id}` has no [labels.accounts] entry"
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

pub fn summary(inventory: &Inventory) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "{} label(s), {} sample(s); {} need work\n",
        inventory.labels.len(),
        inventory.sample_count(),
        inventory.needs_work()
    ));
    for report in inventory.labels.iter().chain(inventory.orphans.iter()) {
        out.push_str(&format!(
            "  {:<32} {:<20} {:>4}  {}\n",
            report.label,
            report.status.as_str(),
            report.messages,
            report.parser.as_deref().unwrap_or("-")
        ));
    }
    out
}

pub fn render(context: &Context<'_>, inventory: &Inventory) -> String {
    let mut out = String::new();
    out.push_str("# Task: author parsers for routed labels\n\n");
    out.push_str(
        "Every routed Gmail label is an explicit instruction to import that mail. One label \
         maps to one parser and one ledger account. gmailctl owns routing and classification; \
         do not re-implement it in a parser, and do not guess.\n\n",
    );

    out.push_str("## Environment\n\n");
    out.push_str(&format!(
        "- abmx config: `{}`\n",
        context
            .config_path
            .map(|path| path.display().to_string())
            .unwrap_or_else(|| "(none; run `abmx setup` first)".to_owned())
    ));
    out.push_str(&format!(
        "- parser corpus: `{}`\n",
        context.corpus.display()
    ));
    out.push_str(&format!(
        "- sample work dir: `{}`\n",
        context.work_dir.display()
    ));
    out.push_str(&format!(
        "- label prefix: `{}`; the remainder is the parser id\n",
        context.prefix
    ));
    out.push_str(&format!(
        "- spec format: `spec_version = {}` (extraction only; there is no `[match]` section)\n",
        abmx_parser::SPEC_VERSION
    ));
    out.push_str("- config contract:\n\n```toml\n[labels]\n");
    out.push_str(&format!(
        "prefix = \"{}\"\n\n[labels.accounts]\n",
        context.prefix
    ));
    out.push_str("\"<parser-id>\" = \"<actual-account-uuid>\"\n```\n\n");

    out.push_str("## Inventory\n\n");
    out.push_str("| label | status | messages | parser | account |\n");
    out.push_str("| --- | --- | --- | --- | --- |\n");
    for report in inventory.labels.iter().chain(inventory.orphans.iter()) {
        out.push_str(&format!(
            "| `{}` | `{}` | {} | {} | {} |\n",
            report.label,
            report.status.as_str(),
            report.messages,
            report
                .parser
                .as_deref()
                .map(|parser| format!("`{parser}`"))
                .unwrap_or_else(|| "—".to_owned()),
            report
                .account
                .as_deref()
                .map(|account| format!("`{account}`"))
                .unwrap_or_else(|| "—".to_owned()),
        ));
    }
    out.push('\n');

    if !inventory.orphans.is_empty() {
        out.push_str(
            "> `orphan_account` entries name a parser with no routed Gmail label. Add a gmailctl \
             rule, or remove the `[labels.accounts]` entry.\n\n",
        );
    }

    let working: Vec<&LabelReport> = inventory
        .labels
        .iter()
        .chain(inventory.orphans.iter())
        .filter(|report| report.status.needs_work())
        .collect();

    if working.is_empty() {
        out.push_str("Every managed label is `ok`. Nothing to author.\n");
        return out;
    }

    out.push_str("## Labels that need work\n\n");
    for (index, report) in working.iter().enumerate() {
        out.push_str(&format!(
            "### {}. `{}` — `{}`\n\n",
            index + 1,
            report.label,
            report.status.as_str()
        ));
        out.push_str(&format!("- messages: {}\n", report.messages));
        out.push_str(&format!(
            "- parser: {}\n",
            report.parser.as_deref().unwrap_or("(none)")
        ));
        out.push_str(&format!(
            "- account: {}\n\n",
            report.account.as_deref().unwrap_or("(none)")
        ));

        for note in &report.notes {
            out.push_str(&format!("- **{note}**\n"));
        }
        if !report.notes.is_empty() {
            out.push('\n');
        }

        match report.status {
            LabelStatus::MixedShapes => {
                out.push_str(
                    "**Do not write a parser for this label.** The samples are different \
                     templates. Fix the gmailctl rule so one label covers one template, then \
                     re-run `abmx author`.\n\n",
                );
            }
            LabelStatus::InsufficientSamples => {
                out.push_str(
                    "Fewer than three samples. Add more mail to this label before trusting a \
                     parser; `labels.list` cannot be inferred from one message.\n\n",
                );
            }
            LabelStatus::Empty => {
                out.push_str("The label has no mail. Nothing to author yet.\n\n");
            }
            LabelStatus::MissingAccount => {
                out.push_str(
                    "Add a `[labels.accounts]` entry for this parser id first. Run \
                     `abmx accounts --json` and ask the user which account to use.\n\n",
                );
            }
            LabelStatus::BadLabel => {
                out.push_str(
                    "The gmailctl rule attached a label whose remainder is not a parser id. \
                     Fix the rule; do not write a parser.\n\n",
                );
            }
            LabelStatus::MissingParser if report.samples.is_empty() => {
                out.push_str("No samples were available. Do not write a parser.\n\n");
            }
            _ => {}
        }

        for (sample_index, sample) in report.samples.iter().enumerate() {
            out.push_str(&format!(
                "#### Sample {} — `{}` ({})\n\n",
                sample_index + 1,
                sample.file_name,
                sample.origin
            ));
            out.push_str(&format!(
                "- from: `{}` <{}>\n- subject: `{}`\n- date: `{}`\n- blocks ({}):\n\n",
                sample.email.from.name.as_deref().unwrap_or(""),
                sample.email.from.email,
                sample.email.subject,
                sample.email.date.to_rfc3339(),
                sample.email.blocks.len()
            ));
            out.push_str("```text\n");
            for (block_index, block) in sample.email.blocks.iter().enumerate() {
                out.push_str(&format!("{block_index:>3}  {}\n", block.text));
            }
            out.push_str("```\n\n");
        }

        if report.samples.len() > 1 {
            if let Some(variance) = samples::variance(&report.samples) {
                out.push_str("#### Variance across the samples\n\n");
                if variance.same_shape {
                    out.push_str("| idx | kind | distinct values |\n| --- | --- | --- |\n");
                    for row in &variance.rows {
                        let kind = if row.label_like {
                            "label?"
                        } else if row.constant {
                            "constant"
                        } else {
                            "value"
                        };
                        out.push_str(&format!(
                            "| {} | {} | {} |\n",
                            row.index,
                            kind,
                            row.values
                                .iter()
                                .take(4)
                                .map(|value| format!("`{}`", value.replace('|', "\\|")))
                                .collect::<Vec<_>>()
                                .join(" · ")
                        ));
                    }
                } else {
                    out.push_str(
                        "Block counts differ. Check whether that is an optional label (fine) or \
                         a different template (mislabeled).\n",
                    );
                }
                out.push('\n');
            }
        }
    }

    out.push_str("## Rules\n\n");
    out.push_str(
        "- Write **one** parser per label. It must parse every sample above as `strict`, \
         non-zero, and not `failed`.\n\
         - If it cannot, stop and report the mismatch. Do not split the label into several \
         parsers and do not relax extraction to make one sample pass.\n\
         - `labels.list` must contain every constant label block, in document order.\n\
         - Use the body date, the printed amount, and the correct `number` style.\n\
         - Ask the user for the ledger account; never invent a UUID.\n\
         - Copy the samples into `<corpus>/<parser-id>/fixtures/01.eml`, `02.eml`, …\n\
         - Verify with `abmx parser check --parser <parser-id>` once the files are in place.\n\
         - If the label is wrong, ask the user to fix the gmailctl rule rather than encoding \
         the mistake in a parser.\n\n",
    );

    out.push_str("## Current parser corpus\n\n");
    if context.summaries.is_empty() {
        out.push_str("The corpus is empty.\n");
    } else {
        out.push_str("| id | ver | bank | kind |\n| --- | --- | --- | --- |\n");
        for summary in context.summaries {
            out.push_str(&format!(
                "| `{}` | {} | {} | {:?} |\n",
                summary.id, summary.version, summary.bank, summary.kind
            ));
        }
    }
    out.push('\n');

    out
}

pub fn render_json(inventory: &Inventory) -> serde_json::Value {
    let reports: Vec<serde_json::Value> = inventory
        .labels
        .iter()
        .chain(inventory.orphans.iter())
        .map(|report| {
            serde_json::json!({
                "label": report.label,
                "status": report.status.as_str(),
                "messages": report.messages,
                "parser": report.parser,
                "account": report.account,
                "notes": report.notes,
                "samples": report.samples.iter().map(|sample| {
                    serde_json::json!({
                        "file": sample.file_name,
                        "origin": sample.origin,
                        "subject": sample.email.subject,
                        "date": sample.email.date.to_rfc3339(),
                        "blocks": sample.email.blocks.iter().map(|block| &block.text).collect::<Vec<_>>(),
                    })
                }).collect::<Vec<_>>(),
            })
        })
        .collect();

    serde_json::json!({ "labels": reports })
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
