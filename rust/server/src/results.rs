//! Leaderboard calculation, ported from Results.py.
//!
//! Row and meta structures match the captured reference payload
//! (rust/tests/reference_payloads.json) generated from the Python server.

use crate::db;
use serde_json::{json, Map, Value};

pub struct FormatInfo {
    pub win_condition: i64,
    pub start_behavior: i64,
    pub team_racing_mode: i64,
}

impl FormatInfo {
    pub fn from_conn(conn: &rusqlite::Connection, format_id: i64) -> Self {
        let (wc, sb, trm) = conn
            .query_row(
                "SELECT win_condition, start_behavior, team_racing_mode FROM race_format WHERE id = ?1",
                [format_id],
                |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?, row.get::<_, i64>(2)?)),
            )
            .unwrap_or((1, 0, 0));
        FormatInfo {
            win_condition: wc,
            start_behavior: sb,
            team_racing_mode: trm,
        }
    }
}


/// One pilot's crossing data for leaderboard calculation.
pub struct PilotLaps {
    pub pilot_id: Option<i64>,
    pub callsign: String,
    pub team_name: Option<String>,
    pub node: usize,
    /// All crossings, including the initial crossing (lap 0).
    /// (lap_time ms, lap_time_stamp ms since race start)
    pub crossings: Vec<(f64, f64)>,
}

const START_BEHAVIOR_FIRST_LAP: i64 = 1;

