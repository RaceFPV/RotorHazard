//! Data export/import, ported from the rh_data_export_json and
//! rh_data_import_json bundled plugins.
//!
//! Exporter names use cleanVarName(label) exactly like the Python plugin
//! registry.  The import implements the "RotorHazard 4.0 JSON (Complete)"
//! upsert semantics (match pilots by callsign, formats/profiles/classes by
//! name, remap heat-node pilot ids through source/db id maps).

use crate::db;
use crate::state::SharedState;
use rusqlite::Connection;
use serde_json::{json, Map, Value};

pub const EXPORTER_LABELS: &[&str] = &[
    "JSON (Friendly) / Pilots",
    "JSON (Friendly) / Heats",
    "JSON (Friendly) / Classes",
    "JSON (Friendly) / Formats",
    "JSON (Friendly) / Results",
    "JSON (Friendly) / All",
    "JSON (Complete) / All",
    "JSON (Complete) / Results",
];

pub const IMPORTER_NAME: &str = "RotorHazard_4_0_JSON__Complete_"; // cleanVarName(label)

/// cleanVarName: non-word chars and leading digits become '_'.
pub fn clean_var_name(s: &str) -> String {
    let mut out = String::new();
    for (i, ch) in s.chars().enumerate() {
        if ch.is_alphanumeric() && !(i == 0 && ch.is_ascii_digit()) {
            out.push(ch);
        } else {
            out.push('_');
        }
    }
    out
}

pub fn exporter_list() -> Value {
    json!({
        "exporters": EXPORTER_LABELS
            .iter()
            .map(|label| json!({"name": clean_var_name(label), "label": label}))
            .collect::<Vec<_>>(),
    })
}

pub fn importer_list() -> Value {
    json!({
        "importers": [
            {
                "name": IMPORTER_NAME,
                "label": "RotorHazard 4.0 JSON (Complete)",
                "settings": [
                    {"name": "reset_pilots", "label": "Reset Pilots", "field_type": "CHECKBOX", "value": false},
                    {"name": "reset_formats", "label": "Reset Formats", "field_type": "CHECKBOX", "value": false},
                    {"name": "reset_profiles", "label": "Reset Profiles", "field_type": "CHECKBOX", "value": false},
                    {"name": "reset_classes", "label": "Reset Classes and Race Data", "field_type": "CHECKBOX", "value": false},
                ],
            }
        ],
    })
}

fn table_dicts(conn: &Connection, table: &str) -> Vec<Value> {
    let mut out = Vec::new();
    let mut stmt = conn
        .prepare(&format!("SELECT * FROM {}", table))
        .unwrap();
    let mut rows = stmt.query([]).unwrap();
    while let Some(row) = rows.next().unwrap() {
        let mut obj = Map::new();
        for (idx, col) in row.as_ref().column_names().iter().enumerate() {
            let val: Value = match row.get_ref(idx) {
                Ok(rusqlite::types::ValueRef::Null) => Value::Null,
                Ok(rusqlite::types::ValueRef::Integer(i)) => json!(i),
                Ok(rusqlite::types::ValueRef::Real(f)) => json!(f),
                Ok(rusqlite::types::ValueRef::Text(t)) => {
                    json!(String::from_utf8_lossy(t).to_string())
                }
                Ok(rusqlite::types::ValueRef::Blob(b)) => {
                    json!(String::from_utf8_lossy(b).to_string())
                }
                Err(_) => Value::Null,
            };
            obj.insert((*col).to_string(), val);
        }
        out.push(Value::Object(obj));
    }
    out
}

