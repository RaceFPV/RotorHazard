//! RotorHazard server -- Rust implementation (Phase 1 skeleton).
//!
//! Serves the existing static frontend, the /api JSON endpoints, and the
//! Socket.IO event surface, against the same database.db schema and seed
//! data as the Python server.  Behavior is gated by the protocol
//! characterization suite in src/tests/protocol/.

mod api;
mod db;
mod export_import;
mod pages;
mod payloads;
mod phase3;
mod results;
mod race;
mod socket;
mod state;
mod util;

use axum::extract::State;
use serde_json::json;
use socketioxide::SocketIo;
use state::{AppState, ServerInfo, SharedState};
use std::sync::{Arc, Mutex};
use tower_http::services::ServeDir;

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info".into()),
        )
        .init();

    let data_dir = std::env::var("RH_DATA_DIR").unwrap_or_else(|_| "data".to_string());
    let static_dir = std::env::var("RH_STATIC_DIR")
        .unwrap_or_else(|_| "src/server/static".to_string());
    let port: u16 = std::env::var("RH_PORT")
        .or_else(|_| std::env::var("HTTP_PORT"))
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(5000);

    // minimal config.json support (same file the Python server writes)
    let (admin_username, admin_password) = read_admin_creds(&data_dir);
    let http_port_cfg = read_config_port(&data_dir).unwrap_or(port);

    let templates_dir = std::env::var("RH_TEMPLATES_DIR")
        .unwrap_or_else(|_| "src/server/templates".to_string());
    if let Err(e) = pages::init(&templates_dir) {
        tracing::warn!("template environment: {}", e);
    }
    let language_file_dir = std::env::var("RH_PROGRAM_DIR")
        .unwrap_or_else(|_| ".".to_string());
    crate::phase3::set_languages(crate::phase3::load_languages(&language_file_dir));

    let conn = db::open_or_init(&data_dir).expect("failed to open/init database");
    let num_nodes: usize = std::env::var("RH_NODES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(0)
        .max(8); // mock-node count, mirroring the Python test configuration

    tracing::info!(
        "RotorHazard (Rust) starting: data dir {}, port {}, {} nodes",
        data_dir, http_port_cfg, num_nodes
    );


    let state: SharedState = Arc::new(AppState {
        db: Arc::new(Mutex::new(conn)),
        race: Arc::new(Mutex::new(state::RaceState::new(num_nodes))),
        num_nodes,
        io: std::sync::OnceLock::new(),
        info: ServerInfo {
            release_version: "4.6.0-dev.1-rust".to_string(),
            server_api: 50,
            node_api_best: 36,
            json_api: 3,
        },
        data_dir: data_dir.clone(),
        admin_username,
        admin_password,
        config_items: Arc::new(Mutex::new(std::collections::HashMap::new())),
    });

    let (socketio_layer, io) = SocketIo::builder()
        .with_state(state.clone())
        .build_layer();
    state.io.set(io.clone()).expect("io set once");

    pages::set_state(state.clone());
    socket::register_handlers(io, state.clone());

    // heartbeat: broadcast node state every 500ms (heartbeat_thread_function)
    {
        let hb_state = state.clone();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(std::time::Duration::from_millis(500));
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                interval.tick().await;
                let payload = db::with_db(&hb_state, |conn| {
                    let profile_id = db::current_profile_id(conn);
                    let freqs = db::profile_frequencies(conn, profile_id);
                    let n = hb_state.num_nodes;
                    json!({
                        "current_rssi": vec![45i64; n],
                        "frequency": (0..n).map(|i| freqs.get(i).map(|(_, _, f)| *f).unwrap_or(0)).collect::<Vec<_>>(),
                        "loop_time": vec![0.5f64; n],
                        "crossing_flag": vec![false; n],
                    })
                });
                socket::broadcast(&hb_state, "heartbeat", payload);
            }
        });
    }

    let app = axum::Router::new()
        .route("/api/status", axum::routing::get(api::status))
        .route("/api/pilot/all", axum::routing::get(api::pilot_all))
        .route("/api/pilot/{pilot_id}", axum::routing::get(api::pilot_one))
        .route("/api/heat/all", axum::routing::get(api::heat_all))
        .route("/api/heat/{heat_id}", axum::routing::get(api::heat_one))
        .route("/api/class/all", axum::routing::get(api::class_all))
        .route("/api/class/{class_id}", axum::routing::get(api::class_one))
        .route("/api/format/all", axum::routing::get(api::format_all))
        .route("/api/format/{format_id}", axum::routing::get(api::format_one))
        .route("/api/profile/all", axum::routing::get(api::profile_all))
        .route("/api/profile/{profile_id}", axum::routing::get(api::profile_one))
        .route("/api/race/all", axum::routing::get(api::race_all))
        .route(
            "/api/race/{heat_id}/{round_id}",
            axum::routing::get(api::race_detail),
        )
        .route("/api/options", axum::routing::get(api::options))
        .route("/api", axum::routing::get(api_docs))
        .route("/", axum::routing::get(render_page))
        .route("/event", axum::routing::get(render_page))
        .route("/results", axum::routing::get(render_page))
        .route("/run", axum::routing::get(render_page))
        .route("/current", axum::routing::get(render_page))
        .route("/marshal", axum::routing::get(render_page))
        .route("/format", axum::routing::get(render_page))
        .route("/settings", axum::routing::get(render_page))
        .route("/scanner", axum::routing::get(render_page))
        .route("/streams", axum::routing::get(render_page))
        .route("/stream/results", axum::routing::get(render_page))
        .route("/imdtabler", axum::routing::get(render_page))
        .route("/database", axum::routing::get(render_page))
        .route("/vrxstatus", axum::routing::get(render_page))
        .nest_service("/static", ServeDir::new(&static_dir))
        .layer(socketio_layer)
        .with_state(state);

    let listener = tokio::net::TcpListener::bind(("0.0.0.0", http_port_cfg))
        .await
        .expect("failed to bind port");
    tracing::info!("Running http server at port {}", http_port_cfg);
    axum::serve(listener, app).await.expect("server error");
}

