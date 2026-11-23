//! Scripted bank parsers: a `parser.rhai` per directory, transactions out.
//!
//! The corpus is runtime data. A parser is a directory:
//!
//! ```text
//! <corpus>/<id>/
//!   parser.rhai
//!   fixtures/
//! ```
//!
//! The directory name is the parser id. `parser.rhai` defines `meta()` and one
//! entry point, `parse_mail(ctx)` or `parse_statement(doc)`. The script only
//! produces observations; identity, offsets, `Strict`, and the statement
//! arithmetic stay in Rust.

pub mod engine;
pub mod mail;
pub mod spec;
pub mod statement;
pub mod util;

use std::path::{Path, PathBuf};

use abmx_doc::Document;
use abmx_tx::Transaction;

pub use engine::{engine, Executor, Script, SCRIPT_FILE};
pub use spec::{InputKind, Meta, SpecSummary, API_VERSION};
pub use util::{is_parser_id, norm};

pub use abmx_mail::{blocks_from_html, parse_eml, MailError};
pub use abmx_tx::{
    json_schema, Address, Block, Extraction, Kind, Money, NormalizedEmail, Source, Status,
    Transaction as Tx,
};

/// Version of the transaction JSON contract.
pub const SCHEMA_VERSION: u32 = 1;

/// Colon-separated list of parser directories. Checked when `--parsers` is absent.
pub const ENV_PARSERS: &str = "ABMX_PARSERS";

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("could not read {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("{path}: {message}")]
    Script { path: PathBuf, message: String },
    #[error("no parser scripts found under {path}{hint}")]
    NoParsers { path: PathBuf, hint: &'static str },
    #[error("{path}: parser directory `{found}` is not a valid id")]
    InvalidId { path: PathBuf, found: String },
    #[error("no parser specs found")]
    NoCorpus { tried: Vec<PathBuf> },
    #[error("no parser scripts in {path} (found via {found_via}); pass --parsers <dir> or remove the directory")]
    EmptyCorpus {
        path: PathBuf,
        found_via: &'static str,
    },
    #[error("{path} does not exist")]
    MissingPath { path: PathBuf },
    #[error(transparent)]
    Mail(#[from] abmx_mail::MailError),
    #[error("no .eml files at {0}")]
    Empty(PathBuf),
}

/// A parser found at lower precedence than another with the same id.
#[derive(Debug, Clone, serde::Serialize)]
pub struct Shadow {
    pub id: String,
    pub skipped: PathBuf,
    pub winner: PathBuf,
}

/// What one email produced.
#[derive(Debug, Clone)]
pub enum Outcome {
    /// A parser ran and produced at least one transaction.
    Parsed(Vec<Transaction>),
    /// No parser with this id exists in the corpus.
    Missing(String),
    /// A parser ran but extraction failed.
    Failed { parser: String, error: String },
}

/// The loaded corpus and the engine its scripts run in.
pub struct Registry {
    pub parsers: Vec<Script>,
    /// Parsers skipped because a higher-precedence directory defines the same id.
    pub shadows: Vec<Shadow>,
    executor: Executor,
}

impl Registry {
    /// Load every `parser.rhai` under one directory.
    pub fn load(dir: impl AsRef<Path>) -> Result<Self, Error> {
        Self::from_dirs(&[dir.as_ref().to_path_buf()])
    }

    /// Load scripts from several directories, highest precedence first.
    pub fn from_dirs(dirs: &[PathBuf]) -> Result<Self, Error> {
        let executor = engine();
        let mut parsers: Vec<Script> = Vec::new();
        let mut shadows: Vec<Shadow> = Vec::new();
        let mut seen: std::collections::HashMap<String, PathBuf> = std::collections::HashMap::new();

        for dir in dirs {
            let mut paths = Vec::new();
            collect_scripts(dir, &mut paths)?;
            paths.sort();

            for path in paths {
                let id = path
                    .parent()
                    .and_then(Path::file_name)
                    .and_then(|name| name.to_str())
                    .unwrap_or_default()
                    .to_owned();

                if !is_parser_id(&id) {
                    return Err(Error::InvalidId { path, found: id });
                }

                if let Some(winner) = seen.get(&id) {
                    shadows.push(Shadow {
                        id,
                        skipped: path,
                        winner: winner.clone(),
                    });
                    continue;
                }

                let script = Script::load(&executor, &path, id.clone()).map_err(|message| {
                    Error::Script {
                        path: path.clone(),
                        message,
                    }
                })?;
                seen.insert(id, path);
                parsers.push(script);
            }
        }

        if parsers.is_empty() {
            let path = dirs.first().cloned().unwrap_or_default();
            let hint = if dirs.iter().any(|dir| has_old_specs(dir)) {
                " (found `config.toml`; parsers are now `parser.rhai`, see `abmx skill receipt` or `abmx skill statement`)"
            } else {
                ""
            };
            return Err(Error::NoParsers { path, hint });
        }

        Ok(Self {
            parsers,
            shadows,
            executor,
        })
    }

    pub fn find(&self, id: &str) -> Option<&Script> {
        self.parsers.iter().find(|script| script.id == id)
    }

    /// Parse with the parser named by a trusted routing decision (a configured
    /// Gmail label). There is no guessing: an unknown id is an error.
    pub fn run(
        &self,
        id: &str,
        email: &abmx_tx::NormalizedEmail,
        account: &str,
    ) -> Result<Outcome, String> {
        let Some(script) = self.find(id) else {
            return Ok(Outcome::Missing(id.to_owned()));
        };
        if script.input() != InputKind::Mail {
            return Ok(Outcome::Failed {
                parser: id.to_owned(),
                error: "this is a statement parser; it cannot parse an email".to_owned(),
            });
        }
        if !script.meta.enabled {
            return Ok(Outcome::Failed {
                parser: id.to_owned(),
                error: "parser is disabled".to_owned(),
            });
        }

        match mail::parse(script, &self.executor, email, account) {
            Ok(transactions) => Ok(Outcome::Parsed(transactions)),
            Err(error) => Ok(Outcome::Failed {
                parser: id.to_owned(),
                error,
            }),
        }
    }

    /// Parse one PDF statement with a named parser.
    pub fn run_statement(
        &self,
        id: &str,
        document: &Document,
        origin: Option<String>,
    ) -> Result<Vec<Transaction>, String> {
        let script = self
            .find(id)
            .ok_or_else(|| format!("no parser `{id}` in the corpus"))?;
        if script.input() != InputKind::Statement {
            return Err(format!("parser `{id}` is not a statement parser"));
        }
        if !script.meta.enabled {
            return Err(format!("parser `{id}` is disabled"));
        }
        statement::parse(script, &self.executor, document, origin)
    }

    /// Statement parsers only, in corpus order.
    pub fn statements(&self) -> impl Iterator<Item = &Script> {
        self.parsers
            .iter()
            .filter(|script| script.input() == InputKind::Statement)
    }

    /// Mail parsers only, in corpus order.
    pub fn mail_parsers(&self) -> impl Iterator<Item = &Script> {
        self.parsers
            .iter()
            .filter(|script| script.input() == InputKind::Mail)
    }

    /// A machine-readable description of what the registry covers.
    pub fn summaries(&self) -> Vec<SpecSummary> {
        self.parsers.iter().map(Script::summary).collect()
    }
}

/// Where to look for parser scripts.
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

/// Work out which parser directories would be used, without reading any script.
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

fn env_list(name: &str) -> Option<Vec<PathBuf>> {
    let value = std::env::var_os(name)?;
    let value = value.to_string_lossy();
    let paths: Vec<PathBuf> = std::env::split_paths(value.as_ref())
        .filter(|path| !path.as_os_str().is_empty())
        .collect();

    (!paths.is_empty()).then_some(paths)
}

fn implicit_chain() -> Vec<(PathBuf, &'static str)> {
    let mut chain = Vec::new();

    if let Some(base) = config_home() {
        chain.push((base.join("abmx").join("parsers"), "xdg"));
    }
    chain.push((PathBuf::from("/etc/abmx/parsers"), "system"));

    chain
}

fn config_home() -> Option<PathBuf> {
    std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|path| !path.as_os_str().is_empty())
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))
}

