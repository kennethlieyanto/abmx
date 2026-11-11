use std::fmt;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::Result;

/// Default number of messages one `fetch` may return.
///
/// A batch is the unit of work: the cursor is only advanced after the batch has
/// been applied, so a large default is fine and a small one is a knob for
/// debugging, not a memory bound.
pub const DEFAULT_LIMIT: usize = 200;

/// Stable identity of a message *at the source*.
///
/// This is deliberately not the `txp` transaction id: one message may yield
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
    /// Human-readable provenance for diagnostics and `source.file`.
    pub origin: String,
    pub bytes: Vec<u8>,
}

impl RawMessage {
    pub fn new(key: impl Into<MessageKey>, origin: impl Into<String>, bytes: Vec<u8>) -> Self {
        Self {
            key: key.into(),
            origin: origin.into(),
            bytes,
        }
    }
}

/// What one `fetch` produced.
#[derive(Debug, Clone, Default)]
pub struct FetchBatch {
    pub messages: Vec<RawMessage>,
    /// Opaque checkpoint to persist *after* the batch is applied.
    ///
    /// `None` means "no progress to record", which is correct for sources that
    /// are cheap to re-scan (a directory) and safe for sources that are not
    /// (the state database filters what has already been seen).
    pub next_cursor: Option<Value>,
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
    pub cursor: Option<&'a Value>,
    pub limit: usize,
}

impl<'a> FetchRequest<'a> {
    pub fn new(cursor: Option<&'a Value>) -> Self {
        Self {
            cursor,
            limit: DEFAULT_LIMIT,
        }
    }

    pub fn with_limit(mut self, limit: usize) -> Self {
        self.limit = limit.max(1);
        self
    }
}

impl Default for FetchRequest<'_> {
    fn default() -> Self {
        Self::new(None)
    }
}

/// A source of raw mail.
///
/// Synchronous on purpose: `txsync` is a batch process driven by a timer, `txp`
/// is synchronous, and a sync trait is trivially usable from an async caller via
/// `spawn_blocking` while the reverse is not. Object-safe so the runner never
/// grows a match arm per source.
pub trait Fetcher {
    /// Stable name, used as the state key for the cursor.
    fn name(&self) -> &str;

    fn fetch(&self, request: FetchRequest<'_>) -> Result<FetchBatch>;
}
