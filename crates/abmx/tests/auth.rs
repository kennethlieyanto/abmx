//! `abmx auth export` / `abmx auth import`: the portable credential bundle.

use std::path::{Path, PathBuf};
use std::process::Command;

use abmx_gmail::{Credentials, TokenStore};

fn config(dir: &Path, token: &str) -> PathBuf {
    let path = dir.join("config.toml");
    std::fs::write(
        &path,
        format!(
            r#"
[fetch.gmail]
token = "{token}"

[labels]
prefix = "receipt/"
"#
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
        .env("ABMX_GMAIL_CLIENT_SECRET", "client-secret")
        .output()
        .unwrap()
}

fn token(path: &Path) -> Credentials {
    TokenStore::new(path)
        .save(&Credentials {
            client_id: "client-id".into(),
            refresh_token: "refresh-token".into(),
            scopes: vec!["https://www.googleapis.com/auth/gmail.readonly".into()],
            client_secret: None,
        })
        .unwrap();
    TokenStore::new(path).load().unwrap().unwrap()
}

#[test]
fn export_writes_a_self_contained_bundle_with_0600() {
    let dir = tempfile::tempdir().unwrap();
    let config = config(dir.path(), "credentials.json");
    let _ = token(&dir.path().join("credentials.json"));

    let out = dir.path().join("bundle.json");
    let output = run(&config, &["auth", "export", "--out", out.to_str().unwrap()]);
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let text = std::fs::read_to_string(&out).unwrap();
    let bundle: Credentials = serde_json::from_str(&text).unwrap();
    assert_eq!(bundle.client_id, "client-id");
    assert_eq!(bundle.refresh_token, "refresh-token");
    assert_eq!(bundle.client_secret.as_deref(), Some("client-secret"));

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&out).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
    }
}

#[test]
fn import_installs_the_bundle_at_the_configured_token_path() {
    let dir = tempfile::tempdir().unwrap();
    let source_config = config(dir.path(), "source.json");
    let _ = token(&dir.path().join("source.json"));

    let bundle_path = dir.path().join("bundle.json");
    let exported = run(
        &source_config,
        &["auth", "export", "--out", bundle_path.to_str().unwrap()],
    );
    assert!(
        exported.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&exported.stderr)
    );

    let target_config = config(dir.path(), "target.json");
    let imported = run(
        &target_config,
        &["auth", "import", "--file", bundle_path.to_str().unwrap()],
    );
    assert!(
        imported.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&imported.stderr)
    );

    let installed = TokenStore::new(dir.path().join("target.json"))
        .load()
        .unwrap()
        .unwrap();
    assert_eq!(installed.client_id, "client-id");
    assert_eq!(installed.client_secret.as_deref(), Some("client-secret"));

    // Without --force, an existing token file is not clobbered.
    let again = run(
        &target_config,
        &["auth", "import", "--file", bundle_path.to_str().unwrap()],
    );
    assert_eq!(again.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&again.stderr);
    assert!(stderr.contains("--force"), "{stderr}");
}

#[test]
fn export_refuses_without_a_client_secret() {
    let dir = tempfile::tempdir().unwrap();
    let config = config(dir.path(), "credentials.json");
    let _ = token(&dir.path().join("credentials.json"));

    let out = dir.path().join("bundle.json");
    let output = Command::new(env!("CARGO_BIN_EXE_abmx"))
        .arg("--config")
        .arg(&config)
        .args(["auth", "export", "--out", out.to_str().unwrap()])
        .env_remove("ABMX_GMAIL_CLIENT_SECRET")
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("client secret"), "{stderr}");

    // The opt-out exists for public OAuth clients.
    let allowed = Command::new(env!("CARGO_BIN_EXE_abmx"))
        .arg("--config")
        .arg(&config)
        .args([
            "auth",
            "export",
            "--out",
            out.to_str().unwrap(),
            "--allow-missing-client-secret",
        ])
        .env_remove("ABMX_GMAIL_CLIENT_SECRET")
        .output()
        .unwrap();
    assert!(
        allowed.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&allowed.stderr)
    );
}
