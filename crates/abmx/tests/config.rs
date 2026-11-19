use std::path::Path;

use abmx::config::Config;
use abmx::paths;
use abmx_gmail::{Credentials, TokenStore};

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
prefix = "receipt/"
since = 2026-11-10

[labels.accounts]
"example.card_debit" = "uuid-example"
"example.qris_payment" = "uuid-wallet"
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

    assert_eq!(config.labels.prefix, "receipt/");
    assert_eq!(
        config.labels.since,
        chrono::NaiveDate::from_ymd_opt(2026, 11, 10)
    );

    let accounts = config.account_map();
    assert_eq!(accounts.get("example.card_debit"), Some("uuid-example"));
    assert_eq!(accounts.get("example.qris_payment"), Some("uuid-wallet"));
    assert_eq!(accounts.len(), 2);

    assert_eq!(
        config.labels.parser_id("receipt/example.card_debit"),
        Some("example.card_debit".to_owned())
    );
    // The prefix is case-insensitive; the parser id is normalised to lower case.
    assert_eq!(
        config.labels.parser_id("Receipt/Example.CARD_DEBIT"),
        Some("example.card_debit".to_owned())
    );
    assert_eq!(config.labels.parser_id("Receipt"), None);
    assert_eq!(config.labels.parser_id("receipt/a/b"), None);
    // Dots are conventional, not required: the gmailctl labels here use
    // `example-qris` as well as `example.card_debit`.
    assert_eq!(
        config.labels.parser_id("receipt/example-qris"),
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
prefix = "receipt/"

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
fn an_empty_account_id_is_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("abmx.toml");
    std::fs::write(
        &path,
        r#"
[labels]
prefix = "receipt/"

[labels.accounts]
"example.card_debit" = ""
"#,
    )
    .unwrap();

    let config = Config::load(&path).unwrap();
    let error = config.validate_labels().unwrap_err().to_string();
    assert!(error.contains("empty account id"), "got: {error}");
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

#[test]
fn the_starter_template_parses_and_has_no_placeholders() {
    let mut config: Config = toml::from_str(abmx::config::INIT_TEMPLATE).unwrap();
    config.validate_labels().unwrap();
    assert!(config.fetch.gmail.is_some());

    // Point the token at a path that cannot exist, so the ambient user token
    // cannot satisfy the credentials check.
    config.fetch.gmail.as_mut().unwrap().token = Some(std::path::PathBuf::from(
        "definitely-missing-credentials.json",
    ));
    assert!(config.gmail_credentials(Path::new(".")).is_err());
    assert!(config.build_sink(Path::new(".")).is_err());
}

#[test]
fn ignore_entries_must_be_parser_ids_and_must_not_be_managed() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("abmx.toml");
    std::fs::write(
        &path,
        r#"
[labels]
prefix = "receipt/"
ignore = ["merchant.promo"]

[labels.accounts]
"example.card_debit" = "uuid"
"#,
    )
    .unwrap();
    Config::load(&path).unwrap().validate_labels().unwrap();

    std::fs::write(
        &path,
        r#"
[labels]
prefix = "receipt/"
ignore = ["example.card_debit"]

[labels.accounts]
"example.card_debit" = "uuid"
"#,
    )
    .unwrap();
    let error = Config::load(&path)
        .unwrap()
        .validate_labels()
        .unwrap_err()
        .to_string();
    assert!(error.contains("both mapped"), "got: {error}");

    std::fs::write(
        &path,
        r#"
[labels]
prefix = "receipt/"
ignore = ["a/b"]
"#,
    )
    .unwrap();
    let error = Config::load(&path)
        .unwrap()
        .validate_labels()
        .unwrap_err()
        .to_string();
    assert!(error.contains("not a parser id"), "got: {error}");
}

#[test]
fn gmail_credentials_come_from_the_oauth_client_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("abmx.toml");
    std::fs::write(
        &path,
        r#"
[fetch.gmail]
credentials_file = "gmail-client.json"
"#,
    )
    .unwrap();
    std::fs::write(
        dir.path().join("gmail-client.json"),
        r#"{"installed":{"client_id":"id.apps.googleusercontent.com","client_secret":"secret"}}"#,
    )
    .unwrap();

    let config = Config::load(&path).unwrap();
    let credentials = config.gmail_credentials(dir.path()).unwrap();
    assert_eq!(credentials.client_id, "id.apps.googleusercontent.com");
    assert_eq!(
        credentials.client_secret.as_deref(),
        Some(expected_client_secret("secret").as_str())
    );
}

#[test]
fn an_explicit_gmail_client_id_wins_over_the_oauth_client_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("abmx.toml");
    std::fs::write(
        &path,
        r#"
[fetch.gmail]
client_id = "explicit.apps.googleusercontent.com"
credentials_file = "gmail-client.json"
"#,
    )
    .unwrap();
    std::fs::write(
        dir.path().join("gmail-client.json"),
        r#"{"installed":{"client_id":"file.apps.googleusercontent.com","client_secret":"secret"}}"#,
    )
    .unwrap();

    let config = Config::load(&path).unwrap();
    let credentials = config.gmail_credentials(dir.path()).unwrap();
    assert_eq!(credentials.client_id, "explicit.apps.googleusercontent.com");
    // The secret still comes from the file when the config has no secret,
    // unless the test environment exports one (it wins by design).
    assert_eq!(
        credentials.client_secret.as_deref(),
        Some(expected_client_secret("secret").as_str())
    );
}

/// The test shell may already export a Gmail secret (direnv loads `.env`).
fn expected_client_secret(from_file: &str) -> String {
    std::env::var("ABMX_GMAIL_CLIENT_SECRET")
        .ok()
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| from_file.to_owned())
}

#[test]
fn an_exported_token_bundle_supplies_the_client_id_and_secret() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    std::fs::write(
        &path,
        r#"
[fetch.gmail]
token = "gmail-credentials.json"

[labels]
prefix = "receipt/"
"#,
    )
    .unwrap();

    TokenStore::new(dir.path().join("gmail-credentials.json"))
        .save(&Credentials {
            client_id: "bundle.apps.googleusercontent.com".into(),
            refresh_token: "refresh".into(),
            scopes: vec![],
            client_secret: Some("bundle-secret".into()),
        })
        .unwrap();

    let config = Config::load(&path).unwrap();
    let credentials = config.gmail_credentials(dir.path()).unwrap();
    assert_eq!(credentials.client_id, "bundle.apps.googleusercontent.com");
    assert_eq!(
        credentials.client_secret.as_deref(),
        Some(expected_client_secret("bundle-secret").as_str())
    );
}

#[test]
fn a_malformed_oauth_client_file_is_reported_with_its_path() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("abmx.toml");
    std::fs::write(
        &path,
        r#"
[fetch.gmail]
credentials_file = "gmail-client.json"
"#,
    )
    .unwrap();
    std::fs::write(dir.path().join("gmail-client.json"), "{}").unwrap();

    let config = Config::load(&path).unwrap();
    let error = format!("{:#}", config.gmail_credentials(dir.path()).unwrap_err());
    assert!(error.contains("gmail-client.json"), "got: {error}");
    assert!(error.contains("no client_id"), "got: {error}");
}
