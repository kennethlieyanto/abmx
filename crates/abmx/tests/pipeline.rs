use std::cell::Cell;
use std::path::PathBuf;

use abmx::fetchers::DirFetcher;
use abmx::state::State;
use abmx::Runner;
use abmx_core::{
    ApplyOptions, Error, FetchBatch, FetchRequest, Fetcher, ImportRow, LabelRoute, LabelRoutes,
    PolicyConfig, RawMessage, Report, Result, Sink, SinkAccount,
};
use abmx_file::FileSink;

const LABEL: &str = "abmx:test";

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

fn routes(account: &str) -> LabelRoutes {
    let mut routes = LabelRoutes::default();
    routes.insert(
        LABEL,
        LabelRoute {
            parser: "test.bank".to_owned(),
            account: account.to_owned(),
        },
    );
    routes
}

fn labels() -> Vec<String> {
    vec![LABEL.to_owned()]
}

struct Fixture {
    dir: tempfile::TempDir,
}

impl Fixture {
    fn new(emails: &[(&str, &str, &str)]) -> Self {
        let dir = tempfile::tempdir().unwrap();

        let parsers = dir.path().join("parsers");
        let parser = parsers.join("test.bank");
        std::fs::create_dir_all(&parser).unwrap();
        std::fs::write(parser.join("config.toml"), PARSER).unwrap();

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

    fn fetcher(&self) -> DirFetcher {
        DirFetcher::labeled(self.mail_dir(), LABEL)
    }

    fn parser(&self) -> abmx_parser::Parser {
        let (parser, _) = abmx_parser::Parser::load(&abmx_parser::Options {
            parsers: vec![self.dir.path().join("parsers")],
        })
        .unwrap();
        parser
    }

    fn runner(&self) -> Runner {
        Runner::new(
            self.parser(),
            PolicyConfig::default(),
            routes("uuid-test"),
            State::open_in_memory().unwrap(),
        )
    }
}

#[test]
fn imports_a_row_once_and_then_skips_the_message() {
    let fixture = Fixture::new(&[("one", "Rp 27.500", "Successful")]);
    let runner = fixture.runner();
    let fetcher = fixture.fetcher();
    let sink = FileSink::new(fixture.out_path());

    let plan = runner.plan(&fetcher, FetchRequest::new(&labels())).unwrap();
    assert_eq!(plan.stats.import, 1);
    assert_eq!(plan.stats.parsed, 1);
    assert_eq!(plan.import_rows()[0].account, "uuid-test");

    let report = runner
        .apply(&sink, &plan, &ApplyOptions::default())
        .unwrap();
    assert_eq!(report.added, 1);
    assert!(report.is_clean());

    let second = runner.plan(&fetcher, FetchRequest::new(&labels())).unwrap();
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
    let fetcher = fixture.fetcher();

    let plan = runner.plan(&fetcher, FetchRequest::new(&labels())).unwrap();
    assert_eq!(plan.stats.skip, 1);
    assert!(plan.import_rows().is_empty());
    assert!(matches!(
        plan.rows[0].decision,
        abmx_core::Decision::Skip(_)
    ));
}

#[test]
fn a_zero_amount_lands_in_review_and_can_be_approved() {
    let fixture = Fixture::new(&[("one", "Rp 0", "Successful")]);
    let runner = fixture.runner();
    let fetcher = fixture.fetcher();
    let sink = FileSink::new(fixture.out_path());

    let plan = runner.plan(&fetcher, FetchRequest::new(&labels())).unwrap();
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
fn an_unmapped_label_is_reviewed_rather_than_guessed() {
    let fixture = Fixture::new(&[("one", "Rp 27.500", "Successful")]);
    let runner = Runner::new(
        fixture.parser(),
        PolicyConfig::default(),
        routes(""),
        State::open_in_memory().unwrap(),
    );

    let plan = runner
        .plan(&fixture.fetcher(), FetchRequest::new(&labels()))
        .unwrap();

    assert_eq!(plan.stats.review, 1);
    assert!(plan.rows[0].account.is_none());
    assert!(matches!(
        plan.rows[0].decision,
        abmx_core::Decision::Review(abmx_core::ReviewReason::UnmappedAccount { .. })
    ));
}

#[test]
fn a_plan_survives_a_json_round_trip() {
    let fixture = Fixture::new(&[("one", "Rp 27.500", "Successful")]);
    let runner = fixture.runner();
    let sink = FileSink::new(fixture.out_path());

    let plan = runner
        .plan(&fixture.fetcher(), FetchRequest::new(&labels()))
        .unwrap();

    let json = serde_json::to_string(&plan).unwrap();
    let restored: abmx_core::Plan = serde_json::from_str(&json).unwrap();
    assert_eq!(restored.rows.len(), plan.rows.len());

    let report = runner
        .apply(&sink, &restored, &ApplyOptions::default())
        .unwrap();
    assert_eq!(report.added, 1);
}

// ------------------------------------------------------------- fake pieces

struct OnceFetcher {
    message: RawMessage,
    calls: Cell<usize>,
}

impl OnceFetcher {
    fn new() -> Self {
        Self {
            message: RawMessage::new(
                "fake:1",
                LABEL,
                "fake:1",
                email("one", "Rp 27.500", "Successful").into_bytes(),
            ),
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
            skipped: 0,
        })
    }
}

/// Returns its message once, then nothing — the shape of a fetcher that has no
/// more mail to deliver.
struct OnceOnlyFetcher {
    message: RawMessage,
    used: Cell<bool>,
}

impl OnceOnlyFetcher {
    fn new() -> Self {
        Self {
            message: RawMessage::new(
                "once:1",
                LABEL,
                "once:1",
                email("one", "Rp 27.500", "Successful").into_bytes(),
            ),
            used: Cell::new(false),
        }
    }
}

impl Fetcher for OnceOnlyFetcher {
    fn name(&self) -> &str {
        "once"
    }

