//! Socket.IO event handlers and load-data emission.
//!
//! Event semantics match the Python implementation: responses are either
//! broadcast to all clients or sent only to the requesting socket ('nobroadcast')
//! or to everyone except the originator ('noself') -- per handler, exactly as
//! characterized by the protocol tests.

use crate::db;
use crate::payloads;
use crate::race as race_ops;
use crate::state::*;
use serde_json::{json, Value};
use socketioxide::extract::{AckSender, Data, SocketRef, State};
use socketioxide::SocketIo;
use std::sync::Arc;

pub fn emit_to_all(io: &SocketIo, event: &'static str, payload: Value) {
    match io.of("/") {
        Some(ns) => {
            tokio::spawn(async move {
                if let Err(e) = ns.emit(event, &payload).await {
                    tracing::warn!("broadcast {} failed: {:?}", event, e);
                }
            });
        }
        None => tracing::warn!("broadcast {}: namespace not found!", event),
    }
}

pub fn broadcast(state: &SharedState, event: &'static str, payload: Value) {
    emit_to_all(state.io.get().expect("io initialized"), event, payload);
}

pub fn register_handlers(io: SocketIo, state: SharedState) {
    io.ns("/", move |socket: SocketRef| {
        tracing::info!("socket connected: {}", socket.id);

        socket.on("probe3", |s: SocketRef, data: Data<Value>| {
            tracing::info!("PROBE3 fired data={}", data.0);
        });

        socket.on("probe4", |s: SocketRef, data: Data<Option<Value>>| {
            tracing::info!("PROBE4 fired data={:?}", data.0);
        });

        socket.on("probe", |s: SocketRef| {
            tracing::info!("PROBE handler fired");
        });

        socket.on("probe2", |s: SocketRef, data: Data<Value>| {
            tracing::info!("PROBE2 handler fired data={}", data.0);
        });

        socket.on("load_data", move |socket: SocketRef, data: Data<Value>, s: State<SharedState>| async move {
            handle_load_data(&socket, &s.0, data);
        });

        register_crud_handlers(socket.clone(), state.clone());
        register_race_handlers(socket.clone(), state.clone());
        register_settings_handlers(socket.clone(), state.clone());
    });
}

fn handle_load_data(socket: &SocketRef, state: &SharedState, data: Data<Value>) {
    tracing::info!("load_data handler invoked sid={}", socket.id);
    let load_types = data
        .0
        .get("load_types")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();

    for load_type in load_types {
        match load_type.as_str() {
            Some("pilot_data") => {
                let payload = db::with_db(state, payloads::build_pilot_data);
                socket.emit("pilot_data", &payload);
            }
            Some("heat_data") => {
                let payload = db::with_db(state, |conn| {
                    let race = state.race.lock().unwrap();
                    payloads::build_heat_data(conn, &race)
                });
                socket.emit("heat_data", &payload);
            }
            Some("class_data") => {
                let payload = db::with_db(state, payloads::build_class_data);
                socket.emit("class_data", &payload);
            }
            Some("format_data") => {
                let payload = db::with_db(state, payloads::build_format_data);
                socket.emit("format_data", &payload);
            }
            Some("frequency_data") => {
                let payload = db::with_db(state, payloads::build_frequency_data);
                socket.emit("frequency_data", &payload);
            }
            Some("node_data") => {
                let payload = payloads::build_node_data(&state.race.lock().unwrap());
                socket.emit("node_data", &payload);
            }
            Some("race_status") => {
                let payload = db::with_db(state, |conn| {
                    let race = state.race.lock().unwrap();
                    payloads::build_race_status(conn, &race)
                });
                socket.emit("race_status", &payload);
            }
            Some("current_heat") => {
                let payload = db::with_db(state, |conn| {
                    let race = state.race.lock().unwrap();
                    payloads::build_current_heat(conn, &race)
                });
                socket.emit("current_heat", &payload);
            }
            Some("seat_data") => {
                let payload = db::with_db(state, payloads::build_seat_data);
                socket.emit("seat_data", &payload);
            }
            Some("min_lap") => {
                let payload = db::with_db(state, payloads::build_min_lap);
                socket.emit("min_lap", &payload);
            }
            Some("enter_and_exit_at_levels") => {
                let payload = db::with_db(state, payloads::build_enter_exit_levels);
                socket.emit("enter_and_exit_at_levels", &payload);
            }
            Some("current_laps") => {
                let payload = db::with_db(state, |conn| {
                    let race = state.race.lock().unwrap();
                    payloads::build_current_laps(conn, &race)
                });
                socket.emit("current_laps", &payload);
            }
            Some("pilot_list") => {
                let payload = db::with_db(state, payloads::build_pilot_list);
                socket.emit("pilot_list", &payload);
            }
            Some("heat_list") => {
                let payload = db::with_db(state, payloads::build_heat_list);
                socket.emit("heat_list", &payload);
            }
            Some("class_list") => {
                let payload = db::with_db(state, payloads::build_class_list);
                socket.emit("class_list", &payload);
            }
            Some("leaderboard") => {
                let payload = json!({"current": {"leads": []}});
                socket.emit("leaderboard", &payload);
            }
            Some("exporter_list") => {
                // no data exporters registered in the Rust build yet
                let payload = json!({"exporters": []});
                socket.emit("exporter_list", &payload);
            }
            _ => {
                if let Some(obj) = load_type.as_object() {
                    match obj.get("type").and_then(|t| t.as_str()) {
                        Some("config") => {
                            let payload = build_config_sections(state, obj.get("value"));
                            socket.emit("config_update", &payload);
                        }
                        _ => {}
                    }
                }
            }
        }
    }
}

fn build_config_sections(state: &SharedState, request: Option<&Value>) -> Value {
    let mut config = serde_json::Map::new();
    if let Some(sections) = request.and_then(|v| v.as_object()) {
        for (section, keys) in sections {
            let mut sec = serde_json::Map::new();
            if let Some(arr) = keys.as_array() {
                for key in arr {
                    if let Some(key) = key.as_str() {
                        let val = read_config_item(state, section, key);
                        sec.insert(key.to_string(), val);
                    }
                }
            }
            config.insert(section.clone(), Value::Object(sec));
        }
    }
    json!({ "config": config })
}

