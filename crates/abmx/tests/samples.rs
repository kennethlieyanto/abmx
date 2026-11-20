//! `abmx receipt samples`: download the newest messages for every non-ignored label.

use std::path::Path;

use abmx::config::Config;
use abmx::samples::{self, Options};
use abmx_gmail::auth::{Credentials, TokenStore};
use abmx_gmail::{GmailConfig, GmailFetcher};
use abmx_testkit::{MockServer, Response};
use base64::Engine;

const EMAIL: &str = "From: Example <example@bank.example>\r\n\
To: me@example.invalid\r\n\
Subject: Internet Transaction Journal\r\n\
Date: Thu, 30 Oct 2026 17:28:17 +0700\r\n\
Message-ID: <samples-1@example.invalid>\r\n\
MIME-Version: 1.0\r\n\
Content-Type: text/html; charset=utf-8\r\n\
\r\n\
<html><body>\
<div>Total Payment</div><div>Rp 27.500</div>\
<div>Transaction Date</div><div>30 Oct 2026 17:28:17</div>\
<div>Payment to</div><div>ExamplePay</div>\
</body></html>\r\n";

fn config(dir: &Path) -> Config {
    let path = dir.join("abmx.toml");
    std::fs::write(
        &path,
        r#"
[sink.file]
path = "ledger.jsonl"

[receipts]
prefix = "receipt/"
ignore = ["ignored.one"]

[receipts.accounts]
"example.card_debit" = "uuid-example"
"#,
    )
    .unwrap();
    Config::load(&path).unwrap()
}

fn fetcher(server: &MockServer, dir: &Path) -> GmailFetcher {
    let token_path = dir.join("credentials.json");
    TokenStore::new(&token_path)
        .save(&Credentials {
            client_id: "client".into(),
            refresh_token: "refresh".into(),
            scopes: vec![],
            client_secret: None,
        })
        .unwrap();

    let mut config = GmailConfig::new("client", &token_path);
    config.base_url = format!("{}/gmail/v1", server.url());
    config.token_url = format!("{}/token", server.url());
    config.prefix = "receipt/".to_owned();
    GmailFetcher::new(config).unwrap()
}

fn token_route(server: &MockServer) {
    server.route(
        "POST",
        "/token",
        vec![Response::json(
            r#"{"access_token":"access","expires_in":3600}"#,
        )],
    );
}

fn raw_routes(server: &MockServer, ids: &[&str]) {
    let raw = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(EMAIL);
    for id in ids {
        server.route(
            "GET",
            &format!("/gmail/v1/users/me/messages/{id}"),
            vec![Response::json(format!(r#"{{"id":"{id}","raw":"{raw}"}}"#))],
        );
    }
}

#[test]
fn downloads_the_newest_three_for_non_ignored_labels() {
    let dir = tempfile::tempdir().unwrap();
    let server = MockServer::start();
    token_route(&server);
    raw_routes(&server, &["m1", "m2", "m3"]);
    server.route(
        "GET",
        "/gmail/v1/users/me/messages",
        vec![Response::json(
            r#"{"messages":[{"id":"m1"},{"id":"m2"},{"id":"m3"}]}"#,
        )],
    );
    server.route(
        "GET",
        "/gmail/v1/users/me/labels",
        vec![Response::json(
            r#"{"labels":[
                {"id":"Label_1","name":"receipt/example.card_debit"},
                {"id":"Label_2","name":"receipt/ignored.one"},
                {"id":"Label_3","name":"receipt/unmanaged.one"}
            ]}"#,
        )],
    );

    let config = config(dir.path());
    let fetcher = fetcher(&server, dir.path());
    let out = dir.path().join("samples");
    let report = samples::download(
        &config,
        &fetcher,
        &Options {
            out: Some(out.clone()),
            samples: 3,
            label: None,
        },
    )
    .unwrap();

    assert_eq!(
        report.labels.len(),
        2,
        "ignored labels are skipped; unmapped ones are sampled"
    );
    assert!(report
        .labels
        .iter()
        .all(|label| label.parser_id != "ignored.one"));

    let label = report
        .labels
        .iter()
        .find(|label| label.parser_id == "example.card_debit")
        .expect("mapped label");
    assert_eq!(label.label, "receipt/example.card_debit");
    assert_eq!(label.requested, 3);
    assert_eq!(label.downloaded, 3);
    assert_eq!(label.messages, ["m1", "m2", "m3"]);
    assert!(label.warnings.is_empty());

    let unmanaged = report
        .labels
        .iter()
        .find(|label| label.parser_id == "unmanaged.one")
        .expect("unmapped label is still evidence");
    assert_eq!(unmanaged.downloaded, 3);

    for index in 1..=3 {
        let path = out
            .join("example.card_debit")
            .join(format!("{index:02}.eml"));
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("Total Payment"), "{}", path.display());
        assert!(text.contains("Rp 27.500"), "{}", path.display());
    }
    let manifest = std::fs::read_to_string(out.join("manifest.json")).unwrap();
    assert!(manifest.contains("\"downloaded\": 3"));
    assert!(manifest.contains("\"parser_id\": \"example.card_debit\""));
    assert!(manifest.contains("\"parser_id\": \"unmanaged.one\""));

    // Sampled mail is private.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let file = out.join("example.card_debit/01.eml");
        let dir_mode = std::fs::metadata(out.join("example.card_debit"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        let file_mode = std::fs::metadata(file).unwrap().permissions().mode() & 0o777;
        assert_eq!(dir_mode, 0o700);
        assert_eq!(file_mode, 0o600);
    }
}

#[test]
fn fewer_samples_is_a_warning_and_stale_files_are_removed() {
    let dir = tempfile::tempdir().unwrap();
    let server = MockServer::start();
    token_route(&server);
    raw_routes(&server, &["m1", "m2"]);
    server.route(
        "GET",
        "/gmail/v1/users/me/messages",
        vec![Response::json(r#"{"messages":[{"id":"m1"},{"id":"m2"}]}"#)],
    );
    server.route(
        "GET",
        "/gmail/v1/users/me/labels",
        vec![Response::json(
            r#"{"labels":[{"id":"Label_1","name":"receipt/example.card_debit"}]}"#,
        )],
    );

    let config = config(dir.path());
    let fetcher = fetcher(&server, dir.path());
    let out = dir.path().join("samples");
    let stale = out.join("example.card_debit/03.eml");
    std::fs::create_dir_all(stale.parent().unwrap()).unwrap();
    std::fs::write(&stale, "old sample").unwrap();

    let report = samples::download(
        &config,
        &fetcher,
        &Options {
            out: Some(out.clone()),
            samples: 3,
            label: None,
        },
    )
    .unwrap();

    let label = &report.labels[0];
    assert_eq!(label.downloaded, 2);
    assert_eq!(label.warnings.len(), 1);
    assert!(
        label.warnings[0].contains("wanted 3"),
        "{:?}",
        label.warnings
    );

    assert!(!stale.exists(), "the stale third sample must be removed");
    assert!(out.join("example.card_debit/01.eml").exists());
    assert!(out.join("example.card_debit/02.eml").exists());
}
