//! Gmail as a [`txsync_core::Fetcher`].
//!
//! Two halves:
//!
//! * [`auth`] — OAuth2 with PKCE and a loopback redirect. Produces a refresh
//!   token that is stored with `0600` permissions.
//! * [`client`] — the read-only Gmail calls: `profile`, `history.list`,
//!   `messages.list`, `messages.get?format=raw`.
//!
//! Incremental sync is the reason this uses the Gmail API rather than IMAP:
//! `historyId` is a real cursor with a server-side definition of "new", so the
//! common case is one request instead of a mailbox scan.

pub mod auth;
pub mod client;
pub mod query;

use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use serde_json::json;

use txsync_core::{FetchBatch, FetchRequest, Fetcher, MessageKey, RawMessage};

pub use auth::{
    authorize, authorize_manual, revoke_refresh_token, Credentials, OAuthConfig, TokenStore,
    REVOKE_URL,
};
pub use client::{GmailClient, GmailError, Profile};
pub use query::build_query;

pub const DEFAULT_BASE_URL: &str = "https://gmail.googleapis.com/gmail/v1";
pub const DEFAULT_USER: &str = "me";

/// Everything the fetcher needs. Built from `txsync.toml` by the CLI.
#[derive(Debug, Clone)]
pub struct GmailConfig {
    /// Gmail user id; `me` means "the authenticated account".
    pub user: String,
    pub base_url: String,
    pub token_url: String,
    pub client_id: String,
    pub client_secret: Option<String>,
    pub token_path: PathBuf,
    /// Gmail search expression used for a full sync.
    pub query: String,
    /// Restrict to these labels. Empty means "whatever the query matches".
    pub labels: Vec<String>,
    /// Floor for full syncs. `0` disables the floor.
    pub lookback_days: u32,
    /// Hard cap per fetch, so one bad query cannot pull years of mail.
    pub max_messages: usize,
}

impl GmailConfig {
    pub fn new(client_id: impl Into<String>, token_path: impl Into<PathBuf>) -> Self {
        Self {
            user: DEFAULT_USER.to_owned(),
            base_url: DEFAULT_BASE_URL.to_owned(),
            token_url: auth::TOKEN_URL.to_owned(),
            client_id: client_id.into(),
            client_secret: None,
            token_path: token_path.into(),
            query: String::new(),
            labels: Vec::new(),
            lookback_days: 7,
            max_messages: txsync_core::DEFAULT_LIMIT,
        }
    }
}

/// The persisted cursor: Gmail's `historyId`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GmailCursor {
    pub history_id: String,
}

pub struct GmailFetcher {
    client: GmailClient,
    query: String,
    labels: Vec<String>,
    max_messages: usize,
}

impl GmailFetcher {
    pub fn new(config: GmailConfig) -> Result<Self, GmailError> {
        let store = TokenStore::new(config.token_path.clone());
        let credentials = store.load()?.ok_or(GmailError::NotAuthorized)?;

        let client = GmailClient::new(client::GmailClientConfig {
            user: config.user,
            base_url: config.base_url,
            token_url: config.token_url,
            client_id: config.client_id,
            client_secret: config.client_secret,
            store,
            credentials,
        });

        Ok(Self {
            client,
            query: config.query,
            labels: config.labels,
            max_messages: config.max_messages.max(1),
        })
    }

    pub fn client(&self) -> &GmailClient {
        &self.client
    }

    fn full_sync(&self, limit: usize) -> Result<(Vec<String>, Option<String>), GmailError> {
        // Read the history point *before* listing. Anything that arrives after
        // this moment is guaranteed to appear in `history.list` from here on.
        let profile = self.client.profile()?;

        let mut ids = Vec::new();
        let mut page_token = None;
        loop {
            let page =
                self.client
                    .list_messages(&self.query, &self.labels, page_token.as_deref(), 500)?;
            ids.extend(page.ids);
            page_token = page.next_page_token;
            if page_token.is_none() || ids.len() >= limit {
                break;
            }
        }

        Ok((ids, Some(profile.history_id)))
    }
}

impl Fetcher for GmailFetcher {
    fn name(&self) -> &str {
        "gmail"
    }

    fn fetch(&self, request: FetchRequest<'_>) -> txsync_core::Result<FetchBatch> {
        let limit = self.max_messages.min(request.limit).max(1);

        let cursor: Option<GmailCursor> = match request.cursor {
            Some(value) => Some(serde_json::from_value(value.clone()).map_err(|error| {
                txsync_core::Error::Fetch(format!("bad gmail cursor: {error}"))
            })?),
            None => None,
        };

        let (mut ids, mut next_history) = match &cursor {
            Some(cursor) => match self.client.history(&cursor.history_id, None) {
                Ok(first) => {
                    let mut ids = first.message_ids;
                    let mut latest = first.history_id;
                    let mut token = first.next_page_token;
                    // `history.list` pages at 500 entries. Walking the pages
                    // matters after a long outage, which is exactly when a
                    // partial read would silently drop mail.
                    while let Some(next) = token {
                        let page = self.client.history(&cursor.history_id, Some(&next))?;
                        ids.extend(page.message_ids);
                        latest = page.history_id;
                        token = page.next_page_token;
                    }
                    (ids, Some(latest))
                }
                // The server only keeps a bounded history window. Falling back
                // to a full sync is correct; the state database filters what has
                // already been seen.
                Err(GmailError::HistoryExpired) => self.full_sync(limit)?,
                Err(error) => return Err(error.into()),
            },
            None => self.full_sync(limit)?,
        };

        let truncated = ids.len() > limit;
        if truncated {
            ids.truncate(limit);
            // Do not advance the cursor over rows we did not fetch: the next run
            // replays and the state database skips what is already applied.
            next_history = cursor.as_ref().map(|cursor| cursor.history_id.clone());
        }

        let mut seen = std::collections::HashSet::new();
        ids.retain(|id| seen.insert(id.clone()));

        let mut messages = Vec::with_capacity(ids.len());
        let mut skipped = 0usize;
        for id in &ids {
            match self.client.raw_message(id) {
                Ok(bytes) => messages.push(RawMessage::new(
                    MessageKey::new(format!("gmail:{id}")),
                    format!("gmail:{id}"),
                    bytes,
                )),
                // Listed but gone by the time we asked: deleted or moved out of
                // the query. Not an error.
                Err(GmailError::Api { status: 404, .. }) => skipped += 1,
                Err(error) => return Err(error.into()),
            }
        }

        let next_cursor = next_history.map(|history_id| json!({ "history_id": history_id }));

        Ok(FetchBatch {
            messages,
            next_cursor,
            skipped,
        })
    }
}

impl From<GmailError> for txsync_core::Error {
    fn from(error: GmailError) -> Self {
        txsync_core::Error::Fetch(error.to_string())
    }
}