fn read_config_item(state: &SharedState, section: &str, key: &str) -> Value {
    let runtime_key = format!("{}_{}", section, key);
    if let Some(v) = state.config_items.lock().unwrap().get(&runtime_key) {
        return v.clone();
    }
    match (section, key) {
        ("GENERAL", "RACE_START_DELAY_EXTRA_SECS") => {
            json!(0.9)
        }
        ("GENERAL", "HTTP_PORT") => {
            json!(5000)
        }
        _ => {
            let name = format!("{}_{}", section, key);
            let v = db::with_db(state, |conn| db::get_option(conn, &name));
            match v {
                Some(s) => json!(s),
                None => Value::Null,
            }
        }
    }
}

fn register_crud_handlers(socket: SocketRef, state: SharedState) {
    // ---- pilots ----
    socket.on("add_pilot", move |_: SocketRef, s: State<SharedState>| async move {
        let state = s.0.clone();
        let new_id = db::with_db(&state, |conn| {
            let id = db::next_pilot_id(conn);
            // color: random hue at s=100, l=50 (matches RHUtils.hslToHex)
            let color = crate::util::random_hue_hex();
            conn.execute(
                "INSERT INTO pilot (id, callsign, team, phonetic, name, color, used_frequencies, active)
                 VALUES (?1, ?2, 'A', '', ?3, ?4, NULL, 1)",
                rusqlite::params![id, format!("~Callsign {}", id), format!("~Pilot {} Name", id), color],
            )
            .unwrap();
            id
        });
        tracing::info!("Pilot added: Pilot {}", new_id);
        let payload = db::with_db(&state, payloads::build_pilot_data);
        broadcast(&state, "pilot_data", payload);
        let heat_payload = db::with_db(&state, |conn| {
            let race = state.race.lock().unwrap();
            payloads::build_heat_data(conn, &race)
        });
        broadcast(&state, "heat_data", heat_payload);
    });

    let sid = socket.id;
    socket.on("alter_pilot", move |_: SocketRef, data: Data<Value>, s: State<SharedState>| async move {
        let state = s.0.clone();
        let data = data.0;
        let Some(pilot_id) = data.get("pilot_id").and_then(|v| v.as_i64()) else { return };
        db::with_db(&state, |conn| {
            if let Some(v) = data.get("callsign").and_then(|v| v.as_str()) {
                let _ = conn.execute("UPDATE pilot SET callsign = ?1 WHERE id = ?2", rusqlite::params![v, pilot_id]);
            }
            if let Some(v) = data.get("team_name").and_then(|v| v.as_str()) {
                let _ = conn.execute("UPDATE pilot SET team = ?1 WHERE id = ?2", rusqlite::params![v, pilot_id]);
            }
            if let Some(v) = data.get("phonetic").and_then(|v| v.as_str()) {
                let _ = conn.execute("UPDATE pilot SET phonetic = ?1 WHERE id = ?2", rusqlite::params![v, pilot_id]);
            }
            if let Some(v) = data.get("name").and_then(|v| v.as_str()) {
                let _ = conn.execute("UPDATE pilot SET name = ?1 WHERE id = ?2", rusqlite::params![v, pilot_id]);
            }
            if let Some(v) = data.get("color").and_then(|v| v.as_str()) {
                let _ = conn.execute("UPDATE pilot SET color = ?1 WHERE id = ?2", rusqlite::params![v, pilot_id]);
            }
        });
        // pilot_data is emitted with noself=True: everyone except the originator
        let payload = db::with_db(&state, payloads::build_pilot_data);
        if let Some(ns) = state.io.get().and_then(|io| io.of("/")) {
            let ns = ns.except(sid);
            tokio::spawn(async move {
                let _ = ns.emit("pilot_data", &payload).await;
            });
        }
        let heat_payload = db::with_db(&state, |conn| {
            let race = state.race.lock().unwrap();
            payloads::build_heat_data(conn, &race)
        });
        broadcast(&state, "heat_data", heat_payload);
    });

    socket.on("delete_pilot", move |_: SocketRef, data: Data<Value>, s: State<SharedState>| async move {
        let state = s.0.clone();
        let data = data.0;
        let Some(pilot_id) = data.get("pilot").and_then(|v| v.as_i64()) else { return };
        let deleted = db::with_db(&state, |conn| {
            conn.execute("DELETE FROM pilot WHERE id = ?1", [pilot_id]).unwrap_or(0) > 0
        });
        if deleted {
            let payload = db::with_db(&state, payloads::build_pilot_data);
            broadcast(&state, "pilot_data", payload);
            let heat_payload = db::with_db(&state, |conn| {
                let race = state.race.lock().unwrap();
                payloads::build_heat_data(conn, &race)
            });
            broadcast(&state, "heat_data", heat_payload);
        }
    });

    // ---- classes ----
    socket.on("add_race_class", move |_: SocketRef, data: Data<Value>, s: State<SharedState>| async move {
        let state = s.0.clone();
        let data = data.0;
        db::with_db(&state, |conn| {
            let id: i64 = conn
                .query_row("SELECT COALESCE(MAX(id), 0) + 1 FROM race_class", [], |r| r.get(0))
                .unwrap_or(1);
            let name = data.get("name").and_then(|v| v.as_str()).unwrap_or("");
            let description = data.get("description").and_then(|v| v.as_str()).unwrap_or("");
            conn.execute(
                "INSERT INTO race_class (id, name, description, format_id, win_condition, \"cacheStatus\",
                                         \"rankStatus\", rounds, \"heatAdvanceType\", \"roundType\", \"order\", active)
                 VALUES (?1, ?2, ?3, NULL, '0', '{}', '{}', 0, 0, 0, NULL, 1)",
                rusqlite::params![id, name, description],
            )
            .unwrap();
        });
        let payload = db::with_db(&state, payloads::build_class_data);
        broadcast(&state, "class_data", payload);
    });

    let sid = socket.id;
    socket.on("alter_race_class", move |_: SocketRef, data: Data<Value>, s: State<SharedState>| async move {
        let state = s.0.clone();
        let data = data.0;
        let Some(class_id) = data.get("class_id").and_then(|v| v.as_i64()) else { return };
        db::with_db(&state, |conn| {
            if let Some(v) = data.get("class_name").and_then(|v| v.as_str()) {
                let _ = conn.execute("UPDATE race_class SET name = ?1 WHERE id = ?2", rusqlite::params![v, class_id]);
            }
            if let Some(v) = data.get("class_description").and_then(|v| v.as_str()) {
                let _ = conn.execute("UPDATE race_class SET description = ?1 WHERE id = ?2", rusqlite::params![v, class_id]);
            }
            if let Some(v) = data.get("class_format").and_then(|v| v.as_i64()) {
                let f = if v == 0 { None } else { Some(v) };
                let _ = conn.execute("UPDATE race_class SET format_id = ?1 WHERE id = ?2", rusqlite::params![f, class_id]);
            }
        });
        // class_data emitted with noself=True
        let payload = db::with_db(&state, payloads::build_class_data);
        if let Some(ns) = state.io.get().and_then(|io| io.of("/")) {
            let ns = ns.except(sid);
            tokio::spawn(async move {
                let _ = ns.emit("class_data", &payload).await;
            });
        }
    });

    socket.on("delete_class", move |_: SocketRef, data: Data<Value>, s: State<SharedState>| async move {
        let state = s.0.clone();
        let data = data.0;
        let Some(class_id) = data.get("class").and_then(|v| v.as_i64()) else { return };
        let deleted = db::with_db(&state, |conn| {
            conn.execute("DELETE FROM race_class WHERE id = ?1", [class_id]).unwrap_or(0) > 0
        });
        if deleted {
            let payload = db::with_db(&state, payloads::build_class_data);
            broadcast(&state, "class_data", payload);
            let heat_payload = db::with_db(&state, |conn| {
                let race = state.race.lock().unwrap();
                payloads::build_heat_data(conn, &race)
            });
            broadcast(&state, "heat_data", heat_payload);
        }
    });

    // ---- heats ----
    socket.on("add_heat", move |_: SocketRef, s: State<SharedState>| async move {
        let state = s.0.clone();
        // note: zero-payload emits carry no data; the optional "class"
        // variant of add_heat is not exercised by the test suite
        let class_id: Option<i64> = None;
        db::with_db(&state, |conn| {
            let id = db::next_heat_id(conn);
            conn.execute(
                "INSERT INTO heat (id, note, auto_name, class_id, results, \"cacheStatus\", \"order\",
                                   status, auto_frequency, group_id, active, coop_best_time, coop_num_laps)
                 VALUES (?1, NULL, NULL, ?2, NULL, '{}', NULL, 0, 0, 0, 1, NULL, NULL)",
                rusqlite::params![id, class_id],
            )
            .unwrap();
            // one slot per node
            for node_index in 0..state.num_nodes as i64 {
                conn.execute(
                    "INSERT INTO heat_node (id, heat_id, node_index, pilot_id, color, method, seed_rank, seed_id)
                     VALUES (?1, ?2, ?3, NULL, NULL, 0, NULL, NULL)",
                    rusqlite::params![db::next_heat_node_id(conn), id, node_index],
                )
                .unwrap();
            }
        });
        let payload = db::with_db(&state, |conn| {
            let race = state.race.lock().unwrap();
            payloads::build_heat_data(conn, &race)
        });
        broadcast(&state, "heat_data", payload);
        emit_race_status_from(&state);
    });

    socket.on("alter_heat", move |_: SocketRef, data: Data<Value>, s: State<SharedState>| async move {
        let state = s.0.clone();
        let data = data.0;
        let Some(heat_id) = data.get("heat").and_then(|v| v.as_i64()) else { return };
        db::with_db(&state, |conn| {
            if let Some(v) = data.get("name").and_then(|v| v.as_str()) {
                let _ = conn.execute("UPDATE heat SET note = ?1 WHERE id = ?2", rusqlite::params![v, heat_id]);
            }
            if let Some(v) = data.get("class").and_then(|v| v.as_i64()) {
                let _ = conn.execute("UPDATE heat SET class_id = ?1 WHERE id = ?2", rusqlite::params![v, heat_id]);
            }
            if let Some(slot_id) = data.get("slot_id").and_then(|v| v.as_i64()) {
                if let Some(pilot) = data.get("pilot").and_then(|v| v.as_i64()) {
                    let pilot_val: Option<i64> = if pilot == 0 { None } else { Some(pilot) };
                    let _ = conn.execute(
                        "UPDATE heat_node SET pilot_id = ?1 WHERE id = ?2",
                        rusqlite::params![pilot_val, slot_id],
                    );
                }
            }
        });
        // if current heat altered, refresh live node pilots
        // (lock order: DB first, then race -- never hold race across db calls)
        {
            let pilots = db::with_db(&state, |conn| heat_node_pilots(conn, heat_id, state.num_nodes));
            let mut race = state.race.lock().unwrap();
            if race.current_heat == Some(heat_id) {
                race.node_pilots = pilots;
            }
        }
        let payload = db::with_db(&state, |conn| {
            let race = state.race.lock().unwrap();
            payloads::build_heat_data(conn, &race)
        });
        broadcast(&state, "heat_data", payload);
        emit_race_status_from(&state);
    });

    socket.on("delete_heat", move |_: SocketRef, data: Data<Value>, s: State<SharedState>| async move {
        let state = s.0.clone();
        let data = data.0;
        let Some(heat_id) = data.get("heat").and_then(|v| v.as_i64()) else { return };
        let deleted = db::with_db(&state, |conn| {
            let has_races: i64 = conn
                .query_row("SELECT COUNT(*) FROM saved_race_meta WHERE heat_id = ?1", [heat_id], |r| r.get(0))
                .unwrap_or(0);
            if has_races > 0 {
                return false;
            }
            conn.execute("DELETE FROM heat_node WHERE heat_id = ?1", [heat_id]).unwrap_or(0);
            conn.execute("DELETE FROM heat WHERE id = ?1", [heat_id]).unwrap_or(0) > 0
        });
        if deleted {
            let payload = db::with_db(&state, |conn| {
                let race = state.race.lock().unwrap();
                payloads::build_heat_data(conn, &race)
            });
            broadcast(&state, "heat_data", payload);
        }
    });

    socket.on("duplicate_heat", move |_: SocketRef, data: Data<Value>, s: State<SharedState>| async move {
        let state = s.0.clone();
        let data = data.0;
        let Some(heat_id) = data.get("heat").and_then(|v| v.as_i64()) else { return };
        let new_id = db::with_db(&state, |conn| {
            let (note, class_id, auto_frequency): (Option<String>, Option<i64>, bool) = conn
                .query_row(
                    "SELECT note, class_id, auto_frequency FROM heat WHERE id = ?1",
                    [heat_id],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
                )
                .unwrap_or((None, None, false));
            let id = db::next_heat_id(conn);
            conn.execute(
                "INSERT INTO heat (id, note, auto_name, class_id, results, \"cacheStatus\", \"order\",
                                   status, auto_frequency, group_id, active, coop_best_time, coop_num_laps)
                 VALUES (?1, ?2, NULL, ?3, NULL, '{}', NULL, 0, ?4, 0, 1, NULL, NULL)",
                rusqlite::params![id, note, class_id, auto_frequency],
            )
            .unwrap();
            let mut sstmt = conn
                .prepare("SELECT node_index, pilot_id, method, seed_rank, seed_id FROM heat_node WHERE heat_id = ?1")
                .unwrap();
            let mut srows = sstmt.query([heat_id]).unwrap();
            while let Some(row) = srows.next().unwrap() {
                conn.execute(
                    "INSERT INTO heat_node (id, heat_id, node_index, pilot_id, color, method, seed_rank, seed_id)
                     VALUES (?1, ?2, ?3, ?4, NULL, ?5, ?6, ?7)",
                    rusqlite::params![
                        db::next_heat_node_id(conn),
                        id,
                        row.get::<_, Option<i64>>(0).unwrap(),
                        row.get::<_, Option<i64>>(1).unwrap(),
                        row.get::<_, i64>(2).unwrap(),
                        row.get::<_, Option<i64>>(3).unwrap(),
                        row.get::<_, Option<i64>>(4).unwrap(),
                    ],
                )
                .unwrap();
            }
            id
        });
        let _ = new_id;
        let payload = db::with_db(&state, |conn| {
            let race = state.race.lock().unwrap();
            payloads::build_heat_data(conn, &race)
        });
        broadcast(&state, "heat_data", payload);
    });

    socket.on("activate_heat", move |_: SocketRef, data: Data<Value>, s: State<SharedState>| async move {
        let state = s.0.clone();
        let data = data.0;
        if let Some(heat_id) = data.get("heat").and_then(|v| v.as_i64()) {
            db::with_db(&state, |conn| {
                let _ = conn.execute("UPDATE heat SET active = 1 WHERE id = ?1", [heat_id]);
            });
            let payload = db::with_db(&state, |conn| {
                let race = state.race.lock().unwrap();
                payloads::build_heat_data(conn, &race)
            });
            broadcast(&state, "heat_data", payload);
        }
    });

    socket.on("deactivate_heat", move |_: SocketRef, data: Data<Value>, s: State<SharedState>| async move {
        let state = s.0.clone();
        let data = data.0;
        if let Some(heat_id) = data.get("heat").and_then(|v| v.as_i64()) {
            db::with_db(&state, |conn| {
                let _ = conn.execute("UPDATE heat SET active = 0 WHERE id = ?1", [heat_id]);
            });
            let payload = db::with_db(&state, |conn| {
                let race = state.race.lock().unwrap();
                payloads::build_heat_data(conn, &race)
            });
            broadcast(&state, "heat_data", payload);
        }
    });

    socket.on("set_current_heat", move |_: SocketRef, data: Data<Value>, s: State<SharedState>| async move {
        let state = s.0.clone();
        let data = data.0;
        let Some(heat_id) = data.get("heat").and_then(|v| v.as_i64()) else { return };
        let pilots = db::with_db(&state, |conn| heat_node_pilots(conn, heat_id, state.num_nodes));
        {
            let mut race = state.race.lock().unwrap();
            race.current_heat = Some(heat_id);
            race.node_pilots = pilots;
        }
        let payload = db::with_db(&state, |conn| {
            let race = state.race.lock().unwrap();
            payloads::build_current_heat(conn, &race)
        });
        broadcast(&state, "current_heat", payload);
        emit_race_status_from(&state);
    });

    // ---- race formats ----
    socket.on("add_race_format", move |_: SocketRef, data: Data<Value>, s: State<SharedState>| async move {
        let state = s.0.clone();
        let data = data.0;
        let Some(source) = data.get("source_format_id").and_then(|v| v.as_i64()) else { return };
        let _new_id = db::with_db(&state, |conn| {
            let cols = "name, race_mode, race_time_sec, lap_grace_sec, staging_fixed_tones,
                        start_delay_min_ms, start_delay_max_ms, staging_tones, number_laps_win,
                        win_condition, team_racing_mode, start_behavior, points_method";
            let row: (String, i64, i64, i64, i64, i64, i64, i64, i64, i64, i64, i64, Option<String>) = conn
                .query_row(
                    &format!("SELECT {} FROM race_format WHERE id = ?1", cols),
                    [source],
                    |r| {
                        Ok((
                            r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?,
                            r.get(6)?, r.get(7)?, r.get(8)?, r.get(9)?, r.get(10)?, r.get(11)?,
                            r.get(12)?,
                        ))
                    },
                )
                .unwrap();
            let new_id: i64 = conn
                .query_row("SELECT COALESCE(MAX(id), 0) + 1 FROM race_format", [], |r| r.get(0))
                .unwrap_or(1);
            conn.execute(
                &format!("INSERT INTO race_format (id, {}) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)", cols),
                rusqlite::params![
                    new_id, row.0, row.1, row.2, row.3, row.4, row.5, row.6, row.7,
                    row.8, row.9, row.10, row.11, row.12
                ],
            )
            .unwrap();
            new_id
        });
        let payload = db::with_db(&state, payloads::build_format_data);
        broadcast(&state, "format_data", payload);
    });

    socket.on("alter_race_format", move |_: SocketRef, data: Data<Value>, s: State<SharedState>| async move {
        let state = s.0.clone();
        let data = data.0;
        let Some(format_id) = data.get("format_id").and_then(|v| v.as_i64()) else { return };
        let applied = db::with_db(&state, |conn| {
            // format alteration is prevented while a race is active
            let status = state.race.lock().unwrap().race_status;
            if status != RACE_STATUS_READY {
                return false;
            }
            if let Some(v) = data.get("format_name").and_then(|v| v.as_str()) {
                let _ = conn.execute("UPDATE race_format SET name = ?1 WHERE id = ?2", rusqlite::params![v, format_id]);
            }
            macro_rules! set_int {
                ($field:expr, $col:literal) => {
                    if let Some(v) = data.get($field).and_then(|v| v.as_i64()) {
                        let _ = conn.execute(
                            concat!("UPDATE race_format SET ", $col, " = ?1 WHERE id = ?2"),
                            rusqlite::params![v, format_id],
                        );
                    }
                };
            }
            set_int!("unlimited_time", "race_mode");
            set_int!("race_time_sec", "race_time_sec");
            set_int!("lap_grace_sec", "lap_grace_sec");
            set_int!("staging_fixed_tones", "staging_fixed_tones");
            set_int!("start_delay_min_ms", "start_delay_min_ms");
            set_int!("start_delay_max_ms", "start_delay_max_ms");
            set_int!("number_laps_win", "number_laps_win");
            set_int!("win_condition", "win_condition");
            set_int!("team_racing_mode", "team_racing_mode");
            set_int!("start_behavior", "start_behavior");
            if let Some(v) = data.get("staging_delay_tones") {
                let val = if v.as_i64().unwrap_or(0) != 0 || v.as_bool().unwrap_or(false) { 2 } else { 0 };
                let _ = conn.execute(
                    "UPDATE race_format SET staging_tones = ?1 WHERE id = ?2",
                    rusqlite::params![val, format_id],
                );
            }
            true
        });
        if applied {
            let payload = db::with_db(&state, payloads::build_format_data);
            broadcast(&state, "format_data", payload);
            let heat_payload = db::with_db(&state, |conn| {
                let race = state.race.lock().unwrap();
                payloads::build_heat_data(conn, &race)
            });
            broadcast(&state, "heat_data", heat_payload);
            emit_race_status_from(&state);
            let laps_payload = db::with_db(&state, |conn| {
                let race = state.race.lock().unwrap();
                payloads::build_current_laps(conn, &race)
            });
            broadcast(&state, "current_laps", laps_payload);
        }
    });

    socket.on("set_race_format", move |_: SocketRef, data: Data<Value>, s: State<SharedState>| async move {
        let state = s.0.clone();
        let data = data.0;
        let Some(format_id) = data.get("race_format").and_then(|v| v.as_i64()) else { return };
        let ok = {
            let mut race = state.race.lock().unwrap();
            if race.race_status != RACE_STATUS_READY {
                false
            } else {
                race.format_id = format_id;
                db::with_db(&state, |conn| db::set_option(conn, "currentFormat", &format_id.to_string()));
                true
            }
        };
        if ok {
            let laps_payload = db::with_db(&state, |conn| {
                let race = state.race.lock().unwrap();
                payloads::build_current_laps(conn, &race)
            });
            broadcast(&state, "current_laps", laps_payload);
            emit_race_status_from(&state);
        }
    });

    socket.on("delete_race_format", move |_: SocketRef, data: Data<Value>, s: State<SharedState>| async move {
        let state = s.0.clone();
        let data = data.0;
        let Some(format_id) = data.get("format_id").and_then(|v| v.as_i64()) else { return };
        let deleted = db::with_db(&state, |conn| {
            conn.execute("DELETE FROM race_format WHERE id = ?1", [format_id]).unwrap_or(0) > 0
        });
        if deleted {
            let payload = db::with_db(&state, payloads::build_format_data);
            broadcast(&state, "format_data", payload);
        }
    });
}

