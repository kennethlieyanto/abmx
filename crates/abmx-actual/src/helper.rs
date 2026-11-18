//! JSON-over-stdio transport to the bundled Node helper.
//!
//! The helper is `crates/abmx-actual/helper/helper.js`. It exists because
//! Actual's official API is a Node library (`@actual-app/api`), not an HTTP
//! API. `abmx` spawns it for one operation at a time and waits for it to exit:
//! no port, no service, and no long-running translator.
//!
//! One request goes in on stdin, one response comes back on stdout. The helper
//! redirects everything the library logs on stdout to stderr, so stdout stays a
//! single JSON document.

use std::io::{Read, Write};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use serde::Deserialize;
use serde_json::Value;

use abmx_core::{Error, Result};

use crate::{ActualAuth, ActualConfig};

/// A way to ask the Actual client to do something. The process transport is the
/// real one; tests substitute an in-memory fake.
pub trait Helper: Send + Sync {
    fn call(&self, method: &str, params: Value) -> Result<Value>;
}

/// Spawns `node helper.js` for each request.
#[derive(Debug, Clone)]
pub struct ProcessHelper {
    config: ActualConfig,
}

impl ProcessHelper {
    pub fn new(config: ActualConfig) -> Self {
        Self { config }
    }

    fn command(&self) -> Command {
        let mut command = Command::new(&self.config.node);
        command
            .arg(&self.config.helper)
            .env("ACTUAL_SERVER_URL", &self.config.server_url)
            .env("ACTUAL_DATA_DIR", &self.config.data_dir)
            // Clear inherited credentials so a stale parent environment cannot
            // silently win over the configured one.
            .env_remove("ACTUAL_API_KEY")
            .env_remove("ACTUAL_SERVER_PASSWORD")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            // Breadcrumbs and stack traces stay in the service log.
            .stderr(Stdio::inherit());

        match &self.config.auth {
            ActualAuth::ApiKey(key) => {
                command.env("ACTUAL_API_KEY", key);
            }
            ActualAuth::ServerPassword(password) => {
                command.env("ACTUAL_SERVER_PASSWORD", password);
            }
        }

        command
    }

    fn run(&self, request: &Value) -> Result<Vec<u8>> {
        let mut child = self.command().spawn().map_err(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                Error::Sink(format!(
                    "could not run `{}`: {error}. Install Node 22+ or set \
                     `[sink.actual].node`; the helper is resolved from \
                     `ABMX_ACTUAL_HELPER` or next to the abmx binary",
                    self.config.node.display()
                ))
            } else {
                Error::Sink(format!("spawning the Actual helper: {error}"))
            }
        })?;

        if let Some(mut stdin) = child.stdin.take() {
            let bytes = serde_json::to_vec(request)
                .map_err(|error| Error::Sink(format!("encoding the helper request: {error}")))?;
            stdin
                .write_all(&bytes)
                .map_err(|error| Error::Sink(format!("writing to the Actual helper: {error}")))?;
            // Dropping stdin closes the pipe, which is the helper's EOF.
        }

        let mut stdout = child
            .stdout
            .take()
            .expect("stdout was configured as a pipe");
        let reader = std::thread::spawn(move || {
            let mut buffer = Vec::new();
            stdout.read_to_end(&mut buffer).map(|_| buffer)
        });

        let deadline = Instant::now() + self.config.timeout;
        let status = loop {
            match child.try_wait() {
                Ok(Some(status)) => break status,
                Ok(None) if Instant::now() >= deadline => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(Error::Sink(format!(
                        "the Actual helper timed out after {}s; raise \
                         `[sink.actual].timeout_secs` if your budget sync is slow",
                        self.config.timeout.as_secs()
                    )));
                }
                Ok(None) => std::thread::sleep(Duration::from_millis(25)),
                Err(error) => {
                    return Err(Error::Sink(format!(
                        "waiting for the Actual helper: {error}"
                    )));
                }
            }
        };

        let bytes = reader
            .join()
            .map_err(|_| Error::Sink("the Actual helper reader panicked".to_owned()))?
            .map_err(|error| Error::Sink(format!("reading the Actual helper: {error}")))?;

        if bytes.is_empty() {
            return Err(Error::Sink(format!(
                "the Actual helper exited with {status} without a response \
                 (see the service log for its stderr)"
            )));
        }

        Ok(bytes)
    }

    fn explain(&self, envelope: &Envelope) -> String {
        let message = envelope
            .error
            .as_deref()
            .unwrap_or("the actual helper reported an error without a message");

        let hint = match envelope.code.as_deref() {
            Some("invalid-password" | "token-expired" | "unauthorized") => {
                "check the Actual API key (Settings → General → Security → API keys) \
                 or the server password"
            }
            Some("budget-not-found") => "check `[sink.actual].budget_sync_id`",
            Some("network-failure" | "network") => {
                "is the Actual server reachable? check `server_url`"
            }
            Some("missing-key" | "decrypt-failure" | "old-key-style") => {
                "check `[sink.actual].budget_password`"
            }
            Some("out-of-sync-migrations") => {
                "the budget was migrated by a newer Actual; update abmx"
            }
            Some("unknown-method") => "this is an abmx bug: the helper and the CLI disagree",
            _ => "",
        };

        if hint.is_empty() {
            format!("actual: {message}")
        } else {
            format!("actual: {message} ({hint})")
        }
    }
}

#[derive(Debug, Deserialize)]
struct Envelope {
    ok: bool,
    #[serde(default)]
    data: Value,
    #[serde(default)]
    error: Option<String>,
    #[serde(default)]
    code: Option<String>,
}

impl Helper for ProcessHelper {
    fn call(&self, method: &str, params: Value) -> Result<Value> {
        let request = serde_json::json!({ "method": method, "params": params });
        let bytes = self.run(&request)?;

        let envelope: Envelope = serde_json::from_slice(&bytes).map_err(|error| {
            Error::Sink(format!(
                "unexpected output from the Actual helper ({error}): {}",
                truncate(&String::from_utf8_lossy(&bytes), 300)
            ))
        })?;

        if envelope.ok {
            Ok(envelope.data)
        } else {
            Err(Error::Sink(self.explain(&envelope)))
        }
    }
}

fn truncate(text: &str, limit: usize) -> String {
    let trimmed = text.trim();
    if trimmed.chars().count() <= limit {
        return trimmed.to_owned();
    }
    let shortened: String = trimmed.chars().take(limit).collect();
    format!("{shortened}…")
}
