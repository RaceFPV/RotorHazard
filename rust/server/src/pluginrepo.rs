//! Community plugin directory (RHCP).
//!
//! The Python server fetches a JSON index of community plugins and a category map
//! from `rhcp.hazardcreative.com`, caches them, and sends both to the plugins page
//! under the `plugin_repo` message. This is the same thing: one fetch, cached with
//! a TTL, shaped exactly like the payload `rh-ui.js` and `plugins.html` expect.
//!
//! Fetching is blocking on purpose - it runs inside `spawn_blocking` from the
//! socket handler, so a slow or missing network never stalls the timer.

use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use serde_json::{json, Map, Value};

pub const DATA_URL: &str = "https://rhcp.hazardcreative.com/v1/plugin/data.json";
pub const CATEGORIES_URL: &str = "https://rhcp.hazardcreative.com/v1/plugin/categories.json";

/// The RHAPI level plugins see through the bridge (`ServerInfo.rhapi_version`).
/// Kept in one place so "supported" means the same thing here and to a plugin.
pub const RHAPI_VERSION: (u64, u64) = (1, 7);

const CACHE_TTL: Duration = Duration::from_secs(3600);
const TIMEOUT: Duration = Duration::from_secs(10);

fn cache() -> &'static Mutex<Option<(Instant, Value)>> {
    static CACHE: OnceLock<Mutex<Option<(Instant, Value)>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(None))
}

fn empty_payload() -> Value {
    json!({ "remote_data": {}, "remote_categories": {} })
}

/// `"1.7"` against `(1, 7)` - true when the requirement is met.
pub fn version_at_least(required: &str, have: (u64, u64)) -> bool {
    let mut parts = required.trim().split('.');
    let major = parts.next().and_then(|p| p.trim().parse::<u64>().ok());
    let minor = parts
        .next()
        .and_then(|p| p.trim().parse::<u64>().ok())
        .unwrap_or(0);
    match major {
        Some(major) => (major, minor) <= have,
        None => false,
    }
}

/// The `plugin_repo` payload for a registry dump and category map.
pub fn payload(data: &Value, categories: &Value, rhapi: (u64, u64)) -> Value {
    let mut remote = Map::new();
    if let Some(entries) = data.as_object() {
        for (id, entry) in entries {
            let mut entry = entry.clone();
            let supported = entry
                .get("manifest")
                .and_then(|manifest| manifest.get("required_rhapi_version"))
                .and_then(|value| value.as_str())
                .map(|required| version_at_least(required, rhapi))
                .unwrap_or(false);
            if let Value::Object(map) = &mut entry {
                map.insert("rh_version_supported".to_string(), json!(supported));
            }
            remote.insert(id.clone(), entry);
        }
    }
    json!({
        "remote_categories": categories.clone(),
        "remote_data": remote,
    })
}

pub fn fetch_json(url: &str) -> Result<Value, String> {
    let response = ureq::get(url)
        .config()
        .timeout_global(Some(TIMEOUT))
        .build()
        .call()
        .map_err(|err| err.to_string())?;
    let body = response
        .into_body()
        .read_to_string()
        .map_err(|err| err.to_string())?;
    serde_json::from_str(&body).map_err(|err| err.to_string())
}

/// The cached directory, fetched when stale. A failed refresh keeps the last
/// good copy (or an empty directory) rather than breaking the page.
pub fn directory(rhapi: (u64, u64), force: bool) -> Value {
    let mut guard = cache().lock().unwrap();
    if !force {
        if let Some((fetched_at, payload)) = guard.as_ref() {
            if fetched_at.elapsed() < CACHE_TTL {
                return payload.clone();
            }
        }
    }
    match (fetch_json(DATA_URL), fetch_json(CATEGORIES_URL)) {
        (Ok(data), Ok(categories)) => {
            let payload = payload(&data, &categories, rhapi);
            *guard = Some((Instant::now(), payload.clone()));
            payload
        }
        _ => guard
            .as_ref()
            .map(|(_, payload)| payload.clone())
            .unwrap_or_else(empty_payload),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DATA: &str = include_str!("../tests/fixtures/rhcp_data.json");
    const CATEGORIES: &str = include_str!("../tests/fixtures/rhcp_categories.json");

    #[test]
    fn requirement_comparison_uses_major_minor() {
        assert!(version_at_least("1.7", (1, 7)));
        assert!(version_at_least("1.1", (1, 7)));
        assert!(!version_at_least("1.8", (1, 7)));
        assert!(!version_at_least("2.0", (1, 7)));
        assert!(version_at_least("1", (1, 7)), "a bare major means .0");
        assert!(!version_at_least("not-a-version", (1, 7)));
    }

    #[test]
    fn payload_marks_plugins_the_server_can_run() {
        let data: Value = serde_json::from_str(DATA).unwrap();
        let categories: Value = serde_json::from_str(CATEGORIES).unwrap();
        let out = payload(&data, &categories, RHAPI_VERSION);

        let remote = out.get("remote_data").and_then(|v| v.as_object()).unwrap();
        assert_eq!(remote.len(), 2, "every entry is forwarded");

        let supported = remote
            .values()
            .find(|entry| entry["manifest"]["domain"] == "fixture_ok")
            .expect("fixture_ok present");
        assert_eq!(supported["rh_version_supported"], json!(true));
        assert_eq!(supported["repository"], json!("RaceFPV/fixture-ok"));
        assert_eq!(supported["manifest"]["name"], json!("Fixture OK"));

        let unsupported = remote
            .values()
            .find(|entry| entry["manifest"]["domain"] == "fixture_future")
            .expect("fixture_future present");
        assert_eq!(
            unsupported["rh_version_supported"],
            json!(false),
            "a plugin needing a newer RHAPI is listed but not installable"
        );

        assert_eq!(
            out["remote_categories"], categories,
            "categories pass through untouched"
        );
    }

    #[test]
    fn a_release_list_survives_the_round_trip() {
        let data: Value = serde_json::from_str(DATA).unwrap();
        let out = payload(&data, &json!({}), RHAPI_VERSION);
        let entry = out["remote_data"]
            .as_object()
            .unwrap()
            .values()
            .find(|entry| entry["manifest"]["domain"] == "fixture_ok")
            .unwrap();
        let releases = entry["releases"].as_array().unwrap();
        assert_eq!(releases.len(), 1);
        assert_eq!(releases[0]["tag_name"], json!("1.0.0"));
    }

    #[test]
    fn an_empty_registry_is_an_empty_payload() {
        let out = payload(&json!({}), &json!({}), RHAPI_VERSION);
        assert_eq!(out, empty_payload());
    }
}
