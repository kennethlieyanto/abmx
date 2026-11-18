//! The whole path, with the third-party boundaries faked: Gmail over HTTP and
//! the Actual helper over its JSON protocol.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use abmx::state::State;
use abmx::Runner;
use abmx_actual::{ActualConfig, ActualSink, Helper, NotesConfig};
use abmx_core::{AccountMap, ApplyOptions, Error, FetchRequest};
use abmx_gmail::auth::{Credentials, TokenStore};
use abmx_gmail::{GmailConfig, GmailFetcher};
use abmx_testkit::{MockServer, Response};
use base64::Engine;
use serde_json::{json, Value};

const LABEL: &str = "Receipt/abmx:test.bank";

const PARSER: &str = r#"
spec_version = 2
version = 1

[labels]
list = ["Amount", "Merchant", "Transaction Date", "Status"]

[defaults]
bank = "test"
currency = "IDR"
tz = "Asia/Jakarta"
kind = "purchase"

[status]
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
    config.prefix = "Receipt/abmx:".to_owned();
    GmailFetcher::new(config).unwrap()
}

fn accounts() -> AccountMap {
    let mut accounts = AccountMap::new();
    accounts.insert("test.bank", "acc-1");
    accounts
}

/// Records the request abmx would hand to the Node helper. `import` reports one
/// added row, matching what Actual's reconciling import answers.
#[derive(Clone, Default)]
struct FakeActual {
    calls: Arc<Mutex<Vec<(String, Value)>>>,
}

impl FakeActual {
    fn calls(&self) -> Vec<(String, Value)> {
        self.calls.lock().unwrap().clone()
    }
}

impl Helper for FakeActual {
    fn call(&self, method: &str, params: Value) -> Result<Value, Error> {
        self.calls.lock().unwrap().push((method.to_owned(), params));
        match method {
            "import" => Ok(json!({ "added": 1, "updated": 0 })),
            other => Err(Error::Sink(format!("unexpected helper call: {other}"))),
        }
    }
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
    gmail.route(
        "GET",
        "/gmail/v1/users/me/labels",
        vec![Response::json(
            r#"{"labels":[{"id":"Label_1","name":"Receipt/abmx:test.bank"}]}"#,
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

    let actual = FakeActual::default();

    let (parser, _) = abmx_parser::Parser::load(&abmx_parser::Options {
        parsers: vec![parsers],
    })
    .unwrap();

    let runner = Runner::new(
        parser,
        "Receipt/abmx:",
        accounts(),
        State::open_in_memory().unwrap(),
    );

    let fetcher = gmail_fetcher(&gmail, dir.path());
    let sink = ActualSink::with_helper(
        ActualConfig::new("http://actual.invalid", "key", "sync-id"),
        NotesConfig::default(),
        Box::new(actual.clone()),
    );

    let labels = vec![LABEL.to_owned()];
    let plan = runner.plan(&fetcher, FetchRequest::new(&labels)).unwrap();
    assert_eq!(plan.stats.fetched, 1);
    assert_eq!(plan.stats.import, 1);

    let report = runner
        .apply(&sink, &plan, &ApplyOptions::default())
        .unwrap();
    assert_eq!(report.added, 1);
    assert!(report.is_clean());

    let request = actual
        .calls()
        .into_iter()
        .find(|(method, _)| method == "import")
        .expect("the sink called Actual")
        .1;

    let transaction = &request["transactions"][0];
    assert_eq!(transaction["amount"], -2_750_000);
    assert_eq!(transaction["imported_id"], plan.rows[0].id);
    assert_eq!(transaction["payee_name"], "ExamplePay");
    assert_eq!(transaction["date"], "2026-10-30");

    // A second run must be a no-op: the message is already recorded as
    // delivered, even though the label is scanned again.
    let second = runner.plan(&fetcher, FetchRequest::new(&labels)).unwrap();
    assert_eq!(second.stats.fetched, 0);
    assert_eq!(second.stats.parsed, 0);
    assert_eq!(second.stats.skipped, 1);
    assert!(second.import_rows().is_empty());
}
