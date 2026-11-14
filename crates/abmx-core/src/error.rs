use std::path::PathBuf;

/// One error type for the whole pipeline.
///
/// The variants are deliberately coarse: a caller almost always wants "did this
/// work, and if not, which stage failed", not a precise taxonomy. The message
/// carries the detail, and `source()` keeps the underlying cause.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("fetch: {0}")]
    Fetch(String),

    #[error("sink: {0}")]
    Sink(String),

    #[error("parse: {0}")]
    Parse(String),

    #[error("state: {0}")]
    State(String),

    #[error("config: {0}")]
    Config(String),

    #[error("io: {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error(transparent)]
    Json(#[from] serde_json::Error),

    #[error(transparent)]
    Parser(#[from] abmx_parser::Error),
}

impl Error {
    pub fn io(path: impl Into<PathBuf>, source: std::io::Error) -> Self {
        Self::Io {
            path: path.into(),
            source,
        }
    }

    /// A short, stable label for exit codes and logs.
    pub fn stage(&self) -> &'static str {
        match self {
            Self::Fetch(_) => "fetch",
            Self::Sink(_) => "sink",
            Self::Parse(_) => "parse",
            Self::State(_) => "state",
            Self::Config(_) => "config",
            Self::Io { .. } => "io",
            Self::Json(_) => "json",
            Self::Parser(_) => "abmx-parser",
        }
    }
}

pub type Result<T, E = Error> = std::result::Result<T, E>;