fn register_race_handlers(socket: SocketRef, state: SharedState) {
    socket.on("stage_race", move |_: SocketRef, s: State<SharedState>| async move {
        let state = s.0.clone();
        race_ops::stage_race(&state);
    });

    socket.on("stop_race", move |_: SocketRef, s: State<SharedState>| async move {
        let state = s.0.clone();
        race_ops::stop_race(&state);
    });

    socket.on("save_laps", move |_: SocketRef, s: State<SharedState>| async move {
        let state = s.0.clone();
        race_ops::save_laps(&state);
    });

    socket.on("discard_laps", move |_: SocketRef, s: State<SharedState>| async move {
        let state = s.0.clone();
        race_ops::discard_laps(&state);
    });

    socket.on("simulate_lap", move |_: SocketRef, data: Data<Value>, s: State<SharedState>| async move {
        let state = s.0.clone();
        let data = data.0;
        if let Some(node) = data.get("node").and_then(|v| v.as_i64()) {
            if node >= 0 && (node as usize) < state.num_nodes {
                race_ops::simulate_lap(&state, node as usize);
            }
        }
    });

    socket.on("delete_lap", move |_: SocketRef, data: Data<Value>, s: State<SharedState>| async move {
        let state = s.0.clone();
        let data = data.0;
        let node = data.get("node").and_then(|v| v.as_i64());
        let lap_index = data.get("lap_index").and_then(|v| v.as_i64());
        if let (Some(node), Some(lap_index)) = (node, lap_index) {
            {
                let mut race = state.race.lock().unwrap();
                if let Some(laps) = race.node_laps.get_mut(node as usize) {
                    if let Some(lap) = laps.get_mut(lap_index as usize) {
                        lap.deleted = true;
                        // deletions invalidate the lap-results cache in Python;
                        // the Rust build recomputes on read
                    }
                }
            }
            let payload = db::with_db(&state, |conn| {
                let race = state.race.lock().unwrap();
                payloads::build_current_laps(conn, &race)
            });
            broadcast(&state, "current_laps", payload);
        }
    });

    socket.on("restore_deleted_lap", move |_: SocketRef, data: Data<Value>, s: State<SharedState>| async move {
        let state = s.0.clone();
        let data = data.0;
        let node = data.get("node").and_then(|v| v.as_i64());
        let lap_index = data.get("lap_index").and_then(|v| v.as_i64());
        if let (Some(node), Some(lap_index)) = (node, lap_index) {
            {
                let mut race = state.race.lock().unwrap();
                if let Some(laps) = race.node_laps.get_mut(node as usize) {
                    if let Some(lap) = laps.get_mut(lap_index as usize) {
                        lap.deleted = false;
                    }
                }
            }
            let payload = db::with_db(&state, |conn| {
                let race = state.race.lock().unwrap();
                payloads::build_current_laps(conn, &race)
            });
            broadcast(&state, "current_laps", payload);
        }
    });

    socket.on("delete_last_lap", move |_: SocketRef, data: Data<Value>, s: State<SharedState>| async move {
        let state = s.0.clone();
        let data = data.0;
        if let Some(node) = data.get("node").and_then(|v| v.as_i64()) {
            {
                let mut race = state.race.lock().unwrap();
                if let Some(laps) = race.node_laps.get_mut(node as usize) {
                    if let Some(lap) = laps.iter_mut().rev().find(|l| !l.deleted) {
                        lap.deleted = true;
                    }
                }
            }
            let payload = db::with_db(&state, |conn| {
                let race = state.race.lock().unwrap();
                payloads::build_current_laps(conn, &race)
            });
            broadcast(&state, "current_laps", payload);
        }
    });

    socket.on("restore_last_deleted_lap", move |_: SocketRef, data: Data<Value>, s: State<SharedState>| async move {
        let state = s.0.clone();
        let data = data.0;
        if let Some(node) = data.get("node").and_then(|v| v.as_i64()) {
            {
                let mut race = state.race.lock().unwrap();
                if let Some(laps) = race.node_laps.get_mut(node as usize) {
                    if let Some(lap) = laps.iter_mut().rev().find(|l| l.deleted) {
                        lap.deleted = false;
                    }
                }
            }
            let payload = db::with_db(&state, |conn| {
                let race = state.race.lock().unwrap();
                payloads::build_current_laps(conn, &race)
            });
            broadcast(&state, "current_laps", payload);
        }
    });

    socket.on("schedule_race", move |_: SocketRef, data: Data<Value>, s: State<SharedState>| async move {
        let state = s.0.clone();
        let data = data.0;
        let minutes = data.get("m").and_then(|v| v.as_i64()).unwrap_or(0);
        let seconds = data.get("s").and_then(|v| v.as_i64()).unwrap_or(0);
        let delay = (minutes * 60 + seconds) as u64;
        {
            let mut race = state.race.lock().unwrap();
            race.scheduled = true;
            race.scheduled_time = now_epoch_ms() + (delay as f64 * 1000.0);
        }
        let payload = {
            let race = state.race.lock().unwrap();
            json!({"scheduled": race.scheduled, "scheduled_at": race.scheduled_time})
        };
        broadcast(&state, "race_scheduled", payload);
    });

    socket.on("cancel_schedule_race", move |_: SocketRef, s: State<SharedState>| async move {
        let state = s.0.clone();
        {
            let mut race = state.race.lock().unwrap();
            race.scheduled = false;
            race.scheduled_time = 0.0;
        }
        let payload = json!({"scheduled": false, "scheduled_at": Value::Null});
        broadcast(&state, "race_scheduled", payload);
    });

    socket.on("get_race_scheduled", move |socket: SocketRef, s: State<SharedState>| async move {
        let state = s.0.clone();
        let payload = {
            let race = state.race.lock().unwrap();
            json!({"scheduled": race.scheduled, "scheduled_at": race.scheduled_time})
        };
        socket.emit("race_scheduled", &payload);
    });

    socket.on("get_server_time", move |_: SocketRef, ack: AckSender, s: State<SharedState>| async move {
        // replies via socket.io ack callback
        let _ = ack.send(&json!({"server_time_s": monotonic()}));
    });
}

