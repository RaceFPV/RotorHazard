//! HTTP /api routes, matching the Python json_endpoints blueprint shapes
//! (including the 201 status codes and the open-by-design auth posture).

use crate::db;
use crate::payloads;
use crate::race as race_ops;
use crate::state::*;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::{json, Value};

fn api_response(status: StatusCode, body: Value) -> Response {
    (
        status,
        [
            ("content-type", "application/json"),
            ("access-control-allow-origin", "*"),
        ],
        Json(body),
    )
        .into_response()
}

pub async fn status(State(state): State<SharedState>) -> impl IntoResponse {
    let (race_status, current_heat, num_nodes) = {
        let race = state.race.lock().unwrap();
        (race.race_status, race.current_heat, state.num_nodes)
    };
    let (profile, format) = db::with_db(&state, |conn| {
        (
            db::current_profile_id(conn),
            db::current_format_id(conn),
        )
    });
    let body = json!({
        "status": {
            "server_info": {
                "server_api": state.info.server_api,
                "json_api": state.info.json_api,
                "node_api_best": state.info.node_api_best,
                "release_version": state.info.release_version,
                "node_api_match": true,
                "node_api_lowest": 0,
                "node_api_levels": [Value::Null],
            },
            "state": {
                "current_heat": current_heat,
                "num_nodes": num_nodes,
                "race_status": race_status,
                "currentProfile": profile.to_string(),
                "currentFormat": format.to_string(),
                "currentHeat": current_heat
                    .map(|h| h.to_string())
                    .unwrap_or_else(|| "None".to_string()),
            },
        }
    });
    api_response(StatusCode::CREATED, body)
}

pub async fn pilot_all(State(state): State<SharedState>) -> impl IntoResponse {
    // the HTTP API serializes the DB models directly (key "id"), unlike the
    // socket payloads which use "pilot_id"
    let pilots = db::with_db(&state, |conn| http_pilot_models(conn));
    api_response(StatusCode::CREATED, json!({ "pilots": pilots }))
}

pub fn http_pilot_models(conn: &rusqlite::Connection) -> Vec<Value> {
    let mut out = Vec::new();
    let mut stmt = conn
        .prepare("SELECT id, callsign, team, phonetic, name, color, used_frequencies, active FROM pilot ORDER BY id")
        .unwrap();
    let mut rows = stmt.query([]).unwrap();
    while let Some(row) = rows.next().unwrap() {
        let used_freqs: Option<String> = row.get(6).unwrap();
        out.push(json!({
            "id": row.get::<_, i64>(0).unwrap(),
            "callsign": row.get::<_, String>(1).unwrap(),
            "team": row.get::<_, String>(2).unwrap(),
            "phonetic": row.get::<_, String>(3).unwrap(),
            "name": row.get::<_, String>(4).unwrap(),
            "color": row.get::<_, Option<String>>(5).unwrap(),
            "used_frequencies": used_freqs
                .and_then(|s| serde_json::from_str::<Value>(&s).ok())
                .unwrap_or(Value::Null),
            "active": row.get::<_, bool>(7).unwrap(),
        }));
    }
    out
}

pub async fn pilot_one(State(state): State<SharedState>, Path(id): Path<i64>) -> impl IntoResponse {
    let pilot = db::with_db(&state, |conn| {
        http_pilot_models(conn)
            .into_iter()
            .find(|p| p["id"].as_i64() == Some(id))
    });
    api_response(StatusCode::CREATED, json!({ "pilot": pilot }))
}

pub async fn heat_all(State(state): State<SharedState>) -> impl IntoResponse {
    let (heats, _unused) = db::with_db(&state, |conn| {
        let race = state.race.lock().unwrap();
        (payloads::build_heat_data(conn, &race), ())
    });
    api_response(StatusCode::CREATED, json!({ "heats": heats["heats"] }))
}

pub async fn heat_one(State(state): State<SharedState>, Path(id): Path<i64>) -> impl IntoResponse {
    // matches the Python endpoint: {'heat': {'setup': <heat data or None>, 'leaderboard': false}}
    let heats = db::with_db(&state, |conn| {
        let race = state.race.lock().unwrap();
        payloads::build_heat_data(conn, &race)
    });
    let setup = heats["heats"]
        .as_array()
        .and_then(|arr| {
            arr.iter()
                .find(|h| h["id"].as_i64() == Some(id))
                .cloned()
        });
    api_response(StatusCode::CREATED, json!({ "heat": {"setup": setup, "leaderboard": false} }))
}

pub async fn class_all(State(state): State<SharedState>) -> impl IntoResponse {
    let classes = db::with_db(&state, payloads::build_class_data);
    api_response(StatusCode::CREATED, json!({ "classes": classes["classes"] }))
}

