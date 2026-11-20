use std::path::{Path, PathBuf};

use abmx::authoring;

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

fn write_sample(dir: &Path, name: &str, message_id: &str, amount: &str, status: &str) -> PathBuf {
    std::fs::create_dir_all(dir).unwrap();
    let path = dir.join(name);
    std::fs::write(&path, email(message_id, amount, status)).unwrap();
    path
}

#[test]
fn verify_reports_strict_heuristic_and_unmatched() {
    let dir = tempfile::tempdir().unwrap();
    let spec = dir.path().join("candidate.toml");
    std::fs::write(&spec, PARSER).unwrap();

    let mail = dir.path().join("mail");
    write_sample(&mail, "01.eml", "one", "Rp 27.500", "Successful");

    let report = authoring::verify::verify(&spec, Some("test.bank"), Some(&mail)).unwrap();
    assert_eq!(report.strict(), 1);
    assert_eq!(report.exit_code(), 0);

    let zero = dir.path().join("zero");
    write_sample(&zero, "01.eml", "two", "Rp 0", "Successful");
    let report = authoring::verify::verify(&spec, Some("test.bank"), Some(&zero)).unwrap();
    assert_eq!(report.heuristic(), 1);
    assert_eq!(report.exit_code(), 3);

    let bad = dir.path().join("bad");
    write_sample(&bad, "01.eml", "three", "not an amount", "Successful");
    let report = authoring::verify::verify(&spec, Some("test.bank"), Some(&bad)).unwrap();
    assert_eq!(report.failures(), 1);
    assert_eq!(report.exit_code(), 2);
}

#[test]
fn verify_requires_a_parser_id() {
    let dir = tempfile::tempdir().unwrap();
    let spec = dir.path().join("candidate.toml");
    std::fs::write(&spec, PARSER).unwrap();
    let mail = dir.path().join("mail");
    write_sample(&mail, "01.eml", "one", "Rp 27.500", "Successful");

    // The temp directory name has no dot, so the id cannot be derived.
    let error = authoring::verify::verify(&spec, None, Some(&mail)).unwrap_err();
    assert!(error.to_string().contains("no parser id"), "got: {error}");
}

#[test]
fn the_authoring_skill_is_one_self_contained_document() {
    let text = authoring::skill::render_authoring().unwrap();
    assert!(text.starts_with("---\nname: abmx parser authoring"));
    assert!(text.contains("description:"));
    assert!(text.contains("abmx receipt samples"));
    assert!(text.contains("samples/<parser-id>"));
    assert!(text.contains("# bank parser spec reference"));
    assert!(text.contains("## Verifying a parser"));
    assert!(text.contains("# Parser authoring pitfalls"));
    assert!(text.contains("[receipts.accounts]"));
    assert!(
        !text.contains("references/"),
        "the skill must not link references"
    );
}

#[test]
fn dump_renders_text_and_json() {
    let dir = tempfile::tempdir().unwrap();
    let mail = dir.path().join("mail");
    write_sample(&mail, "01.eml", "one", "Rp 27.500", "Successful");

    let emails = authoring::dump::load(&mail).unwrap();
    let text = authoring::dump::render_text(&emails);
    assert!(text.contains("blocks (8):"));
    assert!(text.contains("Amount"));

    let json = authoring::dump::render_json(&emails).unwrap();
    let value: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!(value.as_array().unwrap().len(), 1);
    assert_eq!(value[0]["subject"], "Payment successful");
}
