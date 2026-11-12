use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use serde::Deserialize;
use txsync_actual::{ActualConfig, ActualSink, ClearedPolicy, NotesConfig};
use txsync_core::{AccountMap, PolicyConfig, Sink};
use txsync_file::FileSink;
use txsync_gmail::{build_query, GmailConfig, GmailFetcher};

use crate::paths;

#[derive(Debug, Clone, Default, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub general: General,
    #[serde(default)]
    pub fetch: FetchSection,
    #[serde(default)]
    pub sink: SinkSection,
    /// `txp` account key → sink account id. Never inferred.
    #[serde(default)]
    pub accounts: BTreeMap<String, String>,
    #[serde(default)]
    pub policy: PolicyConfig,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct General {
    /// Explicit parser directories, in precedence order. Empty means `txp`'s
    /// implicit discovery (`./parsers`, XDG, `/etc`).
    #[serde(default)]
    pub parsers: Vec<PathBuf>,
    #[serde(default)]
    pub router: Option<PathBuf>,
    /// SQLite state file. Defaults to `$XDG_STATE_HOME/txsync/state.db`.
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
    /// Appended to the query built from the parser corpus.
    #[serde(default)]
    pub query_extra: Option<String>,
    #[serde(default)]
    pub lookback_days: Option<u32>,
    #[serde(default)]
    pub labels: Vec<String>,
    #[serde(default)]
    pub max_messages: Option<usize>,
    /// Loopback port for `txsync auth gmail`; `0` picks one.
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
    /// `txsync --config /etc/txsync/config.toml` behaves predictably.
    pub fn base_dir(path: &Path) -> PathBuf {
        path.parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from("."))
    }

    pub fn account_map(&self) -> AccountMap {
        AccountMap::from(self.accounts.clone())
    }

    pub fn state_path(&self, base: &Path) -> PathBuf {
        self.general
            .state
            .as_ref()
            .map(|path| paths::resolve_path(base, path))
            .unwrap_or_else(paths::default_state_db)
    }

    pub fn txp_options(&self, base: &Path) -> txp::Options {
        txp::Options {
            parsers: self
                .general
                .parsers
                .iter()
                .map(|path| paths::resolve_path(base, path))
                .collect(),
            router: self
                .general
                .router
                .as_ref()
                .map(|path| paths::resolve_path(base, path)),
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
            "TXSYNC_GMAIL_CLIENT_SECRET",
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
                "TXSYNC_ACTUAL_API_KEY",
                actual.api_key.as_deref(),
                actual
                    .api_key_file
                    .as_ref()
                    .map(|path| paths::resolve_path(base, path))
                    .as_deref(),
            )?
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "[sink.actual] needs `api_key`, `api_key_file`, or TXSYNC_ACTUAL_API_KEY"
                )
            })?;

            let budget_password = paths::resolve_secret(
                "TXSYNC_ACTUAL_BUDGET_PASSWORD",
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

    /// Build the Gmail fetcher. `senders` comes from the loaded `txp` corpus, so
    /// there is exactly one list of bank addresses in the system.
    pub fn build_gmail(&self, base: &Path, senders: &[String]) -> Result<GmailFetcher> {
        let Some(gmail) = &self.fetch.gmail else {
            bail!("no fetcher configured: add [fetch.gmail] or use --from-dir");
        };

        let client_secret = self.gmail_client_secret(base)?;
        let token_path = self.gmail_token_path(base);

        let lookback_days = gmail.lookback_days.unwrap_or(7);
        let query = build_query(senders, lookback_days, gmail.query_extra.as_deref());

        let mut config = GmailConfig::new(&gmail.client_id, token_path);
        config.client_secret = client_secret;
        config.query = query;
        config.labels = gmail.labels.clone();
        config.lookback_days = lookback_days;
        config.max_messages = gmail.max_messages.unwrap_or(txsync_core::DEFAULT_LIMIT);

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

pub const INIT_TEMPLATE: &str = r#"# txsync configuration
#
# Secrets belong in the environment, not here:
#   TXSYNC_GMAIL_CLIENT_SECRET
#   TXSYNC_ACTUAL_API_KEY
#   TXSYNC_ACTUAL_BUDGET_PASSWORD
# ...or in files referenced by `*_file`, which is what systemd's
# LoadCredential wants.

[general]
# Explicit parser directories for txp, highest precedence first.
# Empty means txp's implicit discovery (./parsers, XDG, /etc).
parsers = []
# state = "~/.local/state/txsync/state.db"

[fetch.gmail]
client_id = "REPLACE.apps.googleusercontent.com"
# client_secret_file = "/run/credentials/txsync.service/gmail-secret"
# token = "~/.local/state/txsync/gmail-credentials.json"
lookback_days = 7
# Appended to the query txsync builds from the txp parser corpus.
# query_extra = "-label:txsync-ignored"
# labels = ["INBOX"]
max_messages = 200

[sink.actual]
url = "http://localhost:5007"
budget_sync_id = "REPLACE"
# api_key_file = "/run/credentials/txsync.service/actual-api-key"
# budget_password_file = "/run/credentials/txsync.service/actual-budget-password"
chunk_size = 50
# never | settled | always. `never` leaves imported rows uncleared so clearing
# stays your reconciliation step.
cleared = "never"

# Map txp's parser-declared account keys to Actual account UUIDs.
# Run `txsync accounts` to see what Actual calls them.
[accounts]
# "example-account" = "00000000-0000-0000-0000-000000000000"
# "wallet-account" = "00000000-0000-0000-0000-000000000000"

[policy]
# heuristic = "review"   # review | import | skip
# failed = "skip"        # skip | review | import
"#;
