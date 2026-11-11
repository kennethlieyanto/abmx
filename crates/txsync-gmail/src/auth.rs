//! OAuth2 for Gmail: PKCE, loopback redirect, refresh token on disk.
//!
//! The flow is the standard "desktop app" one. Google retired the out-of-band
//! flow, so a loopback listener is the only headless-friendly option that does
//! not require a public redirect URL. On a server without a browser, run
//! `txsync auth gmail` on a machine that has one and copy the credential file.

use std::fs::{self, OpenOptions};
use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use base64::Engine;
use rand::Rng;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use url::Url;

use crate::client::GmailError;

pub const AUTH_URL: &str = "https://accounts.google.com/o/oauth2/v2/auth";
pub const TOKEN_URL: &str = "https://oauth2.googleapis.com/token";
/// Read-only is all `txsync` needs. Never request more.
pub const SCOPE: &str = "https://www.googleapis.com/auth/gmail.readonly";

/// What survives between runs.
///
/// The access token is deliberately *not* persisted: it is short-lived, and
/// keeping one secret on disk instead of two is a smaller blast radius.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Credentials {
    pub client_id: String,
    pub refresh_token: String,
    #[serde(default)]
    pub scopes: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct TokenStore {
    path: PathBuf,
}

impl TokenStore {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn load(&self) -> Result<Option<Credentials>, GmailError> {
        match fs::read(&self.path) {
            Ok(bytes) => serde_json::from_slice(&bytes)
                .map(Some)
                .map_err(|error| GmailError::Auth(format!("{}: {error}", self.path.display()))),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(GmailError::Auth(format!(
                "reading {}: {error}",
                self.path.display()
            ))),
        }
    }

    pub fn save(&self, credentials: &Credentials) -> Result<(), GmailError> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent).map_err(|error| {
                GmailError::Auth(format!("creating {}: {error}", parent.display()))
            })?;
        }

        let json = serde_json::to_vec_pretty(credentials)
            .map_err(|error| GmailError::Auth(error.to_string()))?;

        let mut options = OpenOptions::new();
        options.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&self.path).map_err(|error| {
            GmailError::Auth(format!("opening {}: {error}", self.path.display()))
        })?;
        file.write_all(&json).map_err(|error| {
            GmailError::Auth(format!("writing {}: {error}", self.path.display()))
        })?;

        // `mode` only applies on creation; tighten an existing file too.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = fs::set_permissions(&self.path, fs::Permissions::from_mode(0o600));
        }

        Ok(())
    }
}

#[derive(Debug, Clone)]
pub struct OAuthConfig {
    pub client_id: String,
    pub client_secret: Option<String>,
    pub auth_url: String,
    pub token_url: String,
    /// `0` picks an ephemeral port.
    pub port: u16,
    pub open_browser: bool,
    pub timeout: Duration,
}