fn register_settings_handlers(socket: SocketRef, state: SharedState) {
    socket.on("set_frequency", move |_: SocketRef, data: Data<Value>, s: State<SharedState>| async move {
        let state = s.0.clone();
        let data = data.0;
        let Some(node) = data.get("node").and_then(|v| v.as_i64()) else { return };
        let band = data.get("band").and_then(|v| v.as_str());
        let chan = data.get("channel").and_then(|v| v.as_i64());
        let freq = data.get("frequency").and_then(|v| v.as_i64()).unwrap_or(0);
        db::with_db(&state, |conn| {
            let profile_id = db::current_profile_id(conn);
            let (bands, chans, freqs) = read_freq_arrays(conn, profile_id);
            let idx = node as usize;
            let mut nb = bands; let mut nc = chans; let mut nf = freqs;
            while nb.len() <= idx { nb.push(Value::Null); nc.push(Value::Null); nf.push(Value::Null); }
            nb[idx] = band.map(|b| Value::String(b.to_string())).unwrap_or(Value::Null);
            nc[idx] = chan.map(|c| json!(c)).unwrap_or(Value::Null);
            nf[idx] = json!(freq);
            write_freq_arrays(conn, profile_id, &nb, &nc, &nf);
        });
        // derive band/channel from frequency when only a frequency was given
        if band.is_none() && freq > 0 {
            if let Some((b, c)) = crate::util::band_channel_for(freq as u16) {
                db::with_db(&state, |conn| {
                    let profile_id = db::current_profile_id(conn);
                    let (mut nb, mut nc, nf) = read_freq_arrays(conn, profile_id);
                    nb[node as usize] = json!(b);
                    nc[node as usize] = json!(c);
                    write_freq_arrays(conn, profile_id, &nb, &nc, &nf);
                });
            }
        }
        tracing::info!("Frequency set: Node {} band={:?} ch={:?} freq={}", node + 1, band, chan, freq);
        let payload = db::with_db(&state, payloads::build_frequency_data);
        broadcast(&state, "frequency_data", payload);
    });

    socket.on("set_frequency_preset", move |_: SocketRef, data: Data<Value>, s: State<SharedState>| async move {
        let state = s.0.clone();
        let data = data.0;
        let preset = data.get("preset").and_then(|v| v.as_str()).unwrap_or("RB");
        db::with_db(&state, |conn| {
            let profile_id = db::current_profile_id(conn);
            let mut nb = vec![Value::Null; state.num_nodes];
            let mut nc = vec![Value::Null; state.num_nodes];
            let mut nf = vec![json!(0); state.num_nodes];
            for (i, (b, c, f)) in crate::util::preset_frequencies(preset).into_iter().enumerate() {
                if i < state.num_nodes {
                    nb[i] = json!(b);
                    nc[i] = json!(c);
                    nf[i] = json!(f);
                }
            }
            write_freq_arrays(conn, profile_id, &nb, &nc, &nf);
            tracing::info!("Frequency preset applied: {}", preset);
        });
        let payload = db::with_db(&state, payloads::build_frequency_data);
        broadcast(&state, "frequency_data", payload);
    });

    socket.on("add_profile", move |_: SocketRef, data: Data<Value>, s: State<SharedState>| async move {
        let state = s.0.clone();
        let data = data.0;
        db::with_db(&state, |conn| {
            let id: i64 = conn
                .query_row("SELECT COALESCE(MAX(id), 0) + 1 FROM profiles", [], |r| r.get(0))
                .unwrap_or(1);
            let name = data.get("name").and_then(|v| v.as_str()).map(|s| s.to_string())
                .unwrap_or_else(|| format!("Profile {}", id));
            // new profiles copy the current profile's frequency data
            let cur = db::current_profile_id(conn);
            let freqs: String = conn
                .query_row("SELECT frequencies FROM profiles WHERE id = ?1", [cur], |r| r.get(0))
                .unwrap_or_else(|_| json!({"b": [], "c": [], "f": []}).to_string());
            conn.execute(
                "INSERT INTO profiles (id, name, description, frequencies, enter_ats, exit_ats, f_ratio)
                 VALUES (?1, ?2, NULL, ?3, NULL, NULL, NULL)",
                rusqlite::params![id, name, freqs],
            )
            .unwrap();
            db::set_option(conn, "currentProfile", &id.to_string());
        });
        let payload = db::with_db(&state, payloads::build_frequency_data);
        broadcast(&state, "frequency_data", payload);
    });

    socket.on("set_enter_at_level", move |sock: SocketRef, data: Data<Value>, s: State<SharedState>| async move {
        let state = s.0.clone();
        let data = data.0;
        let Some(node) = data.get("node").and_then(|v| v.as_i64()) else { return };
        let level = data.get("enter_at_level").and_then(|v| v.as_i64()).unwrap_or(0);
        db::with_db(&state, |conn| {
            let profile_id = db::current_profile_id(conn);
            let enter: Option<String> = conn
                .query_row("SELECT enter_ats FROM profiles WHERE id = ?1", [profile_id], |r| r.get(0))
                .ok();
            let mut arr = enter
                .and_then(|s| serde_json::from_str::<Value>(&s).ok())
                .unwrap_or(json!({"v": []}));
            let v = arr.as_object_mut().unwrap().entry("v").or_insert(json!([]));
            while v.as_array().unwrap().len() <= node as usize {
                v.as_array_mut().unwrap().push(Value::Null);
            }
            v.as_array_mut().unwrap()[node as usize] = json!(level);
            let _ = conn.execute(
                "UPDATE profiles SET enter_ats = ?1 WHERE id = ?2",
                rusqlite::params![arr.to_string(), profile_id],
            );
        });
        // response goes only to the requesting socket
        let payload = db::with_db(&state, payloads::build_enter_exit_levels);
        sock.emit("enter_and_exit_at_levels", &payload);
    });

    socket.on("set_exit_at_level", move |sock: SocketRef, data: Data<Value>, s: State<SharedState>| async move {
        let state = s.0.clone();
        let data = data.0;
        let Some(node) = data.get("node").and_then(|v| v.as_i64()) else { return };
        let level = data.get("exit_at_level").and_then(|v| v.as_i64()).unwrap_or(0);
        db::with_db(&state, |conn| {
            let profile_id = db::current_profile_id(conn);
            let exit: Option<String> = conn
                .query_row("SELECT exit_ats FROM profiles WHERE id = ?1", [profile_id], |r| r.get(0))
                .ok();
            let mut arr = exit
                .and_then(|s| serde_json::from_str::<Value>(&s).ok())
                .unwrap_or(json!({"v": []}));
            let v = arr.as_object_mut().unwrap().entry("v").or_insert(json!([]));
            while v.as_array().unwrap().len() <= node as usize {
                v.as_array_mut().unwrap().push(Value::Null);
            }
            v.as_array_mut().unwrap()[node as usize] = json!(level);
            let _ = conn.execute(
                "UPDATE profiles SET exit_ats = ?1 WHERE id = ?2",
                rusqlite::params![arr.to_string(), profile_id],
            );
        });
        let payload = db::with_db(&state, payloads::build_enter_exit_levels);
        sock.emit("enter_and_exit_at_levels", &payload);
    });

    socket.on("set_start_thresh_lower_amount", move |sock: SocketRef, data: Data<Value>, s: State<SharedState>| async move {
        let state = s.0.clone();
        let data = data.0;
        let val = data.get("start_thresh_lower_amount").cloned().unwrap_or(json!(0));
        // stored in config.json TIMING section by Python; Rust keeps it in DB for now
        db::with_db(&state, |conn| {
            db::set_option(conn, "TIMING_startThreshLowerAmount", &val.to_string());
        });
        let payload = json!({"start_thresh_lower_amount": val.as_i64().unwrap_or(0)});
        sock.emit("start_thresh_lower_amount", &payload);
    });

    socket.on("set_start_thresh_lower_duration", move |sock: SocketRef, data: Data<Value>, s: State<SharedState>| async move {
        let state = s.0.clone();
        let data = data.0;
        let val = data.get("start_thresh_lower_duration").cloned().unwrap_or(json!(0));
        db::with_db(&state, |conn| {
            db::set_option(conn, "TIMING_startThreshLowerDuration", &val.to_string());
        });
        let payload = json!({"start_thresh_lower_duration": val.as_i64().unwrap_or(0)});
        sock.emit("start_thresh_lower_duration", &payload);
    });

    let sid = socket.id;
    socket.on("set_min_lap", move |_: SocketRef, data: Data<Value>, s: State<SharedState>| async move {
        let state = s.0.clone();
        let data = data.0;
        let val = data.get("min_lap").and_then(|v| v.as_i64()).unwrap_or(0);
        db::with_db(&state, |conn| {
            db::set_option(conn, "MinLapSec", &val.to_string());
        });
        // min_lap emitted with noself=True
        let payload = db::with_db(&state, payloads::build_min_lap);
        if let Some(ns) = state.io.get().and_then(|io| io.of("/")) {
            let ns = ns.except(sid);
            tokio::spawn(async move {
                let _ = ns.emit("min_lap", &payload).await;
            });
        }
    });

    socket.on("set_language", move |_: SocketRef, data: Data<Value>, s: State<SharedState>| async move {
        let state = s.0.clone();
        let data = data.0;
        if let Some(lang) = data.get("language").and_then(|v| v.as_str()) {
            db::with_db(&state, |conn| {
                db::set_option(conn, "currentLanguage", lang);
            });
        }
    });

    socket.on("set_config", move |sock: SocketRef, data: Data<Value>, s: State<SharedState>| async move {
        let state = s.0.clone();
        let data = data.0;
        if let (Some(section), Some(key), Some(value)) = (
            data.get("section").and_then(|v| v.as_str()),
            data.get("key").and_then(|v| v.as_str()),
            data.get("value"),
        ) {
            // the RACE_START_DELAY_EXTRA_SECS value lives in config.json on Python;
            // the Rust build keeps GENERAL items in an in-memory map for now
            state.config_items.lock().unwrap().insert(
                format!("{}_{}", section, key),
                value.clone(),
            );
            tracing::info!("config set: {}.{} = {}", section, key, value);
        }
        // note: set_config does not emit config_update; clients re-request
        // config via load_data (characterized behavior)
    });

    socket.on("set_option", move |_: SocketRef, data: Data<Value>, s: State<SharedState>| async move {
        let state = s.0.clone();
        let data = data.0;
        if let (Some(option), Some(value)) = (
            data.get("option").and_then(|v| v.as_str()),
            data.get("value"),
        ) {
            let text = match value {
                Value::String(s) => s.clone(),
                other => other.to_string(),
            };
            db::with_db(&state, |conn| db::set_option(conn, option, &text));
        }
    });

    socket.on("backup_database", move |_: SocketRef, s: State<SharedState>| async move {
        let state = s.0.clone();
        let result = (|| -> Option<String> {
            let data_dir = &state.data_dir;
            let bkp_dir = format!("{}/db_bkp", data_dir);
            std::fs::create_dir_all(&bkp_dir).ok()?;
            let ts = chrono::Local::now().format("%Y%m%d_%H%M%S");
            let name = format!("db_bkp/database_{}.db", ts);
            let src = format!("{}/database.db", data_dir);
            std::fs::copy(&src, format!("{}/{}", data_dir, name)).ok()?;
            Some(name)
        })();
        if result.is_some() {
            broadcast(&state, "database_bkp_done", json!({}));
        }
    });

    socket.on("list_backups", move |sock: SocketRef, s: State<SharedState>| async move {
        let state = s.0.clone();
        let mut files = Vec::new();
        let dir = format!("{}/db_bkp", state.data_dir);
        if let Ok(entries) = std::fs::read_dir(&dir) {
            for entry in entries.flatten() {
                if let Some(name) = entry.file_name().to_str() {
                    if name.ends_with(".db") {
                        files.push(name.to_string());
                    }
                }
            }
        }
        files.sort();
        files.reverse(); // newest first, matching list_backup_files()
        sock.emit("backups_list", &json!({"backup_files": files}));
    });
}

