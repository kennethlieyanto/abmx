//! The whole path, with only the two third-party services faked:
//! Gmail over HTTP → `abmx-parser` (real library, real spec) → policy → Actual over HTTP.

use std::path::PathBuf;

use abmx::state::State;
use abmx::Runner;
use abmx_actual::{ActualConfig, ActualSink, NotesConfig};
use abmx_core::{AccountMap, ApplyOptions, FetchRequest, PolicyConfig};
use abmx_gmail::auth::{Credentials, TokenStore};
use abmx_gmail::{GmailConfig, GmailFetcher};
use abmx_testkit::{MockServer, Response};
use base64::Engine;

const PARSER: &str = r#"
spec_version = 1
version = 1

[match]
senders = ["alerts@bank.example"]
subject = '^Payment successful$'

[labels]
list = ["Amount", "Merchant", "Transaction Date", "Status"]

[defaults]
bank = "test"
account = "test-account"
currency = "IDR"
tz = "Asia/Jakarta"
kind = "purchase"

[status]
source = "field"
label = "Status"
map = { Successful = "settled", Failed = "failed" }

[extract.amount]
label = "Amount"
regex = '(?P<value>[0-9][0-9.,]*)'
number = "id"

[extract.occurred_at]
label = "Transaction Date"
regex = '(?P<value>\d{2} \w{3} \d{4} \d{2}:\d{2}:\d{2})'
formats = ["%d %b %Y %H:%M:%S"]

[extract.counterparty]
label = "Merchant"
"#;

const EMAIL: &str = "From: alerts@bank.example\r\n\
To: me@example.invalid\r\n\
Subject: Payment successful\r\n\
Date: Thu, 30 Oct 2026 17:28:17 +0700\r\n\
Message-ID: <e2e-1@example.invalid>\r\n\
MIME-Version: 1.0\r\n\
Content-Type: text/html; charset=utf-8\r\n\
\r\n\
<html><body>\
<div>Amount</div><div>Rp 27.500</div>\
<div>Merchant</div><div>ExamplePay</div>\
<div>Transaction Date</div><div>30 Oct 2026 17:28:17</div>\
<div>Status</div><div>Successful</div>\
</body></html>\r\n";

fn corpus(dir: &std::path::Path) -> PathBuf {
    let parsers = dir.join("parsers");
    let parser = parsers.join("test.bank");
    std::fs::create_dir_all(&parser).unwrap();
    std::fs::write(parser.join("config.toml"), PARSER).unwrap();
    parsers
}

fn gmail_fetcher(server: &MockServer, dir: &std::path::Path) -> GmailFetcher {
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
    config.query = "from:alerts@bank.example".into();
    GmailFetcher::new(config).unwrap()
}

#[test]
fn gmail_to_parser_to_actual() {
    let dir = tempfile::tempdir().unwrap();
    let parsers = corpus(dir.path());

    let gmail = MockServer::start();
    gmail.route(
        "POST",
        "/token",
        vec![Response::json(
            r#"{"access_token":"access","expires_in":3600}"#,
        )],
    );
    let raw = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(EMAIL);
    gmail.route(
        "GET",
        "/gmail/v1/users/me/messages/m1",
        vec![Response::json(format!(r#"{{"id":"m1","raw":"{raw}"}}"#))],
    );
    gmail.route(
        "GET",
        "/gmail/v1/users/me/messages",
        vec![Response::json(r#"{"messages":[{"id":"m1"}]}"#)],
    );
    gmail.route(
        "GET",
        "/gmail/v1/users/me/profile",
        vec![Response::json(r#"{"historyId":"1000"}"#)],
    );

    let actual = MockServer::start();
    actual.route(
        "POST",
        "/v1/budgets/sync-id/accounts/acc-1/transactions/import",
        vec![Response::json(r#"{"data":{"added":["t1"],"updated":[]}}"#)],
    );

    let (parser, _) = abmx_parser::Parser::load(&abmx_parser::Options {
        parsers: vec![parsers],
        router: None,
    })
    .unwrap();

    let mut accounts = AccountMap::new();
    accounts.insert("test-account", "acc-1");

    let runner = Runner::new(
        parser,
        PolicyConfig::default(),
        accounts,
        State::open_in_memory().unwrap(),
    );

    let fetcher = gmail_fetcher(&gmail, dir.path());
    let sink = ActualSink::new(
        ActualConfig::new(actual.url(), "key", "sync-id"),
        NotesConfig::default(),
    )
    .unwrap();

    let plan = runner.plan(&fetcher, FetchRequest::new(None)).unwrap();
    assert_eq!(plan.stats.fetched, 1);
    assert_eq!(plan.stats.import, 1);

    let report = runner
        .apply(&sink, &plan, &ApplyOptions::default())
        .unwrap();
    assert_eq!(report.added, 1);
    assert!(report.is_clean());

    let request = actual
        .requests()
        .into_iter()
        .find(|request| request.path_only().contains("transactions/import"))
        .expect("the sink called Actual");

    let body = request.json();
    let transaction = &body["transactions"][0];
    assert_eq!(transaction["amount"], -2_750_000);
    assert_eq!(transaction["imported_id"], plan.rows[0].id);
    assert_eq!(transaction["payee_name"], "ExamplePay");
    assert_eq!(transaction["date"], "2026-10-30");

    // A second run must be a no-op: the Gmail cursor advanced, and the message
    // is recorded as delivered.
    gmail.route(
        "GET",
        "/gmail/v1/users/me/history",
        vec![Response::json(r#"{"historyId":"1001","history":[]}"#)],
    );

    let second = runner.plan(&fetcher, FetchRequest::new(None)).unwrap();
    assert_eq!(second.stats.parsed, 0);
    assert!(second.import_rows().is_empty());

    let history = gmail
        .requests()
        .into_iter()
        .find(|request| request.path_only() == "/gmail/v1/users/me/history")
        .expect("the second run used history, not a full scan");
    assert!(history.query.contains("startHistoryId=1000"));
}
