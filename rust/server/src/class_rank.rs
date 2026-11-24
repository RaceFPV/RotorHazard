//! Class ranking methods and race points — port of the
//! rh_class_rank_* / rh_points_by_position bundled plugins plus the
//! dispatch in Results.py (calc_class_ranking_leaderboard) and the
//! race-format points assignment in RHData's results cache rebuild.

use crate::export_import::clean_var_name;
use rusqlite::Connection;
use serde_json::{json, Map, Value};
use std::collections::HashMap;

/// Display names of the registered methods (RaceClassRankMethod labels).
pub const RANK_METHOD_LABELS: &[&str] = &[
    "Last Heat Position",
    "Cumulative Points",
    "Laps/Time: Best X Rounds",
];

pub const POINTS_METHOD_LABELS: &[&str] = &["Position"];

/// Default settings per rank method (label -> defaults), as registered.
pub fn rank_method_defaults(label: &str) -> Value {
    match label {
        "Cumulative Points" => json!({"ascending": false}),
        "Laps/Time: Best X Rounds" => json!({"rounds": 3}),
        _ => json!({}),
    }
}

pub fn rank_method_settings(label: &str) -> Value {
    match label {
        "Cumulative Points" => json!([{"name": "ascending", "label": "Ascending", "field_type": "CHECKBOX", "value": false}]),
        "Laps/Time: Best X Rounds" => json!([{"name": "rounds", "label": "Number of rounds", "field_type": "BASIC_INT", "placeholder": "3"}]),
        "Position" => json!([{"name": "points_list", "label": "Points (CSV)", "field_type": "TEXT", "placeholder": "10,6,3,1"}]),
        _ => json!([]),
    }
}

/// Methods list payload for the 'raceclass_rank_method_list' load type.
pub fn rank_method_list() -> Value {
    let methods: Vec<Value> = RANK_METHOD_LABELS
        .iter()
        .map(|label| {
            json!({
                "name": clean_var_name(label),
                "label": label,
                "default_args": rank_method_defaults(label),
                "settings": rank_method_settings(label),
            })
        })
        .collect();
    json!({"methods": methods})
}

/// Methods list payload for the 'race_points_method_list' load type.
pub fn points_method_list() -> Value {
    let methods: Vec<Value> = POINTS_METHOD_LABELS
        .iter()
        .map(|label| {
            json!({
                "name": clean_var_name(label),
                "label": label,
                "settings": rank_method_settings(label),
            })
        })
        .collect();
    json!({"methods": methods})
}

/// Assigns race points to the primary leaderboard (RacePointsManager
/// assign + rh_points_by_position).  `points_method` is the JSON stored
/// on race_format: {"t": "Position", "s": {"points_list": "10,6,3,1"}}.
pub fn assign_points(leaderboard: &mut Value, points_method: &str) {
    let method: Value = match serde_json::from_str(points_method) {
        Ok(v) => v,
        Err(_) => return,
    };
    let method_type = method.get("t").and_then(|v| v.as_str()).unwrap_or("");
    if clean_var_name(method_type) != clean_var_name("Position") {
        return; // only the Position points method is registered
    }
    let settings = method.get("s");
    let points_list: Vec<i64> = settings
        .and_then(|s| s.get("points_list"))
        .and_then(|v| v.as_str())
        .map(|s| s.split(',').filter_map(|x| x.trim().parse().ok()).collect())
        .unwrap_or_else(|| vec![10, 6, 3, 1]);

    let primary = leaderboard
        .pointer("/meta/primary_leaderboard")
        .and_then(|v| v.as_str())
        .unwrap_or("by_race_time")
        .to_string();
    if let Some(rows) = leaderboard.get_mut(&primary).and_then(|v| v.as_array_mut()) {
        for (idx, row) in rows.iter_mut().enumerate() {
            if let Some(obj) = row.as_object_mut() {
                if let Some(&p) = points_list.get(idx) {
                    obj.insert("points".into(), json!(p));
                }
            }
        }
    }
    if let Some(meta) = leaderboard.get_mut("meta").and_then(|m| m.as_object_mut()) {
        meta.insert("primary_points".into(), json!(true));
    }
}