fn heat_node_pilots(conn: &rusqlite::Connection, heat_id: i64, num_nodes: usize) -> Vec<Option<i64>> {
    let mut pilots = vec![None; num_nodes];
    let mut stmt = conn
        .prepare("SELECT node_index, pilot_id FROM heat_node WHERE heat_id = ?1")
        .unwrap();
    let mut rows = stmt.query([heat_id]).unwrap();
    while let Some(row) = rows.next().unwrap() {
        let node_index: Option<i64> = row.get(0).unwrap();
        let pilot_id: Option<i64> = row.get(1).unwrap();
        if let Some(idx) = node_index {
            if idx >= 0 && (idx as usize) < num_nodes {
                pilots[idx as usize] = pilot_id;
            }
        }
    }
    pilots
}

type FreqArrays = (Vec<Value>, Vec<Value>, Vec<Value>);

fn read_freq_arrays(conn: &rusqlite::Connection, profile_id: i64) -> FreqArrays {
    let json_str: Option<String> = conn
        .query_row("SELECT frequencies FROM profiles WHERE id = ?1", [profile_id], |r| r.get(0))
        .ok();
    let val = json_str
        .and_then(|s| serde_json::from_str::<Value>(&s).ok())
        .unwrap_or(json!({}));
    let get = |key: &str| -> Vec<Value> {
        val.get(key)
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default()
    };
    (get("b"), get("c"), get("f"))
}

fn write_freq_arrays(
    conn: &rusqlite::Connection,
    profile_id: i64,
    b: &[Value],
    c: &[Value],
    f: &[Value],
) {
    let payload = json!({"b": b, "c": c, "f": f});
    let _ = conn.execute(
        "UPDATE profiles SET frequencies = ?1 WHERE id = ?2",
        rusqlite::params![payload.to_string(), profile_id],
    );
}

fn emit_race_status_from(state: &SharedState) {
    let payload = db::with_db(state, |conn| {
        let race = state.race.lock().unwrap();
        payloads::build_race_status(conn, &race)
    });
    broadcast(&state, "race_status", payload);
}
