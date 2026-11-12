use std::cell::Cell;
use std::path::PathBuf;

use txsync::fetchers::DirFetcher;
use txsync::state::State;
use txsync::Runner;
use txsync_core::{
    AccountMap, ApplyOptions, Error, FetchBatch, FetchRequest, Fetcher, ImportRow, PolicyConfig,
    RawMessage, Report, Result, Sink, SinkAccount,
};
use txsync_file::FileSink;

const PARSER: &str = r#"
spec_version = 1
id = "test.bank"
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

fn email(message_id: &str, amount: &str, status: &str) -> String {
    format!(
        "From: alerts@bank.example\r\n\
         To: me@example.invalid\r\n\
         Subject: Payment successful\r\n\
         Date: Thu, 30 Oct 2026 17:28:17 +0700\r\n\
         Message-ID: <{message_id}@example.invalid>\r\n\
         MIME-Version: 1.0\r\n\
         Content-Type: text/html; charset=utf-8\r\n\
         \r\n\
         <html><body>\
         <div>Amount</div><div>{amount}</div>\
         <div>Merchant</div><div>ExamplePay</div>\
         <div>Transaction Date</div><div>30 Oct 2026 17:28:17</div>\
         <div>Status</div><div>{status}</div>\
         </body></html>\r\n"
    )
}

struct Fixture {
    dir: tempfile::TempDir,
}

impl Fixture {
    fn new(emails: &[(&str, &str, &str)]) -> Self {
        let dir = tempfile::tempdir().unwrap();

        let parsers = dir.path().join("parsers");
        std::fs::create_dir_all(&parsers).unwrap();
        std::fs::write(parsers.join("test.bank.toml"), PARSER).unwrap();

        let mail = dir.path().join("mail");
        std::fs::create_dir_all(&mail).unwrap();
        for (index, (message_id, amount, status)) in emails.iter().enumerate() {
            std::fs::write(
                mail.join(format!("{index:02}.eml")),
                email(message_id, amount, status),
            )
            .unwrap();
        }

        Self { dir }
    }

    fn mail_dir(&self) -> PathBuf {
        self.dir.path().join("mail")
    }

    fn out_path(&self) -> PathBuf {
        self.dir.path().join("out.jsonl")
    }

    fn runner(&self) -> Runner {
        let (parser, _) = txp::Parser::load(&txp::Options {
            parsers: vec![self.dir.path().join("parsers")],
            router: None,
        })
        .unwrap();

        let mut accounts = AccountMap::new();
        accounts.insert("test-account", "uuid-test");

        Runner::new(
            parser,
            PolicyConfig::default(),
            accounts,
            State::open_in_memory().unwrap(),
        )
    }
}

#[test]
fn imports_a_row_once_and_then_skips_the_message() {
    let fixture = Fixture::new(&[("one", "Rp 27.500", "Successful")]);
    let runner = fixture.runner();
    let fetcher = DirFetcher::new(fixture.mail_dir());
    let sink = FileSink::new(fixture.out_path());

    let plan = runner.plan(&fetcher, FetchRequest::new(None)).unwrap();
    assert_eq!(plan.stats.import, 1);
    assert_eq!(plan.stats.parsed, 1);
    assert_eq!(plan.import_rows()[0].account, "uuid-test");

    let report = runner
        .apply(&sink, &plan, &ApplyOptions::default())
        .unwrap();
    assert_eq!(report.added, 1);
    assert!(report.is_clean());

    let second = runner.plan(&fetcher, FetchRequest::new(None)).unwrap();
    assert_eq!(second.stats.parsed, 0);
    assert_eq!(second.stats.import, 0);
    assert_eq!(second.sources[0].skipped, 1);

    let written = std::fs::read_to_string(fixture.out_path()).unwrap();
    assert_eq!(written.lines().count(), 1);
    assert!(written.contains("-27500"));
}

