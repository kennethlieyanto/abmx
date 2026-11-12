use txp::{Extraction, Kind, Money, Source, Status};
use txsync_actual::{ActualConfig, ActualSink, ClearedPolicy, NotesConfig};
use txsync_core::{ApplyOptions, ImportRow, Sink};
use txsync_testkit::{MockServer, Response};

fn transaction(id: &str, amount_minor: i64) -> txsync_core::Transaction {
    txsync_core::Transaction {
        source: Source {
            message_id: Some(format!("{id}@example.invalid")),
            file: Some(format!("gmail:{id}")),
            tx_index: 0,
        },
        id: id.into(),
        parser: "example.card_debit".into(),
        parser_version: 1,
        bank: "example".into(),
        account: "example-account".into(),
        kind: Kind::Purchase,
        status: Status::Settled,
        amount: Money {
            amount_minor,
            currency: "IDR".into(),
        },
        currency_exponent: 0,
        amount_decimal: amount_minor.to_string(),
        occurred_at: chrono::DateTime::parse_from_rfc3339("2026-10-30T17:28:17+07:00").unwrap(),
        occurred_on: chrono::NaiveDate::from_ymd_opt(2026, 10, 30).unwrap(),
        timezone: "Asia/Jakarta".into(),
        counterparty: Some("ExamplePay".into()),
        counterparty_account: None,
        account_last4: Some("4821".into()),
        reference_id: Some("REF-1".into()),
        channel: Some("DEBIT ONLINE".into()),
        description: None,
        extra: Default::default(),
        extraction: Extraction::Strict,
        warnings: vec![],
    }
}

fn row(id: &str, amount_minor: i64) -> ImportRow {
    ImportRow {
        tx_id: id.into(),
        account: "acc-1".into(),
        transaction: transaction(id, amount_minor),
    }
}

fn sink_with(server: &MockServer, cleared: ClearedPolicy) -> ActualSink {
    let mut config = ActualConfig::new(server.url(), "secret-key", "sync-id");
    config.cleared = cleared;
    ActualSink::new(config, NotesConfig::default()).unwrap()
}

fn sink(server: &MockServer) -> ActualSink {
    sink_with(server, ClearedPolicy::Never)
}

#[test]
fn accounts_are_read_from_the_wrapper() {
    let server = MockServer::start();
    server.route(
        "GET",
        "/v1/budgets/sync-id/accounts",
        vec![Response::json(
            r#"{"data":[
                {"id":"acc-1","name":"Example Account","offbudget":false,"closed":false},
                {"id":"acc-2","name":"Savings","offbudget":true,"closed":false}
            ]}"#,
        )],
    );

    let accounts = sink(&server).accounts().unwrap();
    assert_eq!(accounts.len(), 2);
    assert_eq!(accounts[0].name, "Example Account");
    assert!(accounts[1].off_budget);

    let request = &server.requests()[0];
    assert_eq!(request.header("x-api-key"), Some("secret-key"));
}