pub async fn class_one(State(state): State<SharedState>, Path(id): Path<i64>) -> impl IntoResponse {
    let classes = db::with_db(&state, payloads::build_class_data);
    let class = classes["classes"]
        .as_array()
        .and_then(|arr| {
            arr.iter()
                .find(|c| c["id"].as_i64() == Some(id))
                .cloned()
        });
    api_response(StatusCode::CREATED, json!({ "class": class }))
}

pub async fn format_all(State(state): State<SharedState>) -> impl IntoResponse {
    let formats = db::with_db(&state, payloads::build_format_data);
    api_response(StatusCode::CREATED, json!({ "formats": formats["formats"] }))
}

pub async fn format_one(State(state): State<SharedState>, Path(id): Path<i64>) -> impl IntoResponse {
    let formats = db::with_db(&state, payloads::build_format_data);
    let format = formats["formats"]
        .as_array()
        .and_then(|arr| {
            arr.iter()
                .find(|f| f["id"].as_i64() == Some(id))
                .cloned()
        });
    api_response(StatusCode::CREATED, json!({ "format": format }))
}

pub async fn profile_all(State(state): State<SharedState>) -> impl IntoResponse {
    let profiles = db::with_db(&state, |conn| {
        let mut out = Vec::new();
        let mut stmt = conn
            .prepare("SELECT id, name, description, frequencies, enter_ats, exit_ats, f_ratio FROM profiles ORDER BY id")
            .unwrap();
        let mut rows = stmt.query([]).unwrap();
        while let Some(row) = rows.next().unwrap() {
            let freqs: String = row.get(3).unwrap();
            out.push(json!({
                "id": row.get::<_, i64>(0).unwrap(),
                "name": row.get::<_, String>(1).unwrap(),
                "description": row.get::<_, Option<String>>(2).unwrap(),
                "frequencies": serde_json::from_str::<Value>(&freqs).unwrap_or(Value::Null),
                "enter_ats": row.get::<_, Option<String>>(4).unwrap()
                    .and_then(|s| serde_json::from_str::<Value>(&s).ok()),
                "exit_ats": row.get::<_, Option<String>>(5).unwrap()
                    .and_then(|s| serde_json::from_str::<Value>(&s).ok()),
                "f_ratio": row.get::<_, Option<i64>>(6).unwrap(),
            }));
        }
        json!({ "profiles": out })
    });
    api_response(StatusCode::CREATED, profiles)
}

pub async fn profile_one(State(state): State<SharedState>, Path(id): Path<i64>) -> impl IntoResponse {
    let all = profile_all_inner(&state);
    let profile = all["profiles"]
        .as_array()
        .and_then(|arr| arr.iter().find(|p| p["id"].as_i64() == Some(id)).cloned());
    api_response(StatusCode::CREATED, json!({ "profile": profile }))
}

fn profile_all_inner(state: &SharedState) -> Value {
    db::with_db(state, |conn| {
        let mut out = Vec::new();
        let mut stmt = conn
            .prepare("SELECT id, name, description, frequencies, enter_ats, exit_ats, f_ratio FROM profiles ORDER BY id")
            .unwrap();
        let mut rows = stmt.query([]).unwrap();
        while let Some(row) = rows.next().unwrap() {
            let freqs: String = row.get(3).unwrap();
            out.push(json!({
                "id": row.get::<_, i64>(0).unwrap(),
                "name": row.get::<_, String>(1).unwrap(),
                "description": row.get::<_, Option<String>>(2).unwrap(),
                "frequencies": serde_json::from_str::<Value>(&freqs).unwrap_or(Value::Null),
                "enter_ats": row.get::<_, Option<String>>(4).unwrap()
                    .and_then(|s| serde_json::from_str::<Value>(&s).ok()),
                "exit_ats": row.get::<_, Option<String>>(5).unwrap()
                    .and_then(|s| serde_json::from_str::<Value>(&s).ok()),
                "f_ratio": row.get::<_, Option<i64>>(6).unwrap(),
            }));
        }
        json!({ "profiles": out })
    })
}

pub async fn race_all(State(state): State<SharedState>) -> impl IntoResponse {
    let body = db::with_db(&state, |conn| {
        let mut heats = Vec::new();
        let mut stmt = conn
            .prepare("SELECT id FROM heat ORDER BY id")
            .unwrap();
        let mut rows = stmt.query([]).unwrap();
        while let Some(row) = rows.next().unwrap() {
            let id: i64 = row.get(0).unwrap();
            heats.push(json!({
                "id": id,
                "rounds": db::max_round(conn, id),
            }));
        }
        json!({
            "races": {
                "heats": heats,
                "leaderboard": {},
            }
        })
    });
    api_response(StatusCode::CREATED, body)
}

