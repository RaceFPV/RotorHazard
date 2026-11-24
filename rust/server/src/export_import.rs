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
    "CSV (Friendly) / Pilots",
    "CSV (Friendly) / Heats",
    "CSV (Friendly) / Classes",
    "CSV (Friendly) / Formats",
    "CSV (Friendly) / Results",
    "CSV (Friendly) / All",
];

// -- CSV export (rh_data_export_csv port) --

/// One CSV cell; QUOTE_NONNUMERIC semantics: strings quoted (with ""
/// escaping), numbers bare, None empty.
enum CsvCell {
    S(String),
    N(i64),
    F(f64),
    Empty,
}

fn csv_cell(v: impl Into<CsvCell>) -> CsvCell {
    v.into()
}

impl From<String> for CsvCell {
    fn from(s: String) -> Self {
        CsvCell::S(s)
    }
}
impl From<&str> for CsvCell {
    fn from(s: &str) -> Self {
        CsvCell::S(s.to_string())
    }
}
impl From<i64> for CsvCell {
    fn from(n: i64) -> Self {
        CsvCell::N(n)
    }
}
impl From<Option<String>> for CsvCell {
    fn from(v: Option<String>) -> Self {
        match v {
            Some(s) => CsvCell::S(s),
            None => CsvCell::Empty,
        }
    }
}
impl From<Option<i64>> for CsvCell {
    fn from(v: Option<i64>) -> Self {
        match v {
            Some(n) => CsvCell::N(n),
            None => CsvCell::Empty,
        }
    }
}
impl From<f64> for CsvCell {
    fn from(f: f64) -> Self {
        CsvCell::F(f)
    }
}

fn csv_row(cells: &[CsvCell]) -> String {
    cells
        .iter()
        .map(|c| match c {
            CsvCell::S(s) => format!("\"{}\"", s.replace('"', "\"\"")),
            CsvCell::N(n) => n.to_string(),
            CsvCell::F(f) => {
                if f.fract() == 0.0 && f.abs() < 1e15 {
                    format!("{}.0", *f as i64)
                } else {
                    f.to_string()
                }
            }
            CsvCell::Empty => String::new(),
        })
        .collect::<Vec<_>>()
        .join(",")
}

fn csv_document(sections: Vec<(&str, Vec<Vec<CsvCell>>)>) -> String {
    // assemble_all layout: [datatype header], rows..., blank line
    let mut out = String::new();
    for (name, rows) in sections {
        out.push_str(&csv_row(&[csv_cell(name)]));
        out.push('\n');
        for row in rows {
            out.push_str(&csv_row(row.as_slice()));
            out.push('\n');
        }
        out.push('\n');
    }
    out
}

fn csv_pilots(conn: &Connection) -> Vec<Vec<CsvCell>> {
    let mut rows = vec![vec![
        csv_cell("Callsign"),
        csv_cell("Name"),
        csv_cell("Team"),
    ]];
    let mut stmt = conn
        .prepare("SELECT callsign, name, team FROM pilot ORDER BY id")
        .unwrap();
    let mut r = stmt.query([]).unwrap();
    while let Some(row) = r.next().unwrap() {
        rows.push(vec![
            csv_cell(row.get::<_, String>(0).unwrap()),
            csv_cell(row.get::<_, Option<String>>(1).unwrap()),
            csv_cell(row.get::<_, Option<String>>(2).unwrap()),
        ]);
    }
    rows
}

fn csv_heats(conn: &Connection) -> Vec<Vec<CsvCell>> {
    let mut rows = vec![vec![
        csv_cell("Name"),
        csv_cell("Class"),
        csv_cell("Pilots"),
    ]];
    let mut stmt = conn
        .prepare(
            "SELECT h.id, COALESCE(h.note, 'Heat ' || h.id), c.name \
             FROM heat h LEFT JOIN race_class c ON h.class_id = c.id ORDER BY h.id",
        )
        .unwrap();
    let mut r = stmt.query([]).unwrap();
    while let Some(row) = r.next().unwrap() {
        let heat_id: i64 = row.get(0).unwrap();
        let displayname: String = row.get(1).unwrap();
        let class_name: Option<String> = row.get(2).unwrap();
        let mut cells = vec![csv_cell(displayname), csv_cell(class_name)];
        let mut sstmt = conn
            .prepare("SELECT pilot_id FROM heat_node WHERE heat_id = ?1")
            .unwrap();
        let mut srows = sstmt.query([heat_id]).unwrap();
        while let Some(srow) = srows.next().unwrap() {
            let pilot_id: Option<i64> = srow.get(0).unwrap();
            match pilot_id {
                Some(pid) => {
                    let callsign = conn
                        .query_row("SELECT callsign FROM pilot WHERE id = ?1", [pid], |r| {
                            r.get::<_, String>(0)
                        })
                        .unwrap_or_default();
                    cells.push(csv_cell(callsign));
                }
                None => cells.push(csv_cell("-")),
            }
        }
        rows.push(cells);
    }
    rows
}