#[test]
fn import_sends_cents_and_the_dedupe_key() {
    let server = MockServer::start();
    server.route(
        "POST",
        "/v1/budgets/sync-id/accounts/acc-1/transactions/import",
        vec![Response::json(r#"{"data":{"added":["t1"],"updated":[]}}"#)],
    );

    let report = sink(&server)
        .apply(&[row("mail_1", -27_500)], &ApplyOptions::default())
        .unwrap();

    assert_eq!(report.added, 1);
    assert!(report.is_clean());

    let request = &server.requests()[0];
    assert_eq!(request.header("x-api-key"), Some("secret-key"));

    let body = request.json();
    assert_eq!(body["reimportDeleted"], true);
    assert_eq!(body["payeeNameNormalization"], "original");
    // The default policy leaves rows uncleared.
    assert_eq!(body["defaultCleared"], false);
    assert_eq!(body["dryRun"], false);

    let transaction = &body["transactions"][0];
    // Rp 27,500 must reach Actual as 2,750,000.
    assert_eq!(transaction["amount"], -2_750_000);
    assert_eq!(transaction["imported_id"], "mail_1");
    assert_eq!(transaction["date"], "2026-10-30");
    assert_eq!(transaction["account"], "acc-1");
    assert_eq!(transaction["payee_name"], "ExamplePay");
    assert_eq!(transaction["cleared"], false);
}

#[test]
fn ledger_deletions_are_not_permanent_by_default() {
    let server = MockServer::start();
    server.route(
        "POST",
        "/v1/budgets/sync-id/accounts/acc-1/transactions/import",
        vec![Response::json(r#"{"data":{"added":["t1"],"updated":[]}}"#)],
    );

    sink(&server)
        .apply(&[row("mail_1", -27_500)], &ApplyOptions::default())
        .unwrap();

    assert_eq!(server.requests()[0].json()["reimportDeleted"], true);
}

#[test]
fn reimport_deleted_can_be_turned_off() {
    let server = MockServer::start();
    server.route(
        "POST",
        "/v1/budgets/sync-id/accounts/acc-1/transactions/import",
        vec![Response::json(r#"{"data":{"added":["t1"],"updated":[]}}"#)],
    );

    let mut config = ActualConfig::new(server.url(), "secret-key", "sync-id");
    config.reimport_deleted = false;
    let sink = ActualSink::new(config, NotesConfig::default()).unwrap();

    sink.apply(&[row("mail_1", -27_500)], &ApplyOptions::default())
        .unwrap();

    assert_eq!(server.requests()[0].json()["reimportDeleted"], false);
}

#[test]
fn the_settled_policy_marks_rows_cleared() {
    let server = MockServer::start();
    server.route(
        "POST",
        "/v1/budgets/sync-id/accounts/acc-1/transactions/import",
        vec![Response::json(r#"{"data":{"added":["t1"],"updated":[]}}"#)],
    );

    let report = sink_with(&server, ClearedPolicy::Settled)
        .apply(&[row("mail_1", -27_500)], &ApplyOptions::default())
        .unwrap();

    assert_eq!(report.added, 1);
    assert_eq!(
        server.requests()[0].json()["transactions"][0]["cleared"],
        true
    );
}

#[test]
fn the_always_policy_also_sets_default_cleared() {
    let server = MockServer::start();
    server.route(
        "POST",
        "/v1/budgets/sync-id/accounts/acc-1/transactions/import",
        vec![Response::json(r#"{"data":{"added":["t1"],"updated":[]}}"#)],
    );

    sink_with(&server, ClearedPolicy::Always)
        .apply(&[row("mail_1", -27_500)], &ApplyOptions::default())
        .unwrap();

    let body = server.requests()[0].json();
    assert_eq!(body["defaultCleared"], true);
    assert_eq!(body["transactions"][0]["cleared"], true);
}

#[test]
fn a_failing_chunk_is_bisected_so_one_bad_row_does_not_lose_the_rest() {
    let server = MockServer::start();
    server.route(
        "POST",
        "/v1/budgets/sync-id/accounts/acc-1/transactions/import",
        vec![
            Response::status(400, r#"{"error":"bad row"}"#),
            Response::json(r#"{"data":{"added":["t1"],"updated":[]}}"#),
            Response::status(400, r#"{"error":"still bad"}"#),
        ],
    );

    let report = sink(&server)
        .apply(
            &[row("mail_good", -1000), row("mail_bad", -2000)],
            &ApplyOptions::default(),
        )
        .unwrap();

    assert_eq!(report.added, 1);
    assert_eq!(report.errors.len(), 1);
    assert_eq!(report.errors[0].tx_id.as_deref(), Some("mail_bad"));
    assert_eq!(server.request_count(), 3);
}

#[test]
fn failed_transactions_are_never_sent() {
    let server = MockServer::start();

    let mut bad = row("mail_failed", -1000);
    bad.transaction.status = Status::Failed;

    let report = sink(&server)
        .apply(&[bad], &ApplyOptions::default())
        .unwrap();

    assert_eq!(report.added, 0);
    assert_eq!(report.skipped.len(), 1);
    assert_eq!(server.request_count(), 0);
}

#[test]
fn dry_run_is_forwarded_rather_than_emulated() {
    let server = MockServer::start();
    server.route(
        "POST",
        "/v1/budgets/sync-id/accounts/acc-1/transactions/import",
        vec![Response::json(r#"{"data":{"added":["t1"],"updated":[]}}"#)],
    );

    let report = sink(&server)
        .apply(
            &[row("mail_1", -1000)],
            &ApplyOptions {
                dry_run: true,
                ..Default::default()
            },
        )
        .unwrap();

    assert!(report.dry_run);
    assert_eq!(server.requests()[0].json()["dryRun"], true);
}

#[test]
fn a_rejected_api_key_is_a_clear_error() {
    let server = MockServer::start();
    server.route(
        "GET",
        "/v1/budgets/sync-id/accounts",
        vec![Response::status(401, r#"{"error":"invalid api key"}"#)],
    );

    let error = sink(&server).accounts().unwrap_err();
    assert!(error.to_string().contains("invalid api key"));
}
