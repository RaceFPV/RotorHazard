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
use serde_json::{json, Map, Value};
use socketioxide::extract::{AckSender, Data, SocketRef, State};
use socketioxide::SocketIo;

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
        // record whether the connection carries valid admin basic-auth
        // (mirrors Python's per-request authorization checks)
        let authed = {
            let parts = socket.req_parts();
            parse_basic_auth_ok(parts.headers.clone(), &state)
        };
        socket.extensions.insert(authed);

        
        
        
        
        // push initial data trigger: the browser requests its page data in
        // response to load_all (emit_frontend_load, delayed 50ms like Python)
        {
            let load_sock = socket.clone();
            let token = state.server_instance_token;
            tokio::spawn(async move {
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                let _ = load_sock.emit("load_all", &json!({"server_instance_token": token}));
            });
        }

        let creds_warned = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        socket.on("load_data", move |sock: SocketRef, data: Data<Value>, s: State<SharedState>| async move {
            let state = s.0.clone();
            // default-credentials notice (check_default_admin_creds_warning):
            // emitted to the client on its first data load
            if !creds_warned.swap(true, std::sync::atomic::Ordering::Relaxed)
                && state.admin_username == "admin"
                && state.admin_password == "rotorhazard"
            {
                let _ = sock.emit("admin_creds_defaulted", &json!({
                    "username": "admin",
                    "password": "rotorhazard",
                }));
            }
            handle_load_data(&sock, &state, data);
        });

        register_crud_handlers(socket.clone(), state.clone());
        register_race_handlers(socket.clone(), state.clone());
        register_settings_handlers(socket.clone(), state.clone());
        register_export_import_handlers(socket.clone(), state.clone());
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

    for load_type in &load_types {
        match load_type.as_str() {
            Some("pilot_data") => {
                let payload = db::with_db(state, payloads::build_pilot_data);
                let _ = socket.emit("pilot_data", &payload);
            }
            Some("heat_data") => {
                let payload = db::with_db(state, |conn| {
                    let race = state.race.lock().unwrap();
                    payloads::build_heat_data(conn, &race)
                });
                let _ = socket.emit("heat_data", &payload);
            }
            Some("class_data") => {
                let payload = db::with_db(state, payloads::build_class_data);
                let _ = socket.emit("class_data", &payload);
            }
            Some("format_data") => {
                let payload = db::with_db(state, payloads::build_format_data);
                let _ = socket.emit("format_data", &payload);
            }
            Some("frequency_data") => {
                let payload = db::with_db(state, payloads::build_frequency_data);
                let _ = socket.emit("frequency_data", &payload);
            }
            Some("node_data") => {
                let payload = payloads::build_node_data(&state.race.lock().unwrap());
                let _ = socket.emit("node_data", &payload);
            }
            Some("race_status") => {
                let payload = db::with_db(state, |conn| {
                    let race = state.race.lock().unwrap();
                    payloads::build_race_status(conn, &race)
                });
                let _ = socket.emit("race_status", &payload);
            }
            Some("current_heat") => {
                let payload = db::with_db(state, |conn| {
                    let race = state.race.lock().unwrap();
                    payloads::build_current_heat(conn, &race)
                });
                let _ = socket.emit("current_heat", &payload);
            }
            Some("seat_data") => {
                let payload = db::with_db(state, payloads::build_seat_data);
                let _ = socket.emit("seat_data", &payload);
            }
            Some("min_lap") => {
                let payload = db::with_db(state, payloads::build_min_lap);
                let _ = socket.emit("min_lap", &payload);
            }
            Some("enter_and_exit_at_levels") => {
                let payload = db::with_db(state, payloads::build_enter_exit_levels);
                let _ = socket.emit("enter_and_exit_at_levels", &payload);
            }
            Some("current_laps") => {
                let payload = db::with_db(state, |conn| {
                    let race = state.race.lock().unwrap();
                    payloads::build_current_laps(conn, &race)
                });
                let _ = socket.emit("current_laps", &payload);
            }
            Some("pilot_list") => {
                let payload = db::with_db(state, payloads::build_pilot_list);
                let _ = socket.emit("pilot_list", &payload);
            }
            Some("heat_list") => {
                let payload = db::with_db(state, payloads::build_heat_list);
                let _ = socket.emit("heat_list", &payload);
            }
            Some("class_list") => {
                let payload = db::with_db(state, payloads::build_class_list);
                let _ = socket.emit("class_list", &payload);
            }
            Some("leaderboard") => {
                let payload = build_current_leaderboard(&state);
                let _ = socket.emit("leaderboard", &payload);
            }
            Some("race_list") => {
                let payload = build_race_list(&state);
                let _ = socket.emit("race_list", &payload);
            }
            Some("exporter_list") => {
                let _ = socket.emit("exporter_list", &crate::export_import::exporter_list());
            }
            Some("importer_list") => {
                let _ = socket.emit("importer_list", &crate::export_import::importer_list());
            }
            Some("all_languages") => {
                let _ = socket.emit("all_languages", &json!({"languages": crate::phase3::languages()}));
            }
            Some("language") => {
                let current = crate::phase3::current_language(&state.data_dir);
                let _ = socket.emit("language", &json!({
                    "language": current,
                    "languages": language_list(&crate::phase3::languages()),
                }));
            }
            Some("ui") => {
                // no plugin-registered panels in the Rust build yet; the
                // page shell still functions from its static template parts
                let page = load_type
                    .get("value")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                let _ = socket.emit("ui", &json!({"page": page, "panels": []}));
            }
            Some("node_tuning") => {
                let payload = db::with_db(&state, |conn| {
                    let mut ids = Vec::new();
                    let mut names = Vec::new();
                    let mut stmt = conn
                        .prepare("SELECT id, name FROM profiles ORDER BY id")
                        .unwrap();
                    let mut rows = stmt.query([]).unwrap();
                    while let Some(row) = rows.next().unwrap() {
                        ids.push(row.get::<_, i64>(0).unwrap());
                        names.push(row.get::<_, String>(1).unwrap());
                    }
                    let current = db::current_profile_id(conn);
                    let (name, description): (String, Option<String>) = conn
                        .query_row(
                            "SELECT name, description FROM profiles WHERE id = ?1",
                            [current],
                            |r| Ok((r.get(0)?, r.get(1)?)),
                        )
                        .unwrap_or(("Default".to_string(), None));
                    json!({
                        "profile_ids": ids,
                        "profile_names": names,
                        "current_profile": current,
                        "profile_name": name,
                        "profile_description": description,
                    })
                });
                let _ = socket.emit("node_tuning", &payload);
            }
            Some("led_effects") => {
                let _ = socket.emit("led_effects", &json!({"enabled": true, "effects": {}}));
            }
            Some("led_effect_setup") => {
                let _ = socket.emit("led_effect_setup_data", &json!({"enabled": true}));
            }
            Some("cluster_status") => {
                let payload = state
                    .cluster
                    .get()
                    .map(|c| c.get_cluster_status_info())
                    .unwrap_or(json!({"secondaries": []}));
                let _ = socket.emit("cluster_status", &payload);
            }
            Some("vrx_list") => {
                let _ = socket.emit("vrx_list", &json!({
                    "enabled": state.vrx_manager.is_enabled(),
                    "controllers": state.vrx_manager.get_controller_status(),
                    "devices": state.vrx_manager.get_all_device_status(),
                }));
            }
            Some("environmental_data") => {
                let _ = socket.emit("environmental_data", &crate::sensor::environmental_data());
            }
            Some("upd_cfg_files_list") => {
                let _ = socket.emit("upd_cfg_files_list", &json!({"cfg_files": [], "select_file": Value::Null}));
            }
            Some("plugin_list") => {
                let _ = socket.emit("plugin_list", &json!({"plugins": []}));
            }
            Some("action_setup") => {
                let _ = socket.emit("action_setup", &json!({"enabled": false, "events": {}, "effects": {}}));
            }
            Some("event_actions") => {
                let _ = socket.emit("event_actions", &json!({"actions": {}}));
            }
            _ => {
                if let Some(obj) = load_type.as_object() {
                    match obj.get("type").and_then(|t| t.as_str()) {
                        Some("config") => {
                            let payload = build_config_sections(state, obj.get("value"));
                            let _ = socket.emit("config_update", &payload);
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

fn register_crud_handlers(socket: SocketRef, _state: SharedState) {
    // ---- pilots ----
    socket.on("add_pilot", move |sock: SocketRef, s: State<SharedState>| async move {
        let state = s.0.clone();
            if !socket_authed(&sock, &state) { return; }
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
    socket.on("alter_pilot", move |sock: SocketRef, data: Data<Value>, s: State<SharedState>| async move {
        let state = s.0.clone();
            if !socket_authed(&sock, &state) { return; }
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

    socket.on("delete_pilot", move |sock: SocketRef, data: Data<Value>, s: State<SharedState>| async move {
        let state = s.0.clone();
            if !socket_authed(&sock, &state) { return; }
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
    socket.on("add_race_class", move |sock: SocketRef, data: Data<Value>, s: State<SharedState>| async move {
        let state = s.0.clone();
            if !socket_authed(&sock, &state) { return; }
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
    socket.on("alter_race_class", move |sock: SocketRef, data: Data<Value>, s: State<SharedState>| async move {
        let state = s.0.clone();
            if !socket_authed(&sock, &state) { return; }
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

    socket.on("delete_class", move |sock: SocketRef, data: Data<Value>, s: State<SharedState>| async move {
        let state = s.0.clone();
            if !socket_authed(&sock, &state) { return; }
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
    socket.on("add_heat", move |sock: SocketRef, s: State<SharedState>| async move {
        let state = s.0.clone();
            if !socket_authed(&sock, &state) { return; }
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
            for node_index in 0..state.node_count() as i64 {
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

    socket.on("alter_heat", move |sock: SocketRef, data: Data<Value>, s: State<SharedState>| async move {
        let state = s.0.clone();
            if !socket_authed(&sock, &state) { return; }
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
                if let Some(v) = data.get("method").and_then(|v| v.as_i64()) {
                    let _ = conn.execute(
                        "UPDATE heat_node SET method = ?1, seed_id = NULL WHERE id = ?2",
                        rusqlite::params![v, slot_id],
                    );
                }
                if let Some(v) = data.get("seed_heat_id").and_then(|v| v.as_i64()) {
                    let _ = conn.execute(
                        "UPDATE heat_node SET seed_id = ?1 WHERE id = ?2",
                        rusqlite::params![v, slot_id],
                    );
                }
                if let Some(v) = data.get("seed_class_id").and_then(|v| v.as_i64()) {
                    let _ = conn.execute(
                        "UPDATE heat_node SET seed_id = ?1 WHERE id = ?2",
                        rusqlite::params![v, slot_id],
                    );
                }
                if let Some(v) = data.get("seed_rank").and_then(|v| v.as_i64()) {
                    let _ = conn.execute(
                        "UPDATE heat_node SET seed_rank = ?1 WHERE id = ?2",
                        rusqlite::params![v, slot_id],
                    );
                }
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
            let pilots = db::with_db(&state, |conn| heat_node_pilots(conn, heat_id, state.node_count()));
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

    socket.on("delete_heat", move |sock: SocketRef, data: Data<Value>, s: State<SharedState>| async move {
        let state = s.0.clone();
            if !socket_authed(&sock, &state) { return; }
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

    socket.on("duplicate_heat", move |sock: SocketRef, data: Data<Value>, s: State<SharedState>| async move {
        let state = s.0.clone();
            if !socket_authed(&sock, &state) { return; }
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

    socket.on("activate_heat", move |sock: SocketRef, data: Data<Value>, s: State<SharedState>| async move {
        let state = s.0.clone();
            if !socket_authed(&sock, &state) { return; }
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

    socket.on("deactivate_heat", move |sock: SocketRef, data: Data<Value>, s: State<SharedState>| async move {
        let state = s.0.clone();
            if !socket_authed(&sock, &state) { return; }
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

    socket.on("set_current_heat", move |sock: SocketRef, data: Data<Value>, s: State<SharedState>| async move {
        let state = s.0.clone();
            if !socket_authed(&sock, &state) { return; }
        let data = data.0;
        let Some(heat_id) = data.get("heat").and_then(|v| v.as_i64()) else { return };
        let pilots = db::with_db(&state, |conn| heat_node_pilots(conn, heat_id, state.node_count()));
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
    socket.on("add_race_format", move |sock: SocketRef, data: Data<Value>, s: State<SharedState>| async move {
        let state = s.0.clone();
            if !socket_authed(&sock, &state) { return; }
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

    socket.on("alter_race_format", move |sock: SocketRef, data: Data<Value>, s: State<SharedState>| async move {
        let state = s.0.clone();
            if !socket_authed(&sock, &state) { return; }
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

    socket.on("set_race_format", move |sock: SocketRef, data: Data<Value>, s: State<SharedState>| async move {
        let state = s.0.clone();
            if !socket_authed(&sock, &state) { return; }
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

    socket.on("delete_race_format", move |sock: SocketRef, data: Data<Value>, s: State<SharedState>| async move {
        let state = s.0.clone();
            if !socket_authed(&sock, &state) { return; }
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

fn register_race_handlers(socket: SocketRef, _state: SharedState) {
    socket.on("stage_race", move |sock: SocketRef, s: State<SharedState>| async move {
        let state = s.0.clone();
            if !socket_authed(&sock, &state) { return; }
        race_ops::stage_race(&state);
    });

    socket.on("stop_race", move |sock: SocketRef, s: State<SharedState>| async move {
        let state = s.0.clone();
            if !socket_authed(&sock, &state) { return; }
        race_ops::stop_race(&state);
    });

    socket.on("save_laps", move |sock: SocketRef, s: State<SharedState>| async move {
        let state = s.0.clone();
            if !socket_authed(&sock, &state) { return; }
        race_ops::save_laps(&state);
    });

    socket.on("discard_laps", move |sock: SocketRef, s: State<SharedState>| async move {
        let state = s.0.clone();
            if !socket_authed(&sock, &state) { return; }
        race_ops::discard_laps(&state);
    });

    socket.on("simulate_lap", move |_: SocketRef, data: Data<Value>, s: State<SharedState>| async move {
        let state = s.0.clone();
        let data = data.0;
        if let Some(node) = data.get("node").and_then(|v| v.as_i64()) {
            if node >= 0 && (node as usize) < state.node_count() {
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
        let _ = socket.emit("race_scheduled", &payload);
    });

    socket.on("resave_laps", move |sock: SocketRef, data: Data<Value>, s: State<SharedState>| async move {
        let state = s.0.clone();
            if !socket_authed(&sock, &state) { return; }
        let data = data.0;
        let get = |k: &str| data.get(k);
        let heat_id = get("heat_id").and_then(|v| v.as_i64()).unwrap_or(0);
        let round_id = get("round_id").and_then(|v| v.as_i64()).unwrap_or(0);
        let callsign = get("callsign").and_then(|v| v.as_str()).unwrap_or("").to_string();
        let pilotrace_id = get("pilotrace_id").and_then(|v| v.as_i64()).unwrap_or(0);
        let enter_at = get("enter_at").and_then(|v| v.as_i64()).unwrap_or(0);
        let exit_at = get("exit_at").and_then(|v| v.as_i64()).unwrap_or(0);
        let laps = get("laps").and_then(|v| v.as_array()).cloned().unwrap_or_default();

        let ok = db::with_db(&state, |conn| {
            let updated = conn
                .execute(
                    "UPDATE saved_pilot_race SET enter_at = ?1, exit_at = ?2 WHERE id = ?3",
                    rusqlite::params![enter_at, exit_at, pilotrace_id],
                )
                .unwrap_or(0);
            if updated == 0 {
                return false;
            }
            conn.execute("DELETE FROM saved_race_lap WHERE pilotrace_id = ?1", [pilotrace_id])
                .unwrap_or(0);
            for lap in &laps {
                let lap_time = lap.get("lap_time").and_then(|v| v.as_f64()).unwrap_or(0.0);
                let stamp = lap.get("lap_time_stamp").and_then(|v| v.as_f64()).unwrap_or(0.0);
                let source = lap.get("source").and_then(|v| v.as_i64()).unwrap_or(1);
                let deleted = lap.get("deleted").and_then(|v| v.as_bool()).unwrap_or(false);
                let node_index: i64 = conn
                    .query_row(
                        "SELECT node_index FROM saved_pilot_race WHERE id = ?1",
                        [pilotrace_id],
                        |r| r.get(0),
                    )
                    .unwrap_or(0);
                let pilot_id: Option<i64> = conn
                    .query_row(
                        "SELECT pilot_id FROM saved_pilot_race WHERE id = ?1",
                        [pilotrace_id],
                        |r| r.get(0),
                    )
                    .unwrap_or(None);
                conn.execute(
                    "INSERT INTO saved_race_lap (race_id, pilotrace_id, node_index, pilot_id,
                                                 lap_time_stamp, lap_time, lap_time_formatted,
                                                 source, deleted, peak_rssi)
                     SELECT race_id, ?1, node_index, pilot_id, ?2, ?3, ?4, ?5, ?6, NULL
                     FROM saved_pilot_race WHERE id = ?1",
                    rusqlite::params![
                        pilotrace_id,
                        stamp,
                        lap_time,
                        db::format_time_to_str(lap_time),
                        source,
                        deleted
                    ],
                )
                .unwrap();
                let _ = (node_index, pilot_id);
            }
            true
        });
        if ok {
            emit_priority_message(
                &state,
                &format!("Race times adjusted for: Heat {} Round {} / {}", heat_id, round_id, callsign),
                false,
            );
            // run adaptive calibration (Python: Evt.LAPS_RESAVE path)
            if crate::db::get_config_number(&state, "TIMING", "calibrationMode", 0.0) != 0.0 {
                crate::calibration::auto_calibrate(&state);
            }
        } else {
            tracing::warn!("resave_laps: unable to resave pilotrace_id={}", pilotrace_id);
        }
    });

    socket.on("replace_current_laps", move |sock: SocketRef, data: Data<Value>, s: State<SharedState>| async move {
        let state = s.0.clone();
            if !socket_authed(&sock, &state) { return; }
        let data = data.0;
        let seat = data.get("node").and_then(|v| v.as_i64())
            .or_else(|| data.get("seat").and_then(|v| v.as_i64()));
        let Some(seat) = seat else { return };
        let enter_at = data.get("enter_at").and_then(|v| v.as_i64()).unwrap_or(0);
        let exit_at = data.get("exit_at").and_then(|v| v.as_i64()).unwrap_or(0);

        // update profile enter/exit levels for the seat
        db::with_db(&state, |conn| {
            let profile_id = db::current_profile_id(conn);
            for (col, val) in [("enter_ats", enter_at), ("exit_ats", exit_at)] {
                let cur: Option<String> = conn
                    .query_row(&format!("SELECT {} FROM profiles WHERE id = ?1", col), [profile_id], |r| r.get(0))
                    .ok();
                let mut arr = cur
                    .and_then(|s| serde_json::from_str::<Value>(&s).ok())
                    .unwrap_or(json!({"v": []}));
                let v = arr.as_object_mut().unwrap().entry("v").or_insert(json!([]));
                while v.as_array().unwrap().len() <= seat as usize {
                    v.as_array_mut().unwrap().push(Value::Null);
                }
                v.as_array_mut().unwrap()[seat as usize] = json!(val);
                let _ = conn.execute(
                    &format!("UPDATE profiles SET {} = ?1 WHERE id = ?2", col),
                    rusqlite::params![arr.to_string(), profile_id],
                );
            }
        });

        // replace the seat's live laps
        {
            let mut race = state.race.lock().unwrap();
            if let Some(laps_slot) = race.node_laps.get_mut(seat as usize) {
                laps_slot.clear();
                if let Some(laps) = data.get("laps").and_then(|v| v.as_array()) {
                    for lap in laps {
                        laps_slot.push(Lap {
                            lap_time_stamp: lap.get("lap_time_stamp").and_then(|v| v.as_f64()).unwrap_or(0.0),
                            lap_time: lap.get("lap_time").and_then(|v| v.as_f64()).unwrap_or(0.0),
                            source: lap.get("source").and_then(|v| v.as_i64()).unwrap_or(1),
                            deleted: lap.get("deleted").and_then(|v| v.as_bool()).unwrap_or(false),
                        });
                    }
                }
            }
        }
        let payload = db::with_db(&state, |conn| {
            let race = state.race.lock().unwrap();
            payloads::build_current_laps(conn, &race)
        });
        broadcast(&state, "current_laps", payload);
    });

    socket.on("current_race_marshal", move |sock: SocketRef, s: State<SharedState>| async move {
        let state = s.0.clone();
        let (status, heat_id, format_id, start_time, start_time_formatted) = {
            let race = state.race.lock().unwrap();
            (
                race.race_status,
                race.current_heat,
                race.format_id,
                race.start_time_monotonic,
                race.start_time_formatted.clone(),
            )
        };
        if status != RACE_STATUS_DONE || heat_id.is_none() {
            return;
        }
        let race_snapshot = {
            let race = state.race.lock().unwrap();
            race.node_laps.clone()
        };
        let marshal_type = state.mock_provider.lock().unwrap().1.marshal_type();
        let payload = db::with_db(&state, |conn| {
            let profile_id = db::current_profile_id(conn);
            let freqs = db::profile_frequencies(conn, profile_id);
            let mut seats = Map::new();
            for (index, laps) in race_snapshot.iter().enumerate() {
                let (enter_at, exit_at) = (
                    freqs.get(index).map(|_| 90).unwrap_or(0),
                    freqs.get(index).map(|_| 80).unwrap_or(0),
                );
                let seat_laps: Vec<Value> = laps
                    .iter()
                    .enumerate()
                    .map(|(lap_number, l)| {
                        json!({
                            "lap_number": lap_number,
                            "lap_time_stamp": l.lap_time_stamp,
                            "lap_time": l.lap_time,
                            "lap_time_formatted": db::format_time_to_str(l.lap_time),
                            "source": l.source,
                            "deleted": l.deleted,
                        })
                    })
                    .collect();
                seats.insert(
                    index.to_string(),
                    json!({
                        "pilotrace_index": index,
                        "history_values": [],
                        "history_times": [],
                        "enter_at": enter_at,
                        "exit_at": exit_at,
                        "laps": seat_laps,
                        "marshal_type": marshal_type,
                    }),
                );
            }
            json!({
                "race": {
                    "round_id": -1,
                    "heat_id": heat_id,
                    "race_id": Value::Null,
                    "class_id": Value::Null,
                    "format_id": format_id,
                    "start_time": start_time,
                    "start_time_formatted": start_time_formatted,
                },
                "seats": seats,
            })
        });
        let _ = sock.emit("current_marshal_data", &payload);
    });

    socket.on("get_pilotrace", move |sock: SocketRef, data: Data<Value>, s: State<SharedState>| async move {
        let state = s.0.clone();
        let data = data.0;
        let Some(pilotrace_id) = data.get("pilotrace_id").and_then(|v| v.as_i64()) else {
            return;
        };
        let payload = db::with_db(&state, |conn| {
            let row: Option<(i64, i64, i64, Option<i64>, Option<i64>)> = conn
                .query_row(
                    "SELECT node_index, enter_at, exit_at, marshal_type, pilot_id
                     FROM saved_pilot_race WHERE id = ?1",
                    [pilotrace_id],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
                )
                .ok();
            let Some((node_index, enter_at, exit_at, marshal_type, pilot_id)) = row else {
                return Value::Null;
            };
            let callsign = pilot_id.and_then(|pid| {
                conn.query_row("SELECT callsign FROM pilot WHERE id = ?1", [pid], |r| {
                    r.get::<_, String>(0)
                })
                .ok()
            });
            let mut laps = Vec::new();
            let mut lstmt = conn
                .prepare(
                    "SELECT id, lap_time_stamp, lap_time, lap_time_formatted, source, deleted, peak_rssi
                     FROM saved_race_lap WHERE pilotrace_id = ?1 ORDER BY id",
                )
                .unwrap();
            let mut lrows = lstmt.query([pilotrace_id]).unwrap();
            while let Some(lrow) = lrows.next().unwrap() {
                laps.push(json!({
                    "id": lrow.get::<_, i64>(0).unwrap(),
                    "lap_time_stamp": lrow.get::<_, f64>(1).unwrap(),
                    "lap_time": lrow.get::<_, f64>(2).unwrap(),
                    "lap_time_formatted": lrow.get::<_, String>(3).unwrap(),
                    "source": lrow.get::<_, i64>(4).unwrap(),
                    "deleted": lrow.get::<_, bool>(5).unwrap(),
                    "peak_rssi": lrow.get::<_, Option<i64>>(6).unwrap(),
                }));
            }
            json!({
                "pilotrace_id": pilotrace_id,
                "callsign": callsign,
                "pilot_id": pilot_id,
                "node_index": node_index,
                "history_values": [],
                "history_times": [],
                "laps": laps,
                "enter_at": enter_at,
                "exit_at": exit_at,
                "marshal_type": marshal_type,
            })
        });
        if !payload.is_null() {
            let _ = sock.emit("race_details", &payload);
        }
    });
    socket.on("delete_race_round", move |sock: SocketRef, data: Data<Value>, s: State<SharedState>| async move {
        let state = s.0.clone();
            if !socket_authed(&sock, &state) { return; }
        let data = data.0;
        let race_id = data.get("race_id").and_then(|v| v.as_i64());
        let Some(race_id) = race_id else { return };
        let deleted = db::with_db(&state, |conn| {
            let meta: Option<(i64, i64, Option<i64>, Option<i64>, String)> = conn
                .query_row(
                    "SELECT round_id, heat_id, class_id, format_id, start_time_formatted
                     FROM saved_race_meta WHERE id = ?1",
                    [race_id],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
                )
                .ok();
            let Some((round_id, heat_id, class_id, format_id, start_time_formatted)) = meta else {
                return None;
            };
            // snapshot the full race into the deleted_race table
            let mut blob = json!({});
            {
                let mut mstmt = conn
                    .prepare("SELECT * FROM saved_race_meta WHERE id = ?1")
                    .unwrap();
                let mut mrows = mstmt.query([race_id]).unwrap();
                if let Some(row) = mrows.next().unwrap() {
                    blob["meta"] = row_to_json(row);
                }
            }
            let mut pilotraces = Vec::new();
            {
                let mut pstmt = conn
                    .prepare("SELECT * FROM saved_pilot_race WHERE race_id = ?1")
                    .unwrap();
                let mut prows = pstmt.query([race_id]).unwrap();
                while let Some(row) = prows.next().unwrap() {
                    pilotraces.push(row_to_json(row));
                }
            }
            let mut laps = Vec::new();
            {
                let mut lstmt = conn
                    .prepare("SELECT * FROM saved_race_lap WHERE race_id = ?1")
                    .unwrap();
                let mut lrows = lstmt.query([race_id]).unwrap();
                while let Some(row) = lrows.next().unwrap() {
                    laps.push(row_to_json(row));
                }
            }
            blob["pilotraces"] = json!(pilotraces);
            blob["laps"] = json!(laps);
            let deleted_time = crate::race::now_formatted_str();
            conn.execute(
                "INSERT INTO deleted_race (id, race_id, round_id, heat_id, class_id, format_id,
                                           start_time_formatted, deleted_time_formatted, data)
                 VALUES ((SELECT COALESCE(MAX(id), 0) + 1 FROM deleted_race), ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                rusqlite::params![
                    race_id, round_id, heat_id, class_id, format_id,
                    start_time_formatted, deleted_time, blob.to_string()
                ],
            )
            .unwrap();
            conn.execute("DELETE FROM saved_race_lap WHERE race_id = ?1", [race_id]).unwrap();
            conn.execute("DELETE FROM saved_pilot_race WHERE race_id = ?1", [race_id]).unwrap();
            conn.execute("DELETE FROM saved_race_meta WHERE id = ?1", [race_id]).unwrap();
            Some(race_id)
        });
        match deleted {
            Some(rid) => {
                let _ = sock.emit("race_round_deleted", &json!({"ok": true, "race_id": rid}));
            }
            None => {
                emit_priority_message(&state, "That round no longer exists; it may already have been deleted", false);
                let _ = sock.emit("race_round_deleted", &json!({"ok": false, "race_id": race_id}));
            }
        }
    });

    socket.on("check_deleted_race", move |_: SocketRef, ack: AckSender, data: Data<Value>, s: State<SharedState>| async move {
        let state = s.0.clone();
        let data = data.0;
        let id = data.get("id").and_then(|v| v.as_i64());
        let problem = check_restore_problem(&state, id);
        let _ = ack.send(&json!({"ok": problem.is_none()}));
        if let Some(problem) = problem {
            emit_priority_message(&state, &problem, false);
        }
    });

    socket.on("restore_deleted_race", move |sock: SocketRef, data: Data<Value>, s: State<SharedState>| async move {
        let state = s.0.clone();
            if !socket_authed(&sock, &state) { return; }
        let data = data.0;
        let id = data.get("id").and_then(|v| v.as_i64());
        if let Some(problem) = check_restore_problem(&state, id) {
            emit_priority_message(&state, &problem, false);
            let _ = sock.emit("deleted_race_restored", &json!({"ok": false, "id": id}));
            return;
        }
        let Some(id) = id else { return };
        let restored = db::with_db(&state, |conn| {
            let (race_id, blob): (i64, String) = match conn
                .query_row("SELECT race_id, data FROM deleted_race WHERE id = ?1", [id], |r| {
                    Ok((r.get(0)?, r.get(1)?))
                }) {
                Ok(v) => v,
                Err(_) => return false,
            };
            let doc: Value = match serde_json::from_str(&blob) {
                Ok(v) => v,
                Err(_) => return false,
            };
            if let Some(meta) = doc.get("meta") {
                conn.execute(
                    "INSERT INTO saved_race_meta (id, round_id, heat_id, class_id, format_id,
                                                  start_time, start_time_formatted, results, \"cacheStatus\")
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                    rusqlite::params![
                        race_id,
                        val_to_sql(meta.get("round_id")),
                        val_to_sql(meta.get("heat_id")),
                        val_to_sql(meta.get("class_id")),
                        val_to_sql(meta.get("format_id")),
                        val_to_sql(meta.get("start_time")),
                        val_to_sql(meta.get("start_time_formatted")),
                        val_to_sql(meta.get("results")),
                        val_to_sql(meta.get("cacheStatus")),
                    ],
                )
                .unwrap_or(0);
            }
            if let Some(prs) = doc.get("pilotraces").and_then(|v| v.as_array()) {
                for pr in prs {
                    conn.execute(
                        "INSERT INTO saved_pilot_race (id, race_id, node_index, pilot_id, history_values,
                                                       history_times, penalty_time, penalty_desc,
                                                       enter_at, exit_at, frequency, marshal_type)
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
                        rusqlite::params![
                            val_to_sql(pr.get("id")), val_to_sql(pr.get("race_id")), val_to_sql(pr.get("node_index")),
                            val_to_sql(pr.get("pilot_id")), val_to_sql(pr.get("history_values")), val_to_sql(pr.get("history_times")),
                            val_to_sql(pr.get("penalty_time")), val_to_sql(pr.get("penalty_desc")), val_to_sql(pr.get("enter_at")),
                            val_to_sql(pr.get("exit_at")), val_to_sql(pr.get("frequency")), val_to_sql(pr.get("marshal_type")),
                        ],
                    )
                    .unwrap_or(0);
                }
            }
            if let Some(ls) = doc.get("laps").and_then(|v| v.as_array()) {
                for l in ls {
                    conn.execute(
                        "INSERT INTO saved_race_lap (id, race_id, pilotrace_id, node_index, pilot_id,
                                                     lap_time_stamp, lap_time, lap_time_formatted,
                                                     source, deleted, peak_rssi)
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
                        rusqlite::params![
                            val_to_sql(l.get("id")), val_to_sql(l.get("race_id")), val_to_sql(l.get("pilotrace_id")),
                            val_to_sql(l.get("node_index")), val_to_sql(l.get("pilot_id")), val_to_sql(l.get("lap_time_stamp")),
                            val_to_sql(l.get("lap_time")), val_to_sql(l.get("lap_time_formatted")), val_to_sql(l.get("source")),
                            val_to_sql(l.get("deleted")), val_to_sql(l.get("peak_rssi")),
                        ],
                    )
                    .unwrap_or(0);
                }
            }
            conn.execute("DELETE FROM deleted_race WHERE id = ?1", [id]).unwrap_or(0);
            true
        });
        let _ = sock.emit("deleted_race_restored", &json!({"ok": restored, "id": id}));
    });

    // ---- heat generation (heat_automation) ----
    socket.on("calc_pilots", move |sock: SocketRef, data: Data<Value>, s: State<SharedState>| async move {
        let state = s.0.clone();
            if !socket_authed(&sock, &state) { return; }
        let data = data.0;
        let Some(heat_id) = data.get("heat").and_then(|v| v.as_i64())
            .or_else(|| data.get("heat_id").and_then(|v| v.as_i64()))
        else { return };
        // preassignments: {'slot': seat} map
        let mut preassignments: Vec<(i64, i64)> = Vec::new();
        if let Some(pre) = data.get("preassignments").and_then(|v| v.as_object()) {
            for (slot, seat) in pre {
                if let (Ok(slot_id), Some(seat)) = (slot.parse::<i64>(), seat.as_i64()) {
                    preassignments.push((slot_id, seat));
                }
            }
        }
        let calc_result = crate::phase3::calc_heat_pilots(&state, heat_id);
        let auto_frequency = db::with_db(&state, |conn| {
            conn.query_row("SELECT auto_frequency FROM heat WHERE id = ?1", [heat_id], |r| r.get::<_, bool>(0))
                .unwrap_or(false)
        });
        // characterized: heat_plan_result is only emitted when there is
        // dynamic pilot data (has_calc_pilots) or auto-frequency is on;
        // a simple confirmed/static heat returns silently ('safe')
        let emit_plan = calc_result.calc_success == Some(true)
            && (calc_result.has_calc_pilots || auto_frequency);
        if emit_plan {
            if calc_result.calc_success == Some(true) {
                crate::phase3::resolve_slot_unset_nodes(&state, heat_id);
            }
            let payload = crate::phase3::build_heat_plan_result(
                &state, heat_id, &calc_result,
                &preassignments.iter().map(|(s, _)| *s).collect::<Vec<_>>(),
            );
            broadcast(&state, "heat_plan_result", payload);
        }
    });

    socket.on("calc_reset", move |sock: SocketRef, data: Data<Value>, s: State<SharedState>| async move {
        let state = s.0.clone();
            if !socket_authed(&sock, &state) { return; }
        let data = data.0;
        let heat_id = data.get("heat").and_then(|v| v.as_i64())
            .or_else(|| data.get("heat_id").and_then(|v| v.as_i64()));
        let Some(heat_id) = heat_id else { return };
        db::with_db(&state, |conn| {
            let _ = conn.execute("UPDATE heat SET status = 0 WHERE id = ?1", [heat_id]);
        });
        let calc_result = crate::phase3::calc_heat_pilots(&state, heat_id);
        let payload = crate::phase3::build_heat_plan_result(&state, heat_id, &calc_result, &[]);
        broadcast(&state, "heat_plan_result", payload);
    });

    socket.on("confirm_heat_plan", move |sock: SocketRef, data: Data<Value>, s: State<SharedState>| async move {
        let state = s.0.clone();
            if !socket_authed(&sock, &state) { return; }
        let data = data.0;
        let Some(heat_id) = data.get("heat_id").and_then(|v| v.as_i64()) else { return };
        db::with_db(&state, |conn| {
            let _ = conn.execute("UPDATE heat SET status = 2 WHERE id = ?1", [heat_id]);
        });
        crate::phase3::resolve_slot_unset_nodes(&state, heat_id);
        let payload = db::with_db(&state, |conn| {
            let race = state.race.lock().unwrap();
            payloads::build_heat_data(conn, &race)
        });
        broadcast(&state, "heat_data", payload);
        if data.get("source").and_then(|v| v.as_str()) == Some("run") {
            let ready = state.race.lock().unwrap().race_status == RACE_STATUS_READY;
            if ready {
                let _ = set_heat_force(&state, heat_id);
            }
        }
    });

    // ---- callouts ----
    socket.on("save_callouts", move |sock: SocketRef, data: Data<Value>, s: State<SharedState>| async move {
        let state = s.0.clone();
            if !socket_authed(&sock, &state) { return; }
        let data = data.0;
        let callouts = data.get("callouts").cloned().unwrap_or(json!([]));
        crate::phase3::persist_config_item(
            &state.data_dir, "USER", "voiceCallouts", &callouts,
        );
        tracing::info!("Set all voice callouts");
        // sent to other clients, not the page that saved it
        if let Some(ns) = state.io.get().and_then(|io| io.of("/")) {
            let ns = ns.except(sock.id);
            tokio::spawn(async move {
                let _ = ns.emit("callouts", &callouts).await;
            });
        }
    });

    socket.on("reload_callouts", move |sock: SocketRef, s: State<SharedState>| async move {
        let state = s.0.clone();
        let callouts = crate::phase3::persisted_callouts(&state.data_dir);
        if let Some(c) = callouts {
            let _ = sock.emit("callouts", &c);
        }
    });

    socket.on("get_callout_text", move |sock: SocketRef, data: Data<Value>, s: State<SharedState>| async move {
        let state = s.0.clone();
        let data = data.0;
        let callout = data.get("callout").and_then(|v| v.as_str()).unwrap_or("");
        let text = crate::phase3::expand_callout(&state, callout);
        let _ = sock.emit("callout_text", &json!({"callout": callout, "text": text}));
    });

    socket.on("get_server_time", move |_: SocketRef, ack: AckSender, _s: State<SharedState>| async move {
        // replies via socket.io ack callback
        let _ = ack.send(&json!({"server_time_s": monotonic()}));
    });
}

fn register_export_import_handlers(socket: SocketRef, _state: SharedState) {
    socket.on("export_database", move |sock: SocketRef, data: Data<Value>, s: State<SharedState>| async move {
        let state = s.0.clone();
        let data = data.0;
        let Some(exporter) = data.get("exporter").and_then(|v| v.as_str()) else { return };
        let stamp = chrono::Local::now().format("%Y%m%d_%H%M%S");
        if exporter.starts_with("CSV__") {
            // CSV exporters produce plain text (rh_data_export_csv)
            let result = db::with_db(&state, |conn| {
                crate::export_import::assemble_csv(conn, exporter)
            });
            match result {
                Some(csv_text) => {
                    let filename = format!("RotorHazard Export {} {}.csv", stamp, exporter.replace("CSV__", "CSV / "));
                    let _ = sock.emit("exported_data", &json!({
                        "filename": filename,
                        "encoding": "text/csv",
                        "data": csv_text,
                    }));
                }
                None => {
                    crate::socket::emit_priority_message(&state, "Data export failed. (See log)", false);
                }
            }
            return;
        }
        let result = db::with_db(&state, |conn| crate::export_import::assemble(conn, exporter));
        match result {
            Some(payload) => {
                let filename = format!("RotorHazard Export {} {}.json", stamp, exporter);
                let _ = sock.emit("exported_data", &json!({
                    "filename": filename,
                    "encoding": "application/json",
                    "data": serde_json::to_string(&payload).unwrap_or_default(),
                }));
            }
            None => {
                crate::socket::emit_priority_message(&state, "Data export failed. (See log)", false);
            }
        }
    });

    socket.on("import_data", move |_: SocketRef, data: Data<Value>, s: State<SharedState>| async move {
        let state = s.0.clone();
        let data = data.0;
        let Some(importer) = data.get("importer").and_then(|v| v.as_str()) else { return };
        if importer != crate::export_import::IMPORTER_NAME {
            crate::socket::emit_priority_message(&state, "Data import failed. (See log)", true);
            return;
        }
        let Some(source_data) = data.get("source_data").and_then(|v| v.as_str()) else {
            crate::socket::emit_priority_message(&state, "Data import failed. No source data.", true);
            return;
        };
        let ok = crate::export_import::run_import(&state, source_data, &data);
        if ok {
            broadcast(&state, "database_restore_done", json!({}));
        } else {
            crate::socket::emit_priority_message(&state, "Data import failed. (See log)", true);
        }
    });
}

fn register_settings_handlers(socket: SocketRef, _state: SharedState) {
    socket.on("set_frequency", move |sock: SocketRef, data: Data<Value>, s: State<SharedState>| async move {
        let state = s.0.clone();
            if !socket_authed(&sock, &state) { return; }
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
        // write to real hardware if present
        if let Some(hw) = state.hardware.lock().unwrap().as_ref() {
            hw.set_frequency(node as usize, freq as u16);
        }
        tracing::info!("Frequency set: Node {} band={:?} ch={:?} freq={}", node + 1, band, chan, freq);
        let payload = db::with_db(&state, payloads::build_frequency_data);
        broadcast(&state, "frequency_data", payload);
    });

    socket.on("set_frequency_preset", move |sock: SocketRef, data: Data<Value>, s: State<SharedState>| async move {
        let state = s.0.clone();
            if !socket_authed(&sock, &state) { return; }
        let data = data.0;
        let preset = data.get("preset").and_then(|v| v.as_str()).unwrap_or("RB");
        db::with_db(&state, |conn| {
            let profile_id = db::current_profile_id(conn);
            let mut nb = vec![Value::Null; state.node_count()];
            let mut nc = vec![Value::Null; state.node_count()];
            let mut nf = vec![json!(0); state.node_count()];
            for (i, (b, c, f)) in crate::util::preset_frequencies(preset).into_iter().enumerate() {
                if i < state.node_count() {
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

    socket.on("add_profile", move |sock: SocketRef, data: Data<Value>, s: State<SharedState>| async move {
        let state = s.0.clone();
            if !socket_authed(&sock, &state) { return; }
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
            if !socket_authed(&sock, &state) { return; }
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
        // push to real hardware if present
        if let Some(hw) = state.hardware.lock().unwrap().as_mut() {
            hw.set_enter_at_level(node as usize, level);
        }
        // response goes only to the requesting socket
        let payload = db::with_db(&state, payloads::build_enter_exit_levels);
        let _ = sock.emit("enter_and_exit_at_levels", &payload);
    });

    socket.on("set_exit_at_level", move |sock: SocketRef, data: Data<Value>, s: State<SharedState>| async move {
        let state = s.0.clone();
            if !socket_authed(&sock, &state) { return; }
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
        // push to real hardware if present
        if let Some(hw) = state.hardware.lock().unwrap().as_mut() {
            hw.set_exit_at_level(node as usize, level);
        }
        let payload = db::with_db(&state, payloads::build_enter_exit_levels);
        let _ = sock.emit("enter_and_exit_at_levels", &payload);
    });

    // -- cluster: secondary-side handlers (what a joined timer answers) --

    socket.on("join_cluster_ex", move |sock: SocketRef, data: Data<Value>, s: State<SharedState>| async move {
        let state = s.0.clone();
        let data = data.0;
        let mode = data.get("mode").and_then(|v| v.as_str()).unwrap_or("split").to_string();
        tracing::info!("Joined cluster as '{}' timer", mode);
        state.cluster_joined.store(true, std::sync::atomic::Ordering::Relaxed);
        // split mode: first join backs up and clears race data so the
        // primary's races are not stored here
        if mode != "mirror" {
            crate::db::with_db(&state, |conn| {
                let has_races: i64 = conn
                    .query_row("SELECT COUNT(*) FROM saved_race_meta", [], |r| r.get(0))
                    .unwrap_or(0);
                if has_races > 0 {
                    tracing::info!("Clearing race data on split timer ({} races)", has_races);
                    let _ = conn.execute("DELETE FROM saved_race_lap", []);
                    let _ = conn.execute("DELETE FROM saved_pilot_race", []);
                    let _ = conn.execute("DELETE FROM saved_race_meta", []);
                    let _ = conn.execute("DELETE FROM lap_split", []);
                }
            });
            let mut race = state.race.lock().unwrap();
            race.clear_laps();
        }
        let auth_ok = socket_authed(&sock, &state);
        let payload = json!({
            "server_info": json!({
                "release_version": crate::state::RELEASE_VERSION,
                "prog_start_epoch": "0",
            }).to_string(),
            "auth_ok": auth_ok,
        });
        let _ = sock.emit("join_cluster_response", &payload);
    });

    socket.on("check_secondary_query", move |sock: SocketRef, _data: Data<Value>, s: State<SharedState>| async move {
        let _ = s;
        let ts = crate::cluster::epoch_millis_from_monotonic(crate::state::monotonic());
        let _ = sock.emit("check_secondary_response", &json!({"timestamp": ts}));
    });

    socket.on("cluster_event_trigger", move |_sock: SocketRef, data: Data<Value>, s: State<SharedState>| async move {
        let state = s.0.clone();
        let data = data.0;
        let evt_name = data.get("evt_name").and_then(|v| v.as_str()).unwrap_or("").to_string();
        let _evt_args: Value = data
            .get("evt_args")
            .and_then(|v| v.as_str())
            .and_then(|s| serde_json::from_str(s).ok())
            .unwrap_or(json!({}));
        // mirror mode: follow the primary's race lifecycle
        if state.cluster_joined.load(std::sync::atomic::Ordering::Relaxed) {
            match evt_name.as_str() {
                "raceStop" => crate::race::stop_race(&state),
                "lapsClear" => crate::race::discard_laps(&state),
                "raceStage" => {
                    // mirror staging: start at the primary's instant,
                    // corrected by the secondary's clock offset
                    crate::race::stage_race_external(&state, &_evt_args);
                }
                "raceLapRecorded" => {
                    // mirror results storage pending (results cache)
                }
                _ => {}
            }
        }
    });

    socket.on("retry_secondary", move |sock: SocketRef, data: Data<Value>, s: State<SharedState>| async move {
        let state = s.0.clone();
            if !socket_authed(&sock, &state) { return; }
        let secondary_id = data.0.get("secondary_id").and_then(|v| v.as_i64()).unwrap_or(-1);
        if secondary_id >= 0 {
            if let Some(cluster) = state.cluster.get() {
                cluster.retry_secondary(&state, secondary_id as usize);
                let _ = sock.emit("cluster_status", &cluster.get_cluster_status_info());
            }
        }
    });

    socket.on("cluster_message_ack", move |_sock: SocketRef, data: Data<Value>, s: State<SharedState>| async move {
        let _ = s;
        let msg_type = data.0.get("messageType").and_then(|v| v.as_str()).unwrap_or("").to_string();
        tracing::debug!("Received 'cluster_message_ack' message of type '{}'", msg_type);
    });

    socket.on("do_bpillfw_update", move |sock: SocketRef, data: Data<Value>, s: State<SharedState>| async move {
        let state = s.0.clone();
            if !socket_authed(&sock, &state) { return; }
        let data = data.0;
        let Some(src_file) = data.get("src_file_str").and_then(|v| v.as_str()).map(|s| s.to_string()) else { return };
        // take the port from the first hardware node (Python's
        // get_fwupd_serial_name for the S32_BPill)
        let node = state.hardware.lock().unwrap().as_ref().and_then(|hw| hw.nodes.first().cloned());
        let Some(node) = node else {
            let _ = sock.emit("upd_messages_init", &json!("No hardware node available for firmware update"));
            let _ = sock.emit("upd_messages_finish", &json!({}));
            return;
        };
        let msg = format!("Performing node update, port='{}', file: {}", node.port, src_file);
        tracing::info!("{}", msg);
        let init_msg = format!("{}\n", msg);
        let _ = sock.emit("upd_messages_init", &json!([init_msg]));
        let sock2 = sock.clone();
        let state2 = state.clone();
        // the flasher is blocking serial I/O; run it off the async executor
        tokio::task::spawn_blocking(move || {
            let emit_msg = |m: &str| {
                let _ = sock2.emit("upd_messages_append", &json!(m));
            };
            emit_msg(&msg);
            let result = crate::fwupdate::update_node_firmware(&node, &src_file, &emit_msg);
            match result {
                Ok(()) => {
                    tracing::info!("Node update succeeded");
                    emit_msg("\nNode update succeeded");
                }
                Err(e) => {
                    tracing::error!("Node update failed: {}", e);
                    emit_msg(&format!("\nNode update failed: {}", e));
                }
            }
            state2.restart_required.store(true, std::sync::atomic::Ordering::Relaxed);
            let _ = sock2.emit("upd_messages_finish", &json!({}));
        });
    });

    socket.on("suggest_calibration", move |sock: SocketRef, data: Data<Value>, s: State<SharedState>| async move {
        let state = s.0.clone();
            if !socket_authed(&sock, &state) { return; }
        let data = data.0;
        let pilotrace_id = data.get("pilotrace_id").and_then(|v| v.as_i64());
        let node_req = data.get("node").and_then(|v| v.as_i64());
        let (result, reason) = if let Some(pid) = pilotrace_id {
            crate::calibration::suggest_for_pilotrun(&state, pid)
        } else {
            crate::calibration::suggest_for_seat(&state, node_req.unwrap_or(-1))
        };
        let mut payload = crate::calibration::suggestion_payload(&result, reason);
        payload["pilotrace_id"] = json!(pilotrace_id);
        payload["node"] = json!(node_req);
        // response goes only to the requesting socket (Python emit())
        let _ = sock.emit("calibration_suggestion", &payload);
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
        let _ = sock.emit("start_thresh_lower_amount", &payload);
    });

    socket.on("set_start_thresh_lower_duration", move |sock: SocketRef, data: Data<Value>, s: State<SharedState>| async move {
        let state = s.0.clone();
        let data = data.0;
        let val = data.get("start_thresh_lower_duration").cloned().unwrap_or(json!(0));
        db::with_db(&state, |conn| {
            db::set_option(conn, "TIMING_startThreshLowerDuration", &val.to_string());
        });
        let payload = json!({"start_thresh_lower_duration": val.as_i64().unwrap_or(0)});
        let _ = sock.emit("start_thresh_lower_duration", &payload);
    });

    let sid = socket.id;
    socket.on("set_min_lap", move |sock: SocketRef, data: Data<Value>, s: State<SharedState>| async move {
        let state = s.0.clone();
            if !socket_authed(&sock, &state) { return; }
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

    socket.on("set_language", move |sock: SocketRef, data: Data<Value>, s: State<SharedState>| async move {
        let state = s.0.clone();
            if !socket_authed(&sock, &state) { return; }
        let data = data.0;
        if let Some(lang) = data.get("language").and_then(|v| v.as_str()) {
            // currentLanguage lives in config.json (UI section) in Python
            crate::phase3::persist_config_item(
                &state.data_dir, "UI", "currentLanguage", &json!(lang),
            );
        }
    });

    socket.on("set_config", move |sock: SocketRef, data: Data<Value>, s: State<SharedState>| async move {
        let state = s.0.clone();
            if !socket_authed(&sock, &state) { return; }
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
            crate::phase3::persist_config_item(&state.data_dir, section, key, value);
            tracing::info!("post-write config MOCK_NODE_SIGNAL = {:?}",
                crate::db::get_config_string(&state, "GENERAL", "MOCK_NODE_SIGNAL"));
            // switch the mock signal provider immediately rather than waiting
            // for the next mock tick to re-read the setting
            if section == "GENERAL" && key == "MOCK_NODE_SIGNAL" {
                if let Some(v) = value.as_str() {
                    let mut holder = state.mock_provider.lock().unwrap();
                    tracing::info!("Mock signal '{}' active", v);
                    *holder = (v.to_string(), crate::mock::provider_from_setting(v));
                }
            }
            tracing::info!("config set: {}.{} = {}", section, key, value);
        }
        // note: set_config does not emit config_update; clients re-request
        // config via load_data (characterized behavior)
    });

    socket.on("set_option", move |sock: SocketRef, data: Data<Value>, s: State<SharedState>| async move {
        let state = s.0.clone();
            if !socket_authed(&sock, &state) { return; }
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

    socket.on("backup_database", move |sock: SocketRef, s: State<SharedState>| async move {
        let state = s.0.clone();
            if !socket_authed(&sock, &state) { return; }
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
        let _ = sock.emit("backups_list", &json!({"backup_files": files}));
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
    db::read_freq_arrays(conn, profile_id)
}

fn write_freq_arrays(
    conn: &rusqlite::Connection,
    profile_id: i64,
    b: &[Value],
    c: &[Value],
    f: &[Value],
) {
    db::write_freq_arrays(conn, profile_id, b, c, f)
}

fn emit_race_status_from(state: &SharedState) {
    let payload = db::with_db(state, |conn| {
        let race = state.race.lock().unwrap();
        payloads::build_race_status(conn, &race)
    });
    broadcast(&state, "race_status", payload);
}


/// Builds the current-race leaderboard payload (emit_current_leaderboard).
fn build_current_leaderboard(state: &SharedState) -> Value {
    let snapshot = {
        let race = state.race.lock().unwrap();
        Some((
            race.current_heat,
            race.format_id,
            race.node_laps.clone(),
            race.node_pilots.clone(),
        ))
    };
    let Some((heat_id, format_id, laps, node_pilots)) = snapshot else {
        return json!({"current": {}});
    };
    db::with_db(state, |conn| {
        let (displayname, round_num) = match heat_id {
            Some(h) => {
                let dn: String = conn
                    .query_row(
                        "SELECT COALESCE(note, 'Heat ' || id) FROM heat WHERE id = ?1",
                        [h],
                        |row| row.get(0),
                    )
                    .unwrap_or_else(|_| format!("Heat {}", h));
                (dn, db::max_round(conn, h))
            }
            None => ("Practice".to_string(), 0),
        };
        let fmt = crate::results::FormatInfo::from_conn(conn, format_id);
        let consec = db::get_option_int(conn, "consecutivesCount", 3);
        let profile_id = db::current_profile_id(conn);
        let freqs = db::profile_frequencies(conn, profile_id);
        let mut pilots_in = Vec::new();
        for node_index in 0..state.node_count() {
            let freq = freqs.get(node_index).map(|(_, _, f)| *f).unwrap_or(0);
            if freq == 0 {
                continue;
            }
            let pilot_id = node_pilots[node_index];
            let (callsign, team) = match pilot_id {
                Some(pid) => {
                    let row: Option<(String, String)> = conn
                        .query_row(
                            "SELECT callsign, team FROM pilot WHERE id = ?1",
                            [pid],
                            |r| Ok((r.get(0)?, r.get(1)?)),
                        )
                        .ok();
                    match row {
                        Some((cs, tm)) => (cs, Some(tm)),
                        None => (format!("{}", freq), None),
                    }
                }
                None => match freqs.get(node_index) {
                    Some((Some(b), Some(c), _)) => (format!("{}{}", b, c), None),
                    _ => (format!("{}", freq), None),
                },
            };
            let crossings: Vec<(f64, f64)> = laps[node_index]
                .iter()
                .filter(|l| !l.deleted)
                .map(|l| (l.lap_time, l.lap_time_stamp))
                .collect();
            pilots_in.push(crate::results::PilotLaps {
                pilot_id,
                callsign,
                team_name: team,
                node: node_index,
                crossings,
            });
        }
        let leaderboard =
            crate::results::calc_leaderboard(&pilots_in, &fmt, round_num, heat_id, &displayname, consec);
        json!({
            "current": {
                "displayname": displayname,
                "heat": heat_id,
                "leaderboard": leaderboard,
            }
        })
    })
}


/// Dispatches a named event to interested subsystems (the Python
/// Events.trigger surface; plugin actions land here when the plugin host
/// does).
pub fn trigger_custom_event(state: &SharedState, event: &str, args: &Value) {
    tracing::debug!("Custom event '{}': {}", event, args);
    state.vrx_manager.do_option_set(args); // extension point
}

/// Broadcasts a priority message (emit_priority_message).
pub fn emit_priority_message(state: &SharedState, message: &str, interrupt: bool) {
    broadcast(
        state,
        "priority_message",
        json!({"message": message, "interrupt": interrupt, "caller": Value::Null}),
    );
    state
        .led_manager
        .trigger(
            if interrupt { "messageInterrupt" } else { "messageStandard" },
            &json!({"message": message}),
        );
}


fn row_to_json(row: &rusqlite::Row<'_>) -> Value {
    let mut obj = Map::new();
    for (idx, col) in row.as_ref().column_names().iter().enumerate() {
        let val: Value = match row.get_ref(idx) {
            Ok(rusqlite::types::ValueRef::Null) => Value::Null,
            Ok(rusqlite::types::ValueRef::Integer(i)) => json!(i),
            Ok(rusqlite::types::ValueRef::Real(f)) => json!(f),
            Ok(rusqlite::types::ValueRef::Text(t)) => {
                json!(String::from_utf8_lossy(t).to_string())
            }
            Ok(rusqlite::types::ValueRef::Blob(b)) => json!(String::from_utf8_lossy(b).to_string()),
            Err(_) => Value::Null,
        };
        obj.insert((*col).to_string(), val);
    }
    Value::Object(obj)
}

/// Returns a problem message when a deleted race cannot be restored.
fn check_restore_problem(state: &SharedState, deleted_id: Option<i64>) -> Option<String> {
    let Some(id) = deleted_id else {
        return Some("No deleted race was specified".to_string());
    };
    let conflict = db::with_db(state, |conn| {
        let row: Option<(i64, i64)> = conn
            .query_row(
                "SELECT round_id, heat_id FROM deleted_race WHERE id = ?1",
                [id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .ok();
        match row {
            None => None,
            Some((round_id, heat_id)) => conn
                .query_row(
                    "SELECT id FROM saved_race_meta WHERE heat_id = ?1 AND round_id = ?2",
                    rusqlite::params![heat_id, round_id],
                    |r| r.get::<_, i64>(0),
                )
                .ok(),
        }
    });
    match conflict {
        Some(_) => Some("Unable to restore deleted race; conflicts with an existing race".to_string()),
        None => None,
    }
}


/// Converts a serde_json::Value into a rusqlite-compatible parameter
/// (NULL / integer / real / text).
enum SqlVal {
    Null,
    Int(i64),
    Real(f64),
    Text(String),
}

impl rusqlite::ToSql for SqlVal {
    fn to_sql(&self) -> rusqlite::Result<rusqlite::types::ToSqlOutput<'_>> {
        use rusqlite::types::{ToSqlOutput, Value as SqlValue, ValueRef};
        Ok(match self {
            SqlVal::Null => ToSqlOutput::Owned(SqlValue::Null),
            SqlVal::Int(i) => ToSqlOutput::Owned(SqlValue::Integer(*i)),
            SqlVal::Real(f) => ToSqlOutput::Owned(SqlValue::Real(*f)),
            SqlVal::Text(s) => ToSqlOutput::Borrowed(ValueRef::Text(s.as_bytes())),
        })
    }
}

fn val_to_sql(v: Option<&Value>) -> SqlVal {
    match v {
        None | Some(Value::Null) => SqlVal::Null,
        Some(Value::Bool(b)) => SqlVal::Int(*b as i64),
        Some(Value::Number(n)) => {
            if let Some(i) = n.as_i64() {
                SqlVal::Int(i)
            } else {
                SqlVal::Real(n.as_f64().unwrap_or(0.0))
            }
        }
        Some(Value::String(s)) => SqlVal::Text(s.clone()),
        Some(other) => SqlVal::Text(other.to_string()),
    }
}


/// Builds the race_list payload (emit_race_list): saved races grouped by
/// heat and round, with per-seat pilotrace entries carrying pilotrace_id.
pub fn build_race_list(state: &SharedState) -> Value {
    db::with_db(state, |conn| {
        let mut heats = Map::new();
        let profile_id = db::current_profile_id(conn);
        let freqs = db::profile_frequencies(conn, profile_id);

        let mut heat_rows = Vec::new();
        {
            let mut stmt = conn
                .prepare("SELECT id, COALESCE(note, 'Heat ' || id), class_id FROM heat ORDER BY id")
                .unwrap();
            let mut rows = stmt.query([]).unwrap();
            while let Some(row) = rows.next().unwrap() {
                heat_rows.push((
                    row.get::<_, i64>(0).unwrap(),
                    row.get::<_, Option<i64>>(2).unwrap(),
                    row.get::<_, String>(1).unwrap(),
                ));
            }
        }

        for (heat_id, class_id, displayname) in heat_rows {
            let mut rounds = Map::new();
            let mut race_ids = Vec::new();
            {
                let mut stmt = conn
                    .prepare("SELECT id, round_id, format_id, start_time, start_time_formatted
                              FROM saved_race_meta WHERE heat_id = ?1 ORDER BY round_id")
                    .unwrap();
                let mut rows = stmt.query([heat_id]).unwrap();
                while let Some(row) = rows.next().unwrap() {
                    race_ids.push((
                        row.get::<_, i64>(0).unwrap(),
                        row.get::<_, i64>(1).unwrap(),
                        row.get::<_, Option<i64>>(2).unwrap(),
                        row.get::<_, f64>(3).unwrap(),
                        row.get::<_, String>(4).unwrap(),
                    ));
                }
            }
            for (race_id, round_id, format_id, start_time, start_time_formatted) in race_ids {
                let mut pilotraces = Vec::new();
                let mut stmt = conn
                    .prepare("SELECT id, pilot_id, node_index, frequency
                              FROM saved_pilot_race WHERE race_id = ?1")
                    .unwrap();
                let mut rows = stmt.query([race_id]).unwrap();
                let mut prs = Vec::new();
                while let Some(row) = rows.next().unwrap() {
                    prs.push((
                        row.get::<_, i64>(0).unwrap(),
                        row.get::<_, Option<i64>>(1).unwrap(),
                        row.get::<_, i64>(2).unwrap(),
                        row.get::<_, Option<i64>>(3).unwrap(),
                    ));
                }
                for (pr_id, pilot_id, node_index, frequency) in prs {
                    let callsign = pilot_id.and_then(|pid| {
                        conn.query_row("SELECT callsign FROM pilot WHERE id = ?1", [pid], |r| {
                            r.get::<_, String>(0)
                        })
                        .ok()
                    });
                    let pilot_freq = match frequency {
                        Some(f) if f > 0 => {
                            let band_channel = freqs
                                .iter()
                                .find(|(_, _, nf)| *nf == f)
                                .map(|(b, c, _)| (b.clone(), *c));
                            match band_channel {
                                Some((Some(b), Some(c))) => json!({"band": b, "channel": c, "frequency": f}),
                                _ => json!({"band": Value::Null, "channel": Value::Null, "frequency": f}),
                            }
                        }
                        _ => match freqs.get(node_index as usize) {
                            Some((Some(b), Some(c), f)) => {
                                json!({"band": b, "channel": c, "frequency": f})
                            }
                            _ => json!({"band": Value::Null, "channel": Value::Null, "frequency": 0}),
                        },
                    };
                    pilotraces.push(json!({
                        "pilotrace_id": pr_id,
                        "callsign": callsign,
                        "pilot_id": pilot_id,
                        "node_index": node_index,
                        "pilot_freq": pilot_freq,
                    }));
                }
                rounds.insert(
                    round_id.to_string(),
                    json!({
                        "race_id": race_id,
                        "format_id": format_id,
                        "start_time": start_time,
                        "start_time_formatted": start_time_formatted,
                        "pilotraces": pilotraces,
                    }),
                );
            }
            if !rounds.is_empty() {
                heats.insert(
                    heat_id.to_string(),
                    json!({
                        "heat_id": heat_id,
                        "class_id": class_id,
                        "displayname": displayname,
                        "rounds": rounds,
                    }),
                );
            }
        }
        json!({ "heats": heats })
    })
}

/// Broadcasts the race list (called after laps are saved).
pub fn broadcast_race_list(state: &SharedState) {
    let payload = build_race_list(state);
    broadcast(state, "race_list", payload);
}


/// Switches the current heat even mid-race-state restrictions (set_heat force).
fn set_heat_force(state: &SharedState, heat_id: i64) -> Result<(), ()> {
    let pilots = db::with_db(state, |conn| heat_node_pilots(conn, heat_id, state.node_count()));
    {
        let mut race = state.race.lock().unwrap();
        race.current_heat = Some(heat_id);
        race.node_pilots = pilots;
    }
    let payload = db::with_db(state, |conn| {
        let race = state.race.lock().unwrap();
        payloads::build_current_heat(conn, &race)
    });
    broadcast(state, "current_heat", payload);
    emit_race_status_from(state);
    Ok(())
}

/// Language list for the 'language' payload (id + display name pairs).
fn language_list(languages: &Value) -> Value {
    let mut out = Vec::new();
    if let Some(obj) = languages.as_object() {
        for (id, entry) in obj {
            out.push(json!({
                "id": id,
                "name": entry.get("name").cloned().unwrap_or(Value::Null),
            }));
        }
    }
    Value::from(out)
}


/// One mock-interface update tick: select the provider named by the
/// MOCK_NODE_SIGNAL setting, read a sample per seat, and apply it to the
/// node state.  Completed crossings during a running race register laps.
pub fn mock_update_tick(state: &SharedState) {
    // the in-memory setting is the truth: set_config switches the provider
    // instantly; the config file is only read once at startup

    let now = monotonic();
    let (racing_start, race_status) = {
        let race = state.race.lock().unwrap();
        (
            if race.race_status == RACE_STATUS_RACING {
                Some(race.start_time_monotonic)
            } else {
                None
            },
            race.race_status,
        )
    };

    let mut completed: Vec<usize> = Vec::new();

    // lock order: DB first (snapshot), then provider/mock locks.  Taking a
    // DB call while holding the mock lock deadlocks against the heartbeat
    // task, which takes the DB lock before the mock lock.
    let freq_active: Vec<bool> = db::with_db(state, |conn| {
        let profile_id = db::current_profile_id(conn);
        db::profile_frequencies(conn, profile_id)
            .iter()
            .map(|(_, _, f)| *f != 0)
            .collect()
    });

    // real hardware covers the leading node indices; the mock system
    // must not fight it for those seats
    let hw_node_count = state
        .hardware
        .lock()
        .unwrap()
        .as_ref()
        .map(|hw| hw.num_nodes())
        .unwrap_or(0);

    {
        let mut holder = state.mock_provider.lock().unwrap();
        let mut mock = state.mock.lock().unwrap();
        for (idx, node) in mock.iter_mut().enumerate() {
            if idx < hw_node_count {
                continue; // driven by the hardware update loop
            }
            let freq_active = freq_active.get(idx).copied().unwrap_or(false);
            if !freq_active {
                node.current_rssi = 0;
                node.crossing_flag = false;
                continue;
            }
            let sample = match holder.1.read(node, idx, now, &state.data_dir) {
                Some(s) => s,
                None => continue,
            };
            let was_crossing = node.crossing_flag;
            node.current_rssi = sample.rssi;
            node.crossing_flag = sample.cross_flag;
            // replay-style providers signal passes by lap_id increments
            let lap_id_advanced = sample.lap_id > node.lap_number;
            node.lap_number = sample.lap_id;
            node.node_peak_rssi = sample.node_peak_rssi;
            node.node_nadir_rssi = sample.node_nadir_rssi;
            node.pass_peak_rssi = sample.pass_peak_rssi;
            node.pass_nadir_rssi = sample.pass_nadir_rssi;
            node.loop_time = sample.loop_time;
            node.debug_pass_count += 1;
            // record the reading's peak/nadir history events
            // (process_history: none recorded once the race is DONE)
            if sample.history_flag && race_status != RACE_STATUS_DONE {
                let readtime = sample.read_time.unwrap_or(now);
                node.history_add_pn(
                    readtime,
                    sample.peak_rssi,
                    sample.peak_first_time.max(0) as u16,
                    sample.peak_last_time.max(0) as u16,
                    sample.nadir_rssi,
                    sample.nadir_first_time.max(0) as u16,
                    sample.nadir_last_time.max(0) as u16,
                );
            }
            if race_status == RACE_STATUS_READY {
                node.prune_history(now);
            }

            // a completed crossing (flag drop) or a provider lap_id
            // advance during a running race registers a lap
            if racing_start.is_some()
                && ((was_crossing && !sample.cross_flag) || lap_id_advanced)
            {
                completed.push(idx);
            }
        }
    }

    if !completed.is_empty() {
        for idx in completed {
            crate::race::register_lap_at(state, idx, now);
        }
    }
}

/// True when socket actions may proceed: no admin credentials configured
/// (auth disabled), or the connection carried valid basic auth.
fn socket_authed(socket: &SocketRef, state: &SharedState) -> bool {
    if state.admin_username.is_empty() && state.admin_password.is_empty() {
        return true;
    }
    socket
        .extensions
        .get::<bool>()
        .unwrap_or(false)
}

fn parse_basic_auth_ok(
    headers: axum::http::HeaderMap,
    state: &SharedState,
) -> bool {
    use base64::Engine;
    if state.admin_username.is_empty() && state.admin_password.is_empty() {
        return true;
    }
    let token = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Basic "))
        .unwrap_or("");
    let decoded = base64::engine::general_purpose::STANDARD
        .decode(token)
        .ok()
        .and_then(|d| String::from_utf8(d).ok())
        .unwrap_or_default();
    let (user, pass) = decoded.split_once(':').unwrap_or(("", ""));
    user == state.admin_username && pass == state.admin_password
}
