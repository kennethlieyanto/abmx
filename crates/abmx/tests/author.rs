//! The Gmail scan behind `abmx author`: metadata pass, one representative
//! fetch, and the candidate prompt. Read-only; no state, no ledger.

use std::collections::BTreeMap;
use std::path::Path;

use abmx::authoring::author::{self, AuthorContext, GroupOutcome};
use abmx_gmail::auth::{Credentials, TokenStore};
use abmx_gmail::{GmailConfig, GmailFetcher};
use abmx_testkit::{MockServer, Response};
use base64::Engine;

const EMAIL: &str = "From: example <example@example.invalid>\r\n\
To: me@example.invalid\r\n\
Subject: Transaksi Online - Debit example\r\n\
Date: Thu, 30 Oct 2026 17:28:17 +0700\r\n\
Message-ID: <scan-1@example.invalid>\r\n\
MIME-Version: 1.0\r\n\
Content-Type: text/html; charset=utf-8\r\n\
\r\n\
<html><body>\
<div>Merchant</div><div>ExamplePay</div>\
<div>Amount</div><div>Rp 27.500</div>\
<div>Transaction Date</div><div>30 Oct 2026 17:28:17</div>\
</body></html>\r\n";

fn fetcher(server: &MockServer, dir: &Path) -> GmailFetcher {
    let token_path = dir.join("credentials.json");
    TokenStore::new(&token_path)
        .save(&Credentials {
            client_id: "client".into(),
            refresh_token: "refresh".into(),
            scopes: vec![],
        })
        .unwrap();

    let mut config = GmailConfig::new("client", &token_path);
    config.base_url = format!("{}/gmail/v1", server.url());
    config.token_url = format!("{}/token", server.url());
    GmailFetcher::new(config).unwrap()
}

#[test]
fn scan_groups_by_sender_and_writes_one_sample() {
    let dir = tempfile::tempdir().unwrap();
    let server = MockServer::start();

    server.route(
        "POST",
        "/token",
        vec![Response::json(
            r#"{"access_token":"access","expires_in":3600}"#,
        )],
    );
    // The most specific route must be registered first: matching is a prefix.
    server.route_query(
        "GET",
        "/gmail/v1/users/me/messages/m1",
        "format=metadata",
        vec![Response::json(
            r#"{"id":"m1","payload":{"headers":[
                {"name":"From","value":"example <example@example.invalid>"},
                {"name":"Subject","value":"Transaksi Online - Debit example"},
                {"name":"Date","value":"Thu, 30 Oct 2026 17:28:17 +0700"}
            ]}}"#,
        )],
    );
    let raw = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(EMAIL);
    server.route_query(
        "GET",
        "/gmail/v1/users/me/messages/m1",
        "format=raw",
        vec![Response::json(format!(r#"{{"id":"m1","raw":"{raw}"}}"#))],
    );
    server.route(
        "GET",
        "/gmail/v1/users/me/messages",
        vec![Response::json(r#"{"messages":[{"id":"m1"}]}"#)],
    );

    let fetcher = fetcher(&server, dir.path());
    let corpus = dir.path().join("corpus");
    let options = author::Options {
        days: 30,
        max_messages: 10,
        max_groups: 5,
        ..author::Options::default()
    };
    let accounts = BTreeMap::new();
    let context = AuthorContext {
        config_path: None,
        corpus: &corpus,
        work_dir: dir.path(),
        accounts: &accounts,
        summaries: &[],
    };

    let scan = author::scan_gmail(&fetcher, None, &options, &context).unwrap();

    assert_eq!(scan.messages, 1);
    assert_eq!(scan.groups.len(), 1);

    let group = &scan.groups[0];
    assert_eq!(group.sender, "example@example.invalid");
    assert_eq!(group.count, 1);
    assert_eq!(group.outcome, GroupOutcome::Candidate);
    assert!(group.sample.as_ref().expect("sample written").is_file());

    let prompt = author::render(&context, &scan);
    assert!(prompt.contains("example@example.invalid"));
    assert!(prompt.contains("Rp 27.500"));
    assert!(prompt.contains("Merchant order receipts"));
    assert!(prompt.contains(&corpus.display().to_string()));
}