struct HeatInfo {
    id: i64,
    display_name: String,
}

fn heats_by_class(conn: &Connection, class_id: i64) -> Vec<HeatInfo> {
    let mut stmt = conn
        .prepare(
            "SELECT id, COALESCE(NULLIF(note, ''), COALESCE(NULLIF(auto_name, ''), 'Heat ' || id)) \
             FROM heat WHERE class_id = ?1 ORDER BY id",
        )
        .unwrap();
    let mut r = stmt.query([class_id]).unwrap();
    let mut out = Vec::new();
    while let Some(row) = r.next().unwrap() {
        out.push(HeatInfo {
            id: row.get(0).unwrap(),
            display_name: row.get(1).unwrap(),
        });
    }
    out
}

/// The heat's current results: its most recent saved race's cached
/// leaderboard (heat_results in Python resolves through the heat's
/// latest race).
fn heat_results(conn: &Connection, heat_id: i64) -> Option<Value> {
    conn.query_row(
        "SELECT results FROM saved_race_meta WHERE heat_id = ?1 AND results IS NOT NULL \
         ORDER BY id DESC LIMIT 1",
        [heat_id],
        |r| r.get::<_, String>(0),
    )
    .ok()
    .and_then(|s| serde_json::from_str(&s).ok())
}

fn primary_board(results: &Value) -> Vec<Value> {
    let primary = results
        .pointer("/meta/primary_leaderboard")
        .and_then(|v| v.as_str())
        .unwrap_or("by_race_time")
        .to_string();
    results
        .get(&primary)
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default()
}

fn new_rank_row(pilot_row: &Value, extra: &[(&str, Value)]) -> Map<String, Value> {
    let mut row = Map::new();
    row.insert(
        "pilot_id".into(),
        pilot_row.get("pilot_id").cloned().unwrap_or(Value::Null),
    );
    row.insert(
        "callsign".into(),
        pilot_row.get("callsign").cloned().unwrap_or(Value::Null),
    );
    row.insert(
        "team_name".into(),
        pilot_row.get("team_name").cloned().unwrap_or(Value::Null),
    );
    for (k, v) in extra {
        row.insert((*k).to_string(), v.clone());
    }
    row
}

/// Applies tie-aware positions: equal sort keys share the earlier rank
/// (the Python loops compare last-rank keys explicitly).
fn assign_positions(rows: &mut [Value], keys: &[&str]) {
    let mut last_rank: i64 = 0;
    let mut last_key: Vec<Value> = Vec::new();
    for (i, row) in rows.iter_mut().enumerate() {
        let key: Vec<Value> = keys
            .iter()
            .map(|k| row.get(*k).cloned().unwrap_or(Value::Null))
            .collect();
        let pos = if i > 0 && key == last_key {
            last_rank
        } else {
            (i + 1) as i64
        };
        last_rank = pos;
        last_key = key;
        if let Some(obj) = row.as_object_mut() {
            obj.insert("position".into(), json!(pos));
        }
    }
}

/// The "Last Heat Position" method: pilots ranked by their final
/// position, favoring later heats in the class.
fn rank_heat_pos(conn: &Connection, class_id: i64) -> Option<(Vec<Value>, Value)> {
    let heats = heats_by_class(conn, class_id);
    let mut leaderboard: Vec<Value> = Vec::new();
    let mut ranked_pilots: Vec<i64> = Vec::new();
    let mut rank_pos: i64 = 1;

    for heat in heats.iter().rev() {
        let results = heat_results(conn, heat.id)?;
        for line in primary_board(&results) {
            let pid = line.get("pilot_id").and_then(|v| v.as_i64());
            if let Some(pid) = pid {
                if ranked_pilots.contains(&pid) {
                    continue;
                }
                let mut row = new_rank_row(
                    &line,
                    &[
                        ("heat", json!(heat.display_name)),
                        ("heat_rank", line.get("position").cloned().unwrap_or(Value::Null)),
                        ("position", json!(rank_pos)),
                    ],
                );
                row.insert("position".into(), json!(rank_pos));
                leaderboard.push(Value::Object(row));
                ranked_pilots.push(pid);
                rank_pos += 1;
            }
        }
    }

    let meta = json!({
        "rank_fields": [
            {"name": "heat", "label": "Heat"},
            {"name": "heat_rank", "label": "Position"},
        ]
    });
    Some((leaderboard, meta))
}

