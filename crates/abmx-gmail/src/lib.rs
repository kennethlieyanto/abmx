//! Gmail as a [`abmx_core::Fetcher`].
//!
//! Routing is label-driven: the user applies one `receipt/<id>` label to a message,
//! and the configured `[labels]` table chooses the parser and ledger account.
//! The fetcher never guesses from sender or subject.

pub mod auth;
pub mod client;
pub mod sample;

use std::collections::{BTreeMap, HashSet};
use std::path::PathBuf;

use abmx_core::{FetchBatch, FetchRequest, Fetcher, MessageKey, RawMessage};

pub use auth::{
    authorize, authorize_manual, revoke_refresh_token, Credentials, OAuthConfig, TokenStore,
    REVOKE_URL,
};
pub use client::{GmailClient, GmailError, GmailLabel, MessageMeta, Profile};
pub use sample::SampleMessage;

pub const DEFAULT_BASE_URL: &str = "https://gmail.googleapis.com/gmail/v1";
pub const DEFAULT_USER: &str = "me";

/// Everything the fetcher needs. Built from `abmx.toml` by the CLI.
#[derive(Debug, Clone)]
pub struct GmailConfig {
    /// Gmail user id; `me` means "the authenticated account".
    pub user: String,
    pub base_url: String,
    pub token_url: String,
    pub client_id: String,
    pub client_secret: Option<String>,
    pub token_path: PathBuf,
    /// Labels under this prefix belong to abmx; the remainder is the parser id.
    pub prefix: String,
    /// Ignore mail received before this date.
    pub since: Option<chrono::NaiveDate>,
    /// Hard cap. Exceeding it is an error, never a silent truncation.
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
            prefix: "receipt/".to_owned(),
            since: None,
            max_messages: 10_000,
        }
    }
}

pub struct GmailFetcher {
    client: GmailClient,
    prefix: String,
    since: Option<chrono::NaiveDate>,
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
            prefix: config.prefix,
            since: config.since,
            max_messages: config.max_messages.max(1),
        })
    }

    pub fn client(&self) -> &GmailClient {
        &self.client
    }

    /// Every Gmail label.
    pub fn labels(&self) -> Result<Vec<GmailLabel>, GmailError> {
        self.client.list_labels()
    }

    /// Every label under the configured prefix, sorted by name.
    ///
    /// The prefix is matched case-insensitively: Gmail preserves the case the
    /// user typed, and `Receipt/` vs `receipt/` must not split the corpus.
    pub fn routed_labels(&self) -> Result<Vec<GmailLabel>, GmailError> {
        let prefix = self.prefix.to_ascii_lowercase();
        let mut labels: Vec<GmailLabel> = self
            .client
            .list_labels()?
            .into_iter()
            .filter(|label| label.name.to_ascii_lowercase().starts_with(&prefix))
            .collect();
        labels.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(labels)
    }

    /// Every message id carrying a label id, fully paginated.
    pub fn message_ids(&self, label_id: &str) -> Result<Vec<String>, GmailError> {
        let mut query = "-in:spam -in:trash".to_owned();
        if let Some(since) = self.since {
            // Gmail's `after:` is exclusive; subtract a day so `since` includes
            // the day the user typed.
            let after = since - chrono::Duration::days(1);
            query.push_str(&format!(" after:{}", after.format("%Y/%m/%d")));
        }

        let mut ids = Vec::new();
        let mut page_token = None;
        loop {
            let page = self.client.list_messages(
                &query,
                &[label_id.to_owned()],
                page_token.as_deref(),
                500,
            )?;
            ids.extend(page.ids);
            page_token = page.next_page_token;
            if page_token.is_none() {
                break;
            }
        }
        Ok(ids)
    }

    /// Full RFC 5322 bytes.
    pub fn raw(&self, id: &str) -> Result<Vec<u8>, GmailError> {
        self.client.raw_message(id)
    }

    /// The Gmail label id for a name. The name is the contract; the id is
    /// opaque and can change if the label is recreated.
    pub fn label_id(&self, name: &str) -> Result<String, GmailError> {
        self.client
            .list_labels()?
            .into_iter()
            .find(|label| label.name == name)
            .map(|label| label.id)
            .ok_or_else(|| GmailError::Malformed(format!("label `{name}` does not exist")))
    }

    /// Every message id under a label name, fully paginated.
    pub fn label_message_ids(&self, name: &str) -> Result<Vec<String>, GmailError> {
        let id = self.label_id(name)?;
        self.message_ids(&id)
    }

    /// The newest `limit` messages under a label name, for `abmx samples`.
    ///
    /// Sampling is authoring evidence, not a sync. It deliberately ignores
    /// `[labels].since`: a quiet import window must not starve a parser of
    /// samples. The label search itself is bounded by `limit`, so this does not
    /// paginate the whole label first.
    pub fn sample_label(
        &self,
        label_name: &str,
        limit: usize,
    ) -> Result<Vec<SampleMessage>, GmailError> {
        self.client.sample_label(label_name, limit)
    }
}

