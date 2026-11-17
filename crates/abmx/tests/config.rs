use std::path::Path;

use abmx::config::Config;
use abmx::paths;

const CONFIG: &str = r#"
[general]
parsers = ["parsers", "/etc/abmx/parsers"]
state = "var/state.db"

[fetch.gmail]
client_id = "id.apps.googleusercontent.com"
max_messages = 50

[sink.file]
path = "out/ledger.jsonl"

[labels]
prefix = "Receipt/abmx:"
since = 2026-11-10

[labels.accounts]
"example.card_debit" = "uuid-example"
"wallet.qris_payment" = "uuid-wallet"
"#;

fn write(dir: &Path) -> std::path::PathBuf {
    let path = dir.join("abmx.toml");
    std::fs::write(&path, CONFIG).unwrap();
    path
}

#[test]
fn relative_paths_resolve_against_the_config_directory() {
    let dir = tempfile::tempdir().unwrap();
    let path = write(dir.path());

    let config = Config::load(&path).unwrap();
    let base = Config::base_dir(&path);

    assert_eq!(base, dir.path());
    assert_eq!(config.state_path(&base), dir.path().join("var/state.db"));

    let options = config.parser_options(&base);
    assert_eq!(options.parsers[0], dir.path().join("parsers"));
    assert_eq!(
        options.parsers[1],
        std::path::PathBuf::from("/etc/abmx/parsers")
    );
}

#[test]
fn labels_round_trip_and_encode_the_parser_id() {
    let dir = tempfile::tempdir().unwrap();
    let path = write(dir.path());
    let config = Config::load(&path).unwrap();

    assert_eq!(config.labels.prefix, "Receipt/abmx:");
    assert_eq!(
        config.labels.since,
        chrono::NaiveDate::from_ymd_opt(2026, 11, 10)
    );

    let accounts = config.account_map();
    assert_eq!(accounts.get("example.card_debit"), Some("uuid-example"));
    assert_eq!(accounts.get("wallet.qris_payment"), Some("uuid-wallet"));
    assert_eq!(accounts.len(), 2);

    assert_eq!(
        config.labels.parser_id("Receipt/abmx:example.card_debit"),
        Some("example.card_debit".to_owned())
    );
    // The prefix is case-insensitive; the parser id is normalised to lower case.
    assert_eq!(
        config.labels.parser_id("receipt/ABMX:example.card_debit"),
        Some("example.card_debit".to_owned())
    );
    assert_eq!(config.labels.parser_id("Receipt"), None);
    assert_eq!(config.labels.parser_id("Receipt/abmx:a/b"), None);
    // Dots are conventional, not required: the gmailctl labels here use
    // `example-qris` as well as `example.card_debit`.
    assert_eq!(
        config.labels.parser_id("Receipt/abmx:example-qris"),
        Some("example-qris".to_owned())
    );

    config.validate_labels().unwrap();
}

#[test]
fn an_invalid_account_key_is_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("abmx.toml");
    std::fs::write(
        &path,
        r#"
[labels]
prefix = "Receipt/abmx:"

[labels.accounts]
"a/b" = "uuid"
"#,
    )
    .unwrap();

    let config = Config::load(&path).unwrap();
    let error = config.validate_labels().unwrap_err().to_string();
    assert!(error.contains("not a parser id"), "got: {error}");
}

#[test]
fn the_file_sink_is_built_when_actual_is_absent() {
    let dir = tempfile::tempdir().unwrap();
    let path = write(dir.path());
    let config = Config::load(&path).unwrap();
    let base = Config::base_dir(&path);

    let sink = config
        .build_sink(&base)
        .map_err(|error| error.to_string())
        .unwrap();
    assert_eq!(sink.name(), "file");
    assert!(!sink.idempotent());
}

#[test]
fn a_missing_sink_is_an_error() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("abmx.toml");
    std::fs::write(&path, "[general]\n").unwrap();

    let config = Config::load(&path).unwrap();
    let error = match config.build_sink(dir.path()) {
        Ok(_) => panic!("expected a configuration error"),
        Err(error) => error.to_string(),
    };
    assert!(error.to_string().contains("no sink configured"));
}

#[test]
fn secrets_prefer_a_literal_then_a_file() {
    // A name no one will have exported in their shell.
    let env = "ABMX_TEST_SECRET_NOT_SET";

    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("secret");
    std::fs::write(&file, "from-file\n").unwrap();

    assert_eq!(
        paths::resolve_secret(env, Some("literal"), Some(&file)).unwrap(),
        Some("literal".to_owned())
    );
    assert_eq!(
        paths::resolve_secret(env, None, Some(&file)).unwrap(),
        Some("from-file".to_owned())
    );
    assert_eq!(paths::resolve_secret(env, None, None).unwrap(), None);
}

#[test]
fn tilde_paths_expand() {
    let expanded = paths::expand_tilde(Path::new("~/state.db"));
    assert!(expanded.is_absolute());
    assert!(expanded.ends_with("state.db"));
}
