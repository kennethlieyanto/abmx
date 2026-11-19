//! Fetch a small set of messages for parser authoring.
//!
//! This is deliberately not a [`Fetcher`](abmx_core::Fetcher): sampling is a
//! read-only authoring aid, not a sync. It never writes a cursor, never touches
//! the state database, and never decides what is new.

use std::collections::HashSet;

use crate::client::{GmailClient, GmailError};

/// One raw message sampled from a label.
#[derive(Debug, Clone)]
pub struct SampleMessage {
    /// Gmail's message id, stable within the mailbox.
    pub id: String,
    /// Raw RFC 5322 bytes, exactly as `abmx-parser` consumes them.
    pub bytes: Vec<u8>,
}

impl GmailClient {
    /// List up to `limit` messages carrying `label` and fetch their raw bytes.
    ///
    /// Spam and trash are excluded so a sample set is not polluted by
    /// filtered mail. Label names are matched by Gmail's search syntax, which
    /// understands nested labels such as `Bank/Example/QRIS`.
    pub fn sample_label(
        &self,
        label: &str,
        limit: usize,
    ) -> Result<Vec<SampleMessage>, GmailError> {
        let limit = limit.max(1);
        let label = label.trim();
        if label.is_empty() {
            return Ok(Vec::new());
        }
        let query = format!("label:\"{}\" -in:spam -in:trash", escape(label));

        let mut ids: Vec<String> = Vec::new();
        let mut seen: HashSet<String> = HashSet::new();
        let mut page_token: Option<String> = None;

        loop {
            let page = self.list_messages(&query, &[], page_token.as_deref(), 500)?;
            for id in page.ids {
                if ids.len() >= limit {
                    break;
                }
                if seen.insert(id.clone()) {
                    ids.push(id);
                }
            }
            page_token = page.next_page_token;
            if page_token.is_none() || ids.len() >= limit {
                break;
            }
        }

        let mut samples = Vec::with_capacity(ids.len());
        for id in ids {
            let bytes = self.raw_message(&id)?;
            samples.push(SampleMessage { id, bytes });
        }
        Ok(samples)
    }
}

/// Gmail search does not define a backslash escape inside a quoted phrase, so
/// the safest thing to do with a quote in a label name is to drop it.
fn escape(label: &str) -> String {
    label.replace('"', "")
}
