//! Builds the Socket.IO / JSON payloads, matching the Python implementation
//! byte-for-byte where the protocol suite asserts shapes.

use crate::db;
use crate::state::*;
use rusqlite::Connection;
use serde_json::{json, Value};

pub fn build_pilot_data(conn: &Connection) -> Value {
    let mut pilots = Vec::new();
    let mut stmt = conn
        .prepare("SELECT id, callsign, team, phonetic, name, color, active FROM pilot ORDER BY id")
        .unwrap();
    let mut rows = stmt.query([]).unwrap();
    while let Some(row) = rows.next().unwrap() {
        let id: i64 = row.get(0).unwrap();
        let callsign: String = row.get(1).unwrap();
        let team: String = row.get(2).unwrap();
        let phonetic: String = row.get(3).unwrap();
        let name: String = row.get(4).unwrap();
        let color: Option<String> = row.get(5).unwrap();
        let active: bool = row.get(6).unwrap();
        pilots.push(json!({
            "pilot_id": id,
            "callsign": callsign,
            "team": team,
            "phonetic": phonetic,
            "name": name,
            "active": active,
            "team_options": "", // full options string is UI-only detail
            "color": color.unwrap_or_default(),
            "locked": false,
        }));
    }
    json!({
        "pilots": pilots,
        "pilotSort": "name",
        "attributes": [],
    })
}

pub fn build_heat_data(conn: &Connection, race: &RaceState) -> Value {
    let mut heats = Vec::new();
    let mut stmt = conn
        .prepare(
            "SELECT id, note, auto_name, class_id, \"order\", status, auto_frequency, active,
                    coop_best_time, coop_num_laps
             FROM heat ORDER BY id",
        )
        .unwrap();
    let mut rows = stmt.query([]).unwrap();
    while let Some(row) = rows.next().unwrap() {
        let id: i64 = row.get(0).unwrap();
        let note: Option<String> = row.get(1).unwrap();
        let auto_name: Option<String> = row.get(2).unwrap();
        let class_id: Option<i64> = row.get(3).unwrap();
        let order: Option<i64> = row.get(4).unwrap();
        let status: i64 = row.get(5).unwrap();
        let auto_frequency: bool = row.get(6).unwrap();
        let active: bool = row.get(7).unwrap();
        let coop_best_time: Option<f64> = row.get(8).unwrap();
        let coop_num_laps: Option<i64> = row.get(9).unwrap();

        let mut slots = Vec::new();
        let mut sstmt = conn
            .prepare(
                "SELECT id, node_index, pilot_id, method, seed_rank, seed_id
                 FROM heat_node WHERE heat_id = ?1",
            )
            .unwrap();
        let mut srows = sstmt.query([id]).unwrap();
        while let Some(srow) = srows.next().unwrap() {
            slots.push(json!({
                "id": srow.get::<_, i64>(0).unwrap(),
                "node_index": srow.get::<_, Option<i64>>(1).unwrap(),
                "pilot_id": srow.get::<_, Option<i64>>(2).unwrap(),
                "method": srow.get::<_, i64>(3).unwrap(),
                "seed_rank": srow.get::<_, Option<i64>>(4).unwrap(),
                "seed_id": srow.get::<_, Option<i64>>(5).unwrap(),
            }));
        }

        let display_name = note.clone().unwrap_or_else(|| {
            format!(
                "Heat {}",
                auto_name.clone().unwrap_or_else(|| id.to_string())
            )
        });
        let next_round = db::max_round(conn, id);

        heats.push(json!({
            "id": id,
            "displayname": display_name,
            "name": note,
            "auto_name": auto_name,
            "class_id": class_id,
            "group_id": 0,
            "order": order,
            "status": status,
            "auto_frequency": auto_frequency,
            "active": active,
            "coop_best_time": coop_best_time.map(|v| db::format_time_to_str(v * 1000.0)).unwrap_or_default(),
            "coop_num_laps": coop_num_laps,
            "next_round": next_round,
            "slots": slots,
            "dynamic": false,
            "locked": false,
        }));
    }
    let _ = race;
    json!({
        "heats": heats,
        "attributes": [],
    })
}

