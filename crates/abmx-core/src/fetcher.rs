use std::collections::HashSet;
use std::fmt;

use serde::{Deserialize, Serialize};

use crate::error::Result;

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
    /// Messages the fetcher filtered out without returning (deleted, over the
    /// caller's limit, and so on).
    pub skipped: usize,
    /// Messages the fetcher recognised as already delivered and did not
    /// download or return.
    pub already_seen: usize,
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
    /// Upper bound on messages this fetch should return, after skip filtering.
    /// `None` means unlimited.
    pub limit: Option<usize>,
    /// Message keys already delivered. Sources should filter these before
    /// downloading raw bodies; the runner also checks them as a safety net.
    pub skip: Option<&'a HashSet<String>>,
}

impl<'a> FetchRequest<'a> {
    pub fn new(labels: &'a [String]) -> Self {
        Self {
            labels,
            limit: None,
            skip: None,
        }
    }

    pub fn with_limit(mut self, limit: usize) -> Self {
        self.limit = Some(limit.max(1));
        self
    }

    pub fn with_skip(mut self, skip: &'a HashSet<String>) -> Self {
        self.skip = Some(skip);
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