/// The "Cumulative Points" method: total points across all heats.
fn rank_points_total(conn: &Connection, class_id: i64, ascending: bool) -> Option<(Vec<Value>, Value)> {
    let heats = heats_by_class(conn, class_id);
    // pilot_id -> [(points, pilot_row)] preserving first-seen pilot order
    let mut order: Vec<i64> = Vec::new();
    let mut pilotresults: HashMap<String, Vec<(i64, Value)>> = HashMap::new();

    for heat in &heats {
        let mut stmt = conn
            .prepare("SELECT id FROM saved_race_meta WHERE heat_id = ?1 ORDER BY id")
            .unwrap();
        let mut r = stmt.query([heat.id]).unwrap();
        let mut race_ids = Vec::new();
        while let Some(row) = r.next().unwrap() {
            race_ids.push(row.get::<_, i64>(0).unwrap());
        }
        for race_id in race_ids {
            let results: Value = conn
                .query_row(
                    "SELECT results FROM saved_race_meta WHERE id = ?1 AND results IS NOT NULL",
                    [race_id],
                    |r| r.get::<_, String>(0),
                )
                .ok()
                .and_then(|s| serde_json::from_str(&s).ok())?;
            for pilotrow in primary_board(&results) {
                let pid = pilotrow.get("pilot_id").and_then(|v| v.as_i64());
                if let Some(pid) = pid {
                    let key = pid.to_string();
                    if !pilotresults.contains_key(&key) {
                        order.push(pid);
                    }
                    let points = pilotrow.get("points").and_then(|v| v.as_i64()).unwrap_or(0);
                    pilotresults
                        .entry(key)
                        .or_default()
                        .push((points, pilotrow.clone()));
                }
            }
        }
    }

    let mut leaderboard: Vec<Value> = Vec::new();
    for pid in order {
        let list = &pilotresults[&pid.to_string()];
        let total: i64 = list.iter().map(|(p, _)| *p).sum();
        let mut row = new_rank_row(&list[0].1, &[("points", json!(total))]);
        row.insert("points".into(), json!(total));
        leaderboard.push(Value::Object(row));
    }

    leaderboard.sort_by(|a, b| {
        let pa = a["points"].as_i64().unwrap_or(0);
        let pb = b["points"].as_i64().unwrap_or(0);
        if ascending {
            pa.cmp(&pb)
        } else {
            pb.cmp(&pa)
        }
    });
    assign_positions(&mut leaderboard, &["points"]);

    let meta = json!({"rank_fields": [{"name": "points", "label": "Points"}]});
    Some((leaderboard, meta))
}

