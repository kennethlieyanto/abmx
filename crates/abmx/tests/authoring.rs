use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use abmx::authoring;

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
fn prompt_contains_evidence_reference_and_id() {
    let dir = tempfile::tempdir().unwrap();
    let mail = dir.path().join("mail");
    write_sample(&mail, "01.eml", "one", "Rp 27.500", "Successful");
    write_sample(&mail, "02.eml", "two", "Rp 15.000", "Successful");

    let set = authoring::samples::load_path(&mail, 8).unwrap();
    assert_eq!(set.sample_count(), 2);
    assert_eq!(set.groups.len(), 1);

    let context = authoring::PromptContext {
        config_path: None,
        corpus: dir.path().join("parsers"),
        draft_dir: dir.path().join("drafts"),
        input: Some(mail.clone()),
        explicit_id: Some("test.bank".into()),
        id_from: authoring::IdFrom::Infer,
        from_gmail: false,
        summaries: Vec::new(),
        accounts: BTreeMap::new(),
        parser: None,
    };

    let prompt = authoring::render_prompt(&set, &context);
    assert!(prompt.contains("## Samples"));
    assert!(prompt.contains("test.bank"));
    assert!(prompt.contains("blocks (8):"));
    assert!(prompt.contains("| idx | kind | distinct values |"));
    assert!(prompt.contains("spec_version = 1"));
    assert!(prompt.contains("abmx verify parser"));
    assert!(prompt.contains("Do not write into"));
}

#[test]
fn prompt_groups_subdirectories_like_gmail_labels() {
    let dir = tempfile::tempdir().unwrap();
    let mail = dir.path().join("mail");
    write_sample(
        &mail.join("example"),
        "01.eml",
        "one",
        "Rp 27.500",
        "Successful",
    );
    write_sample(
        &mail.join("wallet"),
        "02.eml",
        "two",
        "Rp 15.000",
        "Successful",
    );

    let set = authoring::samples::load_path(&mail, 8).unwrap();
    assert_eq!(set.groups.len(), 2);
    let labels: Vec<&str> = set
        .groups
        .iter()
        .filter_map(|group| group.label.as_deref())
        .collect();
    assert_eq!(labels, vec!["example", "wallet"]);
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

    let other = dir.path().join("other");
    std::fs::create_dir_all(&other).unwrap();
    std::fs::write(
        other.join("01.eml"),
        email("three", "Rp 1.000", "Successful")
            .replace("alerts@bank.example", "other@example.invalid")
            .replace("Subject: Payment successful", "Subject: Something else"),
    )
    .unwrap();
    let report = authoring::verify::verify(&spec, Some("test.bank"), Some(&other)).unwrap();
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
fn skills_install_and_refuse_to_clobber_edits() {
    let dir = tempfile::tempdir().unwrap();
    let written = authoring::skill::write_skills(dir.path(), false).unwrap();
    assert_eq!(written.len(), 5);

    let skill = dir.path().join("abmx-parser-authoring/SKILL.md");
    let text = std::fs::read_to_string(&skill).unwrap();
    assert!(text.starts_with("---\nname: abmx parser authoring"));
    assert!(text.contains("description:"));
    assert!(text.contains("spec-reference.md"));

    // Re-running is idempotent.
    authoring::skill::write_skills(dir.path(), false).unwrap();

    // A local edit is preserved unless --force is passed.
    std::fs::write(&skill, "edited").unwrap();
    assert!(authoring::skill::write_skills(dir.path(), false).is_err());
    authoring::skill::write_skills(dir.path(), true).unwrap();
    assert!(std::fs::read_to_string(&skill).unwrap().starts_with("---"));
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
