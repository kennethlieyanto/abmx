//! `abmx author`: inventory `abmx:*` labels and package the last three samples
//! of each into a prompt.
//!
//! There is no sender clustering, no amount heuristic, and no representative
//! message. The user labels mail; the label is the contract. A label with no
//! parser or no mapping is a defect, not a hint.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use abmx_core::LabelRoutes;
use abmx_gmail::GmailFetcher;
use anyhow::{Context as _, Result};

use super::samples::{self, Sample};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LabelStatus {
    /// Parser exists, mapping exists, and the last samples are all `strict`.
    Ok,
    /// A Gmail `abmx:*` label has no `[labels]` entry.
    MissingMapping,
    /// The configured parser directory does not exist.
    MissingParser,
    /// The parser exists but at least one of the last samples failed.
    ParseFailed,
    /// The last samples do not share a label skeleton: the label is mislabeled.
    MixedShapes,
    /// Fewer than three messages: a parser cannot be trusted yet.
    InsufficientSamples,
    /// The label has no mail.
    Empty,
    /// A `[labels]` entry has no matching Gmail label.
    Orphan,
}

impl LabelStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::MissingMapping => "missing_mapping",
            Self::MissingParser => "missing_parser",
            Self::ParseFailed => "parse_failed",
            Self::MixedShapes => "mixed_shapes",
            Self::InsufficientSamples => "insufficient_samples",
            Self::Empty => "empty",
            Self::Orphan => "orphan_config",
        }
    }

    pub fn needs_work(self) -> bool {
        !matches!(self, Self::Ok)
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
}

pub fn scan_gmail(
    fetcher: &GmailFetcher,
    parser: Option<&abmx_parser::Parser>,
    routes: &LabelRoutes,
    options: &Options,
    work_dir: &Path,
) -> Result<Inventory> {
    let mut labels = fetcher.abmx_labels()?;
    if let Some(only) = &options.label {
        labels.retain(|label| &label.name == only);
        if labels.is_empty() {
            anyhow::bail!("no `abmx:*` label named `{only}`");
        }
    }

    let mut inventory = Inventory::default();
    for label in &labels {
        let route = routes.get(&label.name);
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

        let status = evaluate(parser, route, &samples, &mut notes);
        inventory.labels.push(LabelReport {
            label: label.name.clone(),
            parser: route.map(|route| route.parser.clone()),
            account: route.map(|route| route.account.clone()),
            status,
            messages: ids.len(),
            samples,
            notes,
        });
    }

    let gmail_names: BTreeSet<&str> = labels.iter().map(|label| label.name.as_str()).collect();
    for (name, route) in routes.iter() {
        if !gmail_names.contains(name.as_str()) {
            inventory.orphans.push(LabelReport {
                label: name.clone(),
                parser: Some(route.parser.clone()),
                account: Some(route.account.clone()),
                status: LabelStatus::Orphan,
                messages: 0,
                samples: Vec::new(),
                notes: vec!["[labels] entry has no matching Gmail label".to_owned()],
            });
        }
    }

    Ok(inventory)
}