/// The "Laps/Time: Best X Rounds" method: sums the pilot's best N races.
fn rank_best_rounds(
    conn: &Connection,
    class_id: i64,
    format_start_behavior: i64,
    rounds: i64,
) -> Option<(Vec<Value>, Value)> {
    if rounds < 1 {
        return None;
    }
    let staggered = format_start_behavior == 2; // StartBehavior.STAGGERED
    let heats = heats_by_class(conn, class_id);

    // pilot_id -> race rows (only from the primary by_race_time board)
    let mut order: Vec<i64> = Vec::new();
    let mut pilotresults: HashMap<String, Vec<Value>> = HashMap::new();
    for heat in &heats {
        let mut stmt = conn
            .prepare("SELECT id FROM saved_race_meta WHERE heat_id = ?1 ORDER BY id")
            .unwrap();
        let mut r = stmt.query([heat.id]).unwrap();
        let mut race_ids = Vec::new();
        while let Some(row) = r.next().unwrap() {
            race_ids.push(row.get::<_, i64>(0).unwrap());
        }
        for race_id in race_ids {
            let results: Value = conn
                .query_row(
                    "SELECT results FROM saved_race_meta WHERE id = ?1 AND results IS NOT NULL",
                    [race_id],
                    |r| r.get::<_, String>(0),
                )
                .ok()
                .and_then(|s| serde_json::from_str(&s).ok())?;
            for pilotrow in results
                .get("by_race_time")
                .and_then(|v| v.as_array())
                .cloned()
                .unwrap_or_default()
            {
                if let Some(pid) = pilotrow.get("pilot_id").and_then(|v| v.as_i64()) {
                    let key = pid.to_string();
                    if !pilotresults.contains_key(&key) {
                        order.push(pid);
                    }
                    pilotresults.entry(key).or_default().push(pilotrow);
                }
            }
        }
    }

    let raw_or_inf = |v: &Value| -> f64 {
        v.as_f64().filter(|x| *x > 0.0).unwrap_or(f64::INFINITY)
    };
    let mut leaderboard: Vec<Value> = Vec::new();
    for pid in order {
        let list = &pilotresults[&pid.to_string()];
        let time_field = if staggered {
            "total_time_laps_raw"
        } else {
            "total_time_raw"
        };
        let mut sorted = list.clone();
        sorted.sort_by(|a, b| {
            let la = a.get("laps").and_then(|v| v.as_i64()).unwrap_or(0);
            let lb = b.get("laps").and_then(|v| v.as_i64()).unwrap_or(0);
            lb.cmp(&la).then(
                raw_or_inf(a.get(time_field).unwrap_or(&Value::Null))
                    .partial_cmp(&raw_or_inf(b.get(time_field).unwrap_or(&Value::Null)))
                    .unwrap_or(std::cmp::Ordering::Equal),
            )
        });
        let best = &sorted[..(rounds as usize).min(sorted.len())];

        let mut laps: i64 = 0;
        let mut starts: i64 = 0;
        let mut total_raw: f64 = 0.0;
        let mut total_laps_raw: f64 = 0.0;
        for race in best {
            laps += race.get("laps").and_then(|v| v.as_i64()).unwrap_or(0);
            starts += race.get("starts").and_then(|v| v.as_i64()).unwrap_or(0);
            total_raw += race.get("total_time_raw").and_then(|v| v.as_f64()).unwrap_or(0.0);
            total_laps_raw += race
                .get("total_time_laps_raw")
                .and_then(|v| v.as_f64())
                .unwrap_or(0.0);
        }

        let first = &best[0];
        let mut row = new_rank_row(
            first,
            &[
                ("laps", json!(laps)),
                ("starts", json!(starts)),
                ("total_time_raw", json!(total_raw)),
                ("total_time_laps_raw", json!(total_laps_raw)),
                (
                    "total_time",
                    json!(crate::db::format_time_to_str(total_raw)),
                ),
                (
                    "total_time_laps",
                    json!(crate::db::format_time_to_str(total_laps_raw)),
                ),
            ],
        );
        row.insert("laps".into(), json!(laps));
        leaderboard.push(Value::Object(row));
    }

    let sort_time_field = if staggered {
        "total_time_laps_raw"
    } else {
        "total_time_raw"
    };
    leaderboard.sort_by(|a, b| {
        let la = a["laps"].as_i64().unwrap_or(0);
        let lb = b["laps"].as_i64().unwrap_or(0);
        lb.cmp(&la).then(
            raw_or_inf(a.get(sort_time_field).unwrap_or(&Value::Null))
                .partial_cmp(&raw_or_inf(b.get(sort_time_field).unwrap_or(&Value::Null)))
                .unwrap_or(std::cmp::Ordering::Equal),
        )
    });
    assign_positions(&mut leaderboard, &["laps", sort_time_field]);

    let meta = json!({
        "method_label": format!("Best {} Rounds", rounds),
        "rank_fields": [
            {"name": "laps", "label": "Laps"},
            {"name": sort_time_field, "label": "Total"},
            {"name": "starts", "label": "Starts"},
        ]
    });
    Some((leaderboard, meta))
}

