//! Compare the bundled `@actual-app/api` client with the Actual server.
//!
//! Actual ships the server and the Node client from the same monorepo, and the
//! wrapper this replaced documented the same rule ("the image tag must match
//! your Actual server's version"). A minor mismatch usually works; a year jump
//! (26 → 27) is the compatibility boundary, so `abmx` refuses to write until it
//! is updated.

use std::time::Duration;

use serde_json::json;

use crate::helper::{Helper, ProcessHelper};
use crate::ActualConfig;

#[derive(Debug, Clone, Default)]
pub struct VersionReport {
    /// The `@actual-app/api` version bundled with this abmx build.
    pub helper: Option<String>,
    /// The `build.version` reported by the Actual server's `/info`.
    pub server: Option<String>,
    pub helper_error: Option<String>,
    pub server_error: Option<String>,
}

/// Something the user needs to act on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Concern {
    /// `true` when writing would be unsafe without updating abmx first.
    pub fatal: bool,
    pub message: String,
}

impl VersionReport {
    pub fn concern(&self) -> Option<Concern> {
        let (Some(server), Some(helper)) = (&self.server, &self.helper) else {
            return None;
        };
        if server == helper {
            return None;
        }

        match (major(server), major(helper)) {
            (Some(server_major), Some(helper_major)) if server_major > helper_major => {
                Some(Concern {
                    fatal: true,
                    message: format!(
                        "the Actual server is {server}, but this abmx bundles \
                         @actual-app/api {helper}. Update abmx before writing."
                    ),
                })
            }
            (Some(server_major), Some(helper_major)) if server_major < helper_major => {
                Some(Concern {
                    fatal: false,
                    message: format!(
                        "the Actual server is {server}, older than the bundled client \
                         (@actual-app/api {helper}). Downgrading Actual is not supported \
                         upstream; expect sync errors."
                    ),
                })
            }
            _ => Some(Concern {
                fatal: false,
                message: format!(
                    "the Actual server is {server}; this abmx bundles @actual-app/api \
                     {helper}. Update abmx so the client tracks the server."
                ),
            }),
        }
    }

    /// One line per side, for `abmx status`.
    pub fn describe(&self) -> Vec<String> {
        let helper = match (&self.helper, &self.helper_error) {
            (Some(version), _) => version.clone(),
            (None, Some(error)) => format!("unknown ({error})"),
            (None, None) => "unknown".to_owned(),
        };
        let server = match (&self.server, &self.server_error) {
            (Some(version), _) => version.clone(),
            (None, Some(error)) => format!("unknown ({error})"),
            (None, None) => "unknown".to_owned(),
        };
        vec![
            format!("helper {helper} (@actual-app/api)"),
            format!("server {server}"),
        ]
    }
}

/// Ask the helper which client version it bundles, and the server which version
/// it is. Both halves are best-effort: a status report should still render when
/// one side is unreachable.
pub fn report(config: &ActualConfig) -> VersionReport {
    let helper = ProcessHelper::new(config.clone())
        .call("version", json!({}))
        .map(|data| {
            data.get("api")
                .and_then(|value| value.as_str())
                .map(str::to_owned)
        })
        .map_err(|error| error.to_string())
        .and_then(|value| match value {
            Some(version) => Ok(version),
            None => Err("the helper did not report an @actual-app/api version".to_owned()),
        });

    let server = server_version(&config.server_url, Duration::from_secs(5));

    VersionReport {
        helper: helper.as_ref().ok().cloned(),
        server: server.as_ref().ok().cloned(),
        helper_error: helper.err(),
        server_error: server.err(),
    }
}

/// `GET /info` is unauthenticated, so this works even with a broken API key.
pub fn server_version(server_url: &str, timeout: Duration) -> Result<String, String> {
    let url = format!("{}/info", server_url.trim_end_matches('/'));
    let client = reqwest::blocking::Client::builder()
        .timeout(timeout)
        .build()
        .map_err(|error| format!("building the http client: {error}"))?;

    let response = client
        .get(&url)
        .send()
        .map_err(|error| format!("{url}: {error}"))?;
    let status = response.status();
    let text = response.text().map_err(|error| format!("{url}: {error}"))?;

    if !status.is_success() {
        return Err(format!("{url} returned {status}"));
    }

    let value: serde_json::Value =
        serde_json::from_str(&text).map_err(|error| format!("{url}: {error}"))?;
    value
        .get("build")
        .and_then(|build| build.get("version"))
        .and_then(|version| version.as_str())
        .map(str::to_owned)
        .ok_or_else(|| format!("{url} did not report a build version"))
}

fn major(version: &str) -> Option<u32> {
    version.split('.').next()?.parse().ok()
}

pub(crate) fn truncate(text: &str, limit: usize) -> String {
    let text = text.trim();
    if text.chars().count() <= limit {
        return text.to_owned();
    }
    let shortened: String = text.chars().take(limit).collect();
    format!("{shortened}…")
}

#[cfg(test)]
mod tests {
    use super::VersionReport;

    fn report(server: &str, helper: &str) -> VersionReport {
        VersionReport {
            helper: Some(helper.to_owned()),
            server: Some(server.to_owned()),
            ..VersionReport::default()
        }
    }

    #[test]
    fn matching_versions_are_quiet() {
        assert!(report("26.11.0", "26.11.0").concern().is_none());
    }

    #[test]
    fn a_minor_skew_warns_but_does_not_block() {
        let concern = report("26.12.0", "26.11.0").concern().unwrap();
        assert!(!concern.fatal);
        assert!(concern.message.contains("26.12.0"));
    }

    #[test]
    fn a_year_jump_blocks_the_write() {
        let concern = report("27.1.0", "26.11.0").concern().unwrap();
        assert!(concern.fatal);
        assert!(concern.message.contains("Update abmx"));
    }

    #[test]
    fn an_older_server_only_warns() {
        let concern = report("25.10.0", "26.11.0").concern().unwrap();
        assert!(!concern.fatal);
        assert!(concern.message.contains("older"));
    }

    #[test]
    fn an_unreachable_server_is_not_a_version_problem() {
        let report = VersionReport {
            helper: Some("26.11.0".to_owned()),
            server_error: Some("connection refused".to_owned()),
            ..VersionReport::default()
        };
        assert!(report.concern().is_none());
    }
}
