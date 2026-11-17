use std::path::{Path, PathBuf};

use abmx_actual::{ActualConfig, ActualSink, ClearedPolicy, NotesConfig};
use abmx_core::{AccountMap, LabelRoute, LabelRoutes, PolicyConfig, Sink};
use abmx_file::FileSink;
use abmx_gmail::{GmailConfig, GmailFetcher};
use anyhow::{bail, Context, Result};
use serde::Deserialize;

use crate::paths;

#[derive(Debug, Clone, Default, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub general: General,
    #[serde(default)]
    pub fetch: FetchSection,
    #[serde(default)]
    pub sink: SinkSection,
    /// Gmail label → parser + ledger account. The label is the routing identity.
    #[serde(default)]
    pub labels: LabelRoutes,
    #[serde(default)]
    pub policy: PolicyConfig,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct General {
    /// Explicit parser directories, in precedence order. Empty means `abmx-parser`'s
    /// implicit discovery (XDG, `/etc`).
    #[serde(default)]
    pub parsers: Vec<PathBuf>,
    /// SQLite state file. Defaults to `$XDG_STATE_HOME/abmx/state.db`.
    #[serde(default)]
    pub state: Option<PathBuf>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct FetchSection {
    #[serde(default)]
    pub gmail: Option<GmailSection>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct GmailSection {
    pub client_id: String,
    #[serde(default)]
    pub client_secret: Option<String>,
    #[serde(default)]
    pub client_secret_file: Option<PathBuf>,
    /// Where the refresh token lives. Defaults to the XDG state directory.
    #[serde(default)]
    pub token: Option<PathBuf>,
    /// Hard cap. Exceeding it is an error, never a silent truncation.
    #[serde(default)]
    pub max_messages: Option<usize>,
    /// Loopback port for `abmx auth gmail`; `0` picks one.
    #[serde(default)]
    pub port: Option<u16>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct SinkSection {
    #[serde(default)]
    pub actual: Option<ActualSection>,
    #[serde(default)]
    pub file: Option<FileSection>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ActualSection {
    /// Base URL of `actual-http-api`.
    pub url: String,
    #[serde(default)]
    pub api_key: Option<String>,
    #[serde(default)]
    pub api_key_file: Option<PathBuf>,
    /// Actual's Sync ID.
    pub budget_sync_id: String,
    #[serde(default)]
    pub budget_password: Option<String>,
    #[serde(default)]
    pub budget_password_file: Option<PathBuf>,
    #[serde(default)]
    pub chunk_size: Option<usize>,
    #[serde(default)]
    pub notes_fields: Option<Vec<String>>,
    /// never | settled | always. Defaults to `never`.
    #[serde(default)]
    pub cleared: Option<ClearedPolicy>,
    /// Re-import rows deleted in Actual. Defaults to `true`.
    #[serde(default)]
    pub reimport_deleted: Option<bool>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct FileSection {
    pub path: PathBuf,
}

impl Config {
    pub fn load(path: &Path) -> Result<Self> {
        let text =
            std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
        toml::from_str(&text).with_context(|| format!("parsing {}", path.display()))
    }

    /// Directory the config lives in; relative paths are resolved against it so
    /// `abmx --config /etc/abmx/config.toml` behaves predictably.
    pub fn base_dir(path: &Path) -> PathBuf {
        path.parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from("."))
    }

    pub fn account_map(&self) -> AccountMap {
        self.labels.accounts()
    }

    /// The labels that are enabled and therefore need a parser and an account.
    pub fn active_labels(&self) -> impl Iterator<Item = (&str, &LabelRoute)> {
        self.labels
            .iter()
            .map(|(label, route)| (label.as_str(), route))
    }

    pub fn state_path(&self, base: &Path) -> PathBuf {
        self.general
            .state
            .as_ref()
            .map(|path| paths::resolve_path(base, path))
            .unwrap_or_else(paths::default_state_db)
    }

    pub fn parser_options(&self, base: &Path) -> abmx_parser::Options {
        abmx_parser::Options {
            parsers: self
                .general
                .parsers
                .iter()
                .map(|path| paths::resolve_path(base, path))
                .collect(),
        }
    }

    pub fn gmail_token_path(&self, base: &Path) -> PathBuf {
        self.fetch
            .gmail
            .as_ref()
            .and_then(|gmail| gmail.token.as_ref())
            .map(|path| paths::resolve_path(base, path))
            .unwrap_or_else(paths::default_token_path)
    }

    pub fn gmail_client_secret(&self, base: &Path) -> Result<Option<String>> {
        let Some(gmail) = &self.fetch.gmail else {
            return Ok(None);
        };
        paths::resolve_secret(
            "ABMX_GMAIL_CLIENT_SECRET",
            gmail.client_secret.as_deref(),
            gmail
                .client_secret_file
                .as_ref()
                .map(|path| paths::resolve_path(base, path))
                .as_deref(),
        )
    }

    /// Build the configured sink. `[sink.actual]` wins over `[sink.file]` when
    /// both are present.
    pub fn build_sink(&self, base: &Path) -> Result<Box<dyn Sink>> {
        if let Some(actual) = &self.sink.actual {
            let api_key = paths::resolve_secret(
                "ABMX_ACTUAL_API_KEY",
                actual.api_key.as_deref(),
                actual
                    .api_key_file
                    .as_ref()
                    .map(|path| paths::resolve_path(base, path))
                    .as_deref(),
            )?
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "[sink.actual] needs `api_key`, `api_key_file`, or ABMX_ACTUAL_API_KEY"
                )
            })?;

            let budget_password = paths::resolve_secret(
                "ABMX_ACTUAL_BUDGET_PASSWORD",
                actual.budget_password.as_deref(),
                actual
                    .budget_password_file
                    .as_ref()
                    .map(|path| paths::resolve_path(base, path))
                    .as_deref(),
            )?;

            let notes = actual
                .notes_fields
                .clone()
                .map(|fields| NotesConfig { fields })
                .unwrap_or_default();

            let mut config =
                ActualConfig::new(actual.url.as_str(), api_key, actual.budget_sync_id.as_str());
            config.budget_password = budget_password;
            if let Some(cleared) = actual.cleared {
                config.cleared = cleared;
            }
            if let Some(reimport_deleted) = actual.reimport_deleted {
                config.reimport_deleted = reimport_deleted;
            }

            let sink = ActualSink::new(config, notes)?;
            return Ok(Box::new(sink));
        }

        if let Some(file) = &self.sink.file {
            return Ok(Box::new(FileSink::new(paths::resolve_path(
                base, &file.path,
            ))));
        }

        bail!("no sink configured: add [sink.actual] or [sink.file]")
    }

    pub fn actual_chunk_size(&self) -> usize {
        self.sink
            .actual
            .as_ref()
            .and_then(|actual| actual.chunk_size)
            .unwrap_or(50)
            .max(1)
    }

    /// Build the Gmail fetcher. Routing comes from `[labels]`, not from the
    /// parser corpus.
    pub fn build_gmail(&self, base: &Path) -> Result<GmailFetcher> {
        let Some(gmail) = &self.fetch.gmail else {
            bail!("no fetcher configured: add [fetch.gmail] or use --from-dir");
        };

        let client_secret = self.gmail_client_secret(base)?;
        let token_path = self.gmail_token_path(base);

        let mut config = GmailConfig::new(&gmail.client_id, token_path);
        config.client_secret = client_secret;
        config.max_messages = gmail.max_messages.unwrap_or(10_000);

        GmailFetcher::new(config).map_err(Into::into)
    }

    pub fn gmail_oauth_port(&self) -> u16 {
        self.fetch
            .gmail
            .as_ref()
            .and_then(|gmail| gmail.port)
            .unwrap_or(0)
    }
}

pub const INIT_TEMPLATE: &str = r#"# abmx configuration
#
# Secrets belong in the environment, not here:
#   ABMX_GMAIL_CLIENT_SECRET
#   ABMX_ACTUAL_API_KEY
#   ABMX_ACTUAL_BUDGET_PASSWORD
# ...or in files referenced by `*_file`, which is what systemd's
# LoadCredential wants.

[general]
# Explicit parser directories, highest precedence first.
# Empty means implicit discovery (~/.config/abmx/parsers, /etc/abmx/parsers).
parsers = []
# state = "~/.local/state/abmx/state.db"

[fetch.gmail]
client_id = "REPLACE.apps.googleusercontent.com"
# client_secret_file = "/run/credentials/abmx.service/gmail-secret"
# token = "~/.local/state/abmx/gmail-credentials.json"
# Hard cap. Exceeding it is an error, not a silent truncation.
max_messages = 10000

[sink.actual]
url = "http://localhost:5007"
budget_sync_id = "REPLACE"
# api_key_file = "/run/credentials/abmx.service/actual-api-key"
# budget_password_file = "/run/credentials/abmx.service/actual-budget-password"
chunk_size = 50
# never | settled | always. `never` leaves imported rows uncleared so clearing
# stays your reconciliation step.
cleared = "never"
# true (default): the ledger is a projection of the mailbox, so a rebuild
# recreates rows you deleted. false makes ledger deletions permanent.
# reimport_deleted = true

# Gmail label → parser + Actual account UUID.
# Apply exactly one `abmx:*` label per tracked message, e.g. with a Gmail filter.
# A labeled message with no entry here aborts `abmx sync`; nothing is guessed.
# Run `abmx accounts` to see what Actual calls the accounts.
[labels]
# "abmx:example-qris" = { parser = "example-qris", account = "00000000-0000-0000-0000-000000000000" }
# "abmx:grab"     = { parser = "grab",     account = "00000000-0000-0000-0000-000000000000" }

[policy]
# heuristic = "review"   # review | import | skip
# failed = "skip"        # skip | review | import
"#;