pub fn build_class_data(conn: &Connection) -> Value {
    let mut classes = Vec::new();
    let mut stmt = conn
        .prepare(
            "SELECT id, name, description, format_id, win_condition, rounds, rank_settings
             FROM race_class ORDER BY id",
        )
        .unwrap();
    let mut rows = stmt.query([]).unwrap();
    while let Some(row) = rows.next().unwrap() {
        let rank_settings: Option<String> = row.get(6).unwrap();
        classes.push(json!({
            "id": row.get::<_, i64>(0).unwrap(),
            "name": row.get::<_, Option<String>>(1).unwrap().unwrap_or_default(),
            "displayname": row.get::<_, Option<String>>(1).unwrap().unwrap_or_default(),
            "description": row.get::<_, Option<String>>(2).unwrap().unwrap_or_default(),
            "format": row.get::<_, Option<i64>>(3).unwrap(),
            "win_condition": row.get::<_, String>(4).unwrap(),
            "ranksettings": rank_settings.and_then(|s| serde_json::from_str::<Value>(&s).ok()),
            "rounds": row.get::<_, i64>(5).unwrap(),
        }));
    }
    json!({
        "classes": classes,
        "attributes": [],
    })
}

pub fn build_format_data(conn: &Connection) -> Value {
    let mut formats = Vec::new();
    let mut stmt = conn
        .prepare(
            "SELECT id, name, race_mode, race_time_sec, lap_grace_sec, staging_fixed_tones,
                    staging_tones, start_delay_min_ms, start_delay_max_ms, number_laps_win,
                    win_condition, team_racing_mode, start_behavior, points_method
             FROM race_format ORDER BY id",
        )
        .unwrap();
    let mut rows = stmt.query([]).unwrap();
    while let Some(row) = rows.next().unwrap() {
        let points_method: Option<String> = row.get(13).unwrap();
        formats.push(json!({
            "id": row.get::<_, i64>(0).unwrap(),
            "name": row.get::<_, String>(1).unwrap(),
            "unlimited_time": row.get::<_, i64>(2).unwrap(),
            "race_time_sec": row.get::<_, i64>(3).unwrap(),
            "lap_grace_sec": row.get::<_, i64>(4).unwrap(),
            "staging_fixed_tones": row.get::<_, i64>(5).unwrap(),
            "staging_delay_tones": row.get::<_, i64>(6).unwrap(),
            "start_delay_min": row.get::<_, i64>(7).unwrap(),
            "start_delay_max": row.get::<_, i64>(8).unwrap(),
            "number_laps_win": row.get::<_, i64>(9).unwrap(),
            "win_condition": row.get::<_, i64>(10).unwrap(),
            "team_racing_mode": row.get::<_, i64>(11).unwrap(),
            "start_behavior": row.get::<_, i64>(12).unwrap(),
            "locked": false,
            "points_method": points_method.map(|s| serde_json::from_str::<Value>(&s).unwrap_or(Value::Null)),
            "points_settings": Value::Null,
        }));
    }
    json!({
        "formats": formats,
        "attributes": [],
    })
}

pub fn build_frequency_data(conn: &Connection) -> Value {
    let profile_id = db::current_profile_id(conn);
    let freqs = db::profile_frequencies(conn, profile_id);
    let fdata: Vec<Value> = freqs
        .iter()
        .map(|(band, chan, freq)| {
            json!({
                "band": band,
                "channel": chan,
                "frequency": freq,
            })
        })
        .collect();
    json!({
        "fdata": fdata,
    })
}

pub fn build_node_data(race: &RaceState) -> Value {
    let n = race.node_laps.len();
    json!({
        "node_peak_rssi": vec![0; n],
        "node_nadir_rssi": vec![0; n],
        "pass_peak_rssi": vec![0; n],
        "pass_nadir_rssi": vec![0; n],
        "debug_pass_count": race.node_laps.iter().map(|l| l.len()).collect::<Vec<_>>(),
    })
}

pub fn build_race_status(conn: &Connection, race: &RaceState) -> Value {
    let format_id = race.format_id;
    let (unlimited_time, race_time_sec) = conn
        .query_row(
            "SELECT race_mode, race_time_sec FROM race_format WHERE id = ?1",
            [format_id],
            |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)),
        )
        .unwrap_or((0, 120));
    let next_round = race
        .current_heat
        .map(|h| db::max_round(conn, h))
        .unwrap_or(0);
    let class_id = race
        .current_heat
        .and_then(|h| {
            conn.query_row(
                "SELECT class_id FROM heat WHERE id = ?1",
                [h],
                |row| row.get::<_, Option<i64>>(0),
            )
            .ok()
        })
        .flatten();

    json!({
        "race_status": race.race_status,
        "win_status": 0,
        "race_format_id": format_id,
        "race_heat_id": race.current_heat,
        "race_class_id": class_id,
        "unlimited_time": unlimited_time,
        "race_time_sec": race_time_sec,
        "staging_tones": 0,
        "hide_stage_timer": true,
        "pi_starts_at_s": 0,
        "pi_staging_at_s": 0,
        "show_init_time_flag": true,
        "next_round": next_round,
    })
}

