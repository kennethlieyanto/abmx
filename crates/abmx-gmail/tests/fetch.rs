use abmx_core::{FetchRequest, Fetcher};
use abmx_gmail::auth::{Credentials, TokenStore};
use abmx_gmail::{GmailConfig, GmailFetcher};
use abmx_testkit::{MockServer, Response};
use base64::Engine;

const EMAIL: &[u8] = b"From: alerts@bank.example\r\nSubject: Payment successful\r\nMessage-ID: <m1@example.invalid>\r\n\r\nhello\r\n";

fn raw_json(id: &str, bytes: &[u8]) -> String {
    let encoded = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes);
    format!(r#"{{"id":"{id}","raw":"{encoded}"}}"#)
}

fn fetcher(server: &MockServer, dir: &std::path::Path) -> GmailFetcher {
    let store = TokenStore::new(dir.join("credentials.json"));
    store
        .save(&Credentials {
            client_id: "client".into(),
            refresh_token: "refresh".into(),
            scopes: vec![],
        })
        .unwrap();

    let mut config = GmailConfig::new("client", store.path());
    config.base_url = format!("{}/gmail/v1", server.url());
    config.token_url = format!("{}/token", server.url());
    GmailFetcher::new(config).unwrap()
}

fn token_route(server: &MockServer) {
    server.route(
        "POST",
        "/token",
        vec![Response::json(
            r#"{"access_token":"access","expires_in":3600,"token_type":"Bearer"}"#,
        )],
    );
}

fn labels_route(server: &MockServer) {
    server.route(
        "GET",
        "/gmail/v1/users/me/labels",
        vec![Response::json(
            r#"{"labels":[
                {"id":"Label_1","name":"abmx:test"},
                {"id":"Label_2","name":"abmx:other"},
                {"id":"Label_3","name":"INBOX"}
            ]}"#,
        )],
    );
}

#[test]
fn fetch_is_driven_by_the_configured_label() {
    let dir = tempfile::tempdir().unwrap();
    let server = MockServer::start();
    token_route(&server);
    labels_route(&server);
    server.route(
        "GET",
        "/gmail/v1/users/me/messages/m1",
        vec![Response::json(raw_json("m1", EMAIL))],
    );
    server.route(
        "GET",
        "/gmail/v1/users/me/messages",
        vec![Response::json(r#"{"messages":[{"id":"m1"}]}"#)],
    );

    let fetcher = fetcher(&server, dir.path());
    let labels = vec!["abmx:test".to_owned()];
    let batch = fetcher.fetch(FetchRequest::new(&labels)).unwrap();

    assert_eq!(batch.messages.len(), 1);
    assert_eq!(batch.messages[0].key.as_str(), "gmail:m1");
    assert_eq!(batch.messages[0].label, "abmx:test");
    assert_eq!(batch.messages[0].bytes, EMAIL);

    let list = server
        .requests()
        .into_iter()
        .find(|request| request.path_only() == "/gmail/v1/users/me/messages")
        .expect("messages.list was called");
    assert!(list.query.contains("labelIds=Label_1"));
    assert!(list.query.contains("q=-in%3Aspam"));
}

#[test]
fn a_message_under_two_abmx_labels_is_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let server = MockServer::start();
    token_route(&server);
    labels_route(&server);
    server.route(
        "GET",
        "/gmail/v1/users/me/messages",
        vec![
            Response::json(r#"{"messages":[{"id":"m1"}]}"#),
            Response::json(r#"{"messages":[{"id":"m1"}]}"#),
        ],
    );

    let fetcher = fetcher(&server, dir.path());
    let labels = vec!["abmx:test".to_owned(), "abmx:other".to_owned()];
    let error = fetcher.fetch(FetchRequest::new(&labels)).unwrap_err();
    assert!(error.to_string().contains("two abmx labels"), "{error}");
}

#[test]
fn a_message_deleted_between_list_and_fetch_is_skipped() {
    let dir = tempfile::tempdir().unwrap();
    let server = MockServer::start();
    token_route(&server);
    labels_route(&server);
    server.route(
        "GET",
        "/gmail/v1/users/me/messages/m4",
        vec![Response::status(404, r#"{"error":"not found"}"#)],
    );
    server.route(
        "GET",
        "/gmail/v1/users/me/messages",
        vec![Response::json(r#"{"messages":[{"id":"m4"}]}"#)],
    );

    let fetcher = fetcher(&server, dir.path());
    let labels = vec!["abmx:test".to_owned()];
    let batch = fetcher.fetch(FetchRequest::new(&labels)).unwrap();

    assert!(batch.messages.is_empty());
    assert_eq!(batch.skipped, 1);
}

#[test]
fn exceeding_the_cap_is_an_error_not_a_truncation() {
    let dir = tempfile::tempdir().unwrap();
    let server = MockServer::start();
    token_route(&server);
    labels_route(&server);
    server.route(
        "GET",
        "/gmail/v1/users/me/messages",
        vec![Response::json(r#"{"messages":[{"id":"a"},{"id":"b"}]}"#)],
    );

    let mut config = GmailConfig::new("client", dir.path().join("creds.json"));
    config.base_url = format!("{}/gmail/v1", server.url());
    config.token_url = format!("{}/token", server.url());
    config.max_messages = 1;
    TokenStore::new(&config.token_path)
        .save(&Credentials {
            client_id: "client".into(),
            refresh_token: "refresh".into(),
            scopes: vec![],
        })
        .unwrap();

    let fetcher = GmailFetcher::new(config).unwrap();
    let labels = vec!["abmx:test".to_owned()];
    let error = fetcher.fetch(FetchRequest::new(&labels)).unwrap_err();
    assert!(error.to_string().contains("max_messages"), "{error}");
}

#[test]
fn sampling_a_label_returns_the_newest_messages() {
    let dir = tempfile::tempdir().unwrap();
    let server = MockServer::start();
    token_route(&server);
    labels_route(&server);
    server.route(
        "GET",
        "/gmail/v1/users/me/messages/m1",
        vec![Response::json(raw_json("m1", EMAIL))],
    );
    server.route(
        "GET",
        "/gmail/v1/users/me/messages",
        vec![Response::json(r#"{"messages":[{"id":"m1"}]}"#)],
    );

    let fetcher = fetcher(&server, dir.path());
    let samples = fetcher.sample_label("abmx:test", 5).unwrap();

    assert_eq!(samples.len(), 1);
    assert_eq!(samples[0].id, "m1");
    assert_eq!(samples[0].bytes, EMAIL);
}

#[test]
fn abmx_labels_are_filtered_and_sorted() {
    let dir = tempfile::tempdir().unwrap();
    let server = MockServer::start();
    token_route(&server);
    labels_route(&server);

    let fetcher = fetcher(&server, dir.path());
    let labels: Vec<String> = fetcher
        .abmx_labels()
        .unwrap()
        .into_iter()
        .map(|label| label.name)
        .collect();

    assert_eq!(labels, vec!["abmx:other", "abmx:test"]);
}
