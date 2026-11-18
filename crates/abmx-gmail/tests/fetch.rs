use std::collections::HashSet;

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
    fetcher_with_since(server, dir, None)
}

fn fetcher_with_since(
    server: &MockServer,
    dir: &std::path::Path,
    since: Option<chrono::NaiveDate>,
) -> GmailFetcher {
    let store = TokenStore::new(dir.join("credentials.json"));
    store
        .save(&Credentials {
            client_id: "client".into(),
            refresh_token: "refresh".into(),
            scopes: vec![],
            client_secret: None,
        })
        .unwrap();

    let mut config = GmailConfig::new("client", store.path());
    config.base_url = format!("{}/gmail/v1", server.url());
    config.token_url = format!("{}/token", server.url());
    config.prefix = "abmx:".to_owned();
    config.since = since;
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
fn since_adds_an_after_filter_to_the_query() {
    let dir = tempfile::tempdir().unwrap();
    let server = MockServer::start();
    token_route(&server);
    labels_route(&server);
    server.route(
        "GET",
        "/gmail/v1/users/me/messages",
        vec![Response::json(r#"{"messages":[]}"#)],
    );

    let store = TokenStore::new(dir.path().join("credentials.json"));
    store
        .save(&Credentials {
            client_id: "client".into(),
            refresh_token: "refresh".into(),
            scopes: vec![],
            client_secret: None,
        })
        .unwrap();

    let mut config = GmailConfig::new("client", store.path());
    config.base_url = format!("{}/gmail/v1", server.url());
    config.token_url = format!("{}/token", server.url());
    config.prefix = "abmx:".to_owned();
    config.since = Some(chrono::NaiveDate::from_ymd_opt(2026, 11, 1).unwrap());
    let fetcher = GmailFetcher::new(config).unwrap();

    fetcher
        .fetch(FetchRequest::new(&["abmx:test".to_owned()]))
        .unwrap();

    let list = server
        .requests()
        .into_iter()
        .find(|request| request.path_only() == "/gmail/v1/users/me/messages")
        .expect("messages.list was called");
    // Gmail's `after:` is exclusive, so the fetcher subtracts a day.
    assert!(
        list.query.contains("after%3A2026%2F10%2F31"),
        "query was {}",
        list.query
    );
}

#[test]
fn a_message_under_two_routed_labels_is_rejected() {
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
fn seen_messages_are_not_downloaded() {
    let dir = tempfile::tempdir().unwrap();
    let server = MockServer::start();
    token_route(&server);
    labels_route(&server);
    for id in ["m1", "m2"] {
        server.route(
            "GET",
            &format!("/gmail/v1/users/me/messages/{id}"),
            vec![Response::json(raw_json(id, EMAIL))],
        );
    }
    server.route(
        "GET",
        "/gmail/v1/users/me/messages",
        vec![Response::json(r#"{"messages":[{"id":"m1"},{"id":"m2"}]}"#)],
    );

    let fetcher = fetcher(&server, dir.path());
    let mut skip = HashSet::new();
    skip.insert("gmail:m1".to_owned());
    let labels = vec!["abmx:test".to_owned()];

    let batch = fetcher
        .fetch(FetchRequest::new(&labels).with_skip(&skip))
        .unwrap();
    assert_eq!(batch.messages.len(), 1);
    assert_eq!(batch.messages[0].key.as_str(), "gmail:m2");
    assert_eq!(batch.already_seen, 1);

    let requests = server.requests();
    assert!(
        !requests
            .iter()
            .any(|request| request.path_only() == "/gmail/v1/users/me/messages/m1"),
        "a seen message must not be downloaded"
    );
    assert!(
        requests
            .iter()
            .any(|request| request.path_only() == "/gmail/v1/users/me/messages/m2"),
        "the unseen message must be downloaded"
    );

    // A limit applies to the unseen set, not the full labelled history.
    let batch = fetcher
        .fetch(FetchRequest::new(&labels).with_skip(&skip).with_limit(1))
        .unwrap();
    assert_eq!(batch.messages.len(), 1);
    assert_eq!(batch.messages[0].key.as_str(), "gmail:m2");
    assert_eq!(batch.already_seen, 1);
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
            client_secret: None,
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
fn sampling_ignores_the_sync_window() {
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

    let fetcher = fetcher_with_since(
        &server,
        dir.path(),
        Some(chrono::NaiveDate::from_ymd_opt(2026, 11, 1).unwrap()),
    );
    let samples = fetcher.sample_label("abmx:test", 5).unwrap();
    assert_eq!(samples.len(), 1);

    let list = server
        .requests()
        .into_iter()
        .find(|request| request.path_only() == "/gmail/v1/users/me/messages")
        .expect("messages.list was called");
    // Sampling is authoring evidence: no sync window applies.
    assert!(
        !list.query.contains("after"),
        "query unexpectedly had a window: {}",
        list.query
    );
    assert!(
        list.query.contains("label%3A"),
        "query was not a label search: {}",
        list.query
    );
}

#[test]
fn routed_labels_are_filtered_and_sorted() {
    let dir = tempfile::tempdir().unwrap();
    let server = MockServer::start();
    token_route(&server);
    labels_route(&server);

    let fetcher = fetcher(&server, dir.path());
    let labels: Vec<String> = fetcher
        .routed_labels()
        .unwrap()
        .into_iter()
        .map(|label| label.name)
        .collect();

    assert_eq!(labels, vec!["abmx:other", "abmx:test"]);
}
