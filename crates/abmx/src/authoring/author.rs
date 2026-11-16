//! `abmx author`: discover unknown templates and package them for an agent.
//!
//! The scan is read-only. It never advances the sync cursor, never writes the
//! state database, and never touches a ledger. It produces a prompt on stdout
//! and one representative `.eml` per group under the state directory.

use std::collections::BTreeMap;
use std::io::{IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use anyhow::{bail, Context, Result};
use regex::Regex;

use abmx_gmail::{GmailError, GmailFetcher, MessageMeta};
use abmx_tx::NormalizedEmail;

use super::samples;

/// A sender that survived the scan, with one representative message.
#[derive(Debug, Clone)]
pub struct Group {
    pub sender: String,
    pub count: usize,
    pub subjects: Vec<String>,
    pub outcome: GroupOutcome,
    /// Where the representative `.eml` was written, when one was.
    pub sample: Option<PathBuf>,
    /// Block dump of the representative, for the prompt.
    pub email: Option<NormalizedEmail>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GroupOutcome {
    /// Already routed and parsed by an installed parser.
    Known(String),
    /// Amount and date are present: likely a ledger movement.
    Candidate,
    /// Amount is missing, or the subject is an OTP/security notice.
    Ignored(&'static str),
}

#[derive(Debug, Clone)]
pub struct Scan {
    pub days: u32,
    pub messages: usize,
    pub senders: usize,
    pub groups: Vec<Group>,
}

#[derive(Debug, Clone)]
pub struct Options {
    pub days: u32,
    pub label: Option<String>,
    pub senders: Vec<String>,
    pub max_messages: usize,
    pub max_groups: usize,
    /// Parallel header requests. Capped at 16 to stay under Gmail's quota.
    pub concurrency: usize,
    pub input: Option<PathBuf>,
    pub quiet: bool,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            days: 30,
            label: None,
            senders: Vec::new(),
            max_messages: 1000,
            max_groups: 20,
            concurrency: 8,
            input: None,
            quiet: false,
        }
    }
}

/// A stderr progress line. stdout is the prompt, so nothing here may touch it.
struct Progress {
    label: &'static str,
    total: usize,
    done: AtomicUsize,
    quiet: bool,
    tty: bool,
}

impl Progress {
    fn new(label: &'static str, total: usize, quiet: bool) -> Self {
        let tty = std::io::stderr().is_terminal();
        let progress = Self {
            label,
            total,
            done: AtomicUsize::new(0),
            quiet,
            tty,
        };
        if !quiet && total > 0 {
            progress.print(0);
        }
        progress
    }

    fn tick(&self) {
        let done = self.done.fetch_add(1, Ordering::Relaxed) + 1;
        if self.quiet || self.total == 0 {
            return;
        }
        let step = (self.total / 10).max(1);
        if done != self.total && done % step != 0 {
            return;
        }
        self.print(done);
    }

    fn print(&self, done: usize) {
        let mut err = std::io::stderr().lock();
        if self.tty {
            let _ = write!(err, "\r{} {done}/{}", self.label, self.total);
        } else {
            let _ = writeln!(err, "{} {done}/{}", self.label, self.total);
        }
        let _ = err.flush();
    }

    fn finish(&self) {
        if self.quiet || self.total == 0 {
            return;
        }
        let done = self.done.load(Ordering::Relaxed);
        if self.tty {
            let mut err = std::io::stderr().lock();
            let _ = writeln!(err, "\r{} {done}/{}", self.label, self.total);
            let _ = err.flush();
        }
    }
}

/// Fetch headers for every id, `concurrency` at a time.
///
/// `reqwest::blocking` has no async runtime to fight with, so a scoped thread
/// pool is the simplest correct parallel primitive here. Order does not matter:
/// the caller groups by sender afterwards.
fn fetch_headers(
    fetcher: &GmailFetcher,
    ids: &[String],
    concurrency: usize,
    progress: &Progress,
) -> Result<Vec<MessageMeta>> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }

    let workers = concurrency.clamp(1, 16).min(ids.len());
    let fetch_one = |id: &str| -> Result<Option<MessageMeta>> {
        match fetcher.metadata(id) {
            Ok(meta) => {
                progress.tick();
                Ok(Some(meta))
            }
            // Listed but gone before we asked: deleted or filtered out.
            Err(GmailError::Api { status: 404, .. }) => {
                progress.tick();
                Ok(None)
            }
            Err(error) => Err(error.into()),
        }
    };

    if workers == 1 {
        let mut out = Vec::with_capacity(ids.len());
        for id in ids {
            if let Some(meta) = fetch_one(id)? {
                out.push(meta);
            }
        }
        return Ok(out);
    }

    let chunk_size = ids.len().div_ceil(workers);
    let chunks: Vec<&[String]> = ids.chunks(chunk_size).collect();

    let results: Vec<Result<Vec<MessageMeta>>> = std::thread::scope(|scope| {
        let handles: Vec<_> = chunks
            .iter()
            .map(|chunk| {
                scope.spawn(move || {
                    let mut local = Vec::with_capacity(chunk.len());
                    for id in *chunk {
                        if let Some(meta) = fetch_one(id)? {
                            local.push(meta);
                        }
                    }
                    Ok(local)
                })
            })
            .collect();

        handles
            .into_iter()
            .map(|handle| handle.join().expect("header worker panicked"))
            .collect()
    });

    let mut out = Vec::with_capacity(ids.len());
    for result in results {
        out.extend(result?);
    }
    Ok(out)
}

