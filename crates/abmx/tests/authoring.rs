use std::path::{Path, PathBuf};

use abmx::authoring;

const PARSER: &str = r#"
fn meta() {
    #{
        bank: "test",
        currency: "IDR",
        tz: "Asia/Jakarta",
        offset: "+07:00",
        number: "id",
        kind: "purchase",
        labels: ["Amount", "Merchant", "Transaction Date", "Status"],
        api: 1,
        version: 1,
    }
}

fn parse_mail(ctx) {
    let status = ctx.text_in("Status");
    #{
        amount: ctx.money_regex_in("Amount", `\d[0-9.,]*`, "id"),
        occurred_at: ctx.datetime_in("Transaction Date", ["%d %b %Y %H:%M:%S"], "en"),
        counterparty: ctx.text_in("Merchant"),
        status: if status == "Successful" { "settled" }
                else if status == "Failed" { "failed" }
                else { "unknown" },
    }
}
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
    let spec = dir.path().join("candidate.rhai");
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
    let spec = dir.path().join("candidate.rhai");
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
