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

fn dir_id(dir: &Path) -> Option<String> {
    dir.file_name()
        .and_then(|name| name.to_str())
        .map(str::to_owned)
        .filter(|name| abmx_parse::is_parser_id(name))
}

/// Verify one candidate parser against a file or directory of `.eml` samples.
///
/// `spec` may be a parser directory (`parsers/<id>/`) or a `config.toml`. The
/// parser id is the directory name; pass `id` when the candidate lives in a
/// scratch directory that is not named after the parser.
///
/// The candidate is copied into a temporary corpus, so the live parser corpus is
/// never modified and the candidate cannot be shadowed by a sibling with the
/// same id.
pub fn verify(spec: &Path, id: Option<&str>, input: Option<&Path>) -> Result<VerifyReport> {
    let (spec_file, spec_dir) = if spec.is_dir() {
        (spec.join("config.toml"), spec.to_path_buf())
    } else {
        (
            spec.to_path_buf(),
            spec.parent().unwrap_or(Path::new(".")).to_path_buf(),
        )
    };

    let id = match id.map(str::to_owned).or_else(|| dir_id(&spec_dir)) {
        Some(id) => id,
        None => {
            bail!("no parser id: pass --id, or put the spec in a directory named after the parser")
        }
    };

    if !spec_file.is_file() {
        bail!("{} does not exist", spec_file.display());
    }

    if !abmx_parse::is_parser_id(&id) {
        bail!("parser id `{id}` is not a valid directory name");
    }

    let input = input
        .map(Path::to_path_buf)
        .unwrap_or_else(|| spec_dir.join("fixtures"));

    if !input.exists() {
        bail!("{} does not exist", input.display());
    }

    let text = std::fs::read_to_string(&spec_file)
        .with_context(|| format!("reading {}", spec_file.display()))?;

    let dir = tempfile::tempdir().context("creating a temporary parser corpus")?;
    let candidate = dir.path().join(&id);
    std::fs::create_dir_all(&candidate).context("creating the candidate parser directory")?;
    std::fs::write(candidate.join("config.toml"), &text)
        .context("writing the candidate spec into the temporary corpus")?;

    let (parser, _) = abmx_parser::Parser::load(&abmx_parser::Options {
        parsers: vec![dir.path().to_path_buf()],
    })
    .map_err(|error| anyhow::anyhow!("loading candidate spec `{id}`: {error}"))?;

    let results = parser
        .parse_path_with(&id, &input, "verify")
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
                abmx_parser::Outcome::Missing(reason) => VerifyOutcome::Unmatched(reason),
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
