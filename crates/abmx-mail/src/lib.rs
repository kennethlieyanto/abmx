//! Email normalization: raw RFC 5322 bytes in, uniform blocks out.
//!
//! This is the only crate allowed to touch MIME.  Everything downstream deals
//! with the same shape regardless of how a bank formatted its email.

use abmx_tx::{Address, NormalizedEmail, Source};
use chrono::{FixedOffset, NaiveDate, TimeZone};
use mail_parser::{MessageParser, MimeHeaders, PartType};

mod html;

pub use html::blocks_from_html;

#[derive(Debug, thiserror::Error)]
pub enum MailError {
    #[error("could not parse message")]
    Parse,
    #[error("message has no usable body")]
    EmptyBody,
    #[error("could not read {path}: {message}")]
    Unreadable { path: String, message: String },
}

/// Parse a `.eml` file into the normalized shape used by all parsers.
///
/// `file` is only used for identity when there is no Message-ID header (local
/// fixtures, mostly).
/// One attachment from a message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Attachment {
    pub name: Option<String>,
    pub content_type: Option<String>,
    pub bytes: Vec<u8>,
}

impl Attachment {
    /// True when the part is a PDF, by MIME type or by file name.
    ///
    /// Banks are inconsistent here: some set `application/pdf`, some send
    /// `application/octet-stream` with a `.pdf` name, some do both.
    pub fn is_pdf(&self) -> bool {
        self.content_type
            .as_deref()
            .is_some_and(|value| value.eq_ignore_ascii_case("application/pdf"))
            || self
                .name
                .as_deref()
                .is_some_and(|value| value.to_ascii_lowercase().ends_with(".pdf"))
    }
}

/// Every attachment in a message, decoded.
///
/// This is how an emailed e-statement reaches `abmx-doc`; the message body is
/// irrelevant. Attachment order is the MIME order, which is stable for a given
/// message, so `#att:<index>` is a usable identity.
pub fn attachments(bytes: &[u8]) -> Result<Vec<Attachment>, MailError> {
    let message = MessageParser::new().parse(bytes).ok_or(MailError::Parse)?;
    let mut out = Vec::new();

    for part in message.attachments() {
        let bytes = match &part.body {
            PartType::Binary(data) | PartType::InlineBinary(data) => data.to_vec(),
            _ => continue,
        };

        let content_type = part
            .content_type()
            .and_then(|content_type| content_type.c_subtype.as_ref().map(|_| content_type))
            .map(|content_type| {
                format!(
                    "{}/{}",
                    content_type.c_type,
                    content_type.c_subtype.as_deref().unwrap_or("octet-stream")
                )
            });

        out.push(Attachment {
            name: part.attachment_name().map(str::to_owned),
            content_type,
            bytes,
        });
    }

    Ok(out)
}

pub fn parse_eml(bytes: &[u8], file: Option<String>) -> Result<NormalizedEmail, MailError> {
    let message = MessageParser::new().parse(bytes).ok_or(MailError::Parse)?;

    let from = message
        .from()
        .and_then(|list| {
            list.first().map(|addr| Address {
                name: addr.name().map(str::to_string),
                email: addr.address().unwrap_or_default().to_string(),
            })
        })
        .unwrap_or_default();

    let subject = message.subject().unwrap_or_default().to_string();

    let date = message
        .date()
        .map(|date| {
            let seconds = i32::from(date.tz_hour) * 3600 + i32::from(date.tz_minute) * 60;
            let seconds = if date.tz_before_gmt {
                -seconds
            } else {
                seconds
            };
            let offset = FixedOffset::east_opt(seconds)
                .unwrap_or_else(|| FixedOffset::east_opt(0).expect("UTC offset"));
            let naive = NaiveDate::from_ymd_opt(
                i32::from(date.year),
                u32::from(date.month),
                u32::from(date.day),
            )
            .and_then(|day| {
                day.and_hms_opt(
                    u32::from(date.hour),
                    u32::from(date.minute),
                    u32::from(date.second),
                )
            })
            .unwrap_or_default();

            offset
                .from_local_datetime(&naive)
                .single()
                .unwrap_or_else(|| offset.from_utc_datetime(&naive))
        })
        .unwrap_or_else(|| {
            FixedOffset::east_opt(0)
                .expect("UTC offset")
                .from_utc_datetime(&chrono::NaiveDateTime::default())
        });

    let html = message
        .body_html(0)
        .map(|body| body.into_owned())
        .filter(|body| !body.trim().is_empty());

    let blocks = match html {
        Some(html) => blocks_from_html(&html),
        None => message
            .body_text(0)
            .map(|body| blocks_from_plain_text(&body))
            .unwrap_or_default(),
    };

    if blocks.is_empty() {
        return Err(MailError::EmptyBody);
    }

    Ok(NormalizedEmail {
        source: Source {
            message_id: message.message_id().map(str::to_string),
            file,
            tx_index: 0,
        },
        from,
        subject,
        date,
        blocks: blocks.into_iter().map(abmx_tx::Block::new).collect(),
    })
}

fn blocks_from_plain_text(text: &str) -> Vec<String> {
    text.lines()
        .map(normalize_block)
        .filter(|line| !line.is_empty())
        .collect()
}

/// Collapse whitespace and drop the `:` separator cells that banks put between
/// a label and its value.
pub(crate) fn normalize_block(raw: &str) -> String {
    let mut text = String::with_capacity(raw.len());
    let mut last_was_space = false;

    for ch in raw.chars() {
        if ch.is_whitespace() {
            if !last_was_space {
                text.push(' ');
                last_was_space = true;
            }
        } else {
            text.push(ch);
            last_was_space = false;
        }
    }

    let text = text.trim();
    let text = text.strip_prefix(": ").unwrap_or(text);
    let text = text.strip_prefix(':').unwrap_or(text);
    let text = text.trim();

    if text.is_empty() || text == ":" {
        String::new()
    } else {
        text.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_separator_cells() {
        assert_eq!(normalize_block(": XXXX XXXX XXXX"), "XXXX XXXX XXXX");
        assert_eq!(normalize_block(" :"), "");
        assert_eq!(normalize_block(":"), "");
        assert_eq!(
            normalize_block("Here is : the thing"),
            "Here is : the thing"
        );
    }

    #[test]
    fn collapses_nbsp_and_newlines() {
        assert_eq!(normalize_block(" Total\u{a0}\n Bayar "), "Total Bayar");
    }
}
