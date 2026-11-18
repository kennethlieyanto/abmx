use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use abmx_actual::{ActualConfig, ActualSink, ClearedPolicy, Helper, NotesConfig};
use abmx_core::{ApplyOptions, Error, ImportRow, Result, Sink};
use abmx_parser::{Extraction, Kind, Money, Source, Status};
use serde_json::{json, Value};

/// In-memory stand-in for the Node helper: records every call and answers from a
/// queue, so the tests assert the exact request the real helper would receive.
#[derive(Clone, Default)]
struct FakeHelper {
    calls: Arc<Mutex<Vec<(String, Value)>>>,
    replies: Arc<Mutex<VecDeque<std::result::Result<Value, String>>>>,
}

impl FakeHelper {
    fn new() -> Self {
        Self::default()
    }

    fn reply(&self, value: Value) {
        self.replies.lock().unwrap().push_back(Ok(value));
    }

    fn fail(&self, message: &str) {
        self.replies
            .lock()
            .unwrap()
            .push_back(Err(message.to_owned()));
    }

    fn calls(&self) -> Vec<(String, Value)> {
        self.calls.lock().unwrap().clone()
    }

    fn import_params(&self) -> Value {
        self.calls()
            .into_iter()
            .find(|(method, _)| method == "import")
            .expect("the sink called import")
            .1
    }

    fn count(&self, method: &str) -> usize {
        self.calls()
            .iter()
            .filter(|(called, _)| called == method)
            .count()
    }
}

impl Helper for FakeHelper {
    fn call(&self, method: &str, params: Value) -> Result<Value> {
        self.calls.lock().unwrap().push((method.to_owned(), params));
        self.replies
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or_else(|| Err(format!("unexpected call to `{method}`")))
            .map_err(Error::Sink)
    }
}