impl Fetcher for GmailFetcher {
    fn name(&self) -> &str {
        "gmail"
    }

    fn fetch(&self, request: FetchRequest<'_>) -> abmx_core::Result<FetchBatch> {
        if request.labels.is_empty() {
            return Ok(FetchBatch::default());
        }

        let labels = self.client.list_labels()?;
        let by_name: BTreeMap<&str, &str> = labels
            .iter()
            .map(|label| (label.name.as_str(), label.id.as_str()))
            .collect();

        // Keep newest-first order: Gmail lists messages newest first, and a
        // `--limit` should take the newest unseen messages, not an arbitrary id
        // order. `tagged_by_id` detects a message carrying two labels.
        let mut tagged: Vec<(String, String)> = Vec::new();
        let mut tagged_by_id: BTreeMap<String, String> = BTreeMap::new();
        let mut already_seen = 0usize;
        let mut counted_seen: HashSet<String> = HashSet::new();

        'labels: for name in request.labels {
            let Some(id) = by_name.get(name.as_str()) else {
                return Err(abmx_core::Error::Fetch(format!(
                    "gmail label `{name}` does not exist"
                )));
            };
            for message_id in self.message_ids(id)? {
                let key = format!("gmail:{message_id}");
                if request.skip.is_some_and(|skip| skip.contains(&key)) {
                    if counted_seen.insert(message_id) {
                        already_seen += 1;
                    }
                    continue;
                }

                if let Some(existing) = tagged_by_id.get(&message_id) {
                    if existing != name {
                        return Err(abmx_core::Error::Fetch(format!(
                            "gmail message {message_id} carries two abmx labels (`{existing}` and `{name}`)"
                        )));
                    }
                    continue;
                }

                tagged_by_id.insert(message_id.clone(), name.clone());
                tagged.push((message_id, name.clone()));

                if let Some(limit) = request.limit {
                    if tagged.len() >= limit {
                        break 'labels;
                    }
                }
            }
        }

        if tagged.len() > self.max_messages {
            return Err(abmx_core::Error::Fetch(format!(
                "{} labeled messages exceed max_messages={}; raise it or narrow [labels]",
                tagged.len(),
                self.max_messages
            )));
        }

        let mut messages = Vec::with_capacity(tagged.len());
        let mut skipped = 0usize;
        for (id, label) in tagged {
            match self.client.raw_message(&id) {
                Ok(bytes) => messages.push(RawMessage::new(
                    MessageKey::new(format!("gmail:{id}")),
                    label,
                    format!("gmail:{id}"),
                    bytes,
                )),
                // Listed but gone by the time we asked: deleted or moved.
                Err(GmailError::Api { status: 404, .. }) => skipped += 1,
                Err(error) => return Err(error.into()),
            }
        }

        Ok(FetchBatch {
            messages,
            skipped,
            already_seen,
        })
    }
}

impl From<GmailError> for abmx_core::Error {
    fn from(error: GmailError) -> Self {
        abmx_core::Error::Fetch(error.to_string())
    }
}
