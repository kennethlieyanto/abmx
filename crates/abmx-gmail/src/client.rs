//! Read-only Gmail API calls.
//!
//! Four endpoints: `users.labels.list` to discover `receipt/*` labels,
//! `messages.list` filtered by label id, `messages.get?format=raw` to hand RFC
//! 5322 bytes to `abmx-parser`, and `profile` for diagnostics.

use std::sync::Mutex;
use std::time::{Duration, Instant};

use base64::Engine;
use serde::Deserialize;
use serde_json::Value;
use url::Url;

use crate::auth::{self, Credentials, TokenStore};

#[derive(Debug, thiserror::Error)]
pub enum GmailError {
    #[error("gmail: not authorized; run `abmx auth gmail`")]
    NotAuthorized,

    #[error("gmail: {0}")]
    Auth(String),

    #[error("gmail api {status}: {body}")]
    Api { status: u16, body: String },

    #[error("gmail: {0}")]
    Http(String),

    #[error("gmail: unexpected response: {0}")]
    Malformed(String),
}

#[derive(Debug, Clone)]
pub struct GmailClientConfig {
    pub user: String,
    pub base_url: String,
    pub token_url: String,
    pub client_id: String,
    pub client_secret: Option<String>,
    pub store: TokenStore,
    pub credentials: Credentials,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Profile {
    #[serde(rename = "emailAddress", default)]
    pub email_address: String,
    #[serde(rename = "historyId", default)]
    pub history_id: String,
    #[serde(rename = "messagesTotal", default)]
    pub messages_total: u64,
}

#[derive(Debug, Clone, Default)]
pub struct MessageList {
    pub ids: Vec<String>,
    pub next_page_token: Option<String>,
}

/// One Gmail label, as returned by `users.labels.list`.
#[derive(Debug, Clone, Default)]
pub struct GmailLabel {
    pub id: String,
    pub name: String,
}

/// The headers a scan needs, without downloading the body.
#[derive(Debug, Clone, Default)]
pub struct MessageMeta {
    pub id: String,
    pub from: String,
    pub subject: String,
    pub date: String,
}

#[derive(Debug, Default)]
struct Tokens {
    access_token: Option<String>,
    expires_at: Option<Instant>,
}

pub struct GmailClient {
    http: reqwest::blocking::Client,
    user: String,
    base_url: String,
    token_url: String,
    client_id: String,
    client_secret: Option<String>,
    credentials: Credentials,
    tokens: Mutex<Tokens>,
}

impl GmailClient {
    pub fn new(config: GmailClientConfig) -> Self {
        let http = reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(60))
            .build()
            .unwrap_or_else(|_| reqwest::blocking::Client::new());

        Self {
            http,
            user: config.user,
            base_url: config.base_url.trim_end_matches('/').to_owned(),
            token_url: config.token_url,
            client_id: config.client_id,
            client_secret: config.client_secret,
            credentials: config.credentials,
            tokens: Mutex::new(Tokens::default()),
        }
    }

    /// The account this client is authenticated as.
    pub fn email(&self) -> Result<String, GmailError> {
        Ok(self.profile()?.email_address)
    }

    pub fn profile(&self) -> Result<Profile, GmailError> {
        let url = format!("{}/users/{}/profile", self.base_url, self.user);
        let value = self.get_json(&url)?;
        serde_json::from_value(value)
            .map_err(|error| GmailError::Malformed(format!("profile: {error}")))
    }

    pub fn list_messages(
        &self,
        query: &str,
        labels: &[String],
        page_token: Option<&str>,
        max_results: u32,
    ) -> Result<MessageList, GmailError> {
        let mut url = Url::parse(&format!("{}/users/{}/messages", self.base_url, self.user))
            .map_err(|error| GmailError::Malformed(error.to_string()))?;
        {
            let mut pairs = url.query_pairs_mut();
            if !query.is_empty() {
                pairs.append_pair("q", query);
            }
            for label in labels {
                pairs.append_pair("labelIds", label);
            }
            if let Some(token) = page_token {
                pairs.append_pair("pageToken", token);
            }
            pairs.append_pair("maxResults", &max_results.to_string());
        }

        let value = self.get_json(url.as_str())?;
        let ids = value
            .get("messages")
            .and_then(Value::as_array)
            .map(|messages| {
                messages
                    .iter()
                    .filter_map(|message| message.get("id").and_then(id_string))
                    .collect()
            })
            .unwrap_or_default();

        Ok(MessageList {
            ids,
            next_page_token: value
                .get("nextPageToken")
                .and_then(Value::as_str)
                .map(str::to_owned),
        })
    }

    /// `users.labels.list`. The caller filters by the `abmx:` prefix.
    pub fn list_labels(&self) -> Result<Vec<GmailLabel>, GmailError> {
        let url = format!("{}/users/{}/labels", self.base_url, self.user);
        let value = self.get_json(&url)?;
        let labels = value
            .get("labels")
            .and_then(Value::as_array)
            .map(|labels| {
                labels
                    .iter()
                    .filter_map(|label| {
                        let id = label.get("id").and_then(id_string)?;
                        let name = label.get("name").and_then(Value::as_str)?.to_owned();
                        Some(GmailLabel { id, name })
                    })
                    .collect()
            })
            .unwrap_or_default();
        Ok(labels)
    }

