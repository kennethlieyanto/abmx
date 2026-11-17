//! `abmx-parser` — Indonesian bank transaction email parser.
//!
//! Input: an RFC 5322 message (`.eml` bytes).
//! Output: one [`Transaction`] per email, or an [`Outcome`] explaining why not.
//!
//! Parser specs are runtime configuration. Nothing is embedded in the binary;
//! see [`Options`] and [`discover`] for the search order.

use std::path::{Path, PathBuf};

use abmx_tx::Transaction;

pub use abmx_mail::{blocks_from_html, parse_eml, MailError};
pub use abmx_parse::{Error as ParseError, Outcome, Registry, Shadow, SpecSummary, SPEC_VERSION};
pub use abmx_tx::{
    json_schema, Address, Block, Extraction, Kind, Money, NormalizedEmail, Source, Status,
};

/// Version of the transaction JSON contract.
pub const SCHEMA_VERSION: u32 = 1;

/// Colon-separated list of parser directories. Checked when `--parsers` is absent.
pub const ENV_PARSERS: &str = "ABMX_PARSERS";

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Mail(#[from] abmx_mail::MailError),
    #[error(transparent)]
    Parse(#[from] abmx_parse::Error),
    #[error("no .eml files at {0}")]
    Empty(PathBuf),
    #[error("no parser specs found")]
    NoCorpus { tried: Vec<PathBuf> },
    #[error("no parser specs in {path} (found via {found_via}); pass --parsers <dir> or remove the directory")]
    EmptyCorpus {
        path: PathBuf,
        found_via: &'static str,
    },
    #[error("{path} does not exist")]
    MissingPath { path: PathBuf },
}

/// Where to look for parser specs.
#[derive(Debug, Clone, Default)]
pub struct Options {
    /// `--parsers`, repeated. Takes precedence over [`ENV_PARSERS`].
    pub parsers: Vec<PathBuf>,
}

#[derive(Debug, Clone)]
pub struct Candidate {
    pub path: PathBuf,
    /// `flag`, `env`, `xdg` or `system`.
    pub source: &'static str,
    pub exists: bool,
    pub has_specs: bool,
}

#[derive(Debug, Clone)]
pub struct Discovery {
    /// Every place that was considered, in precedence order.
    pub candidates: Vec<Candidate>,
    /// The directories that will actually be loaded, highest precedence first.
    pub dirs: Vec<PathBuf>,
}

impl Discovery {
    /// Directory state, used by `abmx parser paths`.
    pub fn status(&self, candidate: &Candidate) -> &'static str {
        if !candidate.exists {
            "missing"
        } else if self.dirs.iter().any(|dir| dir == &candidate.path) {
            "used"
        } else if candidate.has_specs {
            "skipped"
        } else {
            "empty"
        }
    }

    /// One line per candidate, in precedence order.
    pub fn describe(&self) -> String {
        self.candidates
            .iter()
            .map(|candidate| {
                format!(
                    "{:<9} {:<7} {}",
                    self.status(candidate),
                    candidate.source,
                    candidate.path.display()
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// Hint for implicit discovery when a lower-precedence corpus was ignored.
    pub fn note(&self) -> Option<String> {
        let skipped = self
            .candidates
            .iter()
            .filter(|candidate| self.status(candidate) == "skipped")
            .count();

        (skipped > 0).then(|| {
            format!(
                "note: {skipped} lower-precedence corpus ignored (first hit wins); pass --parsers to layer"
            )
        })
    }
}

/// Work out which parser directories would be used, without reading any spec.
///
/// Explicit sources (`--parsers`, `$ABMX_PARSERS`) are merged in order. The
/// implicit chain uses exactly one directory: the first that exists, so that a
/// stale user-level corpus can never silently contribute parsers.
pub fn discover(options: &Options) -> Discovery {
    let mut candidates = Vec::new();

    if !options.parsers.is_empty() {
        for dir in &options.parsers {
            let (exists, has_specs) = inspect(dir);
            candidates.push(Candidate {
                path: dir.clone(),
                source: "flag",
                exists,
                has_specs,
            });
        }
    } else if let Some(entries) = env_list(ENV_PARSERS) {
        for dir in entries {
            let (exists, has_specs) = inspect(&dir);
            candidates.push(Candidate {
                path: dir,
                source: "env",
                exists,
                has_specs,
            });
        }
    } else {
        // Keep inspecting after a hit so `abmx parser paths` can show what was skipped.
        for (path, source) in implicit_chain() {
            let (exists, has_specs) = inspect(&path);
            candidates.push(Candidate {
                path,
                source,
                exists,
                has_specs,
            });
        }
    }

    let dirs = select_dirs(&candidates);

    Discovery { candidates, dirs }
}

/// Explicit sources merge; the implicit chain takes the first directory that exists.
fn select_dirs(candidates: &[Candidate]) -> Vec<PathBuf> {
    let explicit = candidates
        .iter()
        .any(|candidate| matches!(candidate.source, "flag" | "env"));

    if explicit {
        return candidates
            .iter()
            .map(|candidate| candidate.path.clone())
            .collect();
    }

    candidates
        .iter()
        .find(|candidate| candidate.exists)
        .map(|candidate| vec![candidate.path.clone()])
        .unwrap_or_default()
}

impl Options {
    pub fn new() -> Self {
        Self::default()
    }
}

/// A loaded registry, ready to parse.
pub struct Parser {
    registry: Registry,
}

impl Parser {
    /// Resolve the corpus and load it.
    pub fn load(options: &Options) -> Result<(Self, Discovery), Error> {
        let discovery = discover(options);

        if discovery.dirs.is_empty() {
            return Err(Error::NoCorpus {
                tried: discovery
                    .candidates
                    .iter()
                    .map(|candidate| candidate.path.clone())
                    .collect(),
            });
        }

        // An empty implicit corpus is a broken install, not a reason to fall
        // back to a lower-precedence directory.
        if discovery.dirs.len() == 1 {
            if let Some(chosen) = discovery
                .candidates
                .iter()
                .find(|candidate| candidate.path == discovery.dirs[0])
            {
                if matches!(chosen.source, "xdg" | "system") && chosen.exists && !chosen.has_specs {
                    return Err(Error::EmptyCorpus {
                        path: chosen.path.clone(),
                        found_via: chosen.source,
                    });
                }
            }
        }

        let registry = Registry::from_dirs(&discovery.dirs)?;
        Ok((Self { registry }, discovery))
    }

    /// Resolve and load with the default search order.
    pub fn discover() -> Result<Self, Error> {
        Self::load(&Options::default()).map(|(parser, _)| parser)
    }

    /// Load exactly one directory, bypassing the search order.
    pub fn from_dir(parsers: impl AsRef<Path>) -> Result<Self, Error> {
        Ok(Self {
            registry: Registry::load(parsers)?,
        })
    }

    pub fn registry(&self) -> &Registry {
        &self.registry
    }

    /// Parse one message with a parser chosen by the caller (a configured
    /// Gmail label). There is no routing and no fallback.
    pub fn parse_bytes_with(
        &self,
        id: &str,
        bytes: &[u8],
        file: Option<String>,
        account: &str,
    ) -> Result<Outcome, Error> {
        let email = abmx_mail::parse_eml(bytes, file)?;
        Ok(self.registry.run(id, &email, account))
    }

    /// Parse a file or every `.eml` under a directory, recursively.
    pub fn parse_path_with(
        &self,
        id: &str,
        path: &Path,
        account: &str,
    ) -> Result<Vec<(PathBuf, Outcome)>, Error> {
        let files = if path.is_dir() {
            let mut files = Vec::new();
            collect_eml_files(path, &mut files);
            files.sort();
            files
        } else {
            vec![path.to_path_buf()]
        };

        if files.is_empty() {
            return Err(Error::Empty(path.to_path_buf()));
        }

        let mut results = Vec::with_capacity(files.len());
        for file in files {
            let bytes = std::fs::read(&file).map_err(|error| {
                Error::Mail(abmx_mail::MailError::Unreadable {
                    path: file.display().to_string(),
                    message: error.to_string(),
                })
            })?;
            let outcome =
                self.parse_bytes_with(id, &bytes, Some(file.display().to_string()), account)?;
            results.push((file, outcome));
        }

        Ok(results)
    }
}

/// Everything the resolved corpus knows how to parse.
pub fn specs() -> Result<Vec<SpecSummary>, Error> {
    Ok(Parser::discover()?.registry().summaries())
}

/// The JSON Schema of [`Transaction`].
pub fn schema() -> serde_json::Value {
    json_schema()
}

/// Re-exported for callers that want to build their own `Outcome` handling.
pub fn transaction_of(outcome: &Outcome) -> Option<&Transaction> {
    match outcome {
        Outcome::Parsed(transaction) => Some(transaction),
        _ => None,
    }
}

fn implicit_chain() -> Vec<(PathBuf, &'static str)> {
    let mut chain = Vec::new();

    if let Some(config) = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))
    {
        chain.push((config.join("abmx/parsers"), "xdg"));
    }

    chain.push((PathBuf::from("/etc/abmx/parsers"), "system"));
    chain
}

fn env_list(name: &str) -> Option<Vec<PathBuf>> {
    let value = std::env::var(name).ok()?;
    let entries: Vec<PathBuf> = std::env::split_paths(&value)
        .filter(|path| !path.as_os_str().is_empty())
        .collect();

    if entries.is_empty() {
        None
    } else {
        Some(entries)
    }
}

fn inspect(dir: &Path) -> (bool, bool) {
    if !dir.is_dir() {
        return (false, false);
    }

    let mut files = Vec::new();
    collect_spec_files(dir, &mut files);
    (true, !files.is_empty())
}

fn collect_spec_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        // ConfigMap volumes keep their payload in `..data` and timestamped
        // `..2026_...` siblings; those are symlink bookkeeping, not parser dirs.
        if is_configmap_bookkeeping(&path) {
            continue;
        }
        if path.is_dir() {
            collect_spec_files(&path, out);
        } else if is_spec_file(&path) {
            out.push(path);
        }
    }
}

