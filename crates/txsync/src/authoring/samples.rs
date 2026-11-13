use std::collections::{BTreeMap, BTreeSet};
use std::io::Read;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};

/// One parsed sample, ready to be shown to an authoring agent.
#[derive(Debug, Clone)]
pub struct Sample {
    /// Where the bytes came from: a file path or a transport key.
    pub origin: String,
    /// The short name shown in the prompt.
    pub file_name: String,
    pub email: txp::NormalizedEmail,
}

/// A batch of samples that are believed to share one structure.
#[derive(Debug, Clone)]
pub struct SampleGroup {
    /// A Gmail label or the first directory component. `None` for a flat dir.
    pub label: Option<String>,
    pub samples: Vec<Sample>,
}

/// A sample that could not be normalized. It still reaches the prompt, because
/// silently dropping mail is how a parser gets written against half its inputs.
#[derive(Debug, Clone)]
pub struct SampleFailure {
    pub origin: String,
    pub error: String,
}

#[derive(Debug, Clone, Default)]
pub struct SampleSet {
    pub groups: Vec<SampleGroup>,
    pub failures: Vec<SampleFailure>,
    /// Samples dropped by `max_samples`.
    pub truncated: usize,
}

impl SampleSet {
    pub fn sample_count(&self) -> usize {
        self.groups.iter().map(|group| group.samples.len()).sum()
    }

    pub fn is_empty(&self) -> bool {
        self.sample_count() == 0 && self.failures.is_empty()
    }
}

/// Parse one message. The error is a string so a bad sample can travel into the
/// prompt without aborting the whole run.
pub fn sample_from_bytes(origin: impl Into<String>, bytes: &[u8]) -> Result<Sample, String> {
    let origin = origin.into();
    let email = txp::parse_eml(bytes, Some(origin.clone()))
        .map_err(|error| format!("{origin}: {error}"))?;
    let file_name = Path::new(&origin)
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_else(|| origin.clone());
    Ok(Sample {
        origin,
        file_name,
        email,
    })
}

/// Every `.eml` file under a path, sorted. A file path yields one entry.
pub fn collect_eml_files(path: &Path) -> Result<Vec<PathBuf>> {
    if path.is_file() {
        return Ok(vec![path.to_path_buf()]);
    }

    if !path.exists() {
        bail!("{} does not exist", path.display());
    }

    let mut files = Vec::new();
    walk(path, &mut files)?;
    files.sort();
    Ok(files)
}

fn walk(dir: &Path, files: &mut Vec<PathBuf>) -> Result<()> {
    let entries = std::fs::read_dir(dir).with_context(|| format!("reading {}", dir.display()))?;
    for entry in entries {
        let entry = entry.with_context(|| format!("reading {}", dir.display()))?;
        let path = entry.path();
        if path.is_dir() {
            walk(&path, files)?;
        } else if path
            .extension()
            .and_then(|extension| extension.to_str())
            .map(|extension| extension.eq_ignore_ascii_case("eml"))
            .unwrap_or(false)
        {
            files.push(path);
        }
    }
    Ok(())
}

/// Load a directory or a single file.
///
/// Immediate subdirectories become groups. That matches Gmail sampling, where
/// each requested label is written to its own directory, and it lets a user
/// pre-group exported samples without renaming anything.
pub fn load_path(path: &Path, max_per_group: usize) -> Result<SampleSet> {
    if path.as_os_str() == "-" {
        return load_stdin(max_per_group);
    }

    let files = collect_eml_files(path)?;
    if files.is_empty() {
        bail!("no .eml files under {}", path.display());
    }

    let mut grouped: BTreeMap<Option<String>, Vec<PathBuf>> = BTreeMap::new();
    for file in files {
        let label = group_label(path, &file);
        grouped.entry(label).or_default().push(file);
    }

    let mut set = SampleSet::default();
    for (label, files) in grouped {
        let mut group = SampleGroup {
            label,
            samples: Vec::new(),
        };

        for file in files {
            if group.samples.len() >= max_per_group.max(1) {
                set.truncated += 1;
                continue;
            }
            let origin = file.display().to_string();
            let bytes =
                std::fs::read(&file).with_context(|| format!("reading {}", file.display()))?;
            match sample_from_bytes(origin.clone(), &bytes) {
                Ok(sample) => group.samples.push(sample),
                Err(error) => set.failures.push(SampleFailure { origin, error }),
            }
        }

        if !group.samples.is_empty() {
            set.groups.push(group);
        }
    }

    Ok(set)
}

fn group_label(root: &Path, file: &Path) -> Option<String> {
    if !root.is_dir() {
        return None;
    }
    let relative = file.strip_prefix(root).ok()?;
    let mut components = relative.components();
    let first = components.next()?.as_os_str().to_string_lossy().to_string();
    if components.next().is_some() {
        Some(first)
    } else {
        None
    }
}

/// Read one RFC 5322 message from stdin.
pub fn load_stdin(max_per_group: usize) -> Result<SampleSet> {
    let mut bytes = Vec::new();
    std::io::stdin()
        .read_to_end(&mut bytes)
        .context("reading stdin")?;
    if bytes.is_empty() {
        bail!("stdin contained no message");
    }

    let mut set = SampleSet::default();
    match sample_from_bytes("stdin", &bytes) {
        Ok(sample) => set.groups.push(SampleGroup {
            label: None,
            samples: vec![sample]
                .into_iter()
                .take(max_per_group.max(1))
                .collect(),
        }),
        Err(error) => set.failures.push(SampleFailure {
            origin: "stdin".to_owned(),
            error,
        }),
    }
    Ok(set)
}

