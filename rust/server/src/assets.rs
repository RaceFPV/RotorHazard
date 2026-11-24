//! Embedded assets: templates, static files, and language.json.
//!
//! Debug builds read these from the source tree at runtime (edit templates
//! without rebuilding); release builds embed them, making the binary
//! self-contained.  Resolution is relative to the crate directory, so the
//! working directory no longer matters.

use rust_embed::RustEmbed;

#[derive(RustEmbed)]
#[folder = "../../src/server/templates"]
pub struct Templates;

#[derive(RustEmbed)]
#[folder = "../../src/server/static"]
pub struct StaticAssets;

#[derive(RustEmbed)]
#[folder = "../../src/server"]
#[include = "language.json"]
pub struct ProgramFiles;

use axum::http::StatusCode;
use axum::response::IntoResponse;

/// Serves an embedded static asset.
pub async fn static_handler(
    axum::extract::Path(path): axum::extract::Path<String>,
) -> impl IntoResponse {
    match StaticAssets::get(&path) {
        Some(file) => {
            let mime = mime_guess::from_path(&path).first_or_octet_stream();
            (
                [(axum::http::header::CONTENT_TYPE, mime.as_ref())],
                file.data,
            )
                .into_response()
        }
        None => (StatusCode::NOT_FOUND, "not found").into_response(),
    }
}

/// language.json contents: embedded first, source-tree fallback second.
pub fn load_languages() -> serde_json::Value {
    if let Some(file) = ProgramFiles::get("language.json") {
        if let Ok(v) = serde_json::from_slice::<serde_json::Value>(&*file.data) {
            return v;
        }
    }
    if let Ok(text) = std::fs::read_to_string("src/server/language.json") {
        if let Ok(v) = serde_json::from_str(&text) {
            return v;
        }
    }
    serde_json::Value::Object(serde_json::Map::new())
}