/// calc_class_ranking_leaderboard: dispatches on the class's
/// win_condition field (the rank-method name) with rank_settings args.
pub fn calc_class_ranking(conn: &Connection, class_id: i64) -> Option<Value> {
    let (win_condition, rank_settings, format_start_behavior): (String, Option<String>, i64) = conn
        .query_row(
            "SELECT win_condition, rank_settings, \
             (SELECT start_behavior FROM race_format f WHERE f.id = c.format_id) \
             FROM race_class c WHERE id = ?1",
            [class_id],
            |r| {
                Ok((
                    r.get(0)?,
                    r.get(1)?,
                    r.get::<_, Option<i64>>(2)?.unwrap_or(0),
                ))
            },
        )
        .ok()?;

    let args: Value = rank_settings
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or(json!({}));

    let (ranking, meta) = match win_condition.as_str() {
        n if n == clean_var_name("Last Heat Position") => rank_heat_pos(conn, class_id)?,
        n if n == clean_var_name("Cumulative Points") => {
            let ascending = args.get("ascending").and_then(|v| v.as_bool()).unwrap_or(false);
            rank_points_total(conn, class_id, ascending)?
        }
        n if n == clean_var_name("Laps/Time: Best X Rounds") => {
            let rounds = args.get("rounds").and_then(|v| v.as_i64()).unwrap_or(3);
            rank_best_rounds(conn, class_id, format_start_behavior, rounds)?
        }
        other => {
            tracing::warn!("Class {} uses unsupported ranking method: {}", class_id, other);
            return None;
        }
    };

    let mut meta_obj = meta;
    if let Some(m) = meta_obj.as_object_mut() {
        if !m.contains_key("method_label") {
            // fill from the registered label when the method didn't set one
            for label in RANK_METHOD_LABELS {
                if clean_var_name(label) == win_condition {
                    m.insert("method_label".into(), json!(label));
                    break;
                }
            }
        }
    }

    Some(json!({"ranking": ranking, "meta": meta_obj}))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn position_points_default_list() {
        let mut board = json!({
            "by_race_time": [
                {"pilot_id": 1, "laps": 5},
                {"pilot_id": 2, "laps": 4},
                {"pilot_id": 3, "laps": 4},
                {"pilot_id": 4, "laps": 2},
            ],
            "meta": {"primary_leaderboard": "by_race_time"},
        });
        assign_points(&mut board, r#"{"t": "Position"}"#);
        let rows = board["by_race_time"].as_array().unwrap();
        assert_eq!(rows[0]["points"], 10);
        assert_eq!(rows[1]["points"], 6);
        assert_eq!(rows[2]["points"], 3);
        assert_eq!(rows[3]["points"], 1); // last list entry
        // a fifth row falls beyond the list: no points key added
        board["by_race_time"].as_array_mut().unwrap().push(json!({"pilot_id": 5}));
        let mut board = board;
        assign_points(&mut board, r#"{"t": "Position"}"#);
        let rows = board["by_race_time"].as_array().unwrap();
        assert!(rows[4].get("points").is_none());
        assert_eq!(board["meta"]["primary_points"], true);
    }

    #[test]
    fn position_points_custom_list() {
        let mut board = json!({
            "by_race_time": [
                {"pilot_id": 1}, {"pilot_id": 2}, {"pilot_id": 3},
            ],
            "meta": {"primary_leaderboard": "by_race_time"},
        });
        assign_points(&mut board, r#"{"t": "Position", "s": {"points_list": "25,17,9"}}"#);
        let rows = board["by_race_time"].as_array().unwrap();
        assert_eq!(rows[0]["points"], 25);
        assert_eq!(rows[1]["points"], 17);
        assert_eq!(rows[2]["points"], 9);
    }

    #[test]
    fn unknown_points_method_is_noop() {
        let mut board = json!({
            "by_race_time": [{"pilot_id": 1}],
            "meta": {"primary_leaderboard": "by_race_time"},
        });
        assign_points(&mut board, r#"{"t": "Complicated"}"#);
        assert!(board["by_race_time"][0].get("points").is_none());
        assert!(board["meta"].get("primary_points").is_none());
    }

    #[test]
    fn tie_positions_share_rank() {
        let mut rows = vec![
            json!({"points": 10}),
            json!({"points": 10}),
            json!({"points": 5}),
        ];
        assign_positions(&mut rows, &["points"]);
        assert_eq!(rows[0]["position"], 1);
        assert_eq!(rows[1]["position"], 1); // tie: same rank
        assert_eq!(rows[2]["position"], 3); // next distinct key keeps its index
    }
}