#[test]
fn a_failed_transaction_is_skipped_not_imported() {
    let fixture = Fixture::new(&[("one", "Rp 15.000", "Failed")]);
    let runner = fixture.runner();
    let fetcher = DirFetcher::new(fixture.mail_dir());

    let plan = runner.plan(&fetcher, FetchRequest::new(None)).unwrap();
    assert_eq!(plan.stats.skip, 1);
    assert!(plan.import_rows().is_empty());
    assert!(matches!(
        plan.rows[0].decision,
        txsync_core::Decision::Skip(_)
    ));
}

#[test]
fn a_zero_amount_lands_in_review_and_can_be_approved() {
    let fixture = Fixture::new(&[("one", "Rp 0", "Successful")]);
    let runner = fixture.runner();
    let fetcher = DirFetcher::new(fixture.mail_dir());
    let sink = FileSink::new(fixture.out_path());

    let plan = runner.plan(&fetcher, FetchRequest::new(None)).unwrap();
    assert_eq!(plan.stats.review, 1);
    assert!(plan.import_rows().is_empty());

    let report = runner
        .apply(&sink, &plan, &ApplyOptions::default())
        .unwrap();
    assert_eq!(report.added, 0);

    let entries = runner.state.review().unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].reason, "zero_amount");
    assert_eq!(entries[0].message, "dir:00.eml");

    let id = entries[0].tx_id.clone();
    let report = runner
        .approve(&sink, std::slice::from_ref(&id), &ApplyOptions::default())
        .unwrap();
    assert_eq!(report.added, 1);
    assert!(runner.state.review().unwrap().is_empty());
    assert!(runner.state.is_seen("dir:00.eml").unwrap());
}

#[test]
fn an_unmapped_account_is_reviewed_rather_than_guessed() {
    let fixture = Fixture::new(&[("one", "Rp 27.500", "Successful")]);
    let (parser, _) = txp::Parser::load(&txp::Options {
        parsers: vec![fixture.dir.path().join("parsers")],
        router: None,
    })
    .unwrap();

    // Deliberately empty mapping.
    let runner = Runner::new(
        parser,
        PolicyConfig::default(),
        AccountMap::new(),
        State::open_in_memory().unwrap(),
    );

    let plan = runner
        .plan(
            &DirFetcher::new(fixture.mail_dir()),
            FetchRequest::new(None),
        )
        .unwrap();

    assert_eq!(plan.stats.review, 1);
    assert!(plan.rows[0].account.is_none());
    assert!(matches!(
        plan.rows[0].decision,
        txsync_core::Decision::Review(txsync_core::ReviewReason::UnmappedAccount { .. })
    ));
}

#[test]
fn a_plan_survives_a_json_round_trip() {
    let fixture = Fixture::new(&[("one", "Rp 27.500", "Successful")]);
    let runner = fixture.runner();
    let sink = FileSink::new(fixture.out_path());

    let plan = runner
        .plan(
            &DirFetcher::new(fixture.mail_dir()),
            FetchRequest::new(None),
        )
        .unwrap();

    let json = serde_json::to_string(&plan).unwrap();
    let restored: txsync_core::Plan = serde_json::from_str(&json).unwrap();
    assert_eq!(restored.rows.len(), plan.rows.len());

    let report = runner
        .apply(&sink, &restored, &ApplyOptions::default())
        .unwrap();
    assert_eq!(report.added, 1);
}

// ------------------------------------------------------------- fake pieces

struct OnceFetcher {
    message: RawMessage,
    cursor: serde_json::Value,
    calls: Cell<usize>,
}

impl OnceFetcher {
    fn new() -> Self {
        Self {
            message: RawMessage::new(
                "fake:1",
                "fake:1",
                email("one", "Rp 27.500", "Successful").into_bytes(),
            ),
            cursor: serde_json::json!({ "history_id": "42" }),
            calls: Cell::new(0),
        }
    }
}

impl Fetcher for OnceFetcher {
    fn name(&self) -> &str {
        "fake"
    }

    fn fetch(&self, _request: FetchRequest<'_>) -> Result<FetchBatch> {
        self.calls.set(self.calls.get() + 1);
        Ok(FetchBatch {
            messages: vec![self.message.clone()],
            next_cursor: Some(self.cursor.clone()),
            skipped: 0,
        })
    }
}

