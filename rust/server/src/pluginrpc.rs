//! RHAPI RPC surface for the out-of-process plugin bridge.
//!
//! Third-party plugins stay unmodified Python; a small bridge daemon
//! (plugin-bridge/rhapi_bridge.py) loads them and proxies their RHAPI
//! calls to this endpoint over localhost socket.io:
//!
//!   bridge → server:  "rhapi_call"  {object, method, args}  [ack]
//!   server → bridge:  ack {result}  or {error}
//!   server → bridge:  normal event broadcasts (race_start, etc.) which
//!                     the bridge dispatches to plugin-registered hooks.
//!
//! The server never runs Python; without the bridge this surface is
//! simply unused.

use crate::db;
use crate::state::SharedState;
use serde_json::{json, Value};

type RpcResult = Result<Value, String>;

/// Dispatches one RHAPI call.  Method names mirror the RHAPI object
/// graph ("db.pilots", "ui.message_speak", ...).
fn dispatch(state: &SharedState, object: &str, method: &str, args: &Value) -> RpcResult {
    let empty = json!({});
    let args = args.as_object().unwrap_or(empty.as_object().unwrap());
    let arg_i64 = |k: &str| args.get(k).and_then(|v| v.as_i64());
    let arg_str = |k: &str| args.get(k).and_then(|v| v.as_str()).map(String::from);

    match (object, method) {
        // ---- db: pilots ----
        ("db", "pilots") => Ok(db::with_db(state, |conn| {
            let mut stmt = conn
                .prepare("SELECT id, callsign, name, team, phonetic FROM pilot ORDER BY id")
                .unwrap();
            let rows = stmt
                .query_map([], |r| {
                    Ok(json!({
                        "pilot_id": r.get::<_, i64>(0)?,
                        "callsign": r.get::<_, String>(1)?,
                        "name": r.get::<_, Option<String>>(2)?,
                        "team": r.get::<_, Option<String>>(3)?,
                        "phonetic": r.get::<_, Option<String>>(4)?,
                    }))
                })
                .unwrap();
            let out: Vec<Value> = rows.filter_map(|r| r.ok()).collect();
            json!(out)
        })),
        ("db", "pilot") => {
            let pilot_id = arg_i64("pilot_id").ok_or("pilot_id required")?;
            Ok(db::with_db(state, |conn| {
                conn.query_row(
                    "SELECT id, callsign, name, team, phonetic FROM pilot WHERE id = ?1",
                    [pilot_id],
                    |r| {
                        Ok(json!({
                            "pilot_id": r.get::<_, i64>(0)?,
                            "callsign": r.get::<_, String>(1)?,
                            "name": r.get::<_, Option<String>>(2)?,
                            "team": r.get::<_, Option<String>>(3)?,
                            "phonetic": r.get::<_, Option<String>>(4)?,
                        }))
                    },
                )
                .map_err(|_| format!("pilot {} not found", pilot_id))
                .unwrap_or(Value::Null)
            }))
        }
        ("db", "pilot_add") => {
            let callsign = arg_str("callsign").ok_or("callsign required")?;
            Ok(db::with_db(state, |conn| {
                conn.execute(
                    "INSERT INTO pilot (callsign, name, phonetic, team, active) \
                     VALUES (?1, ?2, ?3, ?4, 1)",
                    rusqlite::params![
                        callsign,
                        args.get("name").and_then(|v| v.as_str()).unwrap_or(""),
                        args.get("phonetic").and_then(|v| v.as_str()).unwrap_or(""),
                        args.get("team").and_then(|v| v.as_str()).unwrap_or("")
                    ],
                )
                .map(|_| json!(conn.last_insert_rowid()))
                .map_err(|e| e.to_string())
                .unwrap_or_else(|e| json!({"error": e}))
            }))
        }
        ("db", "pilot_alter") => {
            let pilot_id = arg_i64("pilot_id").ok_or("pilot_id required")?;
            Ok(db::with_db(state, |conn| {
                for (field, column) in [
                    ("callsign", "callsign"),
                    ("name", "name"),
                    ("phonetic", "phonetic"),
                    ("team", "team"),
                ] {
                    if let Some(v) = args.get(field).and_then(|v| v.as_str()) {
                        conn.execute(
                            &format!("UPDATE pilot SET {} = ?1 WHERE id = ?2", column),
                            rusqlite::params![v, pilot_id],
                        )
                        .ok();
                    }
                }
                json!(true)
            }))
        }

        // ---- db: heats / classes / formats ----
        ("db", "heats") => Ok(db::with_db(state, |conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT h.id, COALESCE(NULLIF(h.note, ''), COALESCE(NULLIF(h.auto_name, ''), 'Heat ' || h.id)), \
                     h.class_id FROM heat h ORDER BY h.id",
                )
                .unwrap();
            let rows = stmt
                .query_map([], |r| {
                    Ok(json!({
                        "heat_id": r.get::<_, i64>(0)?,
                        "display_name": r.get::<_, String>(1)?,
                        "class_id": r.get::<_, Option<i64>>(2)?,
                    }))
                })
                .unwrap();
            let out: Vec<Value> = rows.filter_map(|r| r.ok()).collect();
            json!(out)
        })),
        ("db", "heat_slots") => {
            let heat_id = arg_i64("heat_id").ok_or("heat_id required")?;
            Ok(db::with_db(state, |conn| {
                let mut stmt = conn
                    .prepare(
                        "SELECT node_index, pilot_id, method, seed_rank, seed_id \
                         FROM heat_node WHERE heat_id = ?1 ORDER BY node_index",
                    )
                    .unwrap();
                let rows = stmt
                    .query_map([heat_id], |r| {
                        Ok(json!({
                            "node_index": r.get::<_, i64>(0)?,
                            "pilot_id": r.get::<_, Option<i64>>(1)?,
                            "method": r.get::<_, i64>(2)?,
                            "seed_rank": r.get::<_, Option<i64>>(3)?,
                            "seed_id": r.get::<_, Option<i64>>(4)?,
                        }))
                    })
                    .unwrap();
                let out: Vec<Value> = rows.filter_map(|r| r.ok()).collect();
                json!(out)
            }))
        }
        ("db", "raceclasses") => Ok(db::with_db(state, |conn| {
            let mut stmt = conn
                .prepare("SELECT id, name, description, format_id, win_condition FROM race_class ORDER BY id")
                .unwrap();
            let rows = stmt
                .query_map([], |r| {
                    Ok(json!({
                        "class_id": r.get::<_, i64>(0)?,
                        "name": r.get::<_, Option<String>>(1)?,
                        "description": r.get::<_, Option<String>>(2)?,
                        "format_id": r.get::<_, Option<i64>>(3)?,
                        "win_condition": r.get::<_, Option<String>>(4)?,
                    }))
                })
                .unwrap();
            let out: Vec<Value> = rows.filter_map(|r| r.ok()).collect();
            json!(out)
        })),
        ("db", "raceformats") => Ok(db::with_db(state, |conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT id, name, race_mode, race_time_sec, win_condition, start_behavior, \
                     points_method FROM race_format ORDER BY id",
                )
                .unwrap();
            let rows = stmt
                .query_map([], |r| {
                    Ok(json!({
                        "format_id": r.get::<_, i64>(0)?,
                        "name": r.get::<_, String>(1)?,
                        "unlimited_time": r.get::<_, i64>(2)?,
                        "race_time_sec": r.get::<_, i64>(3)?,
                        "win_condition": r.get::<_, i64>(4)?,
                        "start_behavior": r.get::<_, i64>(5)?,
                        "points_method": r.get::<_, Option<String>>(6)?,
                    }))
                })
                .unwrap();
            let out: Vec<Value> = rows.filter_map(|r| r.ok()).collect();
            json!(out)
        })),

        // ---- db: saved races ----
        ("db", "saved_races") => Ok(db::with_db(state, |conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT id, heat_id, round_id, class_id, format_id, start_time_formatted \
                     FROM saved_race_meta ORDER BY id",
                )
                .unwrap();
            let rows = stmt
                .query_map([], |r| {
                    Ok(json!({
                        "race_id": r.get::<_, i64>(0)?,
                        "heat_id": r.get::<_, i64>(1)?,
                        "round_id": r.get::<_, i64>(2)?,
                        "class_id": r.get::<_, Option<i64>>(3)?,
                        "format_id": r.get::<_, i64>(4)?,
                        "start_time_formatted": r.get::<_, Option<String>>(5)?,
                    }))
                })
                .unwrap();
            let out: Vec<Value> = rows.filter_map(|r| r.ok()).collect();
            json!(out)
        })),
        ("db", "saved_race_laps") => {
            let race_id = arg_i64("race_id").ok_or("race_id required")?;
            Ok(db::with_db(state, |conn| {
                let mut stmt = conn
                    .prepare(
                        "SELECT node_index, pilot_id, lap_time_stamp, lap_time, deleted \
                         FROM saved_race_lap WHERE race_id = ?1 ORDER BY lap_time_stamp",
                    )
                    .unwrap();
                let rows = stmt
                    .query_map([race_id], |r| {
                        Ok(json!({
                            "node_index": r.get::<_, i64>(0)?,
                            "pilot_id": r.get::<_, i64>(1)?,
                            "lap_time_stamp": r.get::<_, f64>(2)?,
                            "lap_time": r.get::<_, Option<f64>>(3)?,
                            "deleted": r.get::<_, i64>(4)? != 0,
                        }))
                    })
                    .unwrap();
                let out: Vec<Value> = rows.filter_map(|r| r.ok()).collect();
                json!(out)
            }))
        }

        // ---- db: options ----
        ("db", "get_option") => {
            let name = arg_str("name").ok_or("name required")?;
            Ok(db::with_db(state, |conn| {
                let v = conn
                    .query_row(
                        "SELECT option_value FROM global_settings WHERE option_name = ?1",
                        [name],
                        |r| r.get::<_, String>(0),
                    )
                    .unwrap_or_default();
                json!(v)
            }))
        }
        ("db", "set_option") => {
            let name = arg_str("name").ok_or("name required")?;
            let value = arg_str("value").unwrap_or_default();
            Ok(db::with_db(state, |conn| {
                db::set_option(conn, &name, &value);
                json!(true)
            }))
        }

        // ---- race ----
        ("race", "status") => {
            let race = state.race.lock().unwrap();
            Ok(json!({
                "race_status": race.race_status,
                "current_heat": race.current_heat,
                "format_id": race.format_id,
                "start_time_monotonic": race.start_time_monotonic,
            }))
        }
        ("race", "pilots") => {
            let race = state.race.lock().unwrap();
            Ok(json!(race
                .node_pilots
                .iter()
                .enumerate()
                .map(|(idx, p)| json!({"node_index": idx, "pilot_id": p}))
                .collect::<Vec<_>>()))
        }
        ("race", "results") => Ok(db::with_db(state, |conn| {
            let race = state.race.lock().unwrap();
            json!(crate::payloads::build_current_laps(conn, &race))
        })),

        // ---- ui ----
        ("ui", "message_speak") => {
            let message = arg_str("message").unwrap_or_default();
            crate::socket::broadcast(state, "phonetic_text", json!({
                "text": message, "domain": false, "winner_flag": false,
                "priority": false, "resume_queue": false,
            }));
            Ok(json!(true))
        }
        ("ui", "message_notify") => {
            crate::socket::emit_priority_message(state, &arg_str("message").unwrap_or_default(), false);
            Ok(json!(true))
        }
        ("ui", "message_alert") => {
            crate::socket::emit_priority_message(state, &arg_str("message").unwrap_or_default(), true);
            Ok(json!(true))
        }
        ("ui", "clear_messages") => {
            crate::socket::broadcast(state, "clear_priority_messages", json!({}));
            Ok(json!(true))
        }
        ("ui", "register_config_section") => {
            // RHUI.register_config_section: allows a config section to be
            // fetched via a load_data request; SECRETS is refused
            let section = arg_str("section").unwrap_or_default();
            if crate::socket::RESERVED_CONFIG_SECTIONS.contains(&section.as_str()) {
                tracing::warn!("Refusing to register protected config section: {}", section);
                return Ok(json!(false));
            }
            let items = args
                .get("items")
                .and_then(|v| v.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|v| v.as_str().map(|s| s.to_string()))
                        .collect::<Vec<_>>()
                });
            state
                .config_load_items
                .lock()
                .unwrap()
                .insert(section, items);
            Ok(json!(true))
        }

        // ---- config: config.json values (RHAPI config.get) ----
        ("config", "get") => {
            let section = arg_str("section").ok_or("section required")?;
            let key = arg_str("key").ok_or("key required")?;
            Ok(json!(db::get_config_string(state, &section, &key)))
        }

        // ---- race control (RHAPI parity for plugins) ----
        ("race", "stage") => {
            crate::race::stage_race(state);
            Ok(json!(true))
        }
        ("race", "stop") => {
            crate::race::stop_race(state);
            Ok(json!(true))
        }
        ("race", "save") => Ok(json!(crate::race::save_laps(state))),
        ("race", "clear") => {
            crate::race::discard_laps(state);
            Ok(json!(true))
        }
        ("race", "any_laps_recorded") => {
            let race = state.race.lock().unwrap();
            Ok(json!(race.node_laps.iter().any(|laps| !laps.is_empty())))
        }
        ("race", "heat") => {
            let race = state.race.lock().unwrap();
            Ok(json!(race.current_heat))
        }
        ("race", "set_heat") => {
            let heat_id = arg_i64("heat_id").ok_or("heat_id required")?;
            let pilots = db::with_db(state, |conn| {
                let mut slots = vec![None; state.node_count()];
                if let Ok(mut stmt) =
                    conn.prepare("SELECT node_index, pilot_id FROM heat_node WHERE heat_id = ?1")
                {
                    if let Ok(rows) = stmt.query_map([heat_id], |r| {
                        Ok((r.get::<_, i64>(0)?, r.get::<_, Option<i64>>(1)?))
                    }) {
                        for (index, pilot) in rows.flatten() {
                            if let Some(slot) = slots.get_mut(index as usize) {
                                *slot = pilot;
                            }
                        }
                    }
                }
                slots
            });
            {
                let mut race = state.race.lock().unwrap();
                race.current_heat = Some(heat_id);
                race.node_pilots = pilots;
            }
            let payload = db::with_db(state, |conn| {
                let race = state.race.lock().unwrap();
                crate::payloads::build_current_heat(conn, &race)
            });
            crate::socket::broadcast(state, "current_heat", payload);
            Ok(json!(true))
        }

        // ---- db: heats, slots, attributes ----
        ("db", "heat_add") => {
            let name = arg_str("name");
            let id = db::with_db(state, |conn| {
                let id = db::next_heat_id(conn);
                conn.execute(
                    "INSERT INTO heat (id, note, auto_name, class_id, results, \"cacheStatus\", \"order\",
                                       status, auto_frequency, group_id, active, coop_best_time, coop_num_laps)
                     VALUES (?1, ?2, NULL, NULL, NULL, '{}', NULL, 0, 0, 0, 1, NULL, NULL)",
                    rusqlite::params![id, name],
                )
                .ok();
                for node_index in 0..state.node_count() as i64 {
                    conn.execute(
                        "INSERT INTO heat_node (id, heat_id, node_index, pilot_id, color, method, seed_rank, seed_id)
                         VALUES (?1, ?2, ?3, NULL, NULL, 0, NULL, NULL)",
                        rusqlite::params![db::next_heat_node_id(conn), id, node_index],
                    )
                    .ok();
                }
                id
            });
            Ok(json!({"id": id}))
        }
        ("db", "heat") => {
            let heat_id = arg_i64("heat_id").ok_or("heat_id required")?;
            Ok(db::with_db(state, |conn| {
                conn.query_row(
                    "SELECT id, COALESCE(NULLIF(note, ''), COALESCE(NULLIF(auto_name, ''), 'Heat ' || id))
                     FROM heat WHERE id = ?1",
                    [heat_id],
                    |r| Ok(json!({"id": r.get::<_, i64>(0)?, "display_name": r.get::<_, String>(1)?})),
                )
                .unwrap_or(Value::Null)
            }))
        }
        ("db", "heat_alter") => {
            let heat_id = arg_i64("heat_id").ok_or("heat_id required")?;
            let name = arg_str("name");
            let attributes = args.get("attributes").and_then(|v| v.as_object()).cloned();
            db::with_db(state, |conn| {
                if let Some(name) = &name {
                    conn.execute(
                        "UPDATE heat SET note = ?1 WHERE id = ?2",
                        rusqlite::params![name, heat_id],
                    )
                    .ok();
                }
                if let Some(attributes) = &attributes {
                    for (key, value) in attributes {
                        conn.execute(
                            "INSERT OR REPLACE INTO heat_attribute (heat_id, attribute, value)
                             VALUES (?1, ?2, ?3)",
                            rusqlite::params![heat_id, key, value.as_str().unwrap_or("")],
                        )
                        .ok();
                    }
                }
            });
            Ok(json!(true))
        }
        ("db", "slots_by_heat") => {
            let heat_id = arg_i64("heat_id").ok_or("heat_id required")?;
            Ok(db::with_db(state, |conn| {
                let mut stmt = conn
                    .prepare("SELECT id, node_index, pilot_id FROM heat_node WHERE heat_id = ?1 ORDER BY node_index")
                    .unwrap();
                let rows = stmt
                    .query_map([heat_id], |r| {
                        Ok(json!({
                            "id": r.get::<_, i64>(0)?,
                            "node_index": r.get::<_, i64>(1)?,
                            "pilot": r.get::<_, Option<i64>>(2)?,
                        }))
                    })
                    .unwrap();
                let out: Vec<Value> = rows.filter_map(|r| r.ok()).collect();
                json!(out)
            }))
        }
        ("db", "slots_alter_fast") => {
            let slots = args
                .get("slots")
                .and_then(|v| v.as_array())
                .cloned()
                .unwrap_or_default();
            db::with_db(state, |conn| {
                for slot in slots {
                    let slot_id = slot.get("slot_id").and_then(|v| v.as_i64());
                    let pilot = slot.get("pilot").and_then(|v| v.as_i64());
                    if let (Some(slot_id), Some(pilot)) = (slot_id, pilot) {
                        conn.execute(
                            "UPDATE heat_node SET pilot_id = ?1 WHERE id = ?2",
                            rusqlite::params![pilot, slot_id],
                        )
                        .ok();
                    }
                }
            });
            Ok(json!(true))
        }
        ("db", "pilot_attribute_value") => {
            let pilot_id = arg_i64("pilot_id").ok_or("pilot_id required")?;
            let attribute = arg_str("attribute").ok_or("attribute required")?;
            Ok(db::with_db(state, |conn| {
                conn.query_row(
                    "SELECT value FROM pilot_attribute WHERE pilot_id = ?1 AND attribute = ?2",
                    rusqlite::params![pilot_id, attribute],
                    |r| r.get::<_, String>(0),
                )
                .ok()
                .map(|v| json!(v))
                .unwrap_or(Value::Null)
            }))
        }
        ("db", "heat_attribute_value") => {
            let heat_id = arg_i64("heat_id").ok_or("heat_id required")?;
            let attribute = arg_str("attribute").ok_or("attribute required")?;
            Ok(db::with_db(state, |conn| {
                conn.query_row(
                    "SELECT value FROM heat_attribute WHERE heat_id = ?1 AND attribute = ?2",
                    rusqlite::params![heat_id, attribute],
                    |r| r.get::<_, String>(0),
                )
                .ok()
                .map(|v| json!(v))
                .unwrap_or(Value::Null)
            }))
        }

        // ---- ui: broadcasts and registration (no-ops where the Rust UI has
        // no plugin panel host yet) ----
        ("ui", "socket_broadcast") => {
            let event = arg_str("event").unwrap_or_default();
            let payload = args.get("payload").cloned().unwrap_or(json!({}));
            crate::socket::broadcast_payload(state, &event, payload);
            Ok(json!(true))
        }
        ("ui", "broadcast") => {
            match arg_str("what").unwrap_or_default().as_str() {
                "pilots" => {
                    let payload = db::with_db(state, |conn| {
                        crate::payloads::build_pilot_data(conn)
                    });
                    crate::socket::broadcast(state, "pilot_data", payload);
                }
                "heats" => {
                    let payload = db::with_db(state, |conn| {
                        let race = state.race.lock().unwrap();
                        crate::payloads::build_heat_data(conn, &race)
                    });
                    crate::socket::broadcast(state, "heat_data", payload);
                }
                "current_heat" => {
                    let payload = db::with_db(state, |conn| {
                        let race = state.race.lock().unwrap();
                        crate::payloads::build_current_heat(conn, &race)
                    });
                    crate::socket::broadcast(state, "current_heat", payload);
                }
                _ => {}
            }
            Ok(json!(true))
        }
        ("ui", "register_panel") | ("fields", "register_race_attribute")
        | ("fields", "register_pilot_attribute") | ("fields", "register_option") => {
            // No plugin panel host in the Rust UI yet; accepting the call lets
            // plugins that only need registration finish initialising.
            Ok(json!(true))
        }

        _ => Err(format!("Unknown RHAPI method: {}.{}", object, method)),
    }
}


/// Registers the RPC endpoint on a connected socket (called from the
/// socket.io namespace handler).
pub fn register_rpc(
    socket: &socketioxide::extract::SocketRef,
    state: SharedState,
) {
    use socketioxide::extract::{AckSender, Data};
    socket.on(
        "rhapi_call",
        move |ack: AckSender, data: Data<Value>| async move {
            let data = data.0;
            let object = data.get("object").and_then(|v| v.as_str()).unwrap_or("");
            let method = data.get("method").and_then(|v| v.as_str()).unwrap_or("");
            let args = data.get("args").cloned().unwrap_or(json!({}));
            let response = match dispatch(&state, object, method, &args) {
                Ok(result) => json!({"result": result}),
                Err(e) => json!({"error": e}),
            };
            let _ = ack.send(&response);
        },
    );
}
