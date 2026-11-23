//! Server-rendered pages via minijinja over the existing Jinja templates
//! (src/server/templates).  Page contexts mirror the Python route handlers.

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use minijinja::value::Kwargs;
use minijinja::Value;
use minijinja::{context, Environment};
use serde_json::json;
use std::sync::OnceLock;

static ENV: OnceLock<Environment<'static>> = OnceLock::new();
static PAGE_STATE: OnceLock<crate::state::SharedState> = OnceLock::new();

/// Stores the shared state for template-time lookups (getOption/getConfig).
pub fn set_state(state: crate::state::SharedState) {
    let _ = PAGE_STATE.set(state);
}

/// Initializes the template environment; called once at startup.
pub fn init(templates_dir: &str) -> Result<(), String> {
    let mut env = Environment::new();
    env.set_loader(minijinja::path_loader(templates_dir));

    // url_for shim: templates reference static assets via
    // url_for('static', filename='...')
    env.add_global(
        "url_for",
        Value::from_function(
            |_endpoint: Value, kwargs: Kwargs| -> Result<Value, minijinja::Error> {
                let filename: Option<String> = kwargs.get("filename")?;
                Ok(Value::from(format!(
                    "/static/{}",
                    filename.unwrap_or_default()
                )))
            },
        ),
    );

    // getOption(name): option value from the database
    env.add_global(
        "getOption",
        Value::from_function(|name: String| -> Value {
            let v = PAGE_STATE
                .get()
                .and_then(|state| crate::db::with_db(state, |conn| crate::db::get_option(conn, &name)));
            Value::from(v.unwrap_or_default())
        }),
    );

    // getConfig(section, key): config.json item with Config.py defaults
    env.add_global(
        "getConfig",
        Value::from_function(|section: String, key: String| -> Value {
            let data_dir = PAGE_STATE
                .get()
                .map(|s| s.data_dir.clone())
                .unwrap_or_default();
            Value::from_serialize(read_config_item(&data_dir, &section, &key))
        }),
    );

    // get_optionNumeric(name) / get_optionInt(name): integer option values
    for fname in ["get_optionNumeric", "get_optionInt"] {
        env.add_global(
            fname,
            Value::from_function(move |name: String| -> i64 {
                let v = PAGE_STATE.get().and_then(|state| {
                    crate::db::with_db(state, |conn| {
                        crate::db::get_option(conn, &name)
                            .and_then(|s| s.parse::<i64>().ok())
                    })
                });
                v.unwrap_or(0)
            }),
        );
    }

    // translation function: pass-through (English) until language packs are
    // ported; templates call __("...") everywhere
    env.add_global(
        "__",
        Value::from_function(|s: String| -> Value { Value::from(s) }),
    );

    // serverInfo used by the layout (release version in asset URLs / about box)
    env.add_global(
        "serverInfo",
        Value::from_serialize(json!({
            "release_version": "4.6.0-dev.1",
            "server_api": 50,
            "node_api_best": 36,
        })),
    );

    // band/channel frequency table for the layouts (pair list; order decides
    // shared frequencies, matching the Python context processor)
    env.add_global(
        "freq_table",
        Value::from_serialize(
            crate::util::BAND_CHANNEL_FREQUENCIES
                .iter()
                .map(|(b, c, f)| {
                    Value::from_serialize(vec![
                        serde_json::Value::from(format!("{}{}", b, c)),
                        serde_json::Value::from(*f as i64),
                    ])
                })
                .collect::<Vec<_>>(),
        ),
    );

    ENV.set(env)
        .map_err(|_| "template env already initialized".to_string())
}