pub async fn race_detail(
    State(state): State<SharedState>,
    Path((heat_id, round_id)): Path<(i64, i64)>,
) -> impl IntoResponse {
    let body = db::with_db(&state, |conn| {
        let meta_row: Option<(i64, i64, Option<i64>, Option<i64>, f64, String)> = conn
            .query_row(
                "SELECT id, round_id, class_id, format_id, start_time, start_time_formatted
                 FROM saved_race_meta WHERE heat_id = ?1 AND round_id = ?2",
                rusqlite::params![heat_id, round_id],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                        row.get(5)?,
                    ))
                },
            )
            .ok();

        let Some((race_id, _round, class_id, format_id, start_time, start_time_formatted)) = meta_row
        else {
            return json!({ "race": Value::Null });
        };

        let mut nodes = Vec::new();
        let mut prstmt = conn
            .prepare(
                "SELECT id, node_index, pilot_id, enter_at, exit_at, frequency
                 FROM saved_pilot_race WHERE race_id = ?1",
            )
            .unwrap();
        let mut prrows = prstmt.query([race_id]).unwrap();
        let mut node_map: std::collections::BTreeMap<i64, (i64, Option<i64>, Value)> =
            std::collections::BTreeMap::new();
        while let Some(row) = prrows.next().unwrap() {
            let pr_id: i64 = row.get(0).unwrap();
            let node_index: i64 = row.get(1).unwrap();
            let pilot_id: Option<i64> = row.get(2).unwrap();
            let callsign = pilot_id
                .and_then(|pid| {
                    conn.query_row("SELECT callsign FROM pilot WHERE id = ?1", [pid], |r| {
                        r.get::<_, String>(0)
                    })
                    .ok()
                });
            let mut laps = Vec::new();
            let mut lstmt = conn
                .prepare(
                    "SELECT id, lap_time_stamp, lap_time, lap_time_formatted, source, deleted
                     FROM saved_race_lap WHERE pilotrace_id = ?1 ORDER BY id",
                )
                .unwrap();
            let mut lrows = lstmt.query([pr_id]).unwrap();
            while let Some(lrow) = lrows.next().unwrap() {
                laps.push(json!({
                    "id": lrow.get::<_, i64>(0).unwrap(),
                    "lap_time_stamp": lrow.get::<_, f64>(1).unwrap(),
                    "lap_time": lrow.get::<_, f64>(2).unwrap(),
                    "lap_time_formatted": lrow.get::<_, String>(3).unwrap(),
                    "source": lrow.get::<_, i64>(4).unwrap(),
                    "deleted": lrow.get::<_, bool>(5).unwrap(),
                }));
            }
            node_map.insert(
                node_index,
                (
                    pr_id,
                    pilot_id,
                    json!({
                        "callsign": callsign,
                        "pilot_id": pilot_id,
                        "node_index": node_index,
                        "laps": laps,
                    }),
                ),
            );
        }

        // dense node list up to the highest seat with a pilot race
        let max_index = node_map.keys().copied().max().unwrap_or(-1);
        for idx in 0..=max_index {
            match node_map.get(&idx) {
                Some((_, _, node)) => nodes.push(node.clone()),
                None => nodes.push(json!({
                    "callsign": Value::Null,
                    "pilot_id": Value::Null,
                    "node_index": idx,
                    "laps": [],
                })),
            }
        }

        json!({
            "race": {
                "start_time_formatted": start_time_formatted,
                "nodes": nodes,
                "sort": "name",
                "leaderboard": {"by_race_time": []},
                "class_id": class_id,
                "format_id": format_id,
                "start_time": start_time,
            }
        })
    });
    api_response(StatusCode::CREATED, body)
}

pub async fn options(State(state): State<SharedState>) -> impl IntoResponse {
    let body = db::with_db(&state, |conn| {
        let mut map = serde_json::Map::new();
        let mut stmt = conn
            .prepare("SELECT option_name, option_value FROM global_settings")
            .unwrap();
        let mut rows = stmt.query([]).unwrap();
        while let Some(row) = rows.next().unwrap() {
            let name: String = row.get(0).unwrap();
            let value: Option<String> = row.get(1).unwrap();
            if name == "eventResults" || name == "secret_key" {
                continue; // excluded by the Python endpoint
            }
            map.insert(
                name,
                value.map(Value::String).unwrap_or(Value::Null),
            );
        }
        json!({ "options": map })
    });
    api_response(StatusCode::CREATED, body)
}
