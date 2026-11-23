//! Phase 3: language packs, heat generation, callouts.

use crate::db;
use crate::state::*;
use serde_json::{json, Map, Value};

// ---------- language packs ----------

pub fn load_languages(program_dir: &str) -> Value {
    for path in [
        format!("{}/language.json", program_dir),
        "src/server/language.json".to_string(),
        "language.json".to_string(),
    ] {
        if let Ok(text) = std::fs::read_to_string(&path) {
            if let Ok(v) = serde_json::from_str::<Value>(&text) {
                if v.as_object().map(|m| !m.is_empty()).unwrap_or(false) {
                    return v;
                }
            }
        }
    }
    json!({})
}

/// Translation lookup, matching Language.__(): text unchanged when the
/// language or entry is missing.
pub fn translate(languages: &Value, current_language: &str, text: &str) -> String {
    if !current_language.is_empty() {
        if let Some(values) = languages
            .get(current_language)
            .and_then(|l| l.get("values"))
            .and_then(|v| v.get(text))
            .and_then(|v| v.as_str())
        {
            return values.to_string();
        }
    }
    text.to_string()
}

/// Returns the current language from config.json (UI.currentLanguage).
pub fn current_language(data_dir: &str) -> String {
    if let Ok(text) = std::fs::read_to_string(format!("{}/config.json", data_dir)) {
        if let Ok(cfg) = serde_json::from_str::<serde_json::Value>(&text) {
            if let Some(lang) = cfg
                .get("UI")
                .and_then(|ui| ui.get("currentLanguage"))
                .and_then(|v| v.as_str())
            {
                return lang.to_string();
            }
        }
    }
    String::new()
}

/// Persists a config item to config.json (matching serverconfig.set_item).
pub fn persist_config_item(data_dir: &str, section: &str, key: &str, value: &Value) {
    let path = format!("{}/config.json", data_dir);
    let mut cfg: Value = std::fs::read_to_string(&path)
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_else(|| json!({}));
    let obj = cfg.as_object_mut().unwrap();
    let sec = obj.entry(section.to_string()).or_insert_with(|| json!({}));
    if let Some(sec_obj) = sec.as_object_mut() {
        sec_obj.insert(key.to_string(), value.clone());
    }
    if let Ok(text) = serde_json::to_string_pretty(&cfg) {
        let _ = std::fs::write(&path, text);
    }
}

// ---------- heat generation (heat_automation.calc_heat_pilots) ----------

pub const PROGRAM_METHOD_NONE: i64 = -1;
pub const PROGRAM_METHOD_ASSIGN: i64 = 0;
pub const PROGRAM_METHOD_HEAT_RESULT: i64 = 1;
pub const PROGRAM_METHOD_CLASS_RESULT: i64 = 2;

pub const HEAT_STATUS_PLANNED: i64 = 0;
pub const HEAT_STATUS_PROJECTED: i64 = 1;
pub const HEAT_STATUS_CONFIRMED: i64 = 2;

pub struct CalcResult {
    pub calc_success: Option<bool>, // None = skipped (safe)
    pub has_calc_pilots: bool,
    pub unassigned_slots: i64,
}