/// assembles an export payload for the given exporter name.
pub fn assemble(conn: &Connection, exporter_name: &str) -> Option<Value> {
    let mut payload = Map::new();

    if exporter_name == clean_var_name("JSON (Friendly) / Pilots")
        || exporter_name == clean_var_name("JSON (Friendly) / All")
    {
        let mut pilots = Vec::new();
        let mut stmt = conn
            .prepare("SELECT callsign, name, team FROM pilot ORDER BY id")
            .unwrap();
        let mut rows = stmt.query([]).unwrap();
        while let Some(row) = rows.next().unwrap() {
            pilots.push(json!({
                "Callsign": row.get::<_, String>(0).unwrap(),
                "Name": row.get::<_, String>(1).unwrap(),
                "Team": row.get::<_, String>(2).unwrap(),
            }));
        }
        payload.insert("Pilots".into(), json!(pilots));
    }

    if exporter_name == clean_var_name("JSON (Friendly) / Heats")
        || exporter_name == clean_var_name("JSON (Friendly) / All")
    {
        let mut heats = Map::new();
        let mut stmt = conn
            .prepare(
                "SELECT h.id, COALESCE(h.note, 'Heat ' || h.id), c.name
                 FROM heat h LEFT JOIN race_class c ON h.class_id = c.id ORDER BY h.id",
            )
            .unwrap();
        let mut rows = stmt.query([]).unwrap();
        let mut heat_list = Vec::new();
        while let Some(row) = rows.next().unwrap() {
            heat_list.push((
                row.get::<_, i64>(0).unwrap(),
                row.get::<_, String>(1).unwrap(),
                row.get::<_, Option<String>>(2).unwrap(),
            ));
        }
        for (id, displayname, class_name) in heat_list {
            let mut slot_pilots = Map::new();
            let mut sstmt = conn
                .prepare(
                    "SELECT node_index, pilot_id FROM heat_node WHERE heat_id = ?1",
                )
                .unwrap();
            let mut srows = sstmt.query([id]).unwrap();
            while let Some(srow) = srows.next().unwrap() {
                let node_index: Option<i64> = srow.get(0).unwrap();
                let pilot_id: Option<i64> = srow.get(1).unwrap();
                let callsign = pilot_id.and_then(|pid| {
                    conn.query_row("SELECT callsign FROM pilot WHERE id = ?1", [pid], |r| {
                        r.get::<_, String>(0)
                    })
                    .ok()
                });
                if let Some(idx) = node_index {
                    slot_pilots.insert(idx.to_string(), json!(callsign));
                }
            }
            heats.insert(
                id.to_string(),
                json!({
                    "Name": displayname,
                    "Class": class_name,
                    "Pilots": slot_pilots,
                }),
            );
        }
        payload.insert("Heats".into(), Value::Object(heats));
    }

    if exporter_name == clean_var_name("JSON (Friendly) / Classes")
        || exporter_name == clean_var_name("JSON (Friendly) / All")
    {
        let mut classes = Vec::new();
        let mut stmt = conn
            .prepare("SELECT name, description FROM race_class ORDER BY id")
            .unwrap();
        let mut rows = stmt.query([]).unwrap();
        while let Some(row) = rows.next().unwrap() {
            classes.push(json!({
                "Name": row.get::<_, Option<String>>(0).unwrap().unwrap_or_default(),
                "Description": row.get::<_, Option<String>>(1).unwrap().unwrap_or_default(),
            }));
        }
        payload.insert("Classes".into(), json!(classes));
    }

    if exporter_name == clean_var_name("JSON (Friendly) / Formats")
        || exporter_name == clean_var_name("JSON (Friendly) / All")
    {
        let mut formats = Vec::new();
        let mut stmt = conn
            .prepare("SELECT name FROM race_format ORDER BY id")
            .unwrap();
        let mut rows = stmt.query([]).unwrap();
        while let Some(row) = rows.next().unwrap() {
            formats.push(json!({"Name": row.get::<_, String>(0).unwrap()}));
        }
        payload.insert("Formats".into(), json!(formats));
    }

    if exporter_name.starts_with("JSON__Complete") || payload.contains_key("Pilots") {
        // complete sections piggyback on 'All' exporters
        if exporter_name != clean_var_name("JSON (Complete) / Results") {
            payload.insert("Pilot".into(), json!(table_dicts(conn, "pilot")));
            payload.insert("Heat".into(), {
        let mut rows = Vec::new();
        let mut stmt = conn
            .prepare("SELECT id, note, auto_name, class_id, status, auto_frequency, group_id, active
                      FROM heat ORDER BY id")
            .unwrap();
        let mut r = stmt.query([]).unwrap();
        while let Some(row) = r.next().unwrap() {
            rows.push(json!({
                "id": row.get::<_, i64>(0).unwrap(),
                "name": row.get::<_, Option<String>>(1).unwrap(),
                "auto_name": row.get::<_, Option<String>>(2).unwrap(),
                "class_id": row.get::<_, Option<i64>>(3).unwrap(),
                "results": Value::Null,
                "status": row.get::<_, i64>(4).unwrap(),
                "auto_frequency": row.get::<_, bool>(5).unwrap(),
                "group_id": row.get::<_, i64>(6).unwrap(),
                "active": row.get::<_, bool>(7).unwrap(),
            }));
        }
        json!(rows)
    });
            payload.insert("HeatNode".into(), json!(table_dicts(conn, "heat_node")));
            payload.insert("RaceClass".into(), json!(table_dicts(conn, "race_class")));
            payload.insert("RaceFormat".into(), json!(table_dicts(conn, "race_format")));
            // profiles carry frequencies/enter_ats/exit_ats as parsed JSON
            // objects (AlchemyEncoder json.loads behavior in Python)
            payload.insert("Profiles".into(), json!(profiles_model(conn)));
            payload.insert("GlobalSettings".into(), json!(table_dicts(conn, "global_settings")));
        }
    }

    if exporter_name.starts_with("JSON__Complete")
        || exporter_name.contains("Results")
    {
        payload.insert(
            "SavedRaceMeta".into(),
            json!(table_dicts(conn, "saved_race_meta")),
        );
        payload.insert(
            "SavedPilotRace".into(),
            json!(table_dicts(conn, "saved_pilot_race")),
        );
        payload.insert(
            "SavedRaceLap".into(),
            json!(table_dicts(conn, "saved_race_lap")),
        );
    }

    if payload.is_empty() {
        None
    } else {
        Some(Value::Object(payload))
    }
}

/// Profiles serialized with model semantics: JSON-typed fields are emitted
/// as objects, not strings.
fn profiles_model(conn: &Connection) -> Vec<Value> {
    let mut out = Vec::new();
    let mut stmt = conn
        .prepare("SELECT id, name, description, frequencies, enter_ats, exit_ats, f_ratio FROM profiles ORDER BY id")
        .unwrap();
    let mut rows = stmt.query([]).unwrap();
    while let Some(row) = rows.next().unwrap() {
        let as_json = |col: Option<String>| -> Value {
            col.and_then(|s| serde_json::from_str::<Value>(&s).ok()).unwrap_or(Value::Null)
        };
        out.push(json!({
            "id": row.get::<_, i64>(0).unwrap(),
            "name": row.get::<_, String>(1).unwrap(),
            "description": row.get::<_, Option<String>>(2).unwrap(),
            "frequencies": as_json(row.get(3).unwrap()),
            "enter_ats": as_json(row.get(4).unwrap()),
            "exit_ats": as_json(row.get(5).unwrap()),
            "f_ratio": row.get::<_, Option<i64>>(6).unwrap(),
        }));
    }
    out
}

/// Runs the complete-format import; returns true on success.
pub fn run_import(state: &SharedState, source: &str, args: &Value) -> bool {
    let data: Value = match serde_json::from_str(source) {
        Ok(v) => v,
        Err(_) => return false,
    };

    let flag = |name: &str| {
        args.get(name)
            .map(|v| v.as_bool().unwrap_or(false))
            .unwrap_or(false)
    };

    // maps for id remapping (source id -> new db id)
    let mut pilot_map: Map<String, Value> = Map::new();
    let mut class_map: Map<String, Value> = Map::new();
    let mut heat_map: Map<String, Value> = Map::new();

    if let Some(pilots) = data.get("Pilot").and_then(|v| v.as_array()) {
        if flag("reset_pilots") {
            db::with_db(state, |conn| {
                conn.execute("DELETE FROM pilot", []).ok();
            });
        }
        for input_pilot in pilots {
            let callsign = input_pilot.get("callsign").and_then(|v| v.as_str()).unwrap_or("");
            let source_id = input_pilot.get("id").cloned().unwrap_or(Value::Null);
            let new_id = db::with_db(state, |conn| {
                let existing: Option<i64> = conn
                    .query_row("SELECT id FROM pilot WHERE callsign = ?1", [callsign], |r| r.get(0))
                    .ok();
                if let Some(id) = existing {
                    if let Some(v) = input_pilot.get("name").and_then(|v| v.as_str()) {
                        let _ = conn.execute("UPDATE pilot SET name = ?1 WHERE id = ?2", rusqlite::params![v, id]);
                    }
                    if let Some(v) = input_pilot.get("phonetic").and_then(|v| v.as_str()) {
                        let _ = conn.execute("UPDATE pilot SET phonetic = ?1 WHERE id = ?2", rusqlite::params![v, id]);
                    }
                    if let Some(v) = input_pilot.get("team").and_then(|v| v.as_str()) {
                        let _ = conn.execute("UPDATE pilot SET team = ?1 WHERE id = ?2", rusqlite::params![v, id]);
                    }
                    id
                } else {
                    let id = db::next_pilot_id(conn);
                    let name = input_pilot.get("name").and_then(|v| v.as_str()).unwrap_or("");
                    let team = input_pilot.get("team").and_then(|v| v.as_str()).unwrap_or("A");
                    let phonetic = input_pilot.get("phonetic").and_then(|v| v.as_str()).unwrap_or("");
                    let color = input_pilot.get("color").and_then(|v| v.as_str()).unwrap_or("#ffffff");
                    let _ = conn.execute(
                        "INSERT INTO pilot (id, callsign, team, phonetic, name, color, used_frequencies, active)
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6, NULL, 1)",
                        rusqlite::params![id, callsign, team, phonetic, name, color],
                    );
                    id
                }
            });
            let mut entry = Map::new();
            entry.insert("source_id".into(), source_id);
            entry.insert("db_id".into(), json!(new_id));
            pilot_map.insert(callsign.to_string(), Value::Object(entry));
        }
    }

    if let Some(formats) = data.get("RaceFormat").and_then(|v| v.as_array()) {
        if flag("reset_formats") {
            db::with_db(state, |conn| {
                conn.execute("DELETE FROM race_format", []).ok();
            });
        }
        for input_format in formats {
            let name = input_format.get("name").and_then(|v| v.as_str()).unwrap_or("");
            db::with_db(state, |conn| {
                let existing: Option<i64> = conn
                    .query_row("SELECT id FROM race_format WHERE name = ?1", [name], |r| r.get(0))
                    .ok();
                let cols = [
                    ("unlimited_time", "race_mode"),
                    ("race_time_sec", "race_time_sec"),
                    ("lap_grace_sec", "lap_grace_sec"),
                    ("staging_fixed_tones", "staging_fixed_tones"),
                    ("staging_delay_tones", "staging_tones"),
                    ("start_delay_min_ms", "start_delay_min_ms"),
                    ("start_delay_max_ms", "start_delay_max_ms"),
                    ("start_behavior", "start_behavior"),
                    ("win_condition", "win_condition"),
                    ("number_laps_win", "number_laps_win"),
                    ("team_racing_mode", "team_racing_mode"),
                ];
                match existing {
                    Some(id) => {
                        for (key, col) in cols {
                            if let Some(v) = input_format.get(key).and_then(|v| v.as_i64()) {
                                let _ = conn.execute(
                                    &format!("UPDATE race_format SET {} = ?1 WHERE id = ?2", col),
                                    rusqlite::params![v, id],
                                );
                            }
                        }
                    }
                    None => {
                        let id: i64 = conn
                            .query_row("SELECT COALESCE(MAX(id), 0) + 1 FROM race_format", [], |r| r.get(0))
                            .unwrap_or(1);
                        conn.execute(
                            "INSERT INTO race_format (id, name, race_mode, race_time_sec, lap_grace_sec,
                                                      staging_fixed_tones, staging_tones,
                                                      start_delay_min_ms, start_delay_max_ms,
                                                      number_laps_win, win_condition, team_racing_mode,
                                                      start_behavior, points_method)
                             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, NULL)",
                            rusqlite::params![
                                id,
                                name,
                                input_format.get("unlimited_time").and_then(|v| v.as_i64()).unwrap_or(0),
                                input_format.get("race_time_sec").and_then(|v| v.as_i64()).unwrap_or(0),
                                input_format.get("lap_grace_sec").and_then(|v| v.as_i64()).unwrap_or(60),
                                input_format.get("staging_fixed_tones").and_then(|v| v.as_i64()).unwrap_or(3),
                                input_format.get("staging_delay_tones").and_then(|v| v.as_i64()).unwrap_or(0),
                                input_format.get("start_delay_min_ms").and_then(|v| v.as_i64()).unwrap_or(500),
                                input_format.get("start_delay_max_ms").and_then(|v| v.as_i64()).unwrap_or(3500),
                                input_format.get("number_laps_win").and_then(|v| v.as_i64()).unwrap_or(0),
                                input_format.get("win_condition").and_then(|v| v.as_i64()).unwrap_or(1),
                                input_format.get("team_racing_mode").and_then(|v| v.as_i64()).unwrap_or(0),
                                input_format.get("start_behavior").and_then(|v| v.as_i64()).unwrap_or(0),
                            ],
                        )
                        .ok();
                    }
                }
            });
        }
    }

    if let Some(profiles) = data.get("Profiles").and_then(|v| v.as_array()) {
        if flag("reset_profiles") {
            db::with_db(state, |conn| {
                conn.execute("DELETE FROM profiles", []).ok();
            });
        }
        for input_profile in profiles {
            let name = input_profile.get("name").and_then(|v| v.as_str()).unwrap_or("");
            db::with_db(state, |conn| {
                let existing: Option<i64> = conn
                    .query_row("SELECT id FROM profiles WHERE name = ?1", [name], |r| r.get(0))
                    .ok();
                // frequencies may arrive as a JSON string or object; store
                // canonical JSON either way
                let freqs = match input_profile.get("frequencies") {
                    Some(Value::String(s)) => s.clone(),
                    Some(v @ Value::Object(_)) => v.to_string(),
                    _ => json!({"b": [], "c": [], "f": []}).to_string(),
                };
                match existing {
                    Some(id) => {
                        let _ = conn.execute(
                            "UPDATE profiles SET frequencies = ?1 WHERE id = ?2",
                            rusqlite::params![freqs, id],
                        );
                    }
                    None => {
                        let id: i64 = conn
                            .query_row("SELECT COALESCE(MAX(id), 0) + 1 FROM profiles", [], |r| r.get(0))
                            .unwrap_or(1);
                        let _ = conn.execute(
                            "INSERT INTO profiles (id, name, description, frequencies, enter_ats, exit_ats, f_ratio)
                             VALUES (?1, ?2, NULL, ?3, NULL, NULL, NULL)",
                            rusqlite::params![id, name, freqs],
                        );
                    }
                }
            });
        }
    }

    if let Some(settings) = data.get("GlobalSettings").and_then(|v| v.as_array()) {
        const INVALID: &[&str] = &[
            "server_api", "secret_key", "currentProfile", "currentFormat", "currentHeat",
            "eventResults", "eventResults_cacheStatus",
        ];
        for setting in settings {
            let name = setting.get("option_name").and_then(|v| v.as_str()).unwrap_or("");
            if INVALID.contains(&name) {
                continue;
            }
            let value = setting.get("option_value");
            db::with_db(state, |conn| {
                let text = match value {
                    Some(Value::String(s)) => s.clone(),
                    Some(Value::Null) | None => return,
                    Some(other) => other.to_string(),
                };
                db::set_option(conn, name, &text);
            });
        }
    }

    if let Some(classes) = data.get("RaceClass").and_then(|v| v.as_array()) {
        if flag("reset_classes") {
            db::with_db(state, |conn| {
                conn.execute("DELETE FROM heat", []).ok();
                conn.execute("DELETE FROM race_class", []).ok();
            });
        }
        for input_class in classes {
            let name = input_class.get("name").and_then(|v| v.as_str()).unwrap_or("");
            let source_id = input_class.get("id").cloned().unwrap_or(Value::Null);
            let new_id = db::with_db(state, |conn| {
                let existing: Option<i64> = conn
                    .query_row("SELECT id FROM race_class WHERE name = ?1", [name], |r| r.get(0))
                    .ok();
                match existing {
                    Some(id) => {
                        if let Some(v) = input_class.get("description").and_then(|v| v.as_str()) {
                            let _ = conn.execute(
                                "UPDATE race_class SET description = ?1 WHERE id = ?2",
                                rusqlite::params![v, id],
                            );
                        }
                        id
                    }
                    None => {
                        let id: i64 = conn
                            .query_row("SELECT COALESCE(MAX(id), 0) + 1 FROM race_class", [], |r| r.get(0))
                            .unwrap_or(1);
                        let _ = conn.execute(
                            "INSERT INTO race_class (id, name, description, format_id, win_condition,
                                                     \"cacheStatus\", \"rankStatus\", rounds,
                                                     \"heatAdvanceType\", \"roundType\", \"order\", active)
                             VALUES (?1, ?2, '', NULL, '0', '{}', '{}', 0, 0, 0, NULL, 1)",
                            rusqlite::params![id, name],
                        );
                        id
                    }
                }
            });
            let mut entry = Map::new();
            entry.insert("source_id".into(), source_id);
            entry.insert("db_id".into(), json!(new_id));
            class_map.insert(name.to_string(), Value::Object(entry));
        }
    }

    if let Some(heats) = data.get("Heat").and_then(|v| v.as_array()) {
        for input_heat in heats {
            let displayname = input_heat
                .get("name")
                .and_then(|v| v.as_str())
                .or_else(|| input_heat.get("note").and_then(|v| v.as_str()))
                .unwrap_or("");
            let source_id = input_heat.get("id").cloned().unwrap_or(Value::Null);
            let new_id = db::with_db(state, |conn| {
                let existing: Option<i64> = conn
                    .query_row("SELECT id FROM heat WHERE note = ?1", [displayname], |r| r.get(0))
                    .ok();
                match existing {
                    Some(id) => id,
                    None => {
                        let id = db::next_heat_id(conn);
                        let _ = conn.execute(
                            "INSERT INTO heat (id, note, auto_name, class_id, results, \"cacheStatus\",
                                               \"order\", status, auto_frequency, group_id, active,
                                               coop_best_time, coop_num_laps)
                             VALUES (?1, ?2, NULL, NULL, NULL, '{}', NULL, 0, 0, 0, 1, NULL, NULL)",
                            rusqlite::params![id, displayname],
                        );
                        for node_index in 0..state.num_nodes as i64 {
                            let _ = conn.execute(
                                "INSERT INTO heat_node (id, heat_id, node_index, pilot_id, color, method, seed_rank, seed_id)
                                 VALUES (?1, ?2, ?3, NULL, NULL, 0, NULL, NULL)",
                                rusqlite::params![db::next_heat_node_id(conn), id, node_index],
                            );
                        }
                        id
                    }
                }
            });
            let mut entry = Map::new();
            entry.insert("source_id".into(), source_id);
            entry.insert("db_id".into(), json!(new_id));
            heat_map.insert(displayname.to_string(), Value::Object(entry));
        }
    }

    // heat nodes: remap pilots through the source/db maps
    if let Some(heat_nodes) = data.get("HeatNode").and_then(|v| v.as_array()) {
        for input_hn in heat_nodes {
            let heat_source = input_hn.get("heat_id").cloned().unwrap_or(Value::Null);
            let node_index = input_hn.get("node_index").and_then(|v| v.as_i64());
            let method = input_hn.get("method").and_then(|v| v.as_i64()).unwrap_or(0);
            let pilot_source = input_hn.get("pilot_id").cloned().unwrap_or(Value::Null);
            let Some(node_index) = node_index else { continue };

            // find the imported heat by source id
            let target_heat = heat_map
                .values()
                .find(|e| e.get("source_id") == Some(&heat_source))
                .and_then(|e| e.get("db_id").and_then(|v| v.as_i64()));
            let Some(target_heat) = target_heat else { continue };

            // remap pilot
            let mut remapped_pilot: Option<i64> = None;
            if pilot_source.is_null() {
                remapped_pilot = None;
            } else if let Some(entry) = pilot_map.values().find(|e| {
                e.get("source_id") == Some(&pilot_source)
            }) {
                remapped_pilot = entry.get("db_id").and_then(|v| v.as_i64());
            }

            db::with_db(state, |conn| {
                let slot_id: Option<i64> = conn
                    .query_row(
                        "SELECT id FROM heat_node WHERE heat_id = ?1 AND node_index = ?2",
                        rusqlite::params![target_heat, node_index],
                        |r| r.get(0),
                    )
                    .ok();
                if let Some(slot_id) = slot_id {
                    if method == 1 {
                        // ProgramMethod.ASSIGN
                        let _ = conn.execute(
                            "UPDATE heat_node SET pilot_id = ?1, method = 1 WHERE id = ?2",
                            rusqlite::params![remapped_pilot, slot_id],
                        );
                    }
                }
            });
        }
    }

    true
}