/// Renders a page; mirrors the Python render_template contexts.
pub fn render(template: &str, num_nodes: usize, extra: serde_json::Value) -> Response {
    let env = ENV.get().expect("template env not initialized");
    let tmpl = match env.get_template(template) {
        Ok(t) => t,
        Err(e) => {
            tracing::error!("template {} not found: {}", template, e);
            return (StatusCode::INTERNAL_SERVER_ERROR, "template error").into_response();
        }
    };

    let mut ctx = json!({
        "num_nodes": num_nodes,
        "cluster_has_secondaries": false,
        "led_enabled": false,
        "led_events_enabled": false,
        "vrx_enabled": false,
        "server_messages": "",
    });
    if let (Some(base), Some(extra_obj)) = (ctx.as_object_mut(), extra.as_object()) {
        for (k, v) in extra_obj {
            base.insert(k.clone(), v.clone());
        }
    }

    match tmpl.render(context! { ..Value::from_serialize(&ctx) }) {
        Ok(html) => (
            [(axum::http::header::CONTENT_TYPE, "text/html; charset=utf-8")],
            html,
        )
            .into_response(),
        Err(e) => {
            tracing::error!("template {} render error: {}", template, e);
            (StatusCode::INTERNAL_SERVER_ERROR, format!("template error: {}", e)).into_response()
        }
    }
}

/// The page routes and their templates, mirroring the Python route map.
pub const PAGES: &[(&str, &str)] = &[
    ("/", "home.html"),
    ("/event", "event.html"),
    ("/results", "results.html"),
    ("/run", "run.html"),
    ("/current", "current.html"),
    ("/marshal", "marshal.html"),
    ("/format", "format.html"),
    ("/settings", "settings.html"),
    ("/scanner", "scanner.html"),
    ("/streams", "streams.html"),
    ("/stream/results", "streamresults.html"),
    ("/imdtabler", "imdtabler.html"),
    ("/database", "database.html"),
    ("/vrxstatus", "vrxstatus.html"),
];

/// Extra context for pages that need it (Null entries use defaults).
pub fn page_context(path: &str, state: &crate::state::SharedState) -> serde_json::Value {
    let freqs = crate::db::with_db(state, |conn| {
        let profile_id = crate::db::current_profile_id(conn);
        crate::db::profile_frequencies(conn, profile_id)
    });
    let nodes: Vec<serde_json::Value> = freqs
        .iter()
        .enumerate()
        .filter(|(_, (_, _, f))| *f != 0)
        .map(|(idx, (_, _, f))| json!({"freq": f, "index": idx}))
        .collect();

    match path {
        "/run" | "/current" => json!({ "nodes": nodes }),
        "/format" => json!({
            "has_deleted_races": crate::db::with_db(state, |conn| {
                conn.query_row("SELECT COUNT(*) FROM deleted_race", [], |r| r.get::<_, i64>(0))
                    .map(|c| c > 0)
                    .unwrap_or(false)
            }),
        }),
        _ => serde_json::Value::Null,
    }
}

/// Reads a config.json item with Config.py defaults for common keys.
fn read_config_item(data_dir: &str, section: &str, key: &str) -> serde_json::Value {
    let defaults = |s: &str, k: &str| -> serde_json::Value {
        match (s, k) {
            ("GENERAL", "HTTP_PORT") => json!(5000),
            ("GENERAL", "DEBUG") => json!(false),
            ("GENERAL", "SECONDARY_TIMEOUT") => json!(300),
            ("GENERAL", "SHUTDOWN_BUTTON_GPIOPIN") => json!(18),
            ("GENERAL", "SHUTDOWN_BUTTON_DELAYMS") => json!(2500),
            ("GENERAL", "DB_AUTOBKP_NUM_KEEP") => json!(30),
            ("GENERAL", "RACE_START_DELAY_EXTRA_SECS") => json!(0.9),
            ("HARDWARE", "I2C_BUS") => json!(0),
            ("TIMING", "startThreshLowerAmount") => json!("0"),
            ("TIMING", "startThreshLowerDuration") => json!("0"),
            ("TIMING", "calibrationMode") => json!("1"),
            ("TIMING", "MinLapBehavior") => json!("0"),
            _ => serde_json::Value::Null,
        }
    };
    if let Ok(text) = std::fs::read_to_string(format!("{}/config.json", data_dir)) {
        if let Ok(cfg) = serde_json::from_str::<serde_json::Value>(&text) {
            if let Some(v) = cfg
                .get(section)
                .and_then(|sec| sec.get(key))
            {
                return v.clone();
            }
        }
    }
    defaults(section, key)
}
