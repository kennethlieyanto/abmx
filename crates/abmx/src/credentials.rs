//! Parsing for the OAuth client JSON that Google Cloud lets you download.
//!
//! Google uses two envelope names depending on the client type: a desktop app
//! is wrapped in `installed`, a web app in `web`. Both carry the same two
//! values abmx needs. A flat object is also accepted because every other tool
//! (gws, gmailctl) copies those fields around in that shape.

use std::path::Path;

use anyhow::{Context, Result};
use serde::Deserialize;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GoogleClient {
    pub client_id: String,
    pub client_secret: Option<String>,
}

#[derive(Debug, Deserialize)]
struct Envelope {
    #[serde(default)]
    installed: Option<ClientFields>,
    #[serde(default)]
    web: Option<ClientFields>,
    #[serde(default)]
    client_id: Option<String>,
    #[serde(default)]
    client_secret: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ClientFields {
    client_id: Option<String>,
    #[serde(default)]
    client_secret: Option<String>,
}

pub fn load_google_client(path: &Path) -> Result<GoogleClient> {
    let text = std::fs::read_to_string(path)
        .with_context(|| format!("reading OAuth client {}", path.display()))?;
    parse_google_client(&text).with_context(|| format!("parsing OAuth client {}", path.display()))
}

pub fn parse_google_client(text: &str) -> Result<GoogleClient> {
    let envelope: Envelope =
        serde_json::from_str(text).context("not a Google OAuth client JSON")?;

    let fields = envelope.installed.or(envelope.web).unwrap_or(ClientFields {
        client_id: envelope.client_id,
        client_secret: envelope.client_secret,
    });

    let client_id = fields
        .client_id
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
        .ok_or_else(|| anyhow::anyhow!("the OAuth client JSON has no client_id"))?;

    let client_secret = fields
        .client_secret
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty());

    Ok(GoogleClient {
        client_id,
        client_secret,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const DESKTOP: &str = r#"{
      "installed": {
        "client_id": "123-abc.apps.googleusercontent.com",
        "project_id": "abmx-test",
        "auth_uri": "https://accounts.google.com/o/oauth2/auth",
        "token_uri": "https://oauth2.googleapis.com/token",
        "client_secret": "shhh",
        "redirect_uris": ["http://localhost"]
      }
    }"#;

    #[test]
    fn parses_a_desktop_client() {
        let client = parse_google_client(DESKTOP).unwrap();
        assert_eq!(client.client_id, "123-abc.apps.googleusercontent.com");
        assert_eq!(client.client_secret.as_deref(), Some("shhh"));
    }

    #[test]
    fn parses_a_web_client() {
        let client =
            parse_google_client(r#"{"web":{"client_id":"web-id","client_secret":"web-secret"}}"#)
                .unwrap();
        assert_eq!(client.client_id, "web-id");
        assert_eq!(client.client_secret.as_deref(), Some("web-secret"));
    }

    #[test]
    fn parses_a_flat_object() {
        let client =
            parse_google_client(r#"{"client_id":"flat-id","client_secret":"flat-secret"}"#)
                .unwrap();
        assert_eq!(client.client_id, "flat-id");
        assert_eq!(client.client_secret.as_deref(), Some("flat-secret"));
    }

    #[test]
    fn a_missing_client_id_is_an_error() {
        let error = parse_google_client(r#"{"installed":{"client_secret":"only"}}"#)
            .unwrap_err()
            .to_string();
        assert!(error.contains("no client_id"), "got: {error}");
    }

    #[test]
    fn an_empty_client_id_is_an_error() {
        let error = parse_google_client(r#"{"installed":{"client_id":"  "}}"#)
            .unwrap_err()
            .to_string();
        assert!(error.contains("no client_id"), "got: {error}");
    }

    #[test]
    fn invalid_json_is_an_error() {
        assert!(parse_google_client("not json").is_err());
    }
}
