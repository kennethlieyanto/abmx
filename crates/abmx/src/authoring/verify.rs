use std::path::{Path, PathBuf};

use abmx_core::Transaction;
use anyhow::{bail, Context, Result};

#[derive(Debug, Clone)]
pub enum VerifyOutcome {
    Strict(Transaction),
    Heuristic(Transaction),
    Unmatched(String),
    Failed { parser: String, error: String },
}

impl VerifyOutcome {
    pub fn transaction(&self) -> Option<&Transaction> {
        match self {
            Self::Strict(transaction) | Self::Heuristic(transaction) => Some(transaction),
            Self::Unmatched(_) | Self::Failed { .. } => None,
        }
    }

    pub fn label(&self) -> &'static str {
        match self {
            Self::Strict(_) => "strict",
            Self::Heuristic(_) => "heuristic",
            Self::Unmatched(_) => "unmatched",
            Self::Failed { .. } => "failed",
        }
    }
}

#[derive(Debug, Clone)]
pub struct VerifySample {
    pub file: PathBuf,
    pub outcome: VerifyOutcome,
}

#[derive(Debug, Clone)]
pub struct VerifyReport {
    pub spec_id: String,
    pub samples: Vec<VerifySample>,
}

impl VerifyReport {
    pub fn strict(&self) -> usize {
        self.samples
            .iter()
            .filter(|sample| matches!(sample.outcome, VerifyOutcome::Strict(_)))
            .count()
    }

    pub fn heuristic(&self) -> usize {
        self.samples
            .iter()
            .filter(|sample| matches!(sample.outcome, VerifyOutcome::Heuristic(_)))
            .count()
    }

    pub fn failures(&self) -> usize {
        self.samples
            .iter()
            .filter(|sample| {
                matches!(
                    sample.outcome,
                    VerifyOutcome::Unmatched(_) | VerifyOutcome::Failed { .. }
                )
            })
            .count()
    }

    /// abmx-parser's exit-code convention: failures beat heuristic warnings.
    pub fn exit_code(&self) -> i32 {
        if self.failures() > 0 {
            2
        } else if self.heuristic() > 0 {
            3
        } else {
            0
        }
    }
}

/// Verify one candidate spec against a file or directory of `.eml` samples.
///
/// The candidate is copied into a temporary corpus, so the live parser corpus is
/// never modified and the candidate cannot be shadowed by a sibling with the
/// same id.
pub fn verify(spec: &Path, input: &Path) -> Result<VerifyReport> {
    let text =
        std::fs::read_to_string(spec).with_context(|| format!("reading {}", spec.display()))?;
    let value: toml::Value =
        toml::from_str(&text).with_context(|| format!("parsing {} as TOML", spec.display()))?;
    let id = value
        .get("id")
        .and_then(toml::Value::as_str)
        .filter(|id| !id.is_empty())
        .ok_or_else(|| anyhow::anyhow!("{} has no `id`", spec.display()))?
        .to_owned();

    if !input.exists() {
        bail!("{} does not exist", input.display());
    }
    if id.contains('/') || id.contains('\\') || id == "." || id == ".." {
        bail!("parser id `{id}` is not a safe file stem");
    }

    let dir = tempfile::tempdir().context("creating a temporary parser corpus")?;
    std::fs::write(dir.path().join(format!("{id}.toml")), &text)
        .context("writing the candidate spec into the temporary corpus")?;

    let (parser, _) = abmx_parser::Parser::load(&abmx_parser::Options {
        parsers: vec![dir.path().to_path_buf()],
        router: None,
    })
    .map_err(|error| anyhow::anyhow!("loading candidate spec `{id}`: {error}"))?;

    let results = parser
        .parse_path(input)
        .with_context(|| format!("parsing {}", input.display()))?;
    if results.is_empty() {
        bail!("no .eml files under {}", input.display());
    }

    let samples = results
        .into_iter()
        .map(|(file, outcome)| {
            let outcome = match outcome {
                abmx_parser::Outcome::Parsed(transaction) => match transaction.extraction {
                    abmx_parser::Extraction::Strict => VerifyOutcome::Strict(*transaction),
                    abmx_parser::Extraction::Heuristic => VerifyOutcome::Heuristic(*transaction),
                    abmx_parser::Extraction::Failed => VerifyOutcome::Failed {
                        parser: transaction.parser.clone(),
                        error: "abmx-parser marked the extraction failed".to_owned(),
                    },
                },
                abmx_parser::Outcome::Unmatched(reason) => VerifyOutcome::Unmatched(reason),
                abmx_parser::Outcome::Failed { parser, error } => {
                    VerifyOutcome::Failed { parser, error }
                }
            };
            VerifySample { file, outcome }
        })
        .collect();

    Ok(VerifyReport {
        spec_id: id,
        samples,
    })
}
