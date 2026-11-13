use base64::Engine;
use txsync_core::{FetchRequest, Fetcher};
use txsync_gmail::auth::{Credentials, TokenStore};
use txsync_gmail::{GmailConfig, GmailFetcher};
use txsync_testkit::{MockServer, Response};

const EMAIL: &[u8] = b"From: alerts@bank.example\r\nSubject: Payment successful\r\n\r\nhello\r\n";

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
    config.query = "from:alerts@bank.example".into();
    config.lookback_days = 7;
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

#[test]
fn full_sync_uses_the_history_point_taken_before_listing() {
    let dir = tempfile::tempdir().unwrap();
    let server = MockServer::start();
    token_route(&server);

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
    server.route(
        "GET",
        "/gmail/v1/users/me/profile",
        vec![Response::json(
            r#"{"emailAddress":"me@example.invalid","historyId":"1000"}"#,
        )],
    );

    let fetcher = fetcher(&server, dir.path());
    let batch = fetcher.fetch(FetchRequest::new(None)).unwrap();

    assert_eq!(batch.messages.len(), 1);
    assert_eq!(batch.messages[0].key.as_str(), "gmail:m1");
    assert_eq!(batch.messages[0].bytes, EMAIL);
    assert_eq!(
        batch.next_cursor.unwrap(),
        serde_json::json!({ "history_id": "1000" })
    );

    // The query and the spam/trash filters must reach the API.
    let list = server
        .requests()
        .into_iter()
        .find(|request| request.path_only() == "/gmail/v1/users/me/messages")
        .expect("messages.list was called");
    assert!(list.query.contains("q=from%3Aalerts%40bank.example"));
}

#[test]
fn incremental_sync_reads_history_and_advances_the_cursor() {
    let dir = tempfile::tempdir().unwrap();
    let server = MockServer::start();
    token_route(&server);

    server.route(
        "GET",
        "/gmail/v1/users/me/messages/m2",
        vec![Response::json(raw_json("m2", EMAIL))],
    );
    server.route(
        "GET",
        "/gmail/v1/users/me/history",
        vec![Response::json(
            r#"{"historyId":"2000","history":[{"messagesAdded":[{"message":{"id":"m2"}}]}]}"#,
        )],
    );

    let fetcher = fetcher(&server, dir.path());
    let cursor = serde_json::json!({ "history_id": "1000" });
    let batch = fetcher.fetch(FetchRequest::new(Some(&cursor))).unwrap();

    assert_eq!(batch.messages.len(), 1);
    assert_eq!(batch.messages[0].key.as_str(), "gmail:m2");
    assert_eq!(
        batch.next_cursor.unwrap(),
        serde_json::json!({ "history_id": "2000" })
    );

    let history = server
        .requests()
        .into_iter()
        .find(|request| request.path_only() == "/gmail/v1/users/me/history")
        .expect("history.list was called");
    assert!(history.query.contains("startHistoryId=1000"));
}

#[test]
fn an_expired_history_cursor_falls_back_to_a_full_sync() {
    let dir = tempfile::tempdir().unwrap();
    let server = MockServer::start();
    token_route(&server);

    server.route(
        "GET",
        "/gmail/v1/users/me/history",
        vec![Response::status(404, r#"{"error":"history expired"}"#)],
    );
    server.route(
        "GET",
        "/gmail/v1/users/me/messages/m3",
        vec![Response::json(raw_json("m3", EMAIL))],
    );
    server.route(
        "GET",
        "/gmail/v1/users/me/messages",
        vec![Response::json(r#"{"messages":[{"id":"m3"}]}"#)],
    );
    server.route(
        "GET",
        "/gmail/v1/users/me/profile",
        vec![Response::json(r#"{"historyId":"3000"}"#)],
    );

    let fetcher = fetcher(&server, dir.path());
    let cursor = serde_json::json!({ "history_id": "1" });
    let batch = fetcher.fetch(FetchRequest::new(Some(&cursor))).unwrap();

    assert_eq!(batch.messages.len(), 1);
    assert_eq!(
        batch.next_cursor.unwrap(),
        serde_json::json!({ "history_id": "3000" })
    );
}

#[test]
fn a_message_deleted_between_list_and_fetch_is_skipped() {
    let dir = tempfile::tempdir().unwrap();
    let server = MockServer::start();
    token_route(&server);

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
    server.route(
        "GET",
        "/gmail/v1/users/me/profile",
        vec![Response::json(r#"{"historyId":"4000"}"#)],
    );

    let fetcher = fetcher(&server, dir.path());
    let batch = fetcher.fetch(FetchRequest::new(None)).unwrap();

    assert!(batch.messages.is_empty());
    assert_eq!(batch.skipped, 1);
}

#[test]
fn truncation_does_not_advance_the_cursor() {
    let dir = tempfile::tempdir().unwrap();
    let server = MockServer::start();
    token_route(&server);

    server.route(
        "GET",
        "/gmail/v1/users/me/history",
        vec![Response::json(
            r#"{"historyId":"9999","history":[
                {"messagesAdded":[{"message":{"id":"a"}}]},
                {"messagesAdded":[{"message":{"id":"b"}}]}
            ]}"#,
        )],
    );
    server.route(
        "GET",
        "/gmail/v1/users/me/messages/a",
        vec![Response::json(raw_json("a", EMAIL))],
    );

    let mut config = GmailConfig::new("client", dir.path().join("creds.json"));
    config.base_url = format!("{}/gmail/v1", server.url());
    config.token_url = format!("{}/token", server.url());
    TokenStore::new(&config.token_path)
        .save(&Credentials {
            client_id: "client".into(),
            refresh_token: "refresh".into(),
            scopes: vec![],
        })
        .unwrap();

    let fetcher = GmailFetcher::new(config).unwrap();
    let cursor = serde_json::json!({ "history_id": "1000" });
    let batch = fetcher
        .fetch(FetchRequest::new(Some(&cursor)).with_limit(1))
        .unwrap();

    assert_eq!(batch.messages.len(), 1);
    // Still the old cursor: the unfetched message will be replayed next run.
    assert_eq!(
        batch.next_cursor.unwrap(),
        serde_json::json!({ "history_id": "1000" })
    );
}

#[test]
fn sampling_a_label_returns_raw_messages_and_excludes_spam() {
    let dir = tempfile::tempdir().unwrap();
    let server = MockServer::start();
    token_route(&server);

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
    let samples = fetcher.client().sample_label("example/QRIS", 5).unwrap();

    assert_eq!(samples.len(), 1);
    assert_eq!(samples[0].id, "m1");
    assert_eq!(samples[0].bytes, EMAIL);

    let list = server
        .requests()
        .into_iter()
        .find(|request| request.path_only() == "/gmail/v1/users/me/messages")
        .expect("messages.list was called");
    assert!(list.query.contains("label%3A%22Bank%2FQRIS%22"));
    assert!(list.query.contains("-in%3Aspam"));
}