/// Routes that require admin auth when credentials are configured
/// (mirrors the Python @requires_auth set).
const AUTHED_PAGES: &[&str] = &[
    "/first-run", "/run", "/marshal", "/format", "/settings", "/advanced-settings",
    "/scanner", "/updatenodes", "/plugins", "/hardwarelog", "/database", "/vrxstatus",
];

/// Renders the page for the request path (page map in pages::PAGES).
async fn render_page(
    uri: axum::http::Uri,
    headers: axum::http::HeaderMap,
    State(state): State<SharedState>,
) -> impl axum::response::IntoResponse {
    let path = uri.path().to_string();

    // page auth: when admin credentials are configured, authed pages require
    // HTTP basic auth (401 challenge otherwise), matching requires_auth
    if AUTHED_PAGES.contains(&path.as_str())
        && !state.admin_username.is_empty()
        && !state.admin_password.is_empty()
    {
        let authed = headers
            .get(axum::http::header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.strip_prefix("Basic "))
            .and_then(|b| {
                use base64::Engine;
                base64::engine::general_purpose::STANDARD
                    .decode(b)
                    .ok()
            })
            .and_then(|decoded| String::from_utf8(decoded).ok())
            .map(|creds| {
                let (user, pass) = creds.split_once(':').unwrap_or(("", ""));
                user == state.admin_username && pass == state.admin_password
            })
            .unwrap_or(false);
        if !authed {
            let mut resp = axum::http::Response::new(axum::body::Body::empty());
            *resp.status_mut() = axum::http::StatusCode::UNAUTHORIZED;
            resp.headers_mut().insert(
                axum::http::header::WWW_AUTHENTICATE,
                axum::http::HeaderValue::from_static("Basic realm=\"Login Required\""),
            );
            return resp;
        }
    }

    let template = pages::PAGES
        .iter()
        .find(|(p, _)| *p == path)
        .map(|(_, t)| *t)
        .unwrap_or("home.html");
    pages::render(template, state.num_nodes, pages::page_context(&path, &state))
}

async fn api_docs() -> impl axum::response::IntoResponse {
    let body = json!({
        "endpoints": [
            "/api/status", "/api/pilot/all", "/api/pilot/<id>",
            "/api/heat/all", "/api/heat/<id>",
            "/api/class/all", "/api/class/<id>",
            "/api/format/all", "/api/format/<id>",
            "/api/profile/all", "/api/profile/<id>",
            "/api/race/current", "/api/race/all", "/api/race/<heat_id>/<round_id>",
            "/api/options"
        ]
    });
    (
        axum::http::StatusCode::CREATED,
        [(axum::http::header::CONTENT_TYPE, "application/json")],
        axum::Json(body),
    )
}

fn read_admin_creds(data_dir: &str) -> (String, String) {
    let path = format!("{}/config.json", data_dir);
    let default = (String::new(), String::new());
    let Ok(text) = std::fs::read_to_string(&path) else {
        return default;
    };
    let Ok(cfg) = serde_json::from_str::<serde_json::Value>(&text) else {
        return default;
    };
    let secrets = cfg.get("SECRETS");
    let user = secrets
        .and_then(|s| s.get("ADMIN_USERNAME"))
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let pass = secrets
        .and_then(|s| s.get("ADMIN_PASSWORD"))
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    (user, pass)
}

fn read_config_port(data_dir: &str) -> Option<u16> {
    let text = std::fs::read_to_string(format!("{}/config.json", data_dir)).ok()?;
    let cfg = serde_json::from_str::<serde_json::Value>(&text).ok()?;
    cfg.get("GENERAL")?
        .get("HTTP_PORT")?
        .as_u64()
        .map(|p| p as u16)
}