    /// Raw RFC 5322 bytes, exactly as `abmx-parser` wants them.
    pub fn raw_message(&self, id: &str) -> Result<Vec<u8>, GmailError> {
        let url = format!(
            "{}/users/{}/messages/{}?format=raw",
            self.base_url, self.user, id
        );
        let value = self.get_json(&url)?;
        let raw = value
            .get("raw")
            .and_then(Value::as_str)
            .ok_or_else(|| GmailError::Malformed(format!("message {id} has no raw body")))?;

        decode_base64url(raw)
            .map_err(|error| GmailError::Malformed(format!("message {id}: {error}")))
    }

    /// Headers only: `format=metadata` is much cheaper than `format=raw`, so a
    /// scan can classify a whole mailbox before fetching any bodies.
    pub fn metadata_message(&self, id: &str) -> Result<MessageMeta, GmailError> {
        let url = format!(
            "{}/users/{}/messages/{}?format=metadata&metadataHeaders=From&metadataHeaders=Subject&metadataHeaders=Date",
            self.base_url, self.user, id
        );
        let value = self.get_json(&url)?;

        let mut meta = MessageMeta {
            id: id.to_owned(),
            ..MessageMeta::default()
        };
        if let Some(headers) = value
            .get("payload")
            .and_then(|payload| payload.get("headers"))
            .and_then(Value::as_array)
        {
            for header in headers {
                let Some(name) = header.get("name").and_then(Value::as_str) else {
                    continue;
                };
                let value = header.get("value").and_then(Value::as_str).unwrap_or("");
                match name.to_ascii_lowercase().as_str() {
                    "from" => meta.from = value.to_owned(),
                    "subject" => meta.subject = value.to_owned(),
                    "date" => meta.date = value.to_owned(),
                    _ => {}
                }
            }
        }

        Ok(meta)
    }

    fn access_token(&self) -> Result<String, GmailError> {
        let mut tokens = self.tokens.lock().expect("token lock");

        if let (Some(token), Some(expires_at)) = (&tokens.access_token, tokens.expires_at) {
            if expires_at > Instant::now() + Duration::from_secs(60) {
                return Ok(token.clone());
            }
        }

        let response = auth::refresh(
            &self.http,
            &self.token_url,
            &self.client_id,
            self.client_secret.as_deref(),
            &self.credentials.refresh_token,
        )?;

        tokens.access_token = Some(response.access_token.clone());
        tokens.expires_at = Some(
            Instant::now() + Duration::from_secs(response.expires_in.unwrap_or(3600).max(0) as u64),
        );

        Ok(response.access_token)
    }

    fn invalidate(&self) {
        self.tokens.lock().expect("token lock").access_token = None;
    }

    fn get_json(&self, url: &str) -> Result<Value, GmailError> {
        let mut refreshed = false;
        let mut attempt = 0u32;

        loop {
            attempt += 1;
            let token = self.access_token()?;
            let response = self
                .http
                .get(url)
                .bearer_auth(&token)
                .send()
                .map_err(|error| GmailError::Http(error.to_string()))?;

            let status = response.status();
            let retry_after = response
                .headers()
                .get("retry-after")
                .and_then(|value| value.to_str().ok())
                .and_then(|value| value.parse::<u64>().ok())
                .map(Duration::from_secs);
            let body = response
                .text()
                .map_err(|error| GmailError::Http(error.to_string()))?;

            // A token can be revoked server-side before it expires. Refresh once.
            if status == 401 && !refreshed {
                refreshed = true;
                self.invalidate();
                continue;
            }

            // Parallel header fetches can trip the per-user quota. Back off
            // rather than failing a whole scan because one worker was early.
            if (status == 429 || status.is_server_error()) && attempt < 4 {
                let delay = retry_after
                    .unwrap_or_else(|| Duration::from_secs(1 << (attempt - 1)))
                    .min(Duration::from_secs(8));
                std::thread::sleep(delay);
                continue;
            }

            if !status.is_success() {
                return Err(GmailError::Api {
                    status: status.as_u16(),
                    body,
                });
            }

            return serde_json::from_str(&body)
                .map_err(|error| GmailError::Malformed(format!("{error}: {body}")));
        }
    }
}

fn id_string(value: &Value) -> Option<String> {
    value
        .as_str()
        .map(str::to_owned)
        .or_else(|| value.as_i64().map(|number| number.to_string()))
}

fn decode_base64url(input: &str) -> Result<Vec<u8>, base64::DecodeError> {
    use base64::engine::general_purpose::{URL_SAFE, URL_SAFE_NO_PAD};
    URL_SAFE_NO_PAD
        .decode(input)
        .or_else(|_| URL_SAFE.decode(input))
}