fn csv_classes(conn: &Connection) -> Vec<Vec<CsvCell>> {
    let mut rows = vec![vec![
        csv_cell("Name"),
        csv_cell("Description"),
        csv_cell("Race Format"),
    ]];
    let mut stmt = conn
        .prepare(
            "SELECT c.name, c.description, f.name FROM race_class c \
             LEFT JOIN race_format f ON c.format_id = f.id ORDER BY c.id",
        )
        .unwrap();
    let mut r = stmt.query([]).unwrap();
    while let Some(row) = r.next().unwrap() {
        rows.push(vec![
            csv_cell(row.get::<_, Option<String>>(0).unwrap()),
            csv_cell(row.get::<_, Option<String>>(1).unwrap()),
            csv_cell(row.get::<_, Option<String>>(2).unwrap()),
        ]);
    }
    rows
}

fn csv_formats(conn: &Connection) -> Vec<Vec<CsvCell>> {
    // label tables match the Python plugin's translations (English defaults)
    let timer_modes = ["Fixed Time", "No Time Limit"];
    let tones = ["None", "One", "Each Second"];
    let win_conditions = [
        "None",
        "Most Laps in Fastest Time",
        "First to X Laps",
        "Fastest Lap",
        "Fastest Consecutive Laps",
        "Most Laps Only",
        "Most Laps Only with Overtime",
    ];
    let start_behaviors = ["Hole Shot", "First Lap", "Staggered Start"];
    let mut rows = vec![vec![
        csv_cell("Name"),
        csv_cell("Race Clock Mode"),
        csv_cell("Timer Duration (seconds)"),
        csv_cell("Minimum Start Delay"),
        csv_cell("Maximum Start Delay"),
        csv_cell("Staging Tones"),
        csv_cell("First Crossing"),
        csv_cell("Win Condition"),
        csv_cell("Number of Laps to Win"),
        csv_cell("Team Racing Mode"),
    ]];
    let mut stmt = conn
        .prepare(
            "SELECT name, race_mode, race_time_sec, start_delay_min_ms, start_delay_max_ms, \
             staging_delay_tones, start_behavior, win_condition, number_laps_win, team_racing_mode \
             FROM race_format ORDER BY id",
        )
        .unwrap();
    let mut r = stmt.query([]).unwrap();
    while let Some(row) = r.next().unwrap() {
        let unlimited = row.get::<_, i64>(1).unwrap() as usize;
        let tones_idx = row.get::<_, i64>(5).unwrap() as usize;
        let start_idx = row.get::<_, i64>(6).unwrap() as usize;
        let win_idx = row.get::<_, i64>(7).unwrap() as usize;
        rows.push(vec![
            csv_cell(row.get::<_, String>(0).unwrap()),
            csv_cell(
                timer_modes
                    .get(unlimited)
                    .copied()
                    .unwrap_or("Fixed Time")
                    .to_string(),
            ),
            csv_cell(row.get::<_, i64>(2).unwrap()),
            csv_cell(row.get::<_, i64>(3).unwrap()),
            csv_cell(row.get::<_, i64>(4).unwrap()),
            csv_cell(tones.get(tones_idx).copied().unwrap_or("None").to_string()),
            csv_cell(
                start_behaviors
                    .get(start_idx)
                    .copied()
                    .unwrap_or("Hole Shot")
                    .to_string(),
            ),
            csv_cell(
                win_conditions
                    .get(win_idx)
                    .copied()
                    .unwrap_or("None")
                    .to_string(),
            ),
            csv_cell(row.get::<_, i64>(8).unwrap()),
            csv_cell(row.get::<_, i64>(9).unwrap()),
        ]);
    }
    rows
}