/// Calculates the full leaderboard structure for the given pilot laps.
pub fn calc_leaderboard(
    pilots: &[PilotLaps],
    format: &FormatInfo,
    round_num: i64,
    heat_id: Option<i64>,
    heat_displayname: &str,
    consecutives_count: i64,
) -> Value {
    let start_behavior_first_lap = format.start_behavior == START_BEHAVIOR_FIRST_LAP;

    let mut rows: Vec<Value> = Vec::new();
    for p in pilots {
        let crossings = &p.crossings;
        let (total_laps, lap_times): (i64, Vec<f64>) = if !crossings.is_empty() {
            if start_behavior_first_lap {
                (crossings.len() as i64, crossings.iter().map(|c| c.0).collect())
            } else {
                (
                    crossings.len() as i64 - 1,
                    crossings.iter().skip(1).map(|c| c.0).collect(),
                )
            }
        } else {
            (0, Vec::new())
        };

        let total_time: f64 = crossings.iter().map(|c| c.0).sum();
        let mut total_time_laps = total_time;
        if crossings.len() != lap_times.len() && !crossings.is_empty() {
            total_time_laps -= crossings[0].0;
        }

        let mut row = Map::new();
        row.insert("pilot_id".into(), json!(p.pilot_id));
        row.insert("callsign".into(), json!(p.callsign));
        row.insert("team_name".into(), json!(p.team_name));
        row.insert("laps".into(), json!(total_laps));
        row.insert(
            "starts".into(),
            json!(if !crossings.is_empty() { 1 } else { 0 }),
        );
        row.insert("node".into(), json!(p.node as i64));

        let source = json!({
            "round": round_num,
            "heat": heat_id,
            "displayname": heat_displayname,
        });

        if total_laps > 0 {
            let last_lap = lap_times[lap_times.len() - 1];
            let average_lap = total_time_laps / total_laps as f64;
            let fastest_lap = lap_times.iter().cloned().fold(f64::INFINITY, f64::min);

            // best consecutive-lap window of up to consecutives_count laps:
            // candidate windows sorted by (-laps, time); the winner's time is
            // the SUM of its lap times (matching Results.py)
            let mut all_consecutives: Vec<(i64, f64, Option<i64>)> = Vec::new(); // (laps, time, lap_index)
            let n = lap_times.len() as i64;
            if n <= consecutives_count {
                all_consecutives.push((n, total_time_laps, None));
            } else {
                for start in 0..=(n - consecutives_count) {
                    let window: Vec<f64> =
                        lap_times[start as usize..(start + consecutives_count) as usize].to_vec();
                    all_consecutives.push((
                        consecutives_count,
                        window.iter().sum(),
                        Some(start),
                    ));
                }
            }
            all_consecutives.sort_by(|a, b| {
                b.0.cmp(&a.0)
                    .then((b.1 == 0.0).cmp(&(a.1 == 0.0)))
                    .then(a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal))
            });
            let best = &all_consecutives[0];

            let fastest_idx = lap_times
                .iter()
                .enumerate()
                .min_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal))
                .map(|(i, _)| i)
                .unwrap_or(0);

            let mut fastest_source = source.clone();
            fastest_source["lap"] = json!(fastest_idx as i64 + 1);

            let cons_start = best.2.unwrap_or(0) + 1;
            let cons_end = cons_start + best.0 - 1;
            let cons_source = json!({
                "round": round_num,
                "heat": heat_id,
                "displayname": heat_displayname,
                "lap_start": cons_start,
                "lap_end": cons_end,
            });

            row.insert("last_lap".into(), json!(db::format_time_to_str(last_lap)));
            row.insert("average_lap".into(), json!(db::format_time_to_str(average_lap)));
            row.insert("fastest_lap".into(), json!(db::format_time_to_str(fastest_lap)));
            row.insert("consecutives".into(), json!(db::format_time_to_str(best.1)));
            row.insert("consecutives_base".into(), json!(best.0));
            row.insert("consecutive_lap_start".into(), json!(best.2));
            row.insert("fastest_lap_source".into(), fastest_source);
            row.insert("consecutives_source".into(), cons_source);
            row.insert("total_time".into(), json!(db::format_time_to_str(total_time)));
            row.insert("total_time_laps".into(), json!(db::format_time_to_str(total_time_laps)));
            row.insert("total_time_raw".into(), json!(round3(total_time)));
            row.insert("total_time_laps_raw".into(), json!(round3(total_time_laps)));
            row.insert("average_lap_raw".into(), json!(round3(average_lap)));
            row.insert("fastest_lap_raw".into(), json!(fastest_lap));
            row.insert("consecutives_raw".into(), json!(round3(best.1)));
            row.insert("last_lap_raw".into(), json!(last_lap));
        } else {
            // zero-lap rows still carry the first-crossing time as
            // total_time (characterized Python behavior: position is
            // assigned when total_time_raw > 0)
            row.insert("last_lap".into(), Value::Null);
            row.insert("average_lap".into(), json!(0));
            row.insert("fastest_lap".into(), json!(0));
            row.insert("fastest_lap_source".into(), Value::Null);
            row.insert("consecutives_source".into(), Value::Null);
            row.insert("consecutives".into(), Value::Null);
            row.insert("consecutives_base".into(), json!(0));
            row.insert("consecutive_lap_start".into(), Value::Null);
            row.insert("total_time".into(), json!(db::format_time_to_str(total_time)));
            row.insert("total_time_laps".into(), json!(db::format_time_to_str(total_time_laps)));
            row.insert("total_time_raw".into(), json!(round3(total_time)));
            row.insert("total_time_laps_raw".into(), json!(round3(total_time_laps)));
            row.insert("average_lap_raw".into(), json!(0));
            // characterized: fastest_lap_raw is 0 for zero-lap rows while
            // last_lap_raw stays null
            row.insert("fastest_lap_raw".into(), json!(0));
            row.insert("last_lap_raw".into(), Value::Null);
        }

        row.insert("top_speed".into(), Value::Null);
        row.insert("speed_total".into(), json!(0.0));
        row.insert("speed_count".into(), json!(0));
        row.insert("top_speed_source".into(), Value::Null);

        rows.push(Value::Object(row));
    }

    // --- by_race_time: (-laps, total_time), ranking with tie + behind ---
    let mut by_race_time = rows.clone();
    by_race_time.sort_by(|a, b| {
        let la = a["laps"].as_i64().unwrap_or(0);
        let lb = b["laps"].as_i64().unwrap_or(0);
        lb.cmp(&la).then(
            raw_or_inf(&a["total_time_raw"]).partial_cmp(&raw_or_inf(&b["total_time_raw"]))
                .unwrap_or(std::cmp::Ordering::Equal),
        )
    });
    rank_rows(&mut by_race_time, &["laps", "total_time_raw"], "position");
    let leader_laps = by_race_time
        .first()
        .and_then(|r| r["laps"].as_i64())
        .unwrap_or(0);
    for row in by_race_time.iter_mut() {
        let laps = row["laps"].as_i64().unwrap_or(0);
        row["behind"] = json!(leader_laps - laps);
    }

    // --- by_fastest_lap ---
    let mut by_fastest_lap = rows.clone();
    by_fastest_lap.sort_by(|a, b| {
        let fa = raw_or_inf(&a["fastest_lap_raw"]);
        let fb = raw_or_inf(&b["fastest_lap_raw"]);
        fa.partial_cmp(&fb).unwrap_or(std::cmp::Ordering::Equal).then(
            raw_or_inf(&a["total_time_raw"])
                .partial_cmp(&raw_or_inf(&b["total_time_raw"]))
                .unwrap_or(std::cmp::Ordering::Equal),
        )
    });
    rank_rows(&mut by_fastest_lap, &["fastest_lap_raw"], "position");

    // --- by_consecutives: (-base, time, -laps, total_time) ---
    let mut by_consecutives = rows.clone();
    by_consecutives.sort_by(|a, b| {
        let ba = a["consecutives_base"].as_i64().unwrap_or(0);
        let bb = b["consecutives_base"].as_i64().unwrap_or(0);
        bb.cmp(&ba)
            .then(
                raw_or_inf(&a["consecutives_raw"])
                    .partial_cmp(&raw_or_inf(&b["consecutives_raw"]))
                    .unwrap_or(std::cmp::Ordering::Equal),
            )
            .then({
                let la = a["laps"].as_i64().unwrap_or(0);
                let lb2 = b["laps"].as_i64().unwrap_or(0);
                lb2.cmp(&la)
            })
            .then(
                raw_or_inf(&a["total_time_raw"])
                    .partial_cmp(&raw_or_inf(&b["total_time_raw"]))
                    .unwrap_or(std::cmp::Ordering::Equal),
            )
    });
    rank_rows(&mut by_consecutives, &["consecutives_base", "consecutives_raw"], "position");

    // fastest-race-lap meta (phonetic time uses the leading-space format)
    let fastest_race_lap_data = if !by_fastest_lap.is_empty()
        && by_fastest_lap[0]["laps"].as_i64().unwrap_or(0) > 0
    {
        let lead = &by_fastest_lap[0];
        let phonetic_time = phonetic_time_str(lead["fastest_lap_raw"].as_f64().unwrap_or(0.0));
        let callsign = lead["callsign"].as_str().unwrap_or("").to_string();
        let fastest_formatted = lead["fastest_lap"].as_str().unwrap_or("").to_string();
        json!({
            "phonetic": [callsign.to_lowercase(), phonetic_time],
            "text": [callsign, fastest_formatted],
            "source": lead["fastest_lap_source"].clone(),
        })
    } else {
        Value::Null
    };

    json!({
        "by_race_time": by_race_time,
        "by_fastest_lap": by_fastest_lap,
        "by_consecutives": by_consecutives,
        "meta": {
            "primary_leaderboard": "by_race_time",
            "win_condition": format.win_condition,
            "team_racing_mode": format.team_racing_mode,
            "start_behavior": format.start_behavior,
            "consecutives_count": consecutives_count,
            "fastest_race_lap_data": fastest_race_lap_data,
        },
    })
}