/// Everything the prompt needs about the user's installation.
pub struct AuthorContext<'a> {
    pub config_path: Option<&'a Path>,
    pub corpus: &'a Path,
    pub work_dir: &'a Path,
    pub accounts: &'a BTreeMap<String, String>,
    pub summaries: &'a [abmx_parser::SpecSummary],
}

fn outline() -> (&'static Regex, &'static Regex, &'static Regex) {
    use std::sync::OnceLock;
    static AMOUNT: OnceLock<Regex> = OnceLock::new();
    static DATE: OnceLock<Regex> = OnceLock::new();
    static IGNORE: OnceLock<Regex> = OnceLock::new();

    (
        AMOUNT.get_or_init(|| {
            Regex::new(
                r"(?i)(\b(?:rp|idr|usd|sgd|myr|eur|gbp|aud|jpy)\b)|(\b\d{1,3}(?:[.,]\d{3})+\b)|(\b\d+[.,]\d{2}\b)",
            )
            .expect("amount regex")
        }),
        DATE.get_or_init(|| {
            Regex::new(
                r"(?i)(\b\d{4}-\d{2}-\d{2}\b)|(\b\d{1,2}[/-]\d{1,2}[/-]\d{2,4}\b)|(\b\d{1,2}\s+(?:jan|feb|mar|apr|may|jun|jul|aug|sep|oct|nov|dec)[a-z]*\s+\d{2,4}\b)|(\b\d{1,2}:\d{2}(?::\d{2})?\b)",
            )
            .expect("date regex")
        }),
        IGNORE.get_or_init(|| {
            Regex::new(r"(?i)\b(otp|kode|verifikasi|verification|password|login|device|security)\b")
                .expect("ignore regex")
        }),
    )
}

pub fn sender_address(from: &str) -> String {
    let raw = if let (Some(start), Some(end)) = (from.find('<'), from.rfind('>')) {
        &from[start + 1..end]
    } else {
        from
    };
    raw.trim().trim_matches('"').to_ascii_lowercase()
}

fn slug(sender: &str) -> String {
    let mut out = String::with_capacity(sender.len());
    for ch in sender.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
        } else if !out.ends_with('-') {
            out.push('-');
        }
    }
    out.trim_matches('-').to_owned()
}

fn subject_ignored(subject: &str) -> bool {
    outline().2.is_match(subject)
}

fn looks_like_movement(email: &NormalizedEmail) -> bool {
    let (amount, date, _) = outline();
    let has_amount = email
        .blocks
        .iter()
        .any(|block| amount.is_match(&block.text));
    let has_date = email.blocks.iter().any(|block| date.is_match(&block.text));
    has_amount && has_date
}

fn base_query(options: &Options) -> String {
    let mut terms = vec!["-in:spam".to_owned(), "-in:trash".to_owned()];
    if options.days > 0 {
        terms.push(format!("newer_than:{}d", options.days));
    }
    if !options.senders.is_empty() {
        let senders: Vec<String> = options
            .senders
            .iter()
            .map(|sender| sender.trim().to_owned())
            .filter(|sender| !sender.is_empty())
            .collect();
        if senders.len() == 1 {
            terms.push(format!("from:{}", senders[0]));
        } else if senders.len() > 1 {
            terms.push(format!("from:({})", senders.join(" OR ")));
        }
    }
    if let Some(label) = &options.label {
        terms.push(format!("label:{}", label.trim()));
    }
    terms.join(" ")
}