pub fn build_current_heat(conn: &Connection, race: &RaceState) -> Value {
    let mut heat_nodes = serde_json::Map::new();
    for idx in 0..race.node_laps.len() {
        let pilot_id = race.node_pilots[idx];
        let callsign = pilot_id
            .and_then(|pid| {
                conn.query_row(
                    "SELECT callsign FROM pilot WHERE id = ?1",
                    [pid],
                    |row| row.get::<_, String>(0),
                )
                .ok()
            })
            .unwrap_or_else(|| {
                // unassigned seats fall back to the node's default callsign
                let profile_id = db::current_profile_id(conn);
                let freqs = db::profile_frequencies(conn, profile_id);
                match freqs.get(idx) {
                    Some((Some(band), Some(chan), _)) => format!("{}{}", band, chan),
                    _ => "0".to_string(),
                }
            });
        heat_nodes.insert(
            idx.to_string(),
            json!({
                "pilot_id": pilot_id,
                "callsign": callsign,
                "heatNodeColor": Value::Null,
                "pilotColor": Value::Null,
                "activeColor": 0,
            }),
        );
    }
    json!({
        "current_heat": race.current_heat,
        "heatNodes": heat_nodes,
        "heat_format": race.format_id,
        "heat_class": Value::Null,
        "next_round": Value::Null,
    })
}

pub const DEFAULT_SEAT_COLORS: [&str; 8] = [
    "#0022ff", "#ff5500", "#00ff22", "#ff0055", "#ddff00", "#7700ff", "#00ffdd", "#aaaaaa",
];

/// Seat colours as stored in LED.seatColors, falling back to the defaults.
/// The colours were hardcoded, so a saved palette never reached the UI.
pub fn seat_colors(state: &crate::state::SharedState) -> Vec<String> {
    let stored = crate::db::get_config_value(state, "LED", "seatColors")
        .and_then(|v| v.as_array().cloned())
        .map(|arr| {
            arr.iter()
                .map(|c| c.as_str().unwrap_or("").to_string())
                .collect::<Vec<String>>()
        })
        .filter(|v| v.len() >= DEFAULT_SEAT_COLORS.len());

    match stored {
        Some(mut colors) => {
            for (i, default) in DEFAULT_SEAT_COLORS.iter().enumerate() {
                if colors[i].is_empty() {
                    colors[i] = default.to_string();
                }
            }
            colors
        }
        None => DEFAULT_SEAT_COLORS.iter().map(|s| s.to_string()).collect(),
    }
}

pub fn build_seat_data(state: &crate::state::SharedState) -> Value {
    let seat_colors: Vec<String> = seat_colors(state);
    let seats: Vec<Value> = seat_colors
        .iter()
        .enumerate()
        .map(|(i, c)| json!({"seat_id": i, "color": c}))
        .collect();
    json!({
        "seats": seats,
    })
}

pub fn build_min_lap(state: &crate::state::SharedState) -> Value {
    let (min_lap, min_first_crossing) = {
        let conn = state.db.lock().unwrap();
        (
            db::get_option_int(&conn, "MinLapSec", 10),
            db::get_option_int(&conn, "MinFirstCrossingSec", 0),
        )
    };
    // Python reads TIMING.MinLapBehavior; reporting a constant 0 meant the UI
    // could show "off" while the server was discarding short laps.
    let behavior = crate::db::get_config_number(state, "TIMING", "MinLapBehavior", 0.0) as i64;
    json!({
        "min_lap": min_lap,
        "min_first_crossing": min_first_crossing,
        "min_lap_behavior": behavior,
    })
}

