use std::fmt;

use serde::{Deserialize, Serialize};

use crate::error::Result;

/// Default number of messages one `fetch` may return.
///
/// A batch is the unit of work. A large default is fine and a small one is a
/// knob for debugging, not a memory bound.
pub const DEFAULT_LIMIT: usize = 200;

/// Stable identity of a message *at the source*.
///
/// This is deliberately not the `abmx-parser` transaction id: one message may yield
/// several transactions, and a message can be seen before it parses. Prefix by
/// transport (`gmail:18f...`, `dir:2026-10-30.eml`) so keys never collide when
/// two fetchers feed the same state database.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct MessageKey(String);

impl MessageKey {
    pub fn new(key: impl Into<String>) -> Self {
        Self(key.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for MessageKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<String> for MessageKey {
    fn from(value: String) -> Self {
        Self(value)
    }
}

impl From<&str> for MessageKey {
    fn from(value: &str) -> Self {
        Self(value.to_owned())
    }
}

/// One message, exactly as the parser wants it: raw RFC 5322 bytes.
#[derive(Debug, Clone)]
pub struct RawMessage {
    pub key: MessageKey,
    /// The configured routing label this message carried, e.g. `abmx:example-qris`.
    pub label: String,
    /// Human-readable provenance for diagnostics and `source.file`.
    pub origin: String,
    pub bytes: Vec<u8>,
}

impl RawMessage {
    pub fn new(
        key: impl Into<MessageKey>,
        label: impl Into<String>,
        origin: impl Into<String>,
        bytes: Vec<u8>,
    ) -> Self {
        Self {
            key: key.into(),
            label: label.into(),
            origin: origin.into(),
            bytes,
        }
    }
}

/// What one `fetch` produced.
#[derive(Debug, Clone, Default)]
pub struct FetchBatch {
    pub messages: Vec<RawMessage>,
    /// Messages the fetcher filtered out without returning (already labelled,
    /// outside the window, and so on).
    pub skipped: usize,
}

impl FetchBatch {
    pub fn is_empty(&self) -> bool {
        self.messages.is_empty()
    }
}

/// What a fetcher is asked for.
#[derive(Debug, Clone, Copy)]
pub struct FetchRequest<'a> {
    /// The Gmail labels to scan. Each message is tagged with the label it came
    /// from, so the caller can choose the parser without guessing.
    pub labels: &'a [String],
    /// Upper bound requested by the caller.
    ///
    /// Sources that cannot paginate incrementally may ignore it: the Gmail
    /// fetcher currently scans everything under the configured labels, so
    /// `[labels].since` is the real bound there.
    pub limit: usize,
}

impl<'a> FetchRequest<'a> {
    pub fn new(labels: &'a [String]) -> Self {
        Self {
            labels,
            limit: DEFAULT_LIMIT,
        }
    }

    pub fn with_limit(mut self, limit: usize) -> Self {
        self.limit = limit.max(1);
        self
    }
}

/// A source of raw mail.
///
/// Synchronous on purpose: `abmx` is a batch process driven by a timer, `abmx-parser`
/// is synchronous, and a sync trait is trivially usable from an async caller via
/// `spawn_blocking` while the reverse is not. Object-safe so the runner never
/// grows a match arm per source.
pub trait Fetcher {
    /// Stable name, used in diagnostics and state keys.
    fn name(&self) -> &str;

    fn fetch(&self, request: FetchRequest<'_>) -> Result<FetchBatch>;
}