/// Offline mode. Immediate subdirectories are labels; a flat directory uses
/// `--label`, or `abmx:dir`.
pub fn scan_input(
    input: &Path,
    parser: Option<&abmx_parser::Parser>,
    routes: &LabelRoutes,
    options: &Options,
    work_dir: &Path,
) -> Result<Inventory> {
    let set = samples::load_path(input, options.samples.max(1))?;
    let mut inventory = Inventory::default();

    for group in &set.groups {
        let label = group
            .label
            .clone()
            .or_else(|| options.label.clone())
            .unwrap_or_else(|| "abmx:dir".to_owned());
        let route = routes.get(&label);
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

        let status = evaluate(parser, route, &samples, &mut notes);
        inventory.labels.push(LabelReport {
            label,
            parser: route.map(|route| route.parser.clone()),
            account: route.map(|route| route.account.clone()),
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
    route: Option<&abmx_core::LabelRoute>,
    samples: &[Sample],
    notes: &mut Vec<String>,
) -> LabelStatus {
    if samples.is_empty() {
        return LabelStatus::Empty;
    }

    let mixed = mixed_shapes(samples);
    if mixed {
        notes.push(
            "the samples do not share a label skeleton; the Gmail label likely mixes templates"
                .to_owned(),
        );
    }

    let Some(route) = route else {
        return LabelStatus::MissingMapping;
    };

    let Some(parser) = parser else {
        return LabelStatus::MissingParser;
    };

    if parser.registry().find(&route.parser).is_none() {
        notes.push(format!(
            "configured parser `{}` is not in the corpus",
            route.parser
        ));
        return LabelStatus::MissingParser;
    }

    if mixed {
        return LabelStatus::MixedShapes;
    }

    let mut failed = false;
    for sample in samples {
        match parser.parse_bytes_with(
            &route.parser,
            &std::fs::read(&sample.origin).unwrap_or_default(),
            Some(sample.origin.clone()),
            &sample.email.from.email,
        ) {
            Ok(abmx_parser::Outcome::Parsed(transaction)) => {
                if transaction.extraction != abmx_parser::Extraction::Strict {
                    failed = true;
                    notes.push(format!("{}: {}", sample.file_name, "not strict"));
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
            "  {:<24} {:<20} {:>4}  {}\n",
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
    out.push_str("# Task: author parsers for `abmx:*` labels\n\n");
    out.push_str(
        "Every `abmx:*` Gmail label is an explicit instruction to import that mail. \
         One label maps to one parser and one ledger account. There is no sender or \
         subject routing, and no guessing.\n\n",
    );

    out.push_str("## Environment\n\n");
    out.push_str(&format!(
        "- abmx config: `{}`\n",
        context
            .config_path
            .map(|path| path.display().to_string())
            .unwrap_or_else(|| "(none; run `abmx init` first)".to_owned())
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
        "- spec format: `spec_version = {}` (extraction only; there is no `[match]` section)\n",
        abmx_parser::SPEC_VERSION
    ));
    out.push_str("- config contract:\n\n```toml\n[labels]\n");
    out.push_str(
        "\"abmx:<id>\" = { parser = \"<parser-id>\", account = \"<actual-account-uuid>\" }\n```\n\n",
    );

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
            "> `orphan_config` entries name a Gmail label that does not exist. Fix the label \
             name or remove the `[labels]` entry.\n\n",
        );
    }

    let working: Vec<&LabelReport> = inventory
        .labels
        .iter()
        .chain(inventory.orphans.iter())
        .filter(|report| report.status.needs_work())
        .collect();

    if working.is_empty() {
        out.push_str("Every label is `ok`. Nothing to author.\n");
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
                     templates. Fix the Gmail label or the filter so that one label covers one \
                     template, then re-run `abmx author`.\n\n",
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
            LabelStatus::MissingMapping => {
                out.push_str(
                    "Add a `[labels]` entry with the parser id and the ledger account first. \
                     Run `abmx accounts --json` and ask the user which account to use.\n\n",
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
        "- Write **one** parser per label. It must parse every sample above as `strict`.\n\
         - If it cannot, stop and report the mismatch. Do not split the label into several \
         parsers and do not relax extraction to make one sample pass.\n\
         - `labels.list` must contain every constant label block, in document order.\n\
         - Use the body date, the printed amount, and the correct `number` style.\n\
         - Ask the user for the ledger account; never invent a UUID.\n\
         - Copy the samples into `<corpus>/<parser-id>/fixtures/01.eml`, `02.eml`, …\n\
         - Verify with `abmx verify parser <corpus>/<parser-id>` and \
         `abmx verify parser --id <parser-id> --label <label>`.\n\
         - If the label is wrong, ask the user to fix the Gmail label/filter rather than \
         encoding the mistake in a parser.\n\n",
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
        assert_eq!(safe_name("abmx:example-qris"), "abmx-example-qris");
    }

    #[test]
    fn statuses_sort_by_work() {
        assert!(!LabelStatus::Ok.needs_work());
        assert!(LabelStatus::MixedShapes.needs_work());
    }
}
