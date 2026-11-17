use std::path::PathBuf;

use abmx::fetchers::DirFetcher;
use abmx::state::State;
use abmx::Runner;
use abmx_core::{AccountMap, ApplyOptions, BrokenReason, FetchRequest};
use abmx_file::FileSink;

const PREFIX: &str = "Receipt/abmx:";
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

fn accounts(account: &str) -> AccountMap {
    let mut accounts = AccountMap::new();
    accounts.insert("test.bank", account);
    accounts
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
        self.runner_with(accounts("uuid-test"))
    }

    fn runner_with(&self, accounts: AccountMap) -> Runner {
        Runner::new(
            self.parser(),
            PREFIX,
            accounts,
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
fn a_failed_transaction_is_broken_not_imported() {
    let fixture = Fixture::new(&[("one", "Rp 15.000", "Failed")]);
    let runner = fixture.runner();
    let fetcher = fixture.fetcher();

    let plan = runner.plan(&fetcher, FetchRequest::new(&labels())).unwrap();
    assert_eq!(plan.stats.broken, 1);
    assert!(plan.import_rows().is_empty());
    assert_eq!(plan.broken[0].reason, BrokenReason::BankDeclaredFailed);
    assert_eq!(plan.broken[0].label, LABEL);
}

#[test]
fn a_zero_amount_is_broken() {
    let fixture = Fixture::new(&[("one", "Rp 0", "Successful")]);
    let runner = fixture.runner();
    let fetcher = fixture.fetcher();

    let plan = runner.plan(&fetcher, FetchRequest::new(&labels())).unwrap();
    assert_eq!(plan.stats.broken, 1);
    assert!(plan.import_rows().is_empty());
    assert_eq!(plan.broken[0].reason, BrokenReason::ZeroAmount);
}

#[test]
fn an_unmapped_parser_is_broken() {
    let fixture = Fixture::new(&[("one", "Rp 27.500", "Successful")]);
    let runner = fixture.runner_with(AccountMap::new());
    let fetcher = fixture.fetcher();

    let plan = runner.plan(&fetcher, FetchRequest::new(&labels())).unwrap();
    assert_eq!(plan.stats.broken, 1);
    assert!(plan.import_rows().is_empty());
    assert_eq!(
        plan.broken[0].reason,
        BrokenReason::UnmappedAccount {
            parser: "test.bank".into()
        }
    );
}

#[test]
fn a_label_without_a_parser_id_is_unmatched() {
    let fixture = Fixture::new(&[("one", "Rp 27.500", "Successful")]);
    let runner = fixture.runner();
    let fetcher = DirFetcher::labeled(fixture.mail_dir(), "Receipt/abmx:a/b");

    let plan = runner
        .plan(
            &fetcher,
            FetchRequest::new(&["Receipt/abmx:a/b".to_owned()]),
        )
        .unwrap();
    assert_eq!(plan.stats.unmatched, 1);
    assert!(plan.import_rows().is_empty());
}

#[test]
fn a_duplicate_email_is_written_once() {
    let fixture = Fixture::new(&[
        ("same", "Rp 27.500", "Successful"),
        ("same", "Rp 27.500", "Successful"),
    ]);
    let runner = fixture.runner();
    let fetcher = fixture.fetcher();
    let sink = FileSink::new(fixture.out_path());

    let plan = runner.plan(&fetcher, FetchRequest::new(&labels())).unwrap();
    assert_eq!(plan.stats.import, 1);

    let report = runner
        .apply(&sink, &plan, &ApplyOptions::default())
        .unwrap();
    assert_eq!(report.added, 1);
    let written = std::fs::read_to_string(fixture.out_path()).unwrap();
    assert_eq!(written.lines().count(), 1);
}

#[test]
fn apply_writes_no_broken_rows() {
    let fixture = Fixture::new(&[("one", "Rp 0", "Successful")]);
    let runner = fixture.runner();
    let fetcher = fixture.fetcher();
    let sink = FileSink::new(fixture.out_path());

    let plan = runner.plan(&fetcher, FetchRequest::new(&labels())).unwrap();
    let report = runner
        .apply(&sink, &plan, &ApplyOptions::default())
        .unwrap();
    assert_eq!(report.added, 0);
    assert!(report.is_clean());
    assert!(
        !fixture.out_path().exists()
            || std::fs::read_to_string(fixture.out_path())
                .unwrap()
                .is_empty()
    );
}

#[test]
fn a_label_that_does_not_exist_in_the_sink_is_still_planned() {
    // The sink is responsible for rejecting an unknown account id; the runner
    // must not silently drop a mapped row.
    let fixture = Fixture::new(&[("one", "Rp 27.500", "Successful")]);
    let runner = fixture.runner_with(accounts("not-a-real-account"));
    let fetcher = fixture.fetcher();
    let plan = runner.plan(&fetcher, FetchRequest::new(&labels())).unwrap();
    assert_eq!(plan.stats.import, 1);
    assert_eq!(plan.import_rows()[0].account, "not-a-real-account");
}