fn round3(v: f64) -> f64 {
    (v * 1000.0).round() / 1000.0
}


fn raw_or_inf(v: &Value) -> f64 {
    match v.as_f64() {
        Some(x) if x > 0.0 => x,
        _ => f64::INFINITY,
    }
}

/// Assigns 1-based positions; equal adjacent tie-key values share a position
/// (matching Results.py); rows with no time get no position.
fn rank_rows(rows: &mut [Value], tie_keys: &[&str], pos_key: &str) {
    let mut last_pos: Option<i64> = None;
    let mut prev: Option<Vec<Value>> = None;
    for (i, row) in rows.iter_mut().enumerate() {
        let has_time = row
            .get("total_time_raw")
            .and_then(|v| v.as_f64())
            .map(|v| v > 0.0)
            .unwrap_or(false);
        let cur: Vec<Value> = tie_keys.iter().map(|k| row[*k].clone()).collect();
        let pos = if !has_time {
            None
        } else if prev.as_ref() == Some(&cur) {
            last_pos
        } else {
            Some(i as i64 + 1)
        };
        row[pos_key] = json!(pos);
        last_pos = pos;
        prev = Some(cur);
    }
}

/// Phonetic time format ('{m} {s}.{d}'), including the leading-space quirk
/// when minutes are empty (' 1.3').
fn phonetic_time_str(millis: f64) -> String {
    let millis = millis as i64;
    let sign = if millis < 0 { "-" } else { "" };
    let millis = millis.abs();
    let minutes = millis / 60000;
    let seconds = (millis % 60000) / 1000;
    let tenths = (millis % 1000) / 100;
    if minutes <= 0 {
        format!("{} {} {}.{}", sign, "", seconds, tenths)
    } else {
        format!("{}{} {:02}.{}", sign, minutes, seconds, tenths)
    }
}
