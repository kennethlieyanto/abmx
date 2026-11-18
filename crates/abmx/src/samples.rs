//! `abmx samples`: download the evidence a parser author needs.
//!
//! Only labels mapped in `[labels.accounts]` are sampled. The command never
//! parses anything and never requires a parser corpus; it exists so an agent
//! (or a human) can inspect real mail and write a spec from it.

use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};

use abmx_gmail::GmailFetcher;
use anyhow::{bail, Context, Result};
use serde::Serialize;

use crate::config::Config;
use crate::labels::{self, ManagedLabel};
use crate::paths;

#[derive(Debug, Clone)]
pub struct Options {
    /// Where to write the samples. Defaults to the XDG config dir.
    pub out: Option<PathBuf>,
    /// How many of the newest messages to download per label.
    pub samples: usize,
    /// Restrict to one Gmail label name or parser id.
    pub label: Option<String>,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            out: None,
            samples: 3,
            label: None,
        }
    }
}

#[derive(Debug, Serialize)]
pub struct SampleReport {
    pub output: PathBuf,
    pub requested: usize,
    pub labels: Vec<LabelSamples>,
    /// Problems that are not tied to one label (for example orphan mappings).
    pub warnings: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct LabelSamples {
    pub label: String,
    pub parser_id: String,
    pub requested: usize,
    pub downloaded: usize,
    pub messages: Vec<String>,
    pub warnings: Vec<String>,
}

pub fn download(
    config: &Config,
    fetcher: &GmailFetcher,
    options: &Options,
) -> Result<SampleReport> {
    let discovered: Vec<String> = fetcher
        .routed_labels()?
        .into_iter()
        .map(|label| label.name)
        .collect();
    let plan = labels::plan(&discovered, &config.labels);

    let mut targets: Vec<ManagedLabel> = plan.managed;
    if let Some(selector) = &options.label {
        let selector = selector.trim();
        targets.retain(|label| {
            label.label.eq_ignore_ascii_case(selector)
                || label.parser_id.eq_ignore_ascii_case(selector)
        });
        if targets.is_empty() {
            bail!("no managed label matches `{selector}`");
        }
    }

    if targets.is_empty() {
        bail!(
            "no managed labels; run `abmx setup` to choose which labels to import\n  \
             (only [labels.accounts] entries are sampled)"
        );
    }

    let requested = options.samples.max(1);
    let output = options
        .out
        .clone()
        .unwrap_or_else(|| paths::xdg_config_dir().join("abmx/samples"));

    std::fs::create_dir_all(&output).with_context(|| format!("creating {}", output.display()))?;
    restrict(&output, 0o700)?;

    let mut warnings = Vec::new();
    for parser_id in &plan.orphans {
        warnings.push(format!(
            "`{}{parser_id}` is mapped in [labels.accounts] but no such Gmail label exists",
            config.labels.prefix
        ));
    }

    let mut labels = Vec::with_capacity(targets.len());
    for target in &targets {
        labels.push(download_label(fetcher, target, &output, requested)?);
    }

    let report = SampleReport {
        output: output.clone(),
        requested,
        labels,
        warnings,
    };

    write_manifest(&output, &report)?;
    Ok(report)
}

fn download_label(
    fetcher: &GmailFetcher,
    target: &ManagedLabel,
    output: &Path,
    requested: usize,
) -> Result<LabelSamples> {
    let dir = output.join(&target.parser_id);

    // The directory is scratch space. Removing it means a previous 03.eml can
    // never masquerade as a sample from this run.
    if dir.exists() {
        std::fs::remove_dir_all(&dir).with_context(|| format!("clearing {}", dir.display()))?;
    }
    std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
    restrict(&dir, 0o700)?;

    let samples = fetcher.sample_label(&target.label, requested)?;
    let mut messages = Vec::with_capacity(samples.len());
    for (index, sample) in samples.iter().enumerate() {
        let path = dir.join(format!("{:02}.eml", index + 1));
        write_private(&path, &sample.bytes)?;
        messages.push(sample.id.clone());
    }

    let mut warnings = Vec::new();
    if samples.len() < requested {
        warnings.push(format!(
            "wanted {requested} samples, downloaded {}",
            samples.len()
        ));
    }

    Ok(LabelSamples {
        label: target.label.clone(),
        parser_id: target.parser_id.clone(),
        requested,
        downloaded: samples.len(),
        messages,
        warnings,
    })
}

fn write_manifest(output: &Path, report: &SampleReport) -> Result<()> {
    let path = output.join("manifest.json");
    let json = serde_json::to_vec_pretty(report).context("serializing the samples manifest")?;
    write_private(&path, &json)
}

fn write_private(path: &Path, bytes: &[u8]) -> Result<()> {
    let mut options = OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options
        .open(path)
        .with_context(|| format!("opening {}", path.display()))?;
    file.write_all(bytes)
        .with_context(|| format!("writing {}", path.display()))?;
    file.flush()
        .with_context(|| format!("writing {}", path.display()))?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
    }

    Ok(())
}

fn restrict(path: &Path, mode: u32) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))
            .with_context(|| format!("setting permissions on {}", path.display()))?;
    }
    #[cfg(not(unix))]
    let _ = (path, mode);

    Ok(())
}