/// Scan Gmail and write one representative `.eml` per candidate sender.
pub fn scan_gmail(
    fetcher: &GmailFetcher,
    parser: Option<&abmx_parser::Parser>,
    options: &Options,
    context: &AuthorContext<'_>,
) -> Result<Scan> {
    if !options.quiet {
        eprintln!("scanning {}d…", options.days);
    }

    let query = base_query(options);
    let ids = fetcher.list_ids(&query, options.max_messages.max(1))?;

    let headers = Progress::new("headers", ids.len(), options.quiet);
    let metas = fetch_headers(fetcher, &ids, options.concurrency, &headers)?;
    headers.finish();

    let mut buckets: BTreeMap<String, (usize, Vec<String>, String)> = BTreeMap::new();
    for meta in &metas {
        let sender = sender_address(&meta.from);
        let entry = buckets
            .entry(sender)
            .or_insert_with(|| (0, Vec::new(), String::new()));
        entry.0 += 1;
        if entry.2.is_empty() {
            entry.2 = meta.id.clone();
        }
        if entry.1.len() < 3 && !meta.subject.is_empty() {
            entry.1.push(meta.subject.clone());
        }
    }

    let sender_count = buckets.len();
    let mut ordered: Vec<_> = buckets.into_iter().collect();
    ordered.sort_by(|a, b| b.1 .0.cmp(&a.1 .0).then_with(|| a.0.cmp(&b.0)));

    let bodies = Progress::new(
        "groups",
        ordered.len().min(options.max_groups.max(1)),
        options.quiet,
    );
    let mut groups = Vec::new();
    for (sender, (count, subjects, representative)) in ordered {
        if groups.len() >= options.max_groups.max(1) {
            break;
        }
        bodies.tick();

        let ignored_subject = subjects.iter().any(|subject| subject_ignored(subject));
        if ignored_subject {
            groups.push(Group {
                sender,
                count,
                subjects,
                outcome: GroupOutcome::Ignored("otp or security notice"),
                sample: None,
                email: None,
            });
            continue;
        }

        let bytes = match fetcher.raw(&representative) {
            Ok(bytes) => bytes,
            Err(GmailError::Api { status: 404, .. }) => continue,
            Err(error) => return Err(error.into()),
        };

        let email = abmx_parser::parse_eml(&bytes, Some(format!("gmail:{representative}")))
            .map_err(|error| anyhow::anyhow!("gmail:{representative}: {error}"))?;

        let known = parser.and_then(|parser| {
            match parser.parse_bytes(&bytes, Some(representative.clone())) {
                Ok(abmx_parser::Outcome::Parsed(transaction)) => Some(transaction.parser.clone()),
                _ => None,
            }
        });

        let outcome = match known {
            Some(id) => GroupOutcome::Known(id),
            None if looks_like_movement(&email) => GroupOutcome::Candidate,
            None => GroupOutcome::Ignored("no amount or date pattern"),
        };

        let sample = if outcome == GroupOutcome::Candidate {
            let dir = context.work_dir.join(format!("{}-{sender}", slug(&sender)));
            std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
            let path = dir.join("01.eml");
            std::fs::write(&path, &bytes).with_context(|| format!("writing {}", path.display()))?;
            Some(path)
        } else {
            None
        };

        groups.push(Group {
            sender,
            count,
            subjects,
            outcome,
            sample,
            email: Some(email),
        });
    }

    bodies.finish();

    Ok(Scan {
        days: options.days,
        messages: ids.len(),
        senders: sender_count,
        groups,
    })
}