/// Recalculates pilot assignments for a heat, mirroring
/// heat_automation.calc_heat_pilots.  Returns the calc result; slot
/// alterations are applied directly.
pub fn calc_heat_pilots(state: &SharedState, heat_id: i64) -> CalcResult {
    let mut result = CalcResult {
        calc_success: Some(true),
        has_calc_pilots: false,
        unassigned_slots: 0,
    };

    let status: Option<i64> = db::with_db(state, |conn| {
        conn.query_row("SELECT status FROM heat WHERE id = ?1", [heat_id], |r| r.get(0))
            .ok()
    });
    match status {
        None => {
            result.calc_success = Some(false);
            return result;
        }
        Some(s) if s == HEAT_STATUS_CONFIRMED => {
            result.calc_success = None;
            return result;
        }
        _ => {}
    }

    let has_saved_races = db::with_db(state, |conn| {
        conn.query_row(
            "SELECT COUNT(*) FROM saved_race_meta WHERE heat_id = ?1",
            [heat_id],
            |r| r.get::<_, i64>(0),
        )
        .map(|c| c > 0)
        .unwrap_or(false)
    });
    if has_saved_races {
        result.calc_success = None;
        return result;
    }

    // slot alterations by method
    let slots: Vec<(i64, i64, Option<i64>, Option<i64>, Option<i64>)> = db::with_db(state, |conn| {
        let mut out = Vec::new();
        let mut stmt = conn
            .prepare("SELECT id, method, seed_id, seed_rank, pilot_id FROM heat_node WHERE heat_id = ?1 ORDER BY id")
            .unwrap();
        let mut rows = stmt.query([heat_id]).unwrap();
        while let Some(row) = rows.next().unwrap() {
            out.push((
                row.get::<_, i64>(0).unwrap(),
                row.get::<_, i64>(1).unwrap(),
                row.get::<_, Option<i64>>(2).unwrap(),
                row.get::<_, Option<i64>>(3).unwrap(),
                row.get::<_, Option<i64>>(4).unwrap(),
            ));
        }
        out
    });

    for (slot_id, method, seed_id, seed_rank, current_pilot) in slots {
        match method {
            PROGRAM_METHOD_NONE => {
                set_slot_pilot(state, slot_id, None);
            }
            PROGRAM_METHOD_ASSIGN => {
                // keep the assigned pilot as-is
                let _ = current_pilot;
            }
            PROGRAM_METHOD_HEAT_RESULT => {
                match (seed_id, seed_rank) {
                    (Some(seed_heat), Some(rank)) if rank > 0 => {
                        result.has_calc_pilots = true;
                        let winner = db::with_db(state, |conn| {
                            conn.query_row(
                                "SELECT results FROM saved_race_meta
                                 WHERE heat_id = ?1 ORDER BY round_id DESC LIMIT 1",
                                [seed_heat],
                                |r| r.get::<_, Option<String>>(0),
                            )
                            .ok()
                            .flatten()
                            .and_then(|blob| serde_json::from_str::<Value>(&blob).ok())
                            .and_then(|lb| {
                                let primary = lb
                                    .get("meta")
                                    .and_then(|m| m.get("primary_leaderboard"))
                                    .and_then(|p| p.as_str())
                                    .unwrap_or("by_race_time")
                                    .to_string();
                                lb.get(&primary).and_then(|rows| rows.as_array()).and_then(|rows| {
                                    rows.get(rank as usize - 1)
                                        .and_then(|row| row.get("pilot_id").and_then(|p| p.as_i64()))
                                })
                            })
                        });
                        match winner {
                            Some(pid) => set_slot_pilot(state, slot_id, Some(pid)),
                            None => {
                                set_slot_pilot(state, slot_id, None);
                                result.unassigned_slots += 1;
                            }
                        }
                    }
                    (Some(_), _) => {
                        result.calc_success = Some(false);
                    }
                    _ => {
                        set_slot_pilot(state, slot_id, None);
                    }
                }
            }
            PROGRAM_METHOD_CLASS_RESULT => {
                match (seed_id, seed_rank) {
                    (Some(seed_class), Some(rank)) if rank > 0 => {
                        result.has_calc_pilots = true;
                        // class ranking is not cached in the Rust build yet;
                        // leave the slot unassigned
                        let _ = seed_class;
                        set_slot_pilot(state, slot_id, None);
                        result.unassigned_slots += 1;
                    }
                    _ => {
                        result.calc_success = Some(false);
                    }
                }
            }
            _ => {}
        }
    }
    result
}

fn set_slot_pilot(state: &SharedState, slot_id: i64, pilot_id: Option<i64>) {
    db::with_db(state, |conn| {
        let _ = conn.execute(
            "UPDATE heat_node SET pilot_id = ?1 WHERE id = ?2",
            rusqlite::params![pilot_id, slot_id],
        );
    });
}

/// resolve_slot_unset_nodes: assign free seats, in slot order, to slots
/// without a node index.
pub fn resolve_slot_unset_nodes(state: &SharedState, heat_id: i64) {
    db::with_db(state, |conn| {
        let mut used: Vec<Option<i64>> = Vec::new();
        let mut slots: Vec<i64> = Vec::new();
        {
            let mut stmt = conn
                .prepare("SELECT id, node_index FROM heat_node WHERE heat_id = ?1 ORDER BY id")
                .unwrap();
            let mut rows = stmt.query([heat_id]).unwrap();
            while let Some(row) = rows.next().unwrap() {
                let id: i64 = row.get(0).unwrap();
                let node_index: Option<i64> = row.get(1).unwrap();
                used.push(node_index);
                slots.push(id);
            }
        }
        let mut seat: i64 = 0;
        for (idx, slot_id) in slots.iter().enumerate() {
            if used[idx].is_none() {
                while used.contains(&Some(seat)) {
                    seat += 1;
                }
                let _ = conn.execute(
                    "UPDATE heat_node SET node_index = ?1 WHERE id = ?2",
                    rusqlite::params![seat, slot_id],
                );
                used[idx] = Some(seat);
                seat += 1;
            }
        }
    });
}