fn transaction(id: &str, amount_minor: i64) -> abmx_core::Transaction {
    abmx_core::Transaction {
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

fn sink_with(helper: &FakeHelper, cleared: ClearedPolicy) -> ActualSink {
    let mut config = ActualConfig::new("http://actual.invalid", "secret-key", "sync-id");
    config.cleared = cleared;
    ActualSink::with_helper(config, NotesConfig::default(), Box::new(helper.clone()))
}

fn sink(helper: &FakeHelper) -> ActualSink {
    sink_with(helper, ClearedPolicy::Never)
}

#[test]
fn dry_run_previews_the_configured_notes() {
    let mut item = row("one", -1000);
    item.transaction.description = Some("QRIS payment".into());

    let helper = FakeHelper::new();
    let actual = sink(&helper);
    assert_eq!(
        actual.preview_notes(&item.transaction).as_deref(),
        Some("QRIS payment")
    );

    let no_notes = ActualSink::with_helper(
        ActualConfig::new("http://actual.invalid", "secret-key", "sync-id"),
        NotesConfig { fields: Vec::new() },
        Box::new(helper),
    );
    assert_eq!(no_notes.preview_notes(&item.transaction), None);
}

#[test]
fn accounts_are_read_from_the_helper_with_booleans() {
    let helper = FakeHelper::new();
    helper.reply(json!([
        {"id":"acc-1","name":"Example Account","offbudget":false,"closed":false},
        {"id":"acc-2","name":"Savings","offbudget":true,"closed":true}
    ]));

    let accounts = sink(&helper).accounts().unwrap();
    assert_eq!(accounts.len(), 2);
    assert_eq!(accounts[0].name, "Example Account");
    assert!(accounts[1].off_budget);
    assert!(accounts[1].closed);

    let calls = helper.calls();
    assert_eq!(calls[0].0, "accounts");
    assert_eq!(calls[0].1["budgetSyncId"], "sync-id");
}

#[test]
fn the_budget_password_is_passed_to_the_helper() {
    let helper = FakeHelper::new();
    helper.reply(json!([]));

    let mut config = ActualConfig::new("http://actual.invalid", "secret-key", "sync-id");
    config.budget_password = Some("hunter2".into());
    let sink = ActualSink::with_helper(config, NotesConfig::default(), Box::new(helper.clone()));

    sink.accounts().unwrap();
    assert_eq!(helper.calls()[0].1["budgetPassword"], "hunter2");
}

#[test]
fn import_sends_cents_and_the_dedupe_key() {
    let helper = FakeHelper::new();
    helper.reply(json!({"added":1,"updated":0}));

    let report = sink(&helper)
        .apply(&[row("mail_1", -27_500)], &ApplyOptions::default())
        .unwrap();

    assert_eq!(report.added, 1);
    assert!(report.is_clean());

    let params = helper.import_params();
    assert_eq!(params["account"], "acc-1");
    assert_eq!(params["options"]["reimportDeleted"], true);
    assert_eq!(params["options"]["payeeNameNormalization"], "original");
    // The default policy leaves rows uncleared.
    assert_eq!(params["options"]["defaultCleared"], false);
    assert_eq!(params["options"]["dryRun"], false);

    let transaction = &params["transactions"][0];
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
    let helper = FakeHelper::new();
    helper.reply(json!({"added":1,"updated":0}));

    sink(&helper)
        .apply(&[row("mail_1", -27_500)], &ApplyOptions::default())
        .unwrap();

    assert_eq!(helper.import_params()["options"]["reimportDeleted"], true);
}

#[test]
fn reimport_deleted_can_be_turned_off() {
    let helper = FakeHelper::new();
    helper.reply(json!({"added":1,"updated":0}));

    let mut config = ActualConfig::new("http://actual.invalid", "secret-key", "sync-id");
    config.reimport_deleted = false;
    let sink = ActualSink::with_helper(config, NotesConfig::default(), Box::new(helper.clone()));

    sink.apply(&[row("mail_1", -27_500)], &ApplyOptions::default())
        .unwrap();

    assert_eq!(helper.import_params()["options"]["reimportDeleted"], false);
}

#[test]
fn the_settled_policy_marks_rows_cleared() {
    let helper = FakeHelper::new();
    helper.reply(json!({"added":1,"updated":0}));

    let report = sink_with(&helper, ClearedPolicy::Settled)
        .apply(&[row("mail_1", -27_500)], &ApplyOptions::default())
        .unwrap();

    assert_eq!(report.added, 1);
    assert_eq!(helper.import_params()["transactions"][0]["cleared"], true);
}

#[test]
fn the_always_policy_also_sets_default_cleared() {
    let helper = FakeHelper::new();
    helper.reply(json!({"added":1,"updated":0}));

    sink_with(&helper, ClearedPolicy::Always)
        .apply(&[row("mail_1", -27_500)], &ApplyOptions::default())
        .unwrap();

    let params = helper.import_params();
    assert_eq!(params["options"]["defaultCleared"], true);
    assert_eq!(params["transactions"][0]["cleared"], true);
}

#[test]
fn a_failing_chunk_is_bisected_so_one_bad_row_does_not_lose_the_rest() {
    let helper = FakeHelper::new();
    helper.fail("bad row");
    helper.reply(json!({"added":1,"updated":0}));
    helper.fail("still bad");

    let report = sink(&helper)
        .apply(
            &[row("mail_good", -1000), row("mail_bad", -2000)],
            &ApplyOptions::default(),
        )
        .unwrap();

    assert_eq!(report.added, 1);
    assert_eq!(report.errors.len(), 1);
    assert_eq!(report.errors[0].tx_id.as_deref(), Some("mail_bad"));
    assert_eq!(helper.count("import"), 3);
}

#[test]
fn failed_transactions_are_never_sent() {
    let helper = FakeHelper::new();

    let mut bad = row("mail_failed", -1000);
    bad.transaction.status = Status::Failed;

    let report = sink(&helper)
        .apply(&[bad], &ApplyOptions::default())
        .unwrap();

    assert_eq!(report.added, 0);
    assert_eq!(report.skipped.len(), 1);
    assert_eq!(helper.count("import"), 0);
}

#[test]
fn dry_run_is_forwarded_rather_than_emulated() {
    let helper = FakeHelper::new();
    helper.reply(json!({"added":0,"updated":0}));

    let report = sink(&helper)
        .apply(
            &[row("mail_1", -1000)],
            &ApplyOptions {
                dry_run: true,
                ..Default::default()
            },
        )
        .unwrap();

    assert!(report.dry_run);
    assert_eq!(helper.import_params()["options"]["dryRun"], true);
}

#[test]
fn snapshot_maps_the_helper_rows_to_ledger_rows() {
    let helper = FakeHelper::new();
    helper.reply(json!([
        {
            "account": "acc-1",
            "date": "2026-10-30",
            "amount": -2_750_000,
            "imported_id": "mail_1",
            "imported_payee": "ExamplePay"
        },
        {
            "account": "acc-1",
            "date": "not-a-date",
            "amount": -1,
            "imported_id": "mail_2"
        }
    ]));

    let rows = sink(&helper)
        .snapshot(
            &["acc-1".to_owned(), "acc-2".to_owned()],
            chrono::NaiveDate::from_ymd_opt(2026, 10, 1).unwrap(),
            chrono::NaiveDate::from_ymd_opt(2026, 10, 31).unwrap(),
        )
        .unwrap();

    assert_eq!(rows.len(), 1, "rows with an invalid date are skipped");
    assert_eq!(rows[0].account, "acc-1");
    assert_eq!(rows[0].amount_minor, -2_750_000);
    assert_eq!(rows[0].imported_id.as_deref(), Some("mail_1"));
    assert_eq!(rows[0].payee.as_deref(), Some("ExamplePay"));

    let params = &helper.calls()[0].1;
    assert_eq!(params["accounts"], json!(["acc-1", "acc-2"]));
    assert_eq!(params["since"], "2026-10-01");
    assert_eq!(params["until"], "2026-10-31");
}

#[test]
fn a_helper_error_is_a_clear_error() {
    let helper = FakeHelper::new();
    helper.fail("invalid api key (check the Actual API key)");

    let error = sink(&helper).accounts().unwrap_err();
    let message = error.to_string();
    assert!(message.contains("invalid api key"), "{message}");
    assert!(message.contains("check the Actual API key"), "{message}");
}

#[test]
fn a_malformed_helper_response_names_the_offender() {
    let helper = FakeHelper::new();
    helper.reply(json!({"unexpected": true}));

    let error = sink(&helper).accounts().unwrap_err();
    assert!(matches!(error, Error::Sink(_)));
    assert!(error.to_string().contains("not a list"));
}