struct FailingSink {
    fail: Cell<bool>,
}

impl FailingSink {
    fn new(fail: bool) -> Self {
        Self {
            fail: Cell::new(fail),
        }
    }
}

impl Sink for FailingSink {
    fn name(&self) -> &str {
        "failing"
    }

    fn accounts(&self) -> Result<Vec<SinkAccount>> {
        Ok(Vec::new())
    }

    fn apply(&self, rows: &[ImportRow], _options: &ApplyOptions) -> Result<Report> {
        if self.fail.get() {
            return Err(Error::Sink("boom".into()));
        }
        Ok(Report {
            added: rows.len(),
            ..Report::default()
        })
    }
}

#[test]
fn diff_sees_rows_the_file_sink_already_has() {
    let fixture = Fixture::new(&[("one", "Rp 27.500", "Successful")]);
    let runner = fixture.runner();
    let fetcher = DirFetcher::new(fixture.mail_dir());
    let sink = FileSink::new(fixture.out_path());

    let plan = runner.plan(&fetcher, FetchRequest::new(None)).unwrap();
    runner
        .apply(&sink, &plan, &ApplyOptions::default())
        .unwrap();

    let rows = plan.import_rows();
    let date = rows[0].transaction.occurred_on;
    let ledger = sink
        .snapshot(&[rows[0].account.clone()], date, date)
        .unwrap();

    let decision = txsync_core::classify(&rows[0], &ledger);
    assert!(!decision.is_new());
    assert!(decision.is_same_id(&rows[0].tx_id));
    assert!(!decision.is_foreign());
}

#[test]
fn a_failed_apply_does_not_advance_the_cursor() {
    let fixture = Fixture::new(&[("one", "Rp 27.500", "Successful")]);
    let runner = fixture.runner();
    let fetcher = OnceFetcher::new();

    let plan = runner.plan(&fetcher, FetchRequest::new(None)).unwrap();

    let error = runner
        .apply(&FailingSink::new(true), &plan, &ApplyOptions::default())
        .unwrap_err();
    assert!(error.to_string().contains("boom"));

    // Nothing was delivered, so the checkpoint must not move.
    assert!(runner.state.cursor("fake").unwrap().is_none());
    assert!(!runner.state.is_seen("fake:1").unwrap());

    let report = runner
        .apply(&FailingSink::new(false), &plan, &ApplyOptions::default())
        .unwrap();
    assert_eq!(report.added, 1);

    assert_eq!(
        runner.state.cursor("fake").unwrap().unwrap(),
        serde_json::json!({ "history_id": "42" })
    );
}

#[test]
fn a_partial_failure_keeps_the_cursor_so_rows_can_be_retried() {
    struct PartialSink;

    impl Sink for PartialSink {
        fn name(&self) -> &str {
            "partial"
        }

        fn accounts(&self) -> Result<Vec<SinkAccount>> {
            Ok(Vec::new())
        }

        fn apply(&self, rows: &[ImportRow], _options: &ApplyOptions) -> Result<Report> {
            Ok(Report {
                added: rows.len().saturating_sub(1),
                errors: rows
                    .last()
                    .map(|row| txsync_core::RowError {
                        tx_id: Some(row.tx_id.clone()),
                        message: "rejected".into(),
                    })
                    .into_iter()
                    .collect(),
                ..Report::default()
            })
        }
    }

    let fixture = Fixture::new(&[("one", "Rp 27.500", "Successful")]);
    let runner = fixture.runner();
    let fetcher = OnceFetcher::new();

    let plan = runner.plan(&fetcher, FetchRequest::new(None)).unwrap();
    let report = runner
        .apply(&PartialSink, &plan, &ApplyOptions::default())
        .unwrap();

    assert_eq!(report.errors.len(), 1);
    assert!(runner.state.cursor("fake").unwrap().is_none());
    // The one that succeeded is recorded; the failure is not.
    assert!(!runner.state.is_seen("fake:1").unwrap());
}