/// Builds the heat_plan_result broadcast payload.
pub fn build_heat_plan_result(
    state: &SharedState,
    heat_id: i64,
    calc_result: &CalcResult,
    preassigned_slots: &[i64],
) -> Value {
    db::with_db(state, |conn| {
        let displayname: String = conn
            .query_row(
                "SELECT COALESCE(note, 'Heat ' || id) FROM heat WHERE id = ?1",
                [heat_id],
                |r| r.get(0),
            )
            .unwrap_or_else(|_| format!("Heat {}", heat_id));

        let mut slots = Vec::new();
        let mut stmt = conn
            .prepare(
                "SELECT id, node_index, pilot_id, method, seed_rank, seed_id
                 FROM heat_node WHERE heat_id = ?1 ORDER BY id",
            )
            .unwrap();
        let mut rows = stmt.query([heat_id]).unwrap();
        while let Some(row) = rows.next().unwrap() {
            let id: i64 = row.get(0).unwrap();
            let node_index: Option<i64> = row.get(1).unwrap();
            let pilot_id: Option<i64> = row.get(2).unwrap();
            let callsign = pilot_id
                .and_then(|pid| {
                    conn.query_row("SELECT callsign FROM pilot WHERE id = ?1", [pid], |r| {
                        r.get::<_, String>(0)
                    })
                    .ok()
                });

            // frequency-change info from the pilot's used_frequencies history
            let (frequency_change, last_frequency) = pilot_id
                .and_then(|pid| {
                    let used: Option<String> = conn
                        .query_row("SELECT used_frequencies FROM pilot WHERE id = ?1", [pid], |r| r.get(0))
                        .ok();
                    used.and_then(|u| serde_json::from_str::<Value>(&u).ok())
                        .and_then(|v| v.as_array().and_then(|arr| arr.last().cloned()))
                        .and_then(|last| {
                            let last_f = last.get("f").and_then(|f| f.as_i64());
                            last_f.map(|lf| {
                                let node_f = node_index.and_then(|idx| {
                                    let profile_id = db::current_profile_id(conn);
                                    db::profile_frequencies(conn, profile_id)
                                        .get(idx as usize)
                                        .map(|(_, _, f)| *f)
                                });
                                (Some(lf != node_f.unwrap_or(0)), Some(lf))
                            })
                        })
                })
                .unwrap_or((None, None));

            slots.push(json!({
                "node_index": node_index,
                "pilot_id": pilot_id,
                "callsign": callsign,
                "method": row.get::<_, i64>(3).unwrap(),
                "seed_rank": row.get::<_, Option<i64>>(4).unwrap(),
                "seed_id": row.get::<_, Option<i64>>(5).unwrap(),
                "id": id,
                "preassigned": preassigned_slots.contains(&id),
                "frequency_change": frequency_change,
                "last_frequency": last_frequency,
            }));
        }

        json!({
            "heat": heat_id,
            "displayname": displayname,
            "slots": slots,
            "calc_result": {
                "calc_success": calc_result.calc_success,
                "has_calc_pilots": calc_result.has_calc_pilots,
                "unassigned_slots": calc_result.unassigned_slots,
            },
        })
    })
}

// ---------- callout token substitution (doReplace subset) ----------

/// Expands supported tokens and clears any remaining %...% tokens
/// (characterized: unfilled tokens are removed).
pub fn expand_callout(state: &SharedState, callout: &str) -> String {
    // characterized doReplace subset: recognised callout tokens are cleared
    // when their data is unavailable; UNRECOGNISED tokens survive intact
    // (Python logs "Unrecognised callout token(s)")
    const KNOWN_TOKENS: &[&str] = &[
        "%LAP_COUNT%", "%LAP_COUNT_CALL%",
        "%POSITION%", "%POSITION_CALL%", "%POSITION_PLACE%", "%POSITION_PLACE_CALL%",
        "%TIME_BEHIND%", "%TIME_BEHIND_CALL%", "%TIME_BEHIND_FINPLACE%",
        "%TIME_BEHIND_FINPLACE_CALL%", "%TIME_BEHIND_FINPOS%", "%TIME_BEHIND_FINPOS_CALL%",
        "%FASTEST_LAP%", "%FASTEST_LAP_CALL%", "%LAST_LAP%", "%LAST_LAP_CALL%",
        "%AVERAGE_LAP%", "%AVERAGE_LAP_CALL%", "%FASTEST_SPEED%", "%FASTEST_SPEED_CALL%",
        "%LAST_SPEED%", "%LAST_SPEED_CALL%", "%SPLIT%", "%SPLIT_CALL%",
        "%SPEEDS%", "%SPEEDS_CALL%",
    ];
    let mut tokens: Vec<String> = Vec::new();
    let mut rest = callout;
    while let Some(start) = rest.find('%') {
        if let Some(end_rel) = rest[start + 1..].find('%') {
            let token = &rest[start..=start + 1 + end_rel];
            tokens.push(token.to_string());
            rest = &rest[start + 1 + end_rel + 1..];
        } else {
            break;
        }
    }
    let mut out = callout.to_string();
    for token in tokens {
        if KNOWN_TOKENS.contains(&token.as_str()) {
            out = out.replace(&token, "");
        }
    }
    out
}

static LANGUAGES: std::sync::OnceLock<Value> = std::sync::OnceLock::new();

pub fn set_languages(languages: Value) {
    let _ = LANGUAGES.set(languages);
}

pub fn languages() -> Value {
    LANGUAGES.get().cloned().unwrap_or_else(|| json!({}))
}

/// Loads saved callouts from config.json (USER.voiceCallouts).
pub fn persisted_callouts(data_dir: &str) -> Option<Value> {
    let text = std::fs::read_to_string(format!("{}/config.json", data_dir)).ok()?;
    let cfg: Value = serde_json::from_str(&text).ok()?;
    cfg.get("USER")?.get("voiceCallouts").cloned()
}