/// Offline bootstrap: treat each sender inside the samples as a group.
pub fn scan_input(options: &Options, context: &AuthorContext<'_>) -> Result<Scan> {
    let input = options.input.as_ref().context("scan_input needs --input")?;
    let set = samples::load_path(input, options.max_groups.max(1))?;

    let mut groups = Vec::new();
    for sample_group in &set.groups {
        for sender_group in samples::by_sender(&sample_group.samples) {
            if groups.len() >= options.max_groups.max(1) {
                break;
            }
            let Some(sample) = sender_group.samples.first() else {
                continue;
            };
            if sample.origin == "-" {
                bail!("cannot copy a stdin sample into fixtures; pass --input <file>");
            }
            let bytes = std::fs::read(&sample.origin)
                .with_context(|| format!("reading {}", sample.origin))?;

            let dir =
                context
                    .work_dir
                    .join(format!("{}-{}", slug(&sender_group.sender), groups.len()));
            std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
            let path = dir.join("01.eml");
            std::fs::write(&path, &bytes).with_context(|| format!("writing {}", path.display()))?;

            let outcome = if looks_like_movement(&sample.email) {
                GroupOutcome::Candidate
            } else {
                GroupOutcome::Ignored("no amount or date pattern")
            };

            groups.push(Group {
                sender: sender_address(&sender_group.sender),
                count: sender_group.samples.len(),
                subjects: sender_group.subjects.clone(),
                outcome,
                sample: Some(path),
                email: Some(sample.email.clone()),
            });
        }
    }

    let messages = set.groups.iter().map(|group| group.samples.len()).sum();
    Ok(Scan {
        days: 0,
        messages,
        senders: groups.len(),
        groups,
    })
}

/// Human summary, written to stderr.
pub fn summary(scan: &Scan) -> String {
    let mut out = String::new();
    if scan.days == 0 {
        out.push_str(&format!(
            "loaded {} sample(s) across {} sender(s)\n",
            scan.messages, scan.senders
        ));
    } else {
        out.push_str(&format!(
            "scanned {}d: {} messages, {} senders\n",
            scan.days, scan.messages, scan.senders
        ));
    }

    for group in &scan.groups {
        let tag = match &group.outcome {
            GroupOutcome::Candidate => "candidate",
            GroupOutcome::Known(id) => {
                out.push_str(&format!(
                    "  known    {:<32} {:>4}  {}\n",
                    group.sender, group.count, id
                ));
                continue;
            }
            GroupOutcome::Ignored(reason) => {
                out.push_str(&format!(
                    "  ignored  {:<32} {:>4}  {}\n",
                    group.sender, group.count, reason
                ));
                continue;
            }
        };
        let subject = group.subjects.first().cloned().unwrap_or_default();
        out.push_str(&format!(
            "  {tag:<8} {:<32} {:>4}  {}\n",
            group.sender, group.count, subject
        ));
    }

    let candidates = scan
        .groups
        .iter()
        .filter(|group| group.outcome == GroupOutcome::Candidate)
        .count();
    out.push_str(&format!(
        "\n{candidates} candidate(s); prompt on stdout, samples in the work directory\n"
    ));
    out
}

