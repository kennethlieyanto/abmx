use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use abmx_actual::{ActualConfig, ActualSink, ClearedPolicy, NotesConfig};
use abmx_core::{AccountMap, Sink};
use abmx_file::FileSink;
use abmx_gmail::{GmailConfig, GmailFetcher};
use anyhow::{bail, Context, Result};
use serde::Deserialize;

use crate::credentials::{self, GoogleClient};
use crate::paths;

#[derive(Debug, Clone, Default, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub general: General,
    #[serde(default)]
    pub fetch: FetchSection,
    #[serde(default)]
    pub sink: SinkSection,
    #[serde(default)]
    pub labels: LabelsSection,
}

/// How a `receipt/<parser>` Gmail label becomes an import.
#[derive(Debug, Clone, Deserialize)]
pub struct LabelsSection {
    /// Literal (case-insensitive) label prefix. The remainder must be a parser id.
    pub prefix: String,
    /// Ignore mail received before this date.
    #[serde(default, deserialize_with = "deserialize_since")]
    pub since: Option<chrono::NaiveDate>,
    /// Parser id → sink account id. This is the only definition of "managed":
    /// labels without an entry are not fetched.
    #[serde(default)]
    pub accounts: BTreeMap<String, String>,
    /// Parser ids the user explicitly declined to import in `abmx setup`.
    #[serde(default)]
    pub ignore: Vec<String>,
}

impl LabelsSection {
    /// The parser id encoded in a label, when the remainder is valid.
    pub fn parser_id(&self, label: &str) -> Option<String> {
        let prefix = self.prefix.to_ascii_lowercase();
        let lower = label.to_ascii_lowercase();
        let rest = lower.strip_prefix(&prefix)?;
        abmx_parse::is_parser_id(rest).then(|| rest.to_owned())
    }

    /// Whether the user declined this parser id. Gmail label matching is
    /// case-insensitive, so the bookkeeping is too.
    pub fn is_ignored(&self, parser_id: &str) -> bool {
        self.ignore
            .iter()
            .any(|ignored| ignored.eq_ignore_ascii_case(parser_id))
    }
}

/// Accept both a TOML date (`since = 2026-11-10`) and a string.
fn deserialize_since<'de, D>(
    deserializer: D,
) -> std::result::Result<Option<chrono::NaiveDate>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde::de::Error;

    let value = Option::<toml::Value>::deserialize(deserializer)?;
    match value {
        None => Ok(None),
        Some(toml::Value::String(text)) => chrono::NaiveDate::parse_from_str(&text, "%Y-%m-%d")
            .map(Some)
            .map_err(D::Error::custom),
        Some(toml::Value::Datetime(datetime)) => {
            let date = datetime
                .date
                .ok_or_else(|| D::Error::custom("`since` must include a date"))?;
            chrono::NaiveDate::from_ymd_opt(date.year as i32, date.month as u32, date.day as u32)
                .map(Some)
                .ok_or_else(|| D::Error::custom("invalid `since` date"))
        }
        Some(other) => Err(D::Error::custom(format!(
            "`since` must be a date like 2026-11-10, got {other}"
        ))),
    }
}

