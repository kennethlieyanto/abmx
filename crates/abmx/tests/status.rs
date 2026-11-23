//! `abmx status`: the offline health check and the merged labels section.

use std::path::{Path, PathBuf};
use std::process::Command;

fn config(dir: &Path) -> PathBuf {
    let parsers = dir.join("parsers");
    std::fs::create_dir_all(&parsers).unwrap();

    let path = dir.join("abmx.toml");
    std::fs::write(
        &path,
        format!(
            r#"
[general]
state = "state.db"
parsers = ["{}"]

[sink.file]
path = "ledger.jsonl"

[receipts]
prefix = "receipt/"
since = 2026-11-01
ignore = ["merchant.promo"]

[receipts.accounts]
"example.card_debit" = "uuid-example"

[statements]
since = 2026-11-01
ignore = ["example.portfolio_summary"]

[statements.accounts]
"example.consolidated_statement:credit_card" = "uuid-example"
"#,
            parsers.display()
        ),
    )
    .unwrap();
    path
}

fn run(config: &Path, args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_abmx"))
        .arg("--config")
        .arg(config)
        .args(args)
        .output()
        .unwrap()
}

#[test]
fn offline_status_merges_label_checks_and_omits_network_sections() {
    let dir = tempfile::tempdir().unwrap();
    let config = config(dir.path());

    let output = run(&config, &["status", "--offline"]);
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("receipt/example.card_debit -> uuid-example"),
        "{stdout}"
    );
    assert!(
        stdout.contains(
            "financial_statements/example.consolidated_statement [credit_card] -> uuid-example"
        ),
        "{stdout}"
    );
    assert!(stdout.contains("1 ignored label(s)"), "{stdout}");
    assert!(stdout.contains("1 ignored statement label(s)"), "{stdout}");
    assert!(
        stdout.contains("Gmail label inventory skipped (offline)"),
        "{stdout}"
    );
    assert!(!stdout.contains("account mapping"), "{stdout}");
    assert!(!stdout.contains("gmail labels"), "{stdout}");
}

#[test]
fn strict_is_no_longer_a_flag() {
    let dir = tempfile::tempdir().unwrap();
    let config = config(dir.path());

    let output = run(&config, &["status", "--strict"]);
    assert_eq!(output.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("unexpected argument"), "{stderr}");
}

#[test]
fn online_is_no_longer_a_flag() {
    let dir = tempfile::tempdir().unwrap();
    let config = config(dir.path());

    let output = run(&config, &["status", "--online"]);
    assert_eq!(output.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("unexpected argument"), "{stderr}");
}
