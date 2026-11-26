//! Ports of upstream security-hardening changes: Config.py's CORS default
//! flip, server.py's add_security_headers, and the CORS consumer that reads
//! GENERAL.CORS_ALLOWED_HOSTS.

mod common;

use common::*;
use serde_json::json;

/// server.py add_security_headers: every response carries these headers.
#[test]
fn test_security_headers_present_on_pages() {
    let server = spawn_server();
    let resp = ureq::get(&format!("{}/", server.base_url))
        .call()
        .expect("GET /");
    assert_eq!(
        resp.headers()
            .get("x-content-type-options")
            .and_then(|v| v.to_str().ok()),
        Some("nosniff"),
        "missing X-Content-Type-Options header"
    );
    assert_eq!(
        resp.headers()
            .get("referrer-policy")
            .and_then(|v| v.to_str().ok()),
        Some("same-origin"),
        "missing Referrer-Policy header"
    );
}

/// CORS_ALLOWED_HOSTS now defaults to "" (same-origin only), so the API must
/// not advertise a wildcard origin when the setting is absent/empty.
#[test]
fn test_cors_header_absent_by_default() {
    let server = spawn_server();
    let resp = ureq::get(&format!("{}/api/status", server.base_url))
        .call()
        .expect("GET /api/status");
    assert!(
        resp.headers().get("access-control-allow-origin").is_none(),
        "CORS header must not be sent when CORS_ALLOWED_HOSTS is empty"
    );
}

/// A configured CORS_ALLOWED_HOSTS is echoed on API responses (proving the
/// default flip is not inert: the setting is actually consumed).
#[test]
fn test_cors_header_follows_config() {
    let server = spawn_server_with_config(&json!({
        "SECRETS": {"ADMIN_USERNAME": "", "ADMIN_PASSWORD": ""},
        "GENERAL": {"DEBUG": false, "MOCK_NODE_SIGNAL": "0", "CORS_ALLOWED_HOSTS": "*"},
    }));
    let resp = ureq::get(&format!("{}/api/status", server.base_url))
        .call()
        .expect("GET /api/status");
    assert_eq!(
        resp.headers()
            .get("access-control-allow-origin")
            .and_then(|v| v.to_str().ok()),
        Some("*"),
        "configured CORS_ALLOWED_HOSTS should be advertised on the API"
    );
}