fn inspect(dir: &Path) -> (bool, bool) {
    let exists = dir.is_dir();
    if !exists {
        return (false, false);
    }

    let mut paths = Vec::new();
    let has_specs = collect_scripts(dir, &mut paths)
        .ok()
        .is_some_and(|()| !paths.is_empty());

    (exists, has_specs)
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
        self.registry
            .run(id, &email, account)
            .map_err(|message| Error::Script {
                path: PathBuf::from(id),
                message,
            })
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

    /// Parse one PDF with a named statement parser.
    pub fn parse_statement_with(
        &self,
        id: &str,
        document: &Document,
        origin: Option<String>,
    ) -> Result<Vec<Transaction>, String> {
        self.registry.run_statement(id, document, origin)
    }
}

/// Everything the resolved corpus knows how to parse.
pub fn specs() -> Result<Vec<SpecSummary>, Error> {
    Ok(Parser::discover()?.registry().summaries())
}

/// JSON Schema of the transaction contract.
pub fn schema() -> serde_json::Value {
    abmx_tx::json_schema()
}

fn collect_scripts(dir: &Path, out: &mut Vec<PathBuf>) -> Result<(), Error> {
    let entries = std::fs::read_dir(dir).map_err(|source| Error::Io {
        path: dir.to_path_buf(),
        source,
    })?;

    for entry in entries {
        let entry = entry.map_err(|source| Error::Io {
            path: dir.to_path_buf(),
            source,
        })?;
        let path = entry.path();

        // ConfigMap volumes keep their payload in `..data` and timestamped
        // `..2026_...` siblings; those are symlink bookkeeping, not parsers.
        if path
            .file_name()
            .and_then(|name| name.to_str())
            .map(|name| name.starts_with(".."))
            .unwrap_or(false)
        {
            continue;
        }

        if path.is_dir() {
            if path.file_name().and_then(|name| name.to_str()) == Some("fixtures") {
                continue;
            }
            collect_scripts(&path, out)?;
        } else if path.file_name().and_then(|name| name.to_str()) == Some(SCRIPT_FILE) {
            out.push(path);
        }
    }

    Ok(())
}

fn collect_eml_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_eml_files(&path, out);
        } else if path.extension().and_then(|ext| ext.to_str()) == Some("eml") {
            out.push(path);
        }
    }
}

/// True when a corpus root still holds the old TOML layout, which is only used
/// to make the "no parser scripts" error actionable.
fn has_old_specs(dir: &Path) -> bool {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return false;
    };
    entries.flatten().any(|entry| {
        let path = entry.path();
        path.is_dir() && path.join("config.toml").is_file()
    })
}