pub fn build_enter_exit_levels(conn: &Connection) -> Value {
    let profile_id = db::current_profile_id(conn);
    let (enter, exit): (Option<String>, Option<String>) = conn
        .query_row(
            "SELECT enter_ats, exit_ats FROM profiles WHERE id = ?1",
            [profile_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap_or((None, None));
    let enter = enter
        .and_then(|s| serde_json::from_str::<Value>(&s).ok())
        .and_then(|v| v.get("v").cloned())
        .unwrap_or(json!([]));
    let exit = exit
        .and_then(|s| serde_json::from_str::<Value>(&s).ok())
        .and_then(|v| v.get("v").cloned())
        .unwrap_or(json!([]));
    json!({
        "enter_at_levels": enter,
        "exit_at_levels": exit,
    })
}

/// Builds the current-laps payload: {'current': {'node_index': [...]}}.
pub fn build_current_laps(conn: &Connection, race: &RaceState) -> Value {
    let mut nodes = Vec::new();
    for idx in 0..race.node_laps.len() {
        let pilot_id = race.node_pilots[idx];
        let pilot = pilot_id.and_then(|pid| {
            conn.query_row(
                "SELECT id, name, callsign FROM pilot WHERE id = ?1",
                [pid],
                |row| {
                    Ok(json!({
                        "id": row.get::<_, i64>(0)?,
                        "name": row.get::<_, String>(1)?,
                        "callsign": row.get::<_, String>(2)?,
                    }))
                },
            )
            .ok()
        });
        let mut laps = Vec::new();
        for (lap_index, lap) in race.node_laps[idx].iter().enumerate() {
            laps.push(json!({
                "lap_index": lap_index,
                "lap_number": lap_index, // first crossing is lap 0
                "lap_time": lap.lap_time,
                "lap_time_formatted": db::format_time_to_str(lap.lap_time),
                "lap_time_stamp": lap.lap_time_stamp,
                "source": lap.source,
                "deleted": lap.deleted,
                "splits": [],
                "late_lap": false,
            }));
        }
        // Both were hardcoded. Python picks the quickest completed lap
        // (index > 0, so the first crossing does not count) and marks a node
        // finished once it reaches the format's lap limit.
        let mut fastest_lap_index = Value::Null;
        let mut fastest = f64::INFINITY;
        let mut live_laps: i64 = 0;
        for (lap_index, lap) in race.node_laps[idx].iter().enumerate() {
            if !lap.deleted {
                live_laps += 1;
            }
            if lap_index > 0 && lap.lap_time > 0.0 && lap.lap_time < fastest {
                fastest = lap.lap_time;
                fastest_lap_index = json!(lap_index);
            }
        }
        let lap_limit: i64 = conn
            .query_row(
                "SELECT number_laps_win FROM race_format WHERE id = ?1",
                [race.format_id],
                |r| r.get(0),
            )
            .unwrap_or(0);
        let finished_flag = lap_limit > 0 && live_laps >= lap_limit;

        nodes.push(json!({
            "laps": laps,
            "fastest_lap_index": fastest_lap_index,
            "pilot": pilot,
            "finished_flag": finished_flag,
        }));
    }
    json!({
        "current": {"node_index": nodes},
    })
}

pub fn build_pilot_list(conn: &Connection) -> Value {
    let mut pilots = Vec::new();
    let mut stmt = conn
        .prepare("SELECT callsign FROM pilot ORDER BY id")
        .unwrap();
    let mut rows = stmt.query([]).unwrap();
    while let Some(row) = rows.next().unwrap() {
        pilots.push(row.get::<_, String>(0).unwrap());
    }
    json!({ "pilots": pilots })
}

pub fn build_heat_list(conn: &Connection) -> Value {
    let mut heats = Vec::new();
    let mut stmt = conn
        .prepare(
            "SELECT h.id, h.note, h.auto_name, c.name FROM heat h
             LEFT JOIN race_class c ON h.class_id = c.id ORDER BY h.id",
        )
        .unwrap();
    let mut rows = stmt.query([]).unwrap();
    while let Some(row) = rows.next().unwrap() {
        let id: i64 = row.get(0).unwrap();
        let note: Option<String> = row.get(1).unwrap();
        let auto_name: Option<String> = row.get(2).unwrap();
        let class_name: Option<String> = row.get(3).unwrap();
        heats.push(json!({
            "id": id,
            "name": note,
            "auto_name": auto_name,
            "class_name": class_name,
        }));
    }
    json!({ "heats": heats })
}

pub fn build_class_list(conn: &Connection) -> Value {
    let mut classes = Vec::new();
    let mut stmt = conn
        .prepare("SELECT id, name FROM race_class ORDER BY id")
        .unwrap();
    let mut rows = stmt.query([]).unwrap();
    while let Some(row) = rows.next().unwrap() {
        classes.push(json!({
            "id": row.get::<_, i64>(0).unwrap(),
            "name": row.get::<_, Option<String>>(1).unwrap().unwrap_or_default(),
        }));
    }
    json!({ "classes": classes })
}