/// A sender subgroup inside a label.
#[derive(Debug, Clone)]
pub struct SenderGroup<'a> {
    pub sender: String,
    pub subjects: Vec<String>,
    pub samples: Vec<&'a Sample>,
}

/// Group a label's samples by sender, preserving sample order.
pub fn by_sender(samples: &[Sample]) -> Vec<SenderGroup<'_>> {
    let mut senders: BTreeMap<String, Vec<&Sample>> = BTreeMap::new();
    for sample in samples {
        senders
            .entry(sample.email.from.email.clone())
            .or_default()
            .push(sample);
    }

    senders
        .into_iter()
        .map(|(sender, samples)| {
            let subjects: BTreeSet<String> = samples
                .iter()
                .map(|sample| sample.email.subject.clone())
                .collect();
            SenderGroup {
                sender,
                subjects: subjects.into_iter().collect(),
                samples,
            }
        })
        .collect()
}

/// One row of the cross-sample block comparison.
#[derive(Debug, Clone)]
pub struct BlockRow {
    pub index: usize,
    pub values: Vec<String>,
    pub constant: bool,
    pub label_like: bool,
}

#[derive(Debug, Clone)]
pub struct Variance {
    pub block_count: usize,
    pub same_shape: bool,
    pub rows: Vec<BlockRow>,
}

impl Variance {
    /// True when every sample has the same number of blocks and every row is
    /// constant. A template with no varying blocks is almost never a
    /// transaction, but it is a useful signal either way.
    pub fn all_constant(&self) -> bool {
        self.same_shape && self.rows.iter().all(|row| row.constant)
    }
}

/// Compare block lists across samples. Rows are only computed when every
/// sample has the same block count; otherwise `same_shape` is `false` and the
/// prompt asks the user/agent to inspect the dumps.
pub fn variance(samples: &[Sample]) -> Option<Variance> {
    let first = samples.first()?;
    let block_count = first.email.blocks.len();
    let same_shape = samples
        .iter()
        .all(|sample| sample.email.blocks.len() == block_count);

    if !same_shape {
        return Some(Variance {
            block_count,
            same_shape: false,
            rows: Vec::new(),
        });
    }

    let mut rows = Vec::with_capacity(block_count);
    for index in 0..block_count {
        let mut values: Vec<String> = Vec::new();
        for sample in samples {
            let text = sample.email.blocks[index].text.clone();
            if !values.iter().any(|value| value == &text) {
                values.push(text);
            }
        }
        let constant = values.len() == 1;
        let label_like = constant && is_label_like(&values[0]);
        rows.push(BlockRow {
            index,
            values,
            constant,
            label_like,
        });
    }

    Some(Variance {
        block_count,
        same_shape: true,
        rows,
    })
}

/// A presentation-only heuristic for "this constant block looks like a label".
/// Inspired by txp's `infer`, but stricter: values such as `Rp 100.000`,
/// masked card numbers, and multi-word all-caps names are not labels. It never
/// affects parsing, only how the prompt annotates the variance table.
pub fn is_label_like(text: &str) -> bool {
    let trimmed = text.trim();
    let words = trimmed.split_whitespace().count();

    if trimmed.chars().count() > 48 || words > 4 {
        return false;
    }
    if words >= 4 && matches!(trimmed.chars().last(), Some(':' | ',' | '.')) {
        return false;
    }
    if trimmed.contains("XXXX") || trimmed.contains("****") {
        return false;
    }
    let letters: Vec<char> = trimmed.chars().filter(char::is_ascii_alphabetic).collect();
    // A multi-word all-caps block is almost always a value (merchant, name,
    // bank) rather than a label in these templates.
    if words >= 2 && !letters.is_empty() && letters.iter().all(|ch| ch.is_ascii_uppercase()) {
        return false;
    }
    let digits = trimmed.chars().filter(char::is_ascii_digit).count();
    // "Rp 100.000" and "30 Oct 2026" are values, not labels. A real label can
    // contain digits ("Top 3"), but not a number with almost no prose around it.
    if digits >= 3 && letters.len() <= 3 {
        return false;
    }
    !letters.is_empty()
}

/// Turn a label into a stable, lower-case parser id.
///
/// `example/QRIS Payment` -> `example.qris_payment`. A `/` becomes the `bank.type`
/// separator; other separators become `_`.
pub fn slug(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for ch in value.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
        } else if ch == '/' {
            if !out.ends_with('.') {
                out.push('.');
            }
        } else if !out.ends_with('_') && !out.is_empty() {
            out.push('_');
        }
    }
    out.trim_matches(|ch| ch == '.' || ch == '_').to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slug_keeps_bank_separator() {
        assert_eq!(slug("example/QRIS Payment"), "example.qris_payment");
        assert_eq!(slug("  Tax: 2026  "), "tax_2026");
        assert_eq!(slug("simple"), "simple");
    }

    #[test]
    fn label_heuristic_rejects_values() {
        assert!(is_label_like("Amount"));
        assert!(is_label_like("Transaction Date"));
        assert!(!is_label_like("Rp 100.000"));
        assert!(!is_label_like("ANDI PRATAMA"));
        assert!(!is_label_like("XXXX XXXX XXXX"));
        assert!(!is_label_like(
            "This is a long sentence that is clearly prose and not a label"
        ));
    }
}
