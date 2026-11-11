use std::path::Path;

use txsync::config::Config;
use txsync::paths;

const CONFIG: &str = r#"
[general]
parsers = ["parsers", "/etc/txp/parsers"]
state = "var/state.db"

[fetch.gmail]
client_id = "id.apps.googleusercontent.com"
lookback_days = 3
max_messages = 50

[sink.file]
path = "out/ledger.jsonl"

[accounts]
"example-account" = "uuid-example"
"wallet-account" = "uuid-wallet"

[policy]
heuristic = "skip"
failed = "review"
"#;

fn write(dir: &Path) -> std::path::PathBuf {
    let path = dir.join("txsync.toml");
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

    let options = config.txp_options(&base);
    assert_eq!(options.parsers[0], dir.path().join("parsers"));
    assert_eq!(
        options.parsers[1],
        std::path::PathBuf::from("/etc/txp/parsers")
    );
}

#[test]
fn accounts_and_policy_round_trip() {
    let dir = tempfile::tempdir().unwrap();
    let path = write(dir.path());
    let config = Config::load(&path).unwrap();

    let accounts = config.account_map();
    assert_eq!(accounts.get("example-account"), Some("uuid-example"));
    assert_eq!(accounts.get("wallet-account"), Some("uuid-wallet"));
    assert_eq!(accounts.len(), 2);

    assert_eq!(config.policy.heuristic, txsync_core::HeuristicPolicy::Skip);
    assert_eq!(config.policy.failed, txsync_core::FailedPolicy::Review);
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
    let path = dir.path().join("txsync.toml");
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
    let env = "TXSYNC_TEST_SECRET_NOT_SET";

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
