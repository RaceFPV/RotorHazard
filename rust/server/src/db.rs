//! Database open/init and row helpers.
//!
//! The Rust server reads and writes the exact same `database.db` produced by
//! the Python implementation: same schema (schema.sql) and same seed data
//! (seed_data.sql, generated from a fresh Python-created database).

use crate::state::SharedState;
use rusqlite::Connection;

const SCHEMA_SQL: &str = include_str!("../../schema_reference.sql");
const SEED_SQL: &str = include_str!("../../seed_data.sql");

pub fn open_or_init(data_dir: &str) -> Result<Connection, rusqlite::Error> {
    std::fs::create_dir_all(data_dir).ok();
    let db_path = format!("{}/database.db", data_dir);
    let fresh = !std::path::Path::new(&db_path).exists()
        || std::fs::metadata(&db_path).map(|m| m.len() == 0).unwrap_or(true);

    let conn = Connection::open(&db_path)?;
    conn.pragma_update(None, "journal_mode", "WAL").ok();
    conn.execute_batch("PRAGMA foreign_keys = OFF;")?;

    if fresh {
        tracing::info!("No database.db found; creating initial database");
        conn.execute_batch(SCHEMA_SQL)?;
        conn.execute_batch(SEED_SQL)?;
    }
    Ok(conn)
}

pub fn get_option(conn: &Connection, name: &str) -> Option<String> {
    conn.query_row(
        "SELECT option_value FROM global_settings WHERE option_name = ?1",
        [name],
        |row| row.get(0),
    )
    .ok()
}

pub fn get_option_int(conn: &Connection, name: &str, default: i64) -> i64 {
    get_option(conn, name)
        .and_then(|v| v.parse::<i64>().ok())
        .unwrap_or(default)
}

pub fn set_option(conn: &Connection, name: &str, value: &str) {
    let existing: Option<i64> = conn
        .query_row(
            "SELECT id FROM global_settings WHERE option_name = ?1",
            [name],
            |row| row.get(0),
        )
        .ok();
    match existing {
        Some(id) => {
            let _ = conn.execute(
                "UPDATE global_settings SET option_value = ?1 WHERE id = ?2",
                rusqlite::params![value, id],
            );
        }
        None => {
            let _ = conn.execute(
                "INSERT INTO global_settings (option_name, option_value) VALUES (?1, ?2)",
                rusqlite::params![name, value],
            );
        }
    }
}

/// Loads the current profile's frequency table as (band, channel, freq) triples.
pub fn profile_frequencies(conn: &Connection, profile_id: i64) -> Vec<(Option<String>, Option<i64>, i64)> {
    let freqs_json: Option<String> = conn
        .query_row(
            "SELECT frequencies FROM profiles WHERE id = ?1",
            [profile_id],
            |row| row.get(0),
        )
        .ok();

    let mut out = Vec::new();
    if let Some(json) = freqs_json {
        if let Ok(val) = serde_json::from_str::<serde_json::Value>(&json) {
            let bands = val.get("b").and_then(|v| v.as_array()).cloned().unwrap_or_default();
            let chans = val.get("c").and_then(|v| v.as_array()).cloned().unwrap_or_default();
            let freqs = val.get("f").and_then(|v| v.as_array()).cloned().unwrap_or_default();
            for i in 0..freqs.len() {
                let band = bands
                    .get(i)
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string());
                let chan = chans.get(i).and_then(|v| v.as_i64());
                let freq = freqs.get(i).and_then(|v| v.as_i64()).unwrap_or(0);
                out.push((band, chan, freq));
            }
        }
    }
    out
}

pub fn current_profile_id(conn: &Connection) -> i64 {
    get_option_int(conn, "currentProfile", 1)
}

pub fn current_format_id(conn: &Connection) -> i64 {
    get_option_int(conn, "currentFormat", 1)
}

pub fn max_round(conn: &Connection, heat_id: i64) -> i64 {
    conn.query_row(
        "SELECT COALESCE(MAX(round_id), 0) FROM saved_race_meta WHERE heat_id = ?1",
        [heat_id],
        |row| row.get(0),
    )
    .unwrap_or(0)
}

/// Formats milliseconds to the M:SS.mmm form used across RH payloads.
pub fn format_time_to_str(millis: f64) -> String {
    let millis = millis.round() as i64;
    let sign = if millis < 0 { "-" } else { "" };
    let millis = millis.abs();
    let minutes = millis / 60000;
    let seconds = (millis % 60000) / 1000;
    let frac = millis % 1000;
    format!("{}{}:{:02}.{:03}", sign, minutes, seconds, frac)
}

pub fn next_pilot_id(conn: &Connection) -> i64 {
    conn.query_row(
        "SELECT COALESCE(MAX(id), 0) + 1 FROM pilot",
        [],
        |row| row.get(0),
    )
    .unwrap_or(1)
}

pub fn next_heat_id(conn: &Connection) -> i64 {
    conn.query_row(
        "SELECT COALESCE(MAX(id), 0) + 1 FROM heat",
        [],
        |row| row.get(0),
    )
    .unwrap_or(1)
}

pub fn next_heat_node_id(conn: &Connection) -> i64 {
    conn.query_row(
        "SELECT COALESCE(MAX(id), 0) + 1 FROM heat_node",
        [],
        |row| row.get(0),
    )
    .unwrap_or(1)
}


/// Shared-state convenience: run a closure with the DB connection.
pub fn with_db<T>(state: &SharedState, f: impl FnOnce(&Connection) -> T) -> T {
    // poison-tolerant: a panic in one handler must not take down the server
    // (Python recovers from per-handler exceptions the same way)
    let conn = state.db.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    f(&conn)
}


/// Reads an integer config item from config.json (mirroring
/// serverconfig.get_item_int); falls back to zero.
pub fn get_option_or_config_int(
    state: &SharedState,
    _conn: &Connection,
    section: &str,
    key: &str,
) -> i64 {
    if let Ok(text) = std::fs::read_to_string(format!("{}/config.json", state.data_dir)) {
        if let Ok(cfg) = serde_json::from_str::<serde_json::Value>(&text) {
            if let Some(v) = cfg.get(section).and_then(|s| s.get(key)) {
                if let Some(i) = v.as_i64() {
                    return i;
                }
                if let Some(s) = v.as_str() {
                    if let Ok(i) = s.parse::<i64>() {
                        return i;
                    }
                }
            }
        }
    }
    0
}
