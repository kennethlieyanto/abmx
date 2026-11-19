//! Live label inventory used by `abmx status --strict`. Read-only.

use std::path::Path;

use abmx::authoring::author::{self, LabelStatus};
use abmx::config::LabelsSection;
use abmx_gmail::auth::{Credentials, TokenStore};
use abmx_gmail::{GmailConfig, GmailFetcher};
use abmx_testkit::{MockServer, Response};
use base64::Engine;

const EMAIL: &str = "From: Example <example@bank.example>\r\n\
To: me@example.invalid\r\n\
Subject: Internet Transaction Journal\r\n\
Date: Thu, 30 Oct 2026 17:28:17 +0700\r\n\
Message-ID: <scan-1@example.invalid>\r\n\
MIME-Version: 1.0\r\n\
Content-Type: text/html; charset=utf-8\r\n\
\r\n\
<html><body>\
<div>Total Payment</div><div>Rp 27.500</div>\
<div>Transaction Date</div><div>30 Oct 2026 17:28:17</div>\
<div>Payment to</div><div>ExamplePay</div>\
</body></html>\r\n";

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

#[test]
fn scan_takes_the_last_three_and_reports_missing_mapping() {
    let dir = tempfile::tempdir().unwrap();
    let server = MockServer::start();

    server.route(
        "POST",
        "/token",
        vec![Response::json(
            r#"{"access_token":"access","expires_in":3600}"#,
        )],
    );
    server.route(
        "GET",
        "/gmail/v1/users/me/labels",
        vec![Response::json(
            r#"{"labels":[{"id":"Label_1","name":"receipt/example-qris"}]}"#,
        )],
    );

    let raw = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(EMAIL);
    for id in ["m1", "m2", "m3"] {
        server.route(
            "GET",
            &format!("/gmail/v1/users/me/messages/{id}"),
            vec![Response::json(format!(r#"{{"id":"{id}","raw":"{raw}"}}"#))],
        );
    }
    server.route(
        "GET",
        "/gmail/v1/users/me/messages",
        vec![Response::json(
            r#"{"messages":[{"id":"m1"},{"id":"m2"},{"id":"m3"}]}"#,
        )],
    );

    let fetcher = fetcher(&server, dir.path());
    let options = author::Options::default();

    let labels = LabelsSection::default();
    let inventory = author::scan_gmail(&fetcher, None, &labels, &options, dir.path()).unwrap();

    assert_eq!(inventory.labels.len(), 1);
    let report = &inventory.labels[0];
    assert_eq!(report.label, "receipt/example-qris");
    assert_eq!(report.status, LabelStatus::MissingAccount);
    assert_eq!(report.samples.len(), 3);
    assert_eq!(report.messages, 3);
}

#[test]
fn an_ignored_label_is_not_sampled_and_needs_no_work() {
    let dir = tempfile::tempdir().unwrap();
    let server = MockServer::start();

    server.route(
        "POST",
        "/token",
        vec![Response::json(
            r#"{"access_token":"access","expires_in":3600}"#,
        )],
    );
    server.route(
        "GET",
        "/gmail/v1/users/me/labels",
        vec![Response::json(
            r#"{"labels":[{"id":"Label_1","name":"receipt/example-qris"}]}"#,
        )],
    );

    let fetcher = fetcher(&server, dir.path());
    let labels = LabelsSection {
        ignore: vec!["example-qris".to_owned()],
        ..LabelsSection::default()
    };
    let inventory = author::scan_gmail(
        &fetcher,
        None,
        &labels,
        &author::Options::default(),
        dir.path(),
    )
    .unwrap();

    assert_eq!(inventory.labels.len(), 1);
    assert_eq!(inventory.labels[0].status, LabelStatus::Ignored);
    assert_eq!(inventory.labels[0].messages, 0);
    assert_eq!(inventory.needs_work(), 0);
}

#[test]
fn a_configured_label_with_a_parser_is_ok() {
    let dir = tempfile::tempdir().unwrap();
    let server = MockServer::start();

    server.route(
        "POST",
        "/token",
        vec![Response::json(
            r#"{"access_token":"access","expires_in":3600}"#,
        )],
    );
    server.route(
        "GET",
        "/gmail/v1/users/me/labels",
        vec![Response::json(
            r#"{"labels":[{"id":"Label_1","name":"receipt/example-qris"}]}"#,
        )],
    );
    let raw = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(EMAIL);
    for id in ["m1", "m2", "m3"] {
        server.route(
            "GET",
            &format!("/gmail/v1/users/me/messages/{id}"),
            vec![Response::json(format!(r#"{{"id":"{id}","raw":"{raw}"}}"#))],
        );
    }
    server.route(
        "GET",
        "/gmail/v1/users/me/messages",
        vec![Response::json(
            r#"{"messages":[{"id":"m1"},{"id":"m2"},{"id":"m3"}]}"#,
        )],
    );

    let parsers = dir.path().join("parsers/example-qris");
    std::fs::create_dir_all(&parsers).unwrap();
    std::fs::write(
        parsers.join("config.toml"),
        r#"
spec_version = 2
version = 1
[labels]
list = ["Total Payment", "Transaction Date", "Payment to"]
[defaults]
bank = "example"
currency = "IDR"
tz = "Asia/Jakarta"
kind = "purchase"
[extract.amount]
label = "Total Payment"
regex = '(?P<value>[0-9][0-9.,]*)'
number = "id"
[extract.occurred_at]
label = "Transaction Date"
regex = '(?P<value>\d{2} \w{3} \d{4} \d{2}:\d{2}:\d{2})'
formats = ["%d %b %Y %H:%M:%S"]
[extract.counterparty]
label = "Payment to"
"#,
    )
    .unwrap();

    let (parser, _) = abmx_parser::Parser::load(&abmx_parser::Options {
        parsers: vec![dir.path().join("parsers")],
    })
    .unwrap();

    let mut labels = LabelsSection::default();
    labels
        .accounts
        .insert("example-qris".to_owned(), "uuid".to_owned());

    let fetcher = fetcher(&server, dir.path());
    let inventory = author::scan_gmail(
        &fetcher,
        Some(&parser),
        &labels,
        &author::Options::default(),
        dir.path(),
    )
    .unwrap();

    assert_eq!(inventory.labels[0].status, LabelStatus::Ok);
    assert_eq!(inventory.needs_work(), 0);
}
