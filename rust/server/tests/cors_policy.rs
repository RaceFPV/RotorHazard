//! CORS enforcement (GENERAL.CORS_ALLOWED_HOSTS).
//!
//! The reference server hands this to engineio, whose default is same-origin
//! only and which refuses a foreign Origin outright on the Socket.IO endpoint.
//! Browsers do not apply CORS to WebSocket upgrades, so that refusal is what
//! stops a foreign page from opening a socket and driving a race.

mod common;

use common::*;
use serde_json::json;

fn config_with(cors: Option<&str>) -> serde_json::Value {
    let mut general = json!({ "DEBUG": false, "MOCK_NODE_SIGNAL": "0" });
    if let Some(value) = cors {
        general["CORS_ALLOWED_HOSTS"] = json!(value);
    }
    json!({
        "SECRETS": { "ADMIN_USERNAME": "", "ADMIN_PASSWORD": "" },
        "GENERAL": general,
    })
}

fn origin_of(server: &ServerHandle) -> String {
    // base_url is http://127.0.0.1:<port>
    server.base_url.clone()
}

fn acao(headers: &ureq::http::HeaderMap) -> Option<String> {
    headers
        .get("access-control-allow-origin")
        .and_then(|v| v.to_str().ok())
        .map(str::to_string)
}

#[test]
fn the_page_the_server_serves_is_same_origin() {
    let server = spawn_server_with_config(&config_with(None));
    let origin = origin_of(&server);
    let resp = ureq::get(&format!("{}/api/status", server.base_url))
        .header("Origin", &origin)
        .call()
        .expect("GET /api/status with its own origin");
    assert_eq!(
        acao(resp.headers()).as_deref(),
        Some(origin.as_str()),
        "the server's own origin must be answered, or its own page breaks"
    );
    assert!(
        resp.headers()
            .get("vary")
            .and_then(|v| v.to_str().ok())
            .is_some_and(|v| v.contains("Origin")),
        "a response that varies with Origin must say so"
    );
}

#[test]
fn a_foreign_page_cannot_open_a_socket() {
    let server = spawn_server_with_config(&config_with(None));
    let url = format!("{}/socket.io/?EIO=4&transport=polling", server.base_url);
    let err = ureq::get(&url)
        .header("Origin", "https://evil.example")
        .call()
        .expect_err("a foreign Origin must be refused on /socket.io");
    match err {
        ureq::Error::StatusCode(code) => assert_eq!(
            code, 400,
            "engineio answers a foreign origin with 400 Not an accepted origin."
        ),
        other => panic!("expected a 400, got {:?}", other),
    }
    let ok = ureq::get(&url)
        .header("Origin", &origin_of(&server))
        .call()
        .expect("the timer's own page must still connect");
    let body = ok.into_body().read_to_string().unwrap_or_default();
    assert!(
        body.contains("sid"),
        "the handshake should have produced a session, got {:?}",
        &body[..body.len().min(60)]
    );
}

#[test]
fn a_foreign_page_cannot_read_the_api() {
    let server = spawn_server_with_config(&config_with(None));
    let resp = ureq::get(&format!("{}/api/status", server.base_url))
        .header("Origin", "https://evil.example")
        .call()
        .expect("the API itself still answers - only the CORS header is withheld");
    assert_eq!(
        acao(resp.headers()),
        None,
        "no Access-Control-Allow-Origin may be advertised to an unknown origin"
    );
}

#[test]
fn a_configured_origin_is_allowed_and_nothing_else_is() {
    let server = spawn_server_with_config(&config_with(Some("https://ok.example")));
    let allowed = ureq::get(&format!("{}/api/status", server.base_url))
        .header("Origin", "https://ok.example")
        .call()
        .expect("GET /api/status");
    assert_eq!(acao(allowed.headers()).as_deref(), Some("https://ok.example"));

    let other = ureq::get(&format!("{}/api/status", server.base_url))
        .header("Origin", "https://other.example")
        .call()
        .expect("GET /api/status");
    assert_eq!(acao(other.headers()), None);

    let socket = ureq::get(&format!(
        "{}/socket.io/?EIO=4&transport=polling",
        server.base_url
    ))
    .header("Origin", "https://other.example")
    .call();
    assert!(
        socket.is_err(),
        "an origin outside the allow list must not connect"
    );
}

#[test]
fn a_wildcard_echoes_the_caller() {
    let server = spawn_server_with_config(&config_with(Some("*")));
    let resp = ureq::get(&format!("{}/api/status", server.base_url))
        .header("Origin", "https://anywhere.example")
        .call()
        .expect("GET /api/status");
    assert_eq!(
        acao(resp.headers()).as_deref(),
        Some("https://anywhere.example"),
        "with * the origin is echoed; a literal * cannot travel with credentials"
    );
}

#[test]
fn a_preflight_is_answered_for_an_allowed_origin() {
    let server = spawn_server_with_config(&config_with(Some("*")));
    // ureq 3 has no `ureq::request(method, url)`; build the request directly.
    let preflight = ureq::http::Request::builder()
        .method("OPTIONS")
        .uri(format!("{}/api/status", server.base_url))
        .header("Origin", "https://anywhere.example")
        .header("Access-Control-Request-Method", "GET")
        .header("Access-Control-Request-Headers", "x-custom")
        .body(())
        .expect("build the preflight request");
    let resp = ureq::agent().run(preflight).expect("OPTIONS preflight");
    assert_eq!(
        acao(resp.headers()).as_deref(),
        Some("https://anywhere.example")
    );
    assert_eq!(
        resp.headers()
            .get("access-control-allow-headers")
            .and_then(|v| v.to_str().ok()),
        Some("x-custom"),
        "the requested headers are echoed, as engineio does"
    );
}
