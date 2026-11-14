use abmx_gmail::revoke_refresh_token;
use abmx_testkit::{MockServer, Response};

#[test]
fn a_successful_revoke_sends_the_token_as_a_form() {
    let server = MockServer::start();
    server.route("POST", "/revoke", vec![Response::status(200, "")]);

    revoke_refresh_token(&format!("{}/revoke", server.url()), "refresh-token").unwrap();

    let request = &server.requests()[0];
    assert_eq!(request.method, "POST");
    assert_eq!(
        request.header("content-type"),
        Some("application/x-www-form-urlencoded")
    );
    assert!(request.body.contains("token=refresh-token"));
}

#[test]
fn an_already_invalid_token_counts_as_revoked() {
    let server = MockServer::start();
    server.route(
        "POST",
        "/revoke",
        vec![Response::status(400, r#"{"error":"invalid_token"}"#)],
    );

    revoke_refresh_token(&format!("{}/revoke", server.url()), "gone").unwrap();
}

#[test]
fn a_server_error_is_reported() {
    let server = MockServer::start();
    server.route("POST", "/revoke", vec![Response::status(500, "boom")]);

    let error = revoke_refresh_token(&format!("{}/revoke", server.url()), "x").unwrap_err();
    assert!(error.to_string().contains("500"));
}

#[test]
fn an_unreachable_endpoint_is_an_error() {
    assert!(revoke_refresh_token("http://127.0.0.1:1/revoke", "x").is_err());
}
