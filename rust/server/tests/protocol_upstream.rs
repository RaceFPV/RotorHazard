//! Ports of upstream security changes: the add_security_headers
//! after_request hook in server.py adds hardening headers to every response
//! (X-Content-Type-Options: nosniff, Referrer-Policy: same-origin).
//!
//! The CORS_ALLOWED_HOSTS default change has no Rust consumer: the Rust
//! server does not enforce CORS (no socketioxide CORS configuration and no
//! Flask-CORS equivalent), so flipping that default would be an inert edit.
//! The database-backup path validation and hardware_log_init auth guards have
//! no Rust socket handlers (download/restore/delete_database and
//! hardware_log_init are not ported), so they have no surface here either.

mod common;

use common::*;

#[test]
fn test_security_headers_on_page_and_api() {
    with_server(|server, _client, _observer| {
        // A rendered page carries the hardening headers.
        let page = ureq::get(&format!("{}/", server.base_url))
            .call()
            .expect("page request failed");
        assert_eq!(page.status(), 200, "GET / failed");
        let headers = page.headers();
        assert_eq!(
            headers.get("x-content-type-options").and_then(|v| v.to_str().ok()),
            Some("nosniff"),
            "page response missing X-Content-Type-Options"
        );
        assert_eq!(
            headers.get("referrer-policy").and_then(|v| v.to_str().ok()),
            Some("same-origin"),
            "page response missing Referrer-Policy"
        );

        // The JSON API carries them too. The Rust /api/status returns
        // 201 Created (characterized behavior), not 200.
        let api = ureq::get(&format!("{}/api/status", server.base_url))
            .call()
            .expect("api request failed");
        assert_eq!(api.status(), 201, "GET /api/status failed");
        let headers = api.headers();
        assert_eq!(
            headers.get("x-content-type-options").and_then(|v| v.to_str().ok()),
            Some("nosniff"),
            "api response missing X-Content-Type-Options"
        );
        assert_eq!(
            headers.get("referrer-policy").and_then(|v| v.to_str().ok()),
            Some("same-origin"),
            "api response missing Referrer-Policy"
        );
    });
}