impl Default for LabelsSection {
    fn default() -> Self {
        Self {
            prefix: "receipt/".to_owned(),
            since: None,
            accounts: BTreeMap::new(),
            ignore: Vec::new(),
        }
    }
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
    /// Optional when `credentials_file` points at a downloaded OAuth client.
    #[serde(default)]
    pub client_id: Option<String>,
    #[serde(default)]
    pub client_secret: Option<String>,
    #[serde(default)]
    pub client_secret_file: Option<PathBuf>,
    /// Google's downloaded OAuth client JSON (`installed` or `web` shape).
    #[serde(default)]
    pub credentials_file: Option<PathBuf>,
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

/// The effective OAuth client for Gmail after config and environment overrides.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GmailCredentials {
    pub client_id: String,
    pub client_secret: Option<String>,
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
    /// Base URL of the Actual server itself, e.g. `http://localhost:5006`.
    /// Actual has no HTTP API: `abmx` drives the official Node client through a
    /// bundled helper, which talks to this URL.
    #[serde(default)]
    pub server_url: Option<String>,
    /// Deprecated: the `actual-http-api` wrapper URL. Rejected with migration
    /// guidance instead of being silently ignored.
    #[serde(default)]
    pub url: Option<String>,
    /// A budget-scoped Actual API key (Settings → General → Security → API keys).
    #[serde(default)]
    pub api_key: Option<String>,
    #[serde(default)]
    pub api_key_file: Option<PathBuf>,
    /// The Actual server password, for servers too old to have API keys.
    #[serde(default)]
    pub server_password: Option<String>,
    #[serde(default)]
    pub server_password_file: Option<PathBuf>,
    /// Actual's Sync ID.
    pub budget_sync_id: String,
    /// The budget's end-to-end encryption password, if it has one.
    #[serde(default)]
    pub budget_password: Option<String>,
    #[serde(default)]
    pub budget_password_file: Option<PathBuf>,
    /// Where `@actual-app/api` keeps its local copy of the budget. Defaults to
    /// `$XDG_CACHE_HOME/abmx/actual`; keep it persistent or every run
    /// re-downloads the budget.
    #[serde(default)]
    pub data_dir: Option<PathBuf>,
    /// The Node helper script. Defaults to the copy installed next to `abmx`.
    #[serde(default)]
    pub helper: Option<PathBuf>,
    /// Node executable. Defaults to `node` on `PATH`.
    #[serde(default)]
    pub node: Option<PathBuf>,
    /// How long a single helper call may take. Defaults to 120 seconds.
    #[serde(default)]
    pub timeout_secs: Option<u64>,
    #[serde(default)]
    pub chunk_size: Option<usize>,
    /// Parser fields copied into Actual's notes, in order. Defaults to
    /// `["description"]`; an empty list disables notes.
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
        AccountMap::from(self.labels.accounts.clone())
    }

    /// Validate the label contract before a mailbox is touched.
    pub fn validate_labels(&self) -> Result<()> {
        if self.labels.prefix.trim().is_empty() {
            bail!("[labels].prefix must not be empty");
        }
        let mut problems = Vec::new();
        for (parser, account) in &self.labels.accounts {
            if !abmx_parse::is_parser_id(parser) {
                problems.push(format!(
                    "`{parser}` is not a parser id (expected `bank.type`)"
                ));
            }
            if account.trim().is_empty() {
                problems.push(format!("`{parser}` has an empty account id"));
            }
        }
        for parser in &self.labels.ignore {
            if !abmx_parse::is_parser_id(parser) {
                problems.push(format!("`{parser}` in [labels].ignore is not a parser id"));
            }
            if self
                .labels
                .accounts
                .keys()
                .any(|mapped| mapped.eq_ignore_ascii_case(parser))
            {
                problems.push(format!(
                    "`{parser}` is both mapped in [labels.accounts] and ignored"
                ));
            }
        }
        if !problems.is_empty() {
            bail!(
                "[labels] entries must be consistent parser ids:\n  {}",
                problems.join("\n  ")
            );
        }
        Ok(())
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

    /// Resolve the effective Gmail OAuth client.
    ///
    /// Explicit config wins over `credentials_file`, and the secret follows the
    /// usual env → literal → file precedence so systemd can override everything.
    pub fn gmail_credentials(&self, base: &Path) -> Result<GmailCredentials> {
        let Some(gmail) = &self.fetch.gmail else {
            bail!("no [fetch.gmail] section in the config");
        };

        let from_file: Option<GoogleClient> = match &gmail.credentials_file {
            Some(path) => Some(credentials::load_google_client(&paths::resolve_path(
                base, path,
            ))?),
            None => None,
        };

        let client_id = gmail
            .client_id
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_owned)
            .or_else(|| from_file.as_ref().map(|client| client.client_id.clone()))
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "[fetch.gmail] needs `credentials_file` (Google's OAuth client JSON) \
                     or `client_id`"
                )
            })?;

        let client_secret = paths::resolve_secret(
            "ABMX_GMAIL_CLIENT_SECRET",
            gmail.client_secret.as_deref(),
            gmail
                .client_secret_file
                .as_ref()
                .map(|path| paths::resolve_path(base, path))
                .as_deref(),
        )?
        .or_else(|| from_file.and_then(|client| client.client_secret));

        Ok(GmailCredentials {
            client_id,
            client_secret,
        })
    }

    /// Resolve `[sink.actual]` into the sink's own config.
    ///
    /// Secrets follow the usual env → literal → file precedence. The helper and
    /// Node paths are resolved early so `abmx status` can report a broken
    /// install instead of failing halfway through a sync.
    pub fn actual_sink_config(&self, base: &Path) -> Result<Option<ActualConfig>> {
        let Some(actual) = &self.sink.actual else {
            return Ok(None);
        };

        let has_server_url = actual
            .server_url
            .as_deref()
            .is_some_and(|value| !value.trim().is_empty());
        if !has_server_url
            && actual
                .url
                .as_deref()
                .is_some_and(|url| !url.trim().is_empty())
        {
            bail!(
                "[sink.actual].url pointed at actual-http-api, which abmx no longer uses.\n\
                 Replace it with your Actual server:\n\n  \
                 server_url = \"http://localhost:5006\"\n\n\
                 The API key now comes from Actual itself \
                 (Settings → General → Security → API keys). Run `abmx setup` to rewrite \
                 the section."
            );
        }

        let server_url = non_empty_env("ABMX_ACTUAL_SERVER_URL")
            .or_else(|| {
                actual
                    .server_url
                    .as_deref()
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(str::to_owned)
            })
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "[sink.actual] needs `server_url`, the Actual server itself \
                     (for example http://localhost:5006)"
                )
            })?;

        let api_key = paths::resolve_secret(
            "ABMX_ACTUAL_API_KEY",
            actual.api_key.as_deref(),
            actual
                .api_key_file
                .as_ref()
                .map(|path| paths::resolve_path(base, path))
                .as_deref(),
        )?;
        let server_password = paths::resolve_secret(
            "ABMX_ACTUAL_SERVER_PASSWORD",
            actual.server_password.as_deref(),
            actual
                .server_password_file
                .as_ref()
                .map(|path| paths::resolve_path(base, path))
                .as_deref(),
        )?;

        let mut config = match (api_key, server_password) {
            (Some(key), _) => ActualConfig::new(server_url, key, actual.budget_sync_id.as_str()),
            (None, Some(password)) => {
                ActualConfig::with_password(server_url, password, actual.budget_sync_id.as_str())
            }
            (None, None) => bail!(
                "[sink.actual] needs actual credentials: an Actual API key \
                 (`api_key`, `api_key_file`, or ABMX_ACTUAL_API_KEY), or the server \
                 password (`server_password`, `server_password_file`, or \
                 ABMX_ACTUAL_SERVER_PASSWORD)"
            ),
        };

        config.data_dir = non_empty_env("ABMX_ACTUAL_DATA_DIR")
            .map(PathBuf::from)
            .or_else(|| {
                actual
                    .data_dir
                    .as_ref()
                    .map(|path| paths::resolve_path(base, path))
            })
            .unwrap_or_else(paths::default_actual_data_dir);

        config.helper = non_empty_env("ABMX_ACTUAL_HELPER")
            .map(PathBuf::from)
            .or_else(|| {
                actual
                    .helper
                    .as_ref()
                    .map(|path| paths::resolve_path(base, path))
            })
            .or_else(paths::default_actual_helper)
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "the Actual helper script was not found. Install abmx with its \
                     helper (the Nix package and container include it), set \
                     `[sink.actual].helper`, or export ABMX_ACTUAL_HELPER"
                )
            })?;

        config.node = non_empty_env("ABMX_NODE")
            .map(PathBuf::from)
            .or_else(|| actual.node.clone())
            .unwrap_or_else(|| PathBuf::from("node"));

        config.timeout = std::time::Duration::from_secs(actual.timeout_secs.unwrap_or(120).max(1));

        config.budget_password = paths::resolve_secret(
            "ABMX_ACTUAL_BUDGET_PASSWORD",
            actual.budget_password.as_deref(),
            actual
                .budget_password_file
                .as_ref()
                .map(|path| paths::resolve_path(base, path))
                .as_deref(),
        )?;

        if let Some(cleared) = actual.cleared {
            config.cleared = cleared;
        }
        if let Some(reimport_deleted) = actual.reimport_deleted {
            config.reimport_deleted = reimport_deleted;
        }

        Ok(Some(config))
    }

    /// Build the configured sink. `[sink.actual]` wins over `[sink.file]` when
    /// both are present.
    pub fn build_sink(&self, base: &Path) -> Result<Box<dyn Sink>> {
        if let Some(config) = self.actual_sink_config(base)? {
            let notes = self
                .sink
                .actual
                .as_ref()
                .and_then(|actual| actual.notes_fields.clone())
                .map(|fields| NotesConfig { fields })
                .unwrap_or_default();

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

        let credentials = self.gmail_credentials(base)?;
        let token_path = self.gmail_token_path(base);

        let mut config = GmailConfig::new(credentials.client_id, token_path);
        config.client_secret = credentials.client_secret;
        config.max_messages = gmail.max_messages.unwrap_or(10_000);
        config.prefix = self.labels.prefix.clone();
        config.since = self.labels.since;

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
# `abmx setup` fills this in interactively. The comments below are for manual
# edits and for service managers.
#
# Secrets belong in the environment or in files referenced by `*_file`, not in
# this file:
#   ABMX_GMAIL_CLIENT_SECRET
#   ABMX_ACTUAL_API_KEY
#   ABMX_ACTUAL_SERVER_PASSWORD
#   ABMX_ACTUAL_BUDGET_PASSWORD

[general]
# Explicit parser directories, highest precedence first.
# Empty means implicit discovery (~/.config/abmx/parsers, /etc/abmx/parsers).
parsers = []
# state = "~/.local/state/abmx/state.db"

[fetch.gmail]
# Google's downloaded OAuth client JSON (`installed` or `web` shape).
# `abmx setup` copies it here and sets this field.
# credentials_file = "gmail-client.json"
#
# Or set the client id here and keep the secret out of the file:
# client_id = "1234567890-abc.apps.googleusercontent.com"
# client_secret_file = "/run/credentials/abmx.service/gmail-secret"
# token = "~/.local/state/abmx/gmail-credentials.json"
# Hard cap. Exceeding it is an error, not a silent truncation.
max_messages = 10000

# Actual Budget has no HTTP API. abmx drives the official Node client
# (`@actual-app/api`) through a bundled helper, which talks to your Actual
# server directly:
#   abmx  ──►  actual-helper (Node)  ──►  @actual-app/api  ──►  actual-server
#
# `server_url` is the Actual server itself, not a wrapper. `api_key` is a
# budget-scoped key from Actual: Settings → General → Security → API keys.
# Servers too old to have API keys can use `server_password` instead.
#
# [sink.actual]
# server_url = "http://localhost:5006"
# budget_sync_id = "00000000-0000-0000-0000-000000000000"
# api_key_file = "/run/credentials/abmx.service/actual-api-key"
# server_password_file = "/run/credentials/abmx.service/actual-server-password"
# budget_password_file = "/run/credentials/abmx.service/actual-budget-password"
# The local copy of the budget. Keep it persistent; otherwise every run
# downloads the whole budget again.
# data_dir = "~/.cache/abmx/actual"
# The helper and Node binary are found automatically; override them when
# running from a custom prefix.
# helper = "/usr/share/abmx/actual-helper/helper.js"
# node = "node"
# timeout_secs = 120
# chunk_size = 50
# never | settled | always. `never` leaves imported rows uncleared so clearing
# stays your reconciliation step.
# cleared = "never"
# true (default): the ledger is a projection of the mailbox, so a rebuild
# recreates rows you deleted. false makes ledger deletions permanent.
# reimport_deleted = true
# Notes come from parser fields. The default is just `description`; set an
# empty list to disable notes entirely.
# notes_fields = ["description"]
#
# A file sink is useful for tests:
# [sink.file]
# path = "out/ledger.jsonl"

# Gmail labels are the routing contract. The user (via gmailctl) applies
# one `receipt/<parser-id>` label per tracked message; the remainder after
# the prefix is the parser id. Only parser ids in [labels.accounts] are fetched.
# Plain `receipt` and other labels are ignored.
# Run `abmx accounts` to see what Actual calls the accounts.
[labels]
prefix = "receipt/"
# Recommended. Without it, sync scans every labelled message in the mailbox,
# which is slow and may include templates the parser has not seen.
# since = 2026-11-01
# Labels you declined in `abmx setup`; they are not fetched and are not asked
# about again.
# ignore = ["merchant.promo"]

[labels.accounts]
# "example.card_debit"       = "00000000-0000-0000-0000-000000000000"
# "example.journal_transfer" = "00000000-0000-0000-0000-000000000000"
"#;

fn non_empty_env(name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
}