pub fn render(context: &AuthorContext<'_>, scan: &Scan) -> String {
    let mut out = String::new();
    out.push_str("# Task: teach abmx the bank templates in this mailbox\n\n");

    out.push_str(&format!(
        "{}\n\n",
        if scan.days == 0 {
            format!(
                "Loaded {} sample(s) across {} sender(s).",
                scan.messages, scan.senders
            )
        } else {
            format!(
                "Scanned {} days: {} messages across {} senders.",
                scan.days, scan.messages, scan.senders
            )
        }
    ));

    let candidates: Vec<&Group> = scan
        .groups
        .iter()
        .filter(|group| group.outcome == GroupOutcome::Candidate)
        .collect();

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
    if !context.summaries.is_empty() {
        out.push_str("- installed parsers:\n");
        for summary in context.summaries {
            out.push_str(&format!(
                "  - `{}` ({}), senders: {}\n",
                summary.id,
                summary.account,
                summary.senders.join(", ")
            ));
        }
    }
    out.push('\n');

    out.push_str("## Your job\n\n");
    out.push_str(
        "1. Read the candidate groups below. Each is one sender and one representative message.\n",
    );
    out.push_str(
        "2. Decide, per group, whether it reports a **movement of an account the user tracks**. \
         Merchant order receipts (Shopee, Tokopedia, Gojek, ...) are *not* movements when the \
         bank already notifies that account; ignore them unless the user says otherwise.\n",
    );
    out.push_str(
        "3. Ask the user which senders to track, and confirm each parser id (`bank.type`) and \
         account key (`bank-account`). Do not invent either.\n",
    );
    out.push_str(&format!(
        "4. For each accepted parser, create `<corpus>/<id>/fixtures/`, copy the group's sample \
         into it as `01.eml`, and write `<corpus>/<id>/config.toml` (corpus: `{}`).\n",
        context.corpus.display()
    ));
    out.push_str(&format!(
        "5. Verify: `abmx parser check --parser <id> --parsers {}` and \
         `abmx verify parser {}/<id>`. A parser is not done until every sample is `strict`.\n",
        context.corpus.display(),
        context.corpus.display()
    ));
    out.push_str(
        "6. Run `abmx accounts --json`, ask which ledger account the parser's account key \
         maps to, and write it under `[accounts]` in the config file above.\n",
    );
    out.push_str(
        "7. If you cannot make a parser pass, delete its `config.toml` and report why.\n\n",
    );

    out.push_str("### Stop rules\n\n");
    out.push_str(
        "- Never invent a value, a ledger UUID, or an account mapping.\n\
         - If the samples contain no monetary amount, do not create a parser.\n\
         - Do not write into another parser's directory.\n\
         - Do not modify the user's `[fetch.gmail]` or `[sink]` sections.\n\n",
    );

    out.push_str("## Candidates\n\n");
    if candidates.is_empty() {
        out.push_str("None. Report that every candidate is already parsed or ignored.\n");
        return out;
    }

    for (index, group) in candidates.iter().enumerate() {
        out.push_str(&format!(
            "### {}. `{}` — {} message(s)\n\n",
            index + 1,
            group.sender,
            group.count
        ));
        if let Some(path) = &group.sample {
            out.push_str(&format!("- sample: `{}`\n", path.display()));
        }
        if !group.subjects.is_empty() {
            out.push_str("- subjects seen:\n");
            for subject in &group.subjects {
                out.push_str(&format!("  - `{subject}`\n"));
            }
        }
        if let Some(email) = &group.email {
            out.push_str(&format!("\nBlocks ({}):\n\n```text\n", email.blocks.len()));
            for (index, block) in email.blocks.iter().enumerate() {
                out.push_str(&format!("{index:>3}  {}\n", block.text));
            }
            out.push_str("```\n\n");
        }
    }

    out.push_str("## Existing accounts\n\n");
    if context.accounts.is_empty() {
        out.push_str("`[accounts]` is empty. Every parser will need a mapping.\n");
    } else {
        for (key, value) in context.accounts {
            out.push_str(&format!("- `{key}` → `{value}`\n"));
        }
    }

    out
}

/// Reject an `--input` path that does not exist early, so the error names it.
pub fn check_input(options: &Options) -> Result<()> {
    if let Some(input) = &options.input {
        if !input.exists() {
            bail!("{} does not exist", input.display());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sender_address_extracts_and_lowercases() {
        assert_eq!(
            sender_address("example <support@bank.example.com>"),
            "support@bank.example.com"
        );
        assert_eq!(sender_address("  example@example.invalid "), "example@example.invalid");
    }

    #[test]
    fn otp_subjects_are_ignored() {
        assert!(subject_ignored("Kode OTP Anda"));
        assert!(subject_ignored("New Device Accessing myBank"));
        assert!(subject_ignored("Your verification code"));
        assert!(!subject_ignored("Transaksi Online - Debit example"));
    }

    #[test]
    fn amount_and_date_signals() {
        let (amount, date, _) = outline();
        assert!(amount.is_match("Rp 27.500"));
        assert!(amount.is_match("1.234.567,89"));
        assert!(amount.is_match("USD 12.00"));
        assert!(!amount.is_match("Transaksi berhasil"));

        assert!(date.is_match("30 Oct 2026 17:28:17"));
        assert!(date.is_match("2026-10-30"));
        assert!(date.is_match("30/10/2026"));
        assert!(!date.is_match("Nomor referensi 12345"));
    }

    #[test]
    fn base_query_includes_window_and_narrowing() {
        let options = Options {
            days: 30,
            label: Some("receipt".to_owned()),
            senders: vec!["a@b.example".to_owned()],
            ..Options::default()
        };
        let query = base_query(&options);
        assert!(query.contains("newer_than:30d"));
        assert!(query.contains("from:a@b.example"));
        assert!(query.contains("label:receipt"));
    }
}