    fn fetch(&self, _request: FetchRequest<'_>) -> Result<FetchBatch> {
        let already_used = self.used.replace(true);
        Ok(FetchBatch {
            messages: if already_used {
                Vec::new()
            } else {
                vec![self.message.clone()]
            },
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
fn a_message_under_an_unconfigured_label_is_reported() {
    let fixture = Fixture::new(&[("one", "Rp 27.500", "Successful")]);
    let runner = fixture.runner();
    let fetcher = DirFetcher::labeled(fixture.mail_dir(), "abmx:missing");

    let plan = runner.plan(&fetcher, FetchRequest::new(&labels())).unwrap();

    assert_eq!(plan.stats.unmatched, 1);
    let entry = &plan.unmatched[0];
    assert_eq!(entry.code.as_deref(), Some("unmapped_label"));
    assert!(entry.reason.contains("no [labels] entry"));
    assert_eq!(
        entry.date,
        Some(chrono::NaiveDate::from_ymd_opt(2026, 10, 30).unwrap())
    );
}

#[test]
fn fixing_the_mapping_reimports_queued_rows_without_a_reset() {
    let fixture = Fixture::new(&[("one", "Rp 27.500", "Successful")]);
    let fetcher = OnceOnlyFetcher::new();
    let sink = FileSink::new(fixture.out_path());

    let mut runner = Runner::new(
        fixture.parser(),
        PolicyConfig::default(),
        routes(""),
        State::open_in_memory().unwrap(),
    );

    // First run: no mapping, so the row is queued.
    let plan = runner.plan(&fetcher, FetchRequest::new(&labels())).unwrap();
    assert_eq!(plan.stats.review, 1);
    assert_eq!(plan.stats.import, 0);
    runner
        .apply(&sink, &plan, &ApplyOptions::default())
        .unwrap();
    assert_eq!(runner.state.review().unwrap().len(), 1);

    // Fix the mapping. The fetcher now returns nothing, so only the stored
    // review row can be re-decided.
    runner.labels = routes("uuid-test");

    let plan = runner.plan(&fetcher, FetchRequest::new(&labels())).unwrap();
    assert_eq!(plan.stats.fetched, 0);
    assert_eq!(plan.stats.recovered, 1);
    assert_eq!(plan.stats.import, 1);
    assert_eq!(plan.stats.review, 0);

    runner
        .apply(&sink, &plan, &ApplyOptions::default())
        .unwrap();
    assert!(runner.state.review().unwrap().is_empty());
    assert_eq!(
        std::fs::read_to_string(fixture.out_path())
            .unwrap()
            .lines()
            .count(),
        1
    );
}

#[test]
fn diff_sees_rows_the_file_sink_already_has() {
    let fixture = Fixture::new(&[("one", "Rp 27.500", "Successful")]);
    let runner = fixture.runner();
    let fetcher = fixture.fetcher();
    let sink = FileSink::new(fixture.out_path());

    let plan = runner.plan(&fetcher, FetchRequest::new(&labels())).unwrap();
    runner
        .apply(&sink, &plan, &ApplyOptions::default())
        .unwrap();

    let rows = plan.import_rows();
    let date = rows[0].transaction.occurred_on;
    let ledger = sink
        .snapshot(&[rows[0].account.clone()], date, date)
        .unwrap();

    let decision = abmx_core::classify(&rows[0], &ledger);
    assert!(!decision.is_new());
    assert!(decision.is_same_id(&rows[0].tx_id));
    assert!(!decision.is_foreign());
}

#[test]
fn a_failed_apply_does_not_mark_the_message_seen() {
    let fixture = Fixture::new(&[("one", "Rp 27.500", "Successful")]);
    let runner = fixture.runner();
    let fetcher = OnceFetcher::new();

    let plan = runner.plan(&fetcher, FetchRequest::new(&labels())).unwrap();

    let error = runner
        .apply(&FailingSink::new(true), &plan, &ApplyOptions::default())
        .unwrap_err();
    assert!(error.to_string().contains("boom"));
    assert!(!runner.state.is_seen("fake:1").unwrap());

    let report = runner
        .apply(&FailingSink::new(false), &plan, &ApplyOptions::default())
        .unwrap();
    assert_eq!(report.added, 1);
}

#[test]
fn a_partial_failure_only_records_the_rows_that_succeeded() {
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
                    .map(|row| abmx_core::RowError {
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

    let plan = runner.plan(&fetcher, FetchRequest::new(&labels())).unwrap();
    let report = runner
        .apply(&PartialSink, &plan, &ApplyOptions::default())
        .unwrap();

    assert_eq!(report.errors.len(), 1);
    // The one that failed is not recorded; a later run retries it.
    assert!(!runner.state.is_seen("fake:1").unwrap());
}