fn collect_eml_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if is_configmap_bookkeeping(&path) {
            continue;
        }
        if path.is_dir() {
            collect_eml_files(&path, out);
        } else if path.extension().and_then(|value| value.to_str()) == Some("eml") {
            out.push(path);
        }
    }
}

fn is_configmap_bookkeeping(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .map(|name| name.starts_with(".."))
        .unwrap_or(false)
}

fn is_spec_file(path: &Path) -> bool {
    path.file_name().and_then(|name| name.to_str()) == Some("config.toml")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn candidate(path: &str, source: &'static str, exists: bool, has_specs: bool) -> Candidate {
        Candidate {
            path: PathBuf::from(path),
            source,
            exists,
            has_specs,
        }
    }

    fn discovery(candidates: Vec<Candidate>) -> Discovery {
        let dirs = select_dirs(&candidates);
        Discovery { candidates, dirs }
    }

    #[test]
    fn implicit_chain_uses_the_first_existing_directory() {
        let d = discovery(vec![
            candidate("/xdg/abmx/parsers", "xdg", true, true),
            candidate("/etc/abmx/parsers", "system", true, true),
        ]);

        assert_eq!(d.dirs, vec![PathBuf::from("/xdg/abmx/parsers")]);
        assert_eq!(d.status(&d.candidates[1]), "skipped");
        assert!(d.note().is_some());
    }

    #[test]
    fn implicit_chain_stops_at_an_empty_directory() {
        let d = discovery(vec![
            candidate("/xdg/abmx/parsers", "xdg", true, false),
            candidate("/etc/abmx/parsers", "system", true, true),
        ]);

        // The empty directory is still the chosen one; loading then fails loudly
        // instead of silently reading the lower-precedence corpus.
        assert_eq!(d.dirs, vec![PathBuf::from("/xdg/abmx/parsers")]);
        assert_eq!(d.status(&d.candidates[0]), "used");
        assert_eq!(d.status(&d.candidates[1]), "skipped");
    }

    #[test]
    fn implicit_chain_never_reads_the_working_directory() {
        let chain = implicit_chain();
        assert!(chain.iter().all(|(path, _)| path.is_absolute()));
        assert!(chain
            .iter()
            .all(|(_, source)| matches!(*source, "xdg" | "system")));
    }

    #[test]
    fn explicit_sources_merge_in_order() {
        let d = discovery(vec![
            candidate("/override", "flag", true, true),
            candidate("/base", "flag", true, false),
        ]);

        assert_eq!(
            d.dirs,
            vec![PathBuf::from("/override"), PathBuf::from("/base")]
        );
        assert!(d.candidates.iter().all(|c| d.status(c) == "used"));
        assert!(d.note().is_none());
    }

    #[test]
    fn nothing_found_yields_no_dirs() {
        let d = discovery(vec![candidate("/xdg/abmx/parsers", "xdg", false, false)]);

        assert!(d.dirs.is_empty());
        assert_eq!(d.status(&d.candidates[0]), "missing");
    }
}