impl OAuthConfig {
    pub fn new(client_id: impl Into<String>) -> Self {
        Self {
            client_id: client_id.into(),
            client_secret: None,
            auth_url: AUTH_URL.to_owned(),
            token_url: TOKEN_URL.to_owned(),
            port: 0,
            open_browser: true,
            timeout: Duration::from_secs(300),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct TokenResponse {
    pub access_token: String,
    #[serde(default)]
    pub refresh_token: Option<String>,
    #[serde(default)]
    pub expires_in: Option<i64>,
    #[serde(default)]
    pub scope: Option<String>,
}

/// Run the interactive flow and return credentials ready to persist.
pub fn authorize(config: &OAuthConfig) -> Result<Credentials, GmailError> {
    let listener = TcpListener::bind(("127.0.0.1", config.port))
        .map_err(|error| GmailError::Auth(format!("binding loopback listener: {error}")))?;
    listener
        .set_nonblocking(true)
        .map_err(|error| GmailError::Auth(format!("loopback listener: {error}")))?;
    let port = listener
        .local_addr()
        .map_err(|error| GmailError::Auth(format!("loopback listener: {error}")))?
        .port();
    let redirect = format!("http://127.0.0.1:{port}/callback");

    let verifier = pkce_verifier();
    let challenge = pkce_challenge(&verifier);
    let state = pkce_verifier();

    let mut url = Url::parse(&config.auth_url)
        .map_err(|error| GmailError::Auth(format!("auth url: {error}")))?;
    url.query_pairs_mut()
        .append_pair("client_id", &config.client_id)
        .append_pair("redirect_uri", &redirect)
        .append_pair("response_type", "code")
        .append_pair("scope", SCOPE)
        .append_pair("state", &state)
        .append_pair("code_challenge", &challenge)
        .append_pair("code_challenge_method", "S256")
        .append_pair("access_type", "offline")
        // Without `consent` Google only returns a refresh token on the very
        // first grant, which makes re-running auth silently fail later.
        .append_pair("prompt", "consent");

    eprintln!("Open this URL and approve read-only Gmail access:\n\n  {url}\n");
    if config.open_browser {
        let _ = open_in_browser(url.as_str());
    }

    let (code, returned_state) = wait_for_callback(&listener, config.timeout)?;
    if returned_state != state {
        return Err(GmailError::Auth(
            "state mismatch: the callback did not come from this request".to_owned(),
        ));
    }

    let http = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .map_err(|error| GmailError::Http(error.to_string()))?;

    let tokens = exchange_code(
        &http,
        &config.token_url,
        &config.client_id,
        config.client_secret.as_deref(),
        &code,
        &redirect,
        &verifier,
    )?;

    let refresh_token = tokens.refresh_token.ok_or_else(|| {
        GmailError::Auth(
            "no refresh token returned; remove the app's access at \
             https://myaccount.google.com/permissions and try again"
                .to_owned(),
        )
    })?;

    Ok(Credentials {
        client_id: config.client_id.clone(),
        refresh_token,
        scopes: tokens
            .scope
            .map(|scope| scope.split_whitespace().map(str::to_owned).collect())
            .unwrap_or_else(|| vec![SCOPE.to_owned()]),
    })
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn exchange_code(
    http: &reqwest::blocking::Client,
    token_url: &str,
    client_id: &str,
    client_secret: Option<&str>,
    code: &str,
    redirect_uri: &str,
    verifier: &str,
) -> Result<TokenResponse, GmailError> {
    let mut form: Vec<(&str, &str)> = vec![
        ("code", code),
        ("client_id", client_id),
        ("redirect_uri", redirect_uri),
        ("grant_type", "authorization_code"),
        ("code_verifier", verifier),
    ];
    if let Some(secret) = client_secret {
        form.push(("client_secret", secret));
    }
    post_token(http, token_url, &form)
}

pub(crate) fn refresh(
    http: &reqwest::blocking::Client,
    token_url: &str,
    client_id: &str,
    client_secret: Option<&str>,
    refresh_token: &str,
) -> Result<TokenResponse, GmailError> {
    let mut form: Vec<(&str, &str)> = vec![
        ("client_id", client_id),
        ("refresh_token", refresh_token),
        ("grant_type", "refresh_token"),
    ];
    if let Some(secret) = client_secret {
        form.push(("client_secret", secret));
    }
    post_token(http, token_url, &form)
}

fn post_token(
    http: &reqwest::blocking::Client,
    token_url: &str,
    form: &[(&str, &str)],
) -> Result<TokenResponse, GmailError> {
    let response = http
        .post(token_url)
        .form(form)
        .send()
        .map_err(|error| GmailError::Http(error.to_string()))?;

    let status = response.status();
    let body = response
        .text()
        .map_err(|error| GmailError::Http(error.to_string()))?;

    if !status.is_success() {
        return Err(GmailError::Auth(format!(
            "token endpoint returned {status}: {body}"
        )));
    }

    serde_json::from_str(&body)
        .map_err(|error| GmailError::Auth(format!("token response: {error}")))
}

pub fn pkce_verifier() -> String {
    const CHARSET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-._~";
    let mut rng = rand::thread_rng();
    (0..64)
        .map(|_| CHARSET[rng.gen_range(0..CHARSET.len())] as char)
        .collect()
}

pub fn pkce_challenge(verifier: &str) -> String {
    let digest = Sha256::digest(verifier.as_bytes());
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(digest)
}

fn wait_for_callback(
    listener: &TcpListener,
    timeout: Duration,
) -> Result<(String, String), GmailError> {
    let deadline = Instant::now() + timeout;

    while Instant::now() < deadline {
        match listener.accept() {
            Ok((mut stream, _)) => {
                let mut buffer = [0u8; 8192];
                let read = stream
                    .read(&mut buffer)
                    .map_err(|error| GmailError::Auth(error.to_string()))?;
                let request = String::from_utf8_lossy(&buffer[..read]).to_string();
                let path = request
                    .lines()
                    .next()
                    .and_then(|line| line.split_whitespace().nth(1))
                    .unwrap_or_default()
                    .to_owned();

                let Some(query) = path.split_once('?').map(|(_, query)| query) else {
                    respond(&mut stream, 404, "Not found");
                    continue;
                };

                let mut code = None;
                let mut state = None;
                let mut error = None;
                for (key, value) in url::form_urlencoded::parse(query.as_bytes()) {
                    match key.as_ref() {
                        "code" => code = Some(value.into_owned()),
                        "state" => state = Some(value.into_owned()),
                        "error" => error = Some(value.into_owned()),
                        _ => {}
                    }
                }

                if let Some(error) = error {
                    respond(&mut stream, 400, "Authorization was denied.");
                    return Err(GmailError::Auth(format!("authorization denied: {error}")));
                }

                match (code, state) {
                    (Some(code), Some(state)) => {
                        respond(
                            &mut stream,
                            200,
                            "txsync is authorized. You can close this tab.",
                        );
                        return Ok((code, state));
                    }
                    _ => {
                        respond(&mut stream, 400, "Missing code or state.");
                        continue;
                    }
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(25));
            }
            Err(error) => return Err(GmailError::Auth(error.to_string())),
        }
    }

    Err(GmailError::Auth(
        "timed out waiting for the OAuth callback".to_owned(),
    ))
}

fn respond(stream: &mut std::net::TcpStream, status: u16, message: &str) {
    let body = format!(
        "<!doctype html><meta charset=\"utf-8\"><title>txsync</title>\
         <body style=\"font-family:system-ui;padding:3rem\"><p>{message}</p>"
    );
    let response = format!(
        "HTTP/1.1 {status} OK\r\nContent-Type: text/html; charset=utf-8\r\n\
         Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let _ = stream.write_all(response.as_bytes());
    let _ = stream.flush();
}

fn open_in_browser(url: &str) -> std::io::Result<()> {
    #[cfg(target_os = "macos")]
    let program = "open";
    #[cfg(not(target_os = "macos"))]
    let program = "xdg-open";

    std::process::Command::new(program)
        .arg(url)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pkce_challenge_matches_the_rfc_vector() {
        // RFC 7636 appendix B.
        assert_eq!(
            pkce_challenge("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"),
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        );
    }

    #[test]
    fn verifier_is_in_the_unreserved_set() {
        let verifier = pkce_verifier();
        assert_eq!(verifier.len(), 64);
        assert!(verifier
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "-._~".contains(c)));
    }

    #[test]
    fn token_store_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let store = TokenStore::new(dir.path().join("nested/creds.json"));
        assert!(store.load().unwrap().is_none());

        let credentials = Credentials {
            client_id: "client".into(),
            refresh_token: "refresh".into(),
            scopes: vec![SCOPE.into()],
        };
        store.save(&credentials).unwrap();

        let loaded = store.load().unwrap().unwrap();
        assert_eq!(loaded.refresh_token, "refresh");

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(store.path()).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }
    }
}
