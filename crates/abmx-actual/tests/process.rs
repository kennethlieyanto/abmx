use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::Duration;

use abmx_actual::helper::{Helper, ProcessHelper};
use abmx_actual::ActualConfig;
use serde_json::json;

fn script(dir: &Path, body: &str) -> PathBuf {
    let path = dir.join("fake-node.sh");
    std::fs::write(&path, body).unwrap();
    let mut permissions = std::fs::metadata(&path).unwrap().permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(&path, permissions).unwrap();
    path
}

fn config(dir: &Path, script_body: &str) -> ActualConfig {
    let mut config = ActualConfig::new("http://actual.invalid", "key", "sync-id");
    // The transport runs `<node> <helper>`; `/bin/sh` lets the fixture stand in
    // for Node without requiring it in CI.
    config.node = PathBuf::from("/bin/sh");
    config.helper = script(dir, script_body);
    config.data_dir = dir.join("data");
    config
}

#[test]
fn the_process_transport_reads_the_response_envelope() {
    let dir = tempfile::tempdir().unwrap();
    let config = config(
        dir.path(),
        "cat >/dev/null\nprintf '{\"ok\":true,\"data\":{\"api\":\"26.11.0\"}}\\n'\n",
    );
    let data = ProcessHelper::new(config)
        .call("version", json!({}))
        .unwrap();
    assert_eq!(data["api"], "26.11.0");
}

#[test]
fn an_error_envelope_becomes_a_sink_error() {
    let dir = tempfile::tempdir().unwrap();
    let config = config(
        dir.path(),
        "cat >/dev/null\nprintf '{\"ok\":false,\"error\":\"nope\",\"code\":\"invalid-password\"}\\n'\n",
    );
    let error = ProcessHelper::new(config)
        .call("accounts", json!({}))
        .unwrap_err()
        .to_string();
    assert!(error.contains("nope"), "{error}");
    assert!(error.contains("API key"), "{error}");
}

#[test]
fn a_silent_helper_is_a_clear_error() {
    let dir = tempfile::tempdir().unwrap();
    let config = config(dir.path(), "cat >/dev/null\nexit 3\n");
    let error = ProcessHelper::new(config)
        .call("accounts", json!({}))
        .unwrap_err()
        .to_string();
    assert!(error.contains("without a response"), "{error}");
}

#[test]
fn a_hung_helper_times_out_instead_of_blocking_forever() {
    let dir = tempfile::tempdir().unwrap();
    let mut config = config(dir.path(), "cat >/dev/null\nsleep 30\n");
    config.timeout = Duration::from_millis(150);

    let started = std::time::Instant::now();
    let error = ProcessHelper::new(config)
        .call("accounts", json!({}))
        .unwrap_err()
        .to_string();
    assert!(started.elapsed() < Duration::from_secs(5));
    assert!(error.contains("timed out"), "{error}");
}

#[test]
fn a_missing_node_binary_names_the_setting() {
    let dir = tempfile::tempdir().unwrap();
    let mut config = config(dir.path(), "true\n");
    config.node = dir.path().join("no-such-node");

    let error = ProcessHelper::new(config)
        .call("version", json!({}))
        .unwrap_err()
        .to_string();
    assert!(error.contains("could not run"), "{error}");
    assert!(error.contains("node"), "{error}");
}

/// Run manually after `npm ci` in `crates/abmx-actual/helper`:
/// `cargo test -p abmx-actual --test process -- --ignored`
#[test]
#[ignore = "requires npm ci in crates/abmx-actual/helper"]
fn the_bundled_helper_reports_its_api_version() {
    let mut config = ActualConfig::new("http://actual.invalid", "key", "sync-id");
    config.node = PathBuf::from("node");
    config.helper = Path::new(env!("CARGO_MANIFEST_DIR")).join("helper/helper.js");

    let data = ProcessHelper::new(config)
        .call("version", json!({}))
        .unwrap();
    assert!(
        data["api"].as_str().unwrap_or_default().starts_with("26."),
        "{data}"
    );
}