/// Results CSV: one section per saved race (newest last), built from the
/// cached leaderboard in saved_race_meta.
fn csv_results(conn: &Connection) -> Option<Vec<Vec<CsvCell>>> {
    let mut races: Vec<(i64, i64, Option<i64>, String)> = Vec::new();
    let mut stmt = conn
        .prepare(
            "SELECT id, heat_id, round_id, results FROM saved_race_meta \
             WHERE results IS NOT NULL ORDER BY id",
        )
        .unwrap();
    let mut r = stmt.query([]).unwrap();
    while let Some(row) = r.next().unwrap() {
        races.push((
            row.get(0).unwrap(),
            row.get(1).unwrap(),
            row.get(2).unwrap(),
            row.get(3).unwrap(),
        ));
    }
    if races.is_empty() {
        return None;
    }
    let mut rows: Vec<Vec<CsvCell>> = Vec::new();
    for (race_id, heat_id, round_id, results_json) in races {
        let results: Value = serde_json::from_str(&results_json).unwrap_or(json!({}));
        let heat_name: String = conn
            .query_row(
                "SELECT COALESCE(note, 'Heat ' || id) FROM heat WHERE id = ?1",
                [heat_id],
                |r| r.get(0),
            )
            .unwrap_or_else(|_| format!("Heat {}", heat_id));
        rows.push(vec![csv_cell(format!(
            "{} Round {}",
            heat_name,
            round_id.unwrap_or(0)
        ))]);
        rows.push(vec![
            csv_cell("Seat"),
            csv_cell("Rank"),
            csv_cell("Pilot"),
            csv_cell("Laps"),
            csv_cell("Total"),
            csv_cell("Avg."),
            csv_cell("Fastest"),
            csv_cell("Consecutive"),
            csv_cell("Team"),
        ]);
        let board = results
            .get("by_race_time")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();
        for entry in board {
            let consec = format!(
                "{}/{}",
                entry
                    .get("consecutives_base")
                    .and_then(|v| v.as_i64())
                    .unwrap_or(0),
                entry
                    .get("consecutives")
                    .and_then(|v| v.as_i64())
                    .unwrap_or(0)
            );
            rows.push(vec![
                csv_cell(entry.get("node").and_then(|v| v.as_i64()).unwrap_or(0)),
                csv_cell(
                    entry
                        .get("position")
                        .and_then(|v| v.as_i64())
                        .unwrap_or(0),
                ),
                csv_cell(
                    entry
                        .get("callsign")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                ),
                csv_cell(entry.get("laps").and_then(|v| v.as_i64()).unwrap_or(0)),
                csv_cell(
                    entry
                        .get("total_time")
                        .and_then(|v| v.as_f64())
                        .unwrap_or(0.0),
                ),
                csv_cell(
                    entry
                        .get("average_lap")
                        .and_then(|v| v.as_f64())
                        .unwrap_or(0.0),
                ),
                csv_cell(
                    entry
                        .get("fastest_lap")
                        .and_then(|v| v.as_f64())
                        .unwrap_or(0.0),
                ),
                csv_cell(consec),
                csv_cell(
                    entry
                        .get("team_name")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                ),
            ]);
        }
        rows.push(Vec::new());
        let _ = race_id;
    }
    Some(rows)
}

/// Assembles a CSV export for the given exporter name
/// (clean-var-name form: 'CSV__Friendly__Pilots' etc.).
pub fn assemble_csv(conn: &Connection, exporter_name: &str) -> Option<String> {
    let name = exporter_name.to_string();
    if name == clean_var_name("CSV (Friendly) / Pilots") {
        let rows = csv_pilots(conn);
        let mut out = String::new();
        for row in &rows {
            out.push_str(&csv_row(row.as_slice()));
            out.push('\n');
        }
        return Some(out);
    }
    let sections: Vec<(&str, Vec<Vec<CsvCell>>)> = match name.as_str() {
        n if n == clean_var_name("CSV (Friendly) / Heats") => {
            vec![("Heats", csv_heats(conn))]
        }
        n if n == clean_var_name("CSV (Friendly) / Classes") => {
            vec![("Classes", csv_classes(conn))]
        }
        n if n == clean_var_name("CSV (Friendly) / Formats") => {
            vec![("Formats", csv_formats(conn))]
        }
        n if n == clean_var_name("CSV (Friendly) / Results") => {
            csv_results(conn).map(|rows| vec![("Results", rows)])?
        }
        n if n == clean_var_name("CSV (Friendly) / All") => {
            let mut sections = vec![
                ("Pilots", csv_pilots(conn)),
                ("Heats", csv_heats(conn)),
                ("Classes", csv_classes(conn)),
                ("Formats", csv_formats(conn)),
            ];
            if let Some(rows) = csv_results(conn) {
                sections.push(("Results", rows));
            }
            sections
        }
        _ => return None,
    };
    Some(csv_document(sections))
}

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
                        for node_index in 0..state.node_count() as i64 {
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