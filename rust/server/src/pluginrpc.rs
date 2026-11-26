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
                        "id": r.get::<_, i64>(0)?,
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
                            "id": r.get::<_, i64>(0)?,
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
                if let Some(attributes) = args.get("attributes").and_then(|v| v.as_object()) {
                    for (name, value) in attributes {
                        if let Err(err) = conn.execute(
                            "INSERT OR REPLACE INTO pilot_attribute (id, name, value)
                             VALUES (?1, ?2, ?3)",
                            rusqlite::params![pilot_id, name, value.as_str().unwrap_or("")],
                        ) {
                            tracing::warn!("pilot_attribute insert failed: {}", err);
                        }
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
                        "id": r.get::<_, i64>(0)?,
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
                        "id": r.get::<_, i64>(0)?,
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
                        "id": r.get::<_, i64>(0)?,
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
                        "id": r.get::<_, i64>(0)?,
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
            let value = match args.get("value") {
                Some(Value::Bool(flag)) => if *flag { "1" } else { "0" }.to_string(),
                Some(Value::String(text)) => text.clone(),
                Some(other) => other.to_string(),
                None => String::new(),
            };
            Ok(db::with_db(state, |conn| {
                db::set_option(conn, &name, &value);
                json!(true)
            }))
        }

        // ---- race ----
        ("race", "status") => {
            let (status, current_heat, format_id, started) = {
                let race = state.race.lock().unwrap();
                (
                    race.race_status,
                    race.current_heat,
                    race.format_id,
                    race.start_time_monotonic,
                )
            };
            let frequencyset = db::with_db(state, |conn| db::current_profile_id(conn));
            Ok(json!({
                "race_status": status,
                "current_heat": current_heat,
                "format_id": format_id,
                "start_time_monotonic": started,
                "frequencyset": frequencyset,
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
            // RHAPI allows HEAT_ID_NONE (None) to clear the current heat.
            let heat_id = arg_i64("heat_id");
            if heat_id.is_none() {
                let mut race = state.race.lock().unwrap();
                race.current_heat = None;
                race.node_pilots = vec![None; state.node_count()];
                return Ok(json!(true));
            }
            let heat_id = heat_id.unwrap();
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
                        let text = match value {
                            Value::String(text) => text.clone(),
                            other => other.to_string(),
                        };
                        conn.execute(
                            "INSERT OR REPLACE INTO heat_attribute (id, name, value)
                             VALUES (?1, ?2, ?3)",
                            rusqlite::params![heat_id, key, text],
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
                    "SELECT value FROM pilot_attribute WHERE id = ?1 AND name = ?2",
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
                    "SELECT value FROM heat_attribute WHERE id = ?1 AND name = ?2",
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
                "frequencies" | "frequencyset" => {
                    let payload = db::with_db(state, crate::payloads::build_frequency_data);
                    crate::socket::broadcast(state, "frequency_data", payload);
                }
                "race_status" => {
                    let payload = db::with_db(state, |conn| {
                        let race = state.race.lock().unwrap();
                        crate::payloads::build_race_status(conn, &race)
                    });
                    crate::socket::broadcast(state, "race_status", payload);
                }
                "raceclasses" => {
                    let payload = db::with_db(state, crate::payloads::build_class_data);
                    crate::socket::broadcast(state, "class_data", payload);
                }
                "raceformats" => {
                    let payload = db::with_db(state, crate::payloads::build_format_data);
                    crate::socket::broadcast(state, "format_data", payload);
                }
                _ => {}
            }
            Ok(json!(true))
        }
        // ---- plugin panels (rendered by rh-ui.js into #custom-ui) ----
        ("ui", "register_panel") => {
            let name = arg_str("domain").ok_or("domain required")?;
            let label = arg_str("name").unwrap_or_else(|| name.clone());
            let page = arg_str("location").unwrap_or_else(|| "settings".to_string());
            let order = arg_i64("order");
            state.plugin_panels.lock().unwrap().push(json!({
                "name": name,
                "label": label,
                "page": page,
                "order": order,
                "open": false,
            }));
            Ok(json!(true))
        }
        ("fields", "register_option") => {
            let panel = arg_str("panel").unwrap_or_default();
            let field = args.get("field").cloned().unwrap_or(json!({}));
            state
                .plugin_fields
                .lock()
                .unwrap()
                .push(json!({"panel": panel, "field": field}));
            Ok(json!(true))
        }
        ("fields", "register_race_attribute") | ("fields", "register_pilot_attribute") => {
            // Attribute registration only labels the forms built by load_data
            // 'attributes'; the values themselves live in *_attribute tables.
            Ok(json!(true))
        }

        // ---- plugin transport ----
        // The bridge declares which client socket events its plugins listen
        // to; the connect handler registers a forwarder for each name.
        ("plugin", "listen") => {
            let names: Vec<String> = args
                .get("names")
                .and_then(|v| v.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|v| v.as_str().map(String::from))
                        .collect()
                })
                .unwrap_or_default();
            tracing::info!("plugin bridge listens for {} socket event(s)", names.len());
            *state.plugin_listeners.lock().unwrap() = names;
            Ok(json!(true))
        }

        // ---- config write (RHAPI config.set) ----
        ("config", "set") => {
            let section = arg_str("section").ok_or("section required")?;
            let key = arg_str("key").ok_or("key required")?;
            let value = args.get("value").cloned().unwrap_or(Value::Null);
            state
                .config_items
                .lock()
                .unwrap()
                .insert(format!("{}_{}", section, key), value.clone());
            crate::phase3::persist_config_item(&state.data_dir, &section, &key, &value);
            Ok(json!(true))
        }

        // ---- frequency set (RHAPI db.frequencyset_alter) ----
        // FPVTrackSide sends a flat list of frequencies; the profile stores
        // (band, channel, frequency) triples.
        ("db", "frequencyset_alter") => {
            let profile_id = arg_i64("frequencyset_id").ok_or("frequencyset_id required")?;
            let triples: Vec<Value> = args
                .get("frequencies")
                .and_then(|v| v.as_array())
                .map(|list| {
                    list.iter()
                        .map(|f| match f.as_i64() {
                            Some(freq) => json!([Value::Null, Value::Null, freq]),
                            None => json!([Value::Null, Value::Null, Value::Null]),
                        })
                        .collect()
                })
                .unwrap_or_default();
            let stored = serde_json::to_string(&triples).unwrap_or_else(|_| "[]".to_string());
            db::with_db(state, |conn| {
                conn.execute(
                    "UPDATE profiles SET frequencies = ?1 WHERE id = ?2",
                    rusqlite::params![stored, profile_id],
                )
                .ok();
            });
            Ok(json!(true))
        }

        ("db", "heat_delete") => {
            let heat_id = arg_i64("heat_id").ok_or("heat_id required")?;
            db::with_db(state, |conn| {
                conn.execute("DELETE FROM heat_node WHERE heat_id = ?1", [heat_id]).ok();
                conn.execute("DELETE FROM heat WHERE id = ?1", [heat_id]).ok();
            });
            {
                let mut race = state.race.lock().unwrap();
                if race.current_heat == Some(heat_id) {
                    race.current_heat = None;
                    race.node_pilots = vec![None; state.node_count()];
                }
            }
            let payload = db::with_db(state, |conn| {
                let race = state.race.lock().unwrap();
                crate::payloads::build_heat_data(conn, &race)
            });
            crate::socket::broadcast(state, "heat_data", payload);
            Ok(json!(true))
        }

        ("race", "laps") => Ok(db::with_db(state, |conn| {
            let race = state.race.lock().unwrap();
            crate::payloads::build_current_laps(conn, &race)
        })),

        // ---- saved races (marshal path) ----
        ("db", "race_alter") => {
            let race_id = arg_i64("race_id").ok_or("race_id required")?;
            let attributes = args.get("attributes").and_then(|v| v.as_object()).cloned();
            db::with_db(state, |conn| {
                if let Some(attributes) = &attributes {
                    for (name, value) in attributes {
                        // RHAPI stores attribute values as text; plugins pass
                        // integers (race ids) as often as strings, and only a
                        // real null clears an attribute.
                        match value {
                            Value::Null => {
                                conn.execute(
                                    "DELETE FROM saved_race_meta_attribute WHERE id = ?1 AND name = ?2",
                                    rusqlite::params![race_id, name],
                                )
                                .ok();
                            }
                            Value::String(text) => {
                                conn.execute(
                                    "INSERT OR REPLACE INTO saved_race_meta_attribute (id, name, value)
                                     VALUES (?1, ?2, ?3)",
                                    rusqlite::params![race_id, name, text],
                                )
                                .ok();
                            }
                            other => {
                                conn.execute(
                                    "INSERT OR REPLACE INTO saved_race_meta_attribute (id, name, value)
                                     VALUES (?1, ?2, ?3)",
                                    rusqlite::params![race_id, name, other.to_string()],
                                )
                                .ok();
                            }
                        }
                    }
                }
            });
            Ok(json!(true))
        }
        ("db", "race_attribute_value") => {
            let race_id = arg_i64("race_id").ok_or("race_id required")?;
            let name = arg_str("attribute").ok_or("attribute required")?;
            Ok(db::with_db(state, |conn| {
                conn.query_row(
                    "SELECT value FROM saved_race_meta_attribute WHERE id = ?1 AND name = ?2",
                    rusqlite::params![race_id, name],
                    |r| r.get::<_, String>(0),
                )
                .ok()
                .map(|v| json!(v))
                .unwrap_or(Value::Null)
            }))
        }
        ("db", "race_ids_by_attribute") => {
            let name = arg_str("attribute").ok_or("attribute required")?;
            // Plugins pass the looked-up id as an integer or as text.
            let value = args
                .get("value")
                .map(|v| match v {
                    Value::String(text) => text.clone(),
                    other => other.to_string(),
                })
                .unwrap_or_default();
            Ok(db::with_db(state, |conn| {
                let mut stmt = conn
                    .prepare("SELECT id FROM saved_race_meta_attribute WHERE name = ?1 AND value = ?2 ORDER BY id")
                    .unwrap();
                let rows = stmt
                    .query_map(rusqlite::params![name, value], |r| r.get::<_, i64>(0))
                    .unwrap();
                json!(rows.filter_map(|r| r.ok()).collect::<Vec<i64>>())
            }))
        }
        ("db", "pilot_ids_by_attribute") => {
            let name = arg_str("attribute").ok_or("attribute required")?;
            // Plugins pass the looked-up id as an integer or as text.
            let value = args
                .get("value")
                .map(|v| match v {
                    Value::String(text) => text.clone(),
                    other => other.to_string(),
                })
                .unwrap_or_default();
            Ok(db::with_db(state, |conn| {
                let mut stmt = conn
                    .prepare("SELECT id FROM pilot_attribute WHERE name = ?1 AND value = ?2 ORDER BY id")
                    .unwrap();
                let rows = stmt
                    .query_map(rusqlite::params![name, value], |r| r.get::<_, i64>(0))
                    .unwrap();
                json!(rows.filter_map(|r| r.ok()).collect::<Vec<i64>>())
            }))
        }
        ("db", "race_by_id") => {
            let race_id = arg_i64("race_id").ok_or("race_id required")?;
            Ok(db::with_db(state, |conn| {
                conn.query_row(
                    "SELECT id, round_id, heat_id, class_id, format_id, \
                            CAST(start_time AS TEXT) \
                     FROM saved_race_meta WHERE id = ?1",
                    [race_id],
                    |r| {
                        Ok(json!({
                            "id": r.get::<_, i64>(0)?,
                            "round_id": r.get::<_, i64>(1)?,
                            "heat_id": r.get::<_, i64>(2)?,
                            "class_id": r.get::<_, Option<i64>>(3)?,
                            "format_id": r.get::<_, Option<i64>>(4)?,
                            "start_time": r.get::<_, Option<String>>(5)?,
                        }))
                    },
                )
                .unwrap_or(Value::Null)
            }))
        }
        ("db", "pilotruns_by_race") => {
            let race_id = arg_i64("race_id").ok_or("race_id required")?;
            Ok(db::with_db(state, |conn| {
                let mut stmt = conn
                    .prepare(
                        "SELECT id, race_id, node_index, pilot_id, enter_at, exit_at, \
                                history_values, history_times FROM saved_pilot_race \
                         WHERE race_id = ?1 ORDER BY id",
                    )
                    .unwrap();
                let rows = stmt
                    .query_map([race_id], |r| {
                        Ok(json!({
                            "id": r.get::<_, i64>(0)?,
                            "race_id": r.get::<_, i64>(1)?,
                            "node_index": r.get::<_, i64>(2)?,
                            "pilot_id": r.get::<_, i64>(3)?,
                            "enter_at": r.get::<_, Option<i64>>(4)?,
                            "exit_at": r.get::<_, Option<i64>>(5)?,
                            "history_values": r.get::<_, Option<String>>(6)?,
                            "history_times": r.get::<_, Option<String>>(7)?,
                        }))
                    })
                    .unwrap();
                json!(rows.filter_map(|r| r.ok()).collect::<Vec<Value>>())
            }))
        }
        ("db", "laps_by_pilotrun") => {
            let run_id = arg_i64("pilotrun_id").ok_or("pilotrun_id required")?;
            Ok(db::with_db(state, |conn| {
                let mut stmt = conn
                    .prepare(
                        "SELECT id, lap_time_stamp, lap_time, lap_time_formatted, source, deleted, peak_rssi \
                         FROM saved_race_lap WHERE pilotrace_id = ?1 ORDER BY id",
                    )
                    .unwrap();
                let rows = stmt
                    .query_map([run_id], |r| {
                        Ok(json!({
                            "id": r.get::<_, i64>(0)?,
                            "lap_time_stamp": r.get::<_, f64>(1)?,
                            "lap_time": r.get::<_, f64>(2)?,
                            "lap_time_formatted": r.get::<_, Option<String>>(3)?,
                            "source": r.get::<_, Option<i64>>(4)?,
                            "deleted": r.get::<_, Option<i64>>(5)?.unwrap_or(0) != 0,
                            "peak_rssi": r.get::<_, Option<i64>>(6)?,
                        }))
                    })
                    .unwrap();
                json!(rows.filter_map(|r| r.ok()).collect::<Vec<Value>>())
            }))
        }
        ("db", "pilotrun_alter") => {
            let run_id = arg_i64("pilotrun_id").ok_or("pilotrun_id required")?;
            let enter_at = args.get("enter_at").and_then(|v| v.as_i64());
            let exit_at = args.get("exit_at").and_then(|v| v.as_i64());
            let laps = args.get("laps").and_then(|v| v.as_array()).cloned().unwrap_or_default();
            let (race_id, pilot_id) = db::with_db(state, |conn| {
                let ids: (i64, i64) = conn
                    .query_row(
                        "SELECT race_id, pilot_id FROM saved_pilot_race WHERE id = ?1",
                        [run_id],
                        |r| Ok((r.get(0)?, r.get(1)?)),
                    )
                    .unwrap_or((0, 0));
                if enter_at.is_some() {
                    conn.execute(
                        "UPDATE saved_pilot_race SET enter_at = ?1 WHERE id = ?2",
                        rusqlite::params![enter_at, run_id],
                    )
                    .ok();
                }
                if exit_at.is_some() {
                    conn.execute(
                        "UPDATE saved_pilot_race SET exit_at = ?1 WHERE id = ?2",
                        rusqlite::params![exit_at, run_id],
                    )
                    .ok();
                }
                conn.execute("DELETE FROM saved_race_lap WHERE pilotrace_id = ?1", [run_id])
                    .ok();
                for lap in &laps {
                    let stamp = lap.get("lap_time_stamp").and_then(|v| v.as_f64()).unwrap_or(0.0);
                    let time = lap.get("lap_time").and_then(|v| v.as_f64()).unwrap_or(0.0);
                    let formatted = lap.get("lap_time_formatted").and_then(|v| v.as_str());
                    let source = lap.get("source").and_then(|v| v.as_i64()).unwrap_or(4);
                    let deleted = lap.get("deleted").and_then(|v| v.as_bool()).unwrap_or(false);
                    let peak = lap.get("peak_rssi").and_then(|v| v.as_i64());
                    conn.execute(
                        "INSERT INTO saved_race_lap (race_id, pilotrace_id, node_index, pilot_id,
                                                     lap_time_stamp, lap_time, lap_time_formatted,
                                                     source, deleted, peak_rssi)
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                        rusqlite::params![ids.0, run_id,
                                          conn.query_row("SELECT node_index FROM saved_pilot_race WHERE id = ?1",
                                                         [run_id], |r| r.get::<_, i64>(0)).unwrap_or(0),
                                          ids.1, stamp, time, formatted, source,
                                          if deleted { 1 } else { 0 }, peak],
                    )
                    .ok();
                }
                ids
            });
            crate::socket::emit_plugin_event(
                state,
                "lapsResave",
                &json!({"race_id": race_id, "pilot_id": pilot_id}),
            );
            Ok(json!(true))
        }

        // ---- seat colours re-read from config (RHAPI race.update_colors) ----
        ("race", "update_colors") => {
            let payload = crate::payloads::build_seat_data(state);
            crate::socket::broadcast(state, "seat_data", payload);
            Ok(json!(true))
        }

        // ---- server / language ----
        ("server", "data_dir") => Ok(json!(state.data_dir)),
        ("server", "program_dir") => Ok(json!(state.data_dir)),
        ("server", "info") => Ok(json!({
            "release_version": crate::state::RELEASE_VERSION,
            "server_api": state.info.server_api,
            "node_api_best": state.info.node_api_best,
            "json_api": state.info.json_api,
        })),
        ("server", "seat_color_defaults") => Ok(json!(crate::payloads::seat_colors(state))),
        ("language", "languages") => Ok(json!(crate::phase3::languages())),
        ("config", "all") => {
            let text = std::fs::read_to_string(format!("{}/config.json", state.data_dir))
                .map_err(|e| e.to_string())?;
            let parsed: Value = serde_json::from_str(&text).map_err(|e| e.to_string())?;
            Ok(parsed)
        }

        // ---- registries: class rank, points, heat generators, exporters ----
        ("classrank", "methods") => Ok(json!(crate::class_rank::rank_method_list())),
        ("points", "methods") => Ok(json!(crate::class_rank::points_method_list())),
        ("heatgen", "generators") => Ok(json!(crate::heatgen::generator_list())),
        ("io", "exporters") => Ok(json!(crate::export_import::exporter_list())),
        ("io", "importers") => Ok(json!(crate::export_import::importer_list())),

        // ---- LED ----
        ("led", "enabled") => Ok(json!(true)),
        ("led", "effects") => {
            let mut map = serde_json::Map::new();
            for name in state.led_manager.effect_names() {
                map.insert(name.clone(), json!({"name": name, "fields": Value::Null}));
            }
            Ok(json!(map))
        }
        ("led", "activate_effect") => {
            let name = arg_str("effect").ok_or("effect required")?;
            let payload = args.get("args").cloned().unwrap_or(json!({}));
            state.led_manager.trigger(&name, &payload);
            Ok(json!(true))
        }
        ("led", "effect_set") => {
            let event = arg_str("event").ok_or("event required")?;
            let effect = arg_str("effect").ok_or("effect required")?;
            Ok(json!(state.led_manager.set_event_effect(&event, &effect)))
        }
        ("led", "clear") => {
            state.led_manager.trigger_idle(crate::state::RACE_STATUS_READY);
            Ok(json!(true))
        }
        ("led", "display_color") => Ok(json!(true)),
        ("led", "effect_by_event") => Ok(Value::Null),

        // ---- VRx control ----
        ("vrx", "enabled") => Ok(json!(state.vrx_manager.is_enabled())),
        ("vrx", "status") => Ok(state.vrx_manager.get_controller_status()),
        ("vrx", "devices") => Ok(state.vrx_manager.get_all_device_status()),
        ("vrx", "kill") => {
            state.vrx_manager.kill();
            Ok(json!(true))
        }
        ("vrx", "devices_by_pilot") => {
            let seat = arg_i64("seat").unwrap_or(0);
            let pilot_id = arg_i64("pilot_id");
            Ok(json!(state.vrx_manager.get_active_devices(seat, pilot_id)
                .into_iter()
                .map(|(id, _)| id)
                .collect::<Vec<String>>()))
        }

        // ---- ui: dynamic updates and one-off emits ----
        ("ui", "register_markdown") | ("ui", "register_quickbutton")
        | ("ui", "register_script") | ("ui", "register_stylesheet")
        | ("ui", "blueprint_add") => Ok(json!(true)),
        ("ui", "update_field") => {
            crate::socket::broadcast_payload(state, "ui_field_update",
                args.get("field").cloned().unwrap_or(json!({})));
            Ok(json!(true))
        }
        ("ui", "update_markdown") => {
            crate::socket::broadcast_payload(state, "ui_markdown_update",
                args.get("markdown").cloned().unwrap_or(json!({})));
            Ok(json!(true))
        }
        ("ui", "socket_send") => {
            let event = arg_str("event").unwrap_or_default();
            let payload = args.get("payload").cloned().unwrap_or(json!({}));
            crate::socket::broadcast_payload(state, &event, payload);
            Ok(json!(true))
        }
        ("ui", "socket_send_to") => Ok(json!(false)),

        // ---- race snapshot: everything RaceAPI exposes, in one call ----
        ("race", "snapshot") => {
            let (status, heat, start_m, start_ms, start_fmt, end_m, scheduled,
                 scheduled_time, pilots, slot_count) = {
                let race = state.race.lock().unwrap();
                (
                    race.race_status,
                    race.current_heat,
                    race.start_time_monotonic,
                    race.start_time_epoch_ms,
                    race.start_time_formatted.clone(),
                    race.end_time_monotonic,
                    race.scheduled,
                    race.scheduled_time,
                    race.node_pilots.clone(),
                    state.node_count(),
                )
            };
            let base = db::with_db(state, |conn| {
                let race = state.race.lock().unwrap();
                crate::payloads::build_race_status(conn, &race)
            });
            let laps = db::with_db(state, |conn| {
                let race = state.race.lock().unwrap();
                crate::payloads::build_current_laps(conn, &race)
            });
            let status_message = match status {
                1 => "Racing",
                2 => "Race over",
                3 => "Staging",
                _ => "Ready",
            };
            let mut snapshot = base.clone();
            if let Some(map) = snapshot.as_object_mut() {
                map.insert("race_status".into(), json!(status));
                map.insert("race_heat_id".into(), json!(heat));
                map.insert("format_id".into(), json!(
                    base.get("race_format_id").cloned().unwrap_or(json!(1))));
                map.insert("start_time_internal".into(), json!(start_m));
                map.insert("start_time_epoch_ms".into(), json!(start_ms));
                map.insert("start_time".into(), json!(start_fmt));
                map.insert("end_time_internal".into(), json!(end_m));
                map.insert("stage_time_internal".into(), json!(0.0));
                map.insert("scheduled".into(), json!(scheduled));
                map.insert("scheduled_time".into(), json!(scheduled_time));
                map.insert("pilots".into(), json!(pilots));
                map.insert("slots".into(), json!(slot_count));
                map.insert("seat_colors".into(), json!(crate::payloads::seat_colors(state)));
                map.insert("laps".into(), laps.clone());
                map.insert("laps_raw".into(), laps);
                map.insert("status_message".into(), json!(status_message));
                map.insert("phonetic_status_msg".into(), json!(status_message));
                map.insert("seats_finished".into(), json!(vec![false; slot_count]));
                map.insert("teams".into(), json!(Vec::<Option<String>>::new()));
            }
            Ok(snapshot)
        }

        // ---- db: collections ----
        ("db", "options") => Ok(db::with_db(state, |conn| {
            let mut stmt = conn
                .prepare("SELECT option_name, option_value FROM global_settings ORDER BY option_name")
                .unwrap();
            let rows = stmt
                .query_map([], |r| {
                    Ok(json!({
                        "name": r.get::<_, String>(0)?,
                        "value": r.get::<_, Option<String>>(1)?,
                    }))
                })
                .unwrap();
            json!(rows.filter_map(|r| r.ok()).collect::<Vec<Value>>())
        })),
        ("db", "races") => Ok(db::with_db(state, |conn| {
            let mut stmt = conn
                .prepare("SELECT id, round_id, heat_id, class_id, format_id, \
                          CAST(start_time AS TEXT) FROM saved_race_meta ORDER BY id")
                .unwrap();
            let rows = stmt
                .query_map([], |r| Ok(json!({
                    "id": r.get::<_, i64>(0)?,
                    "round_id": r.get::<_, i64>(1)?,
                    "heat_id": r.get::<_, i64>(2)?,
                    "class_id": r.get::<_, Option<i64>>(3)?,
                    "format_id": r.get::<_, Option<i64>>(4)?,
                    "start_time": r.get::<_, Option<String>>(5)?,
                })))
                .unwrap();
            json!(rows.filter_map(|r| r.ok()).collect::<Vec<Value>>())
        })),
        ("db", "frequencysets") => Ok(db::with_db(state, |conn| {
            let mut stmt = conn
                .prepare("SELECT id, name, description FROM profiles ORDER BY id")
                .unwrap();
            let rows = stmt
                .query_map([], |r| Ok(json!({
                    "id": r.get::<_, i64>(0)?,
                    "name": r.get::<_, String>(1)?,
                    "description": r.get::<_, Option<String>>(2)?,
                })))
                .unwrap();
            json!(rows.filter_map(|r| r.ok()).collect::<Vec<Value>>())
        })),
        ("db", "laps") => Ok(db::with_db(state, |conn| {
            let mut stmt = conn
                .prepare("SELECT id, race_id, pilotrace_id, node_index, pilot_id, \
                          lap_time_stamp, lap_time, deleted FROM saved_race_lap ORDER BY id")
                .unwrap();
            let rows = stmt
                .query_map([], |r| Ok(json!({
                    "id": r.get::<_, i64>(0)?,
                    "race_id": r.get::<_, i64>(1)?,
                    "pilotrun_id": r.get::<_, Option<i64>>(2)?,
                    "node_index": r.get::<_, i64>(3)?,
                    "pilot_id": r.get::<_, Option<i64>>(4)?,
                    "lap_time_stamp": r.get::<_, f64>(5)?,
                    "lap_time": r.get::<_, f64>(6)?,
                    "deleted": r.get::<_, Option<i64>>(7)?.unwrap_or(0) != 0,
                })))
                .unwrap();
            json!(rows.filter_map(|r| r.ok()).collect::<Vec<Value>>())
        })),
        ("db", "pilotruns") => Ok(db::with_db(state, |conn| {
            let mut stmt = conn
                .prepare("SELECT id, race_id, node_index, pilot_id FROM saved_pilot_race ORDER BY id")
                .unwrap();
            let rows = stmt
                .query_map([], |r| Ok(json!({
                    "id": r.get::<_, i64>(0)?,
                    "race_id": r.get::<_, i64>(1)?,
                    "node_index": r.get::<_, i64>(2)?,
                    "pilot_id": r.get::<_, i64>(3)?,
                })))
                .unwrap();
            json!(rows.filter_map(|r| r.ok()).collect::<Vec<Value>>())
        })),
        ("db", "deleted_races") => Ok(db::with_db(state, |conn| {
            let mut stmt = conn
                .prepare("SELECT id, race_id, round_id, heat_id, class_id, format_id, \
                          start_time_formatted FROM deleted_race ORDER BY id")
                .unwrap();
            let rows = stmt
                .query_map([], |r| Ok(json!({
                    "id": r.get::<_, i64>(0)?,
                    "race_id": r.get::<_, i64>(1)?,
                    "round_id": r.get::<_, Option<i64>>(2)?,
                    "heat_id": r.get::<_, Option<i64>>(3)?,
                    "class_id": r.get::<_, Option<i64>>(4)?,
                    "format_id": r.get::<_, Option<i64>>(5)?,
                    "start_time": r.get::<_, Option<String>>(6)?,
                })))
                .unwrap();
            json!(rows.filter_map(|r| r.ok()).collect::<Vec<Value>>())
        })),
        ("db", "backup_dir") => Ok(json!(format!("{}/db_bkp", state.data_dir))),
        ("db", "backup_list") => {
            let dir = format!("{}/db_bkp", state.data_dir);
            let mut files = Vec::new();
            if let Ok(entries) = std::fs::read_dir(&dir) {
                for entry in entries.flatten() {
                    if let Some(name) = entry.file_name().to_str() {
                        files.push(name.to_string());
                    }
                }
            }
            files.sort();
            Ok(json!(files))
        }

        // ---- db: lookups and mutations ----
        ("db", "pilot_delete") => {
            let pilot_id = arg_i64("pilot_id").ok_or("pilot_id required")?;
            db::with_db(state, |conn| {
                conn.execute("DELETE FROM pilot WHERE id = ?1", [pilot_id]).ok();
                conn.execute("UPDATE heat_node SET pilot_id = NULL WHERE pilot_id = ?1", [pilot_id]).ok();
            });
            Ok(json!(true))
        }
        ("db", "heat_duplicate") => {
            let heat_id = arg_i64("heat_id").ok_or("heat_id required")?;
            let new_id = db::with_db(state, |conn| {
                let new_id = db::next_heat_id(conn);
                conn.execute(
                    "INSERT INTO heat (id, note, auto_name, class_id, results, \"cacheStatus\", \"order\",
                                       status, auto_frequency, group_id, active, coop_best_time, coop_num_laps)
                     SELECT ?1, note, auto_name, class_id, NULL, '{}', \"order\", 0, auto_frequency,
                            group_id, 1, NULL, NULL FROM heat WHERE id = ?2",
                    rusqlite::params![new_id, heat_id],
                ).ok();
                if let Ok(mut stmt) = conn.prepare(
                    "SELECT node_index, pilot_id, color, method, seed_rank, seed_id FROM heat_node WHERE heat_id = ?1")
                {
                    if let Ok(rows) = stmt.query_map([heat_id], |r| Ok((
                        r.get::<_, i64>(0)?, r.get::<_, Option<i64>>(1)?,
                        r.get::<_, Option<String>>(2)?, r.get::<_, Option<i64>>(3)?,
                        r.get::<_, Option<i64>>(4)?, r.get::<_, Option<i64>>(5)?))) {
                        for row in rows.flatten() {
                            conn.execute(
                                "INSERT INTO heat_node (id, heat_id, node_index, pilot_id, color, method, seed_rank, seed_id)
                                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                                rusqlite::params![db::next_heat_node_id(conn), new_id, row.0, row.1, row.2, row.3, row.4, row.5],
                            ).ok();
                        }
                    }
                }
                new_id
            });
            Ok(json!({"id": new_id}))
        }
        ("db", "heat_max_round") => {
            let heat_id = arg_i64("heat_id").ok_or("heat_id required")?;
            Ok(db::with_db(state, |conn| json!(db::max_round(conn, heat_id))))
        }
        ("db", "heat_ids_by_attribute") => {
            let name = arg_str("attribute").ok_or("attribute required")?;
            let value = args.get("value").map(|v| match v {
                Value::String(t) => t.clone(),
                other => other.to_string(),
            }).unwrap_or_default();
            Ok(db::with_db(state, |conn| {
                let mut stmt = conn.prepare(
                    "SELECT id FROM heat_attribute WHERE name = ?1 AND value = ?2 ORDER BY id").unwrap();
                let rows = stmt.query_map(rusqlite::params![name, value], |r| r.get::<_, i64>(0)).unwrap();
                json!(rows.filter_map(|r| r.ok()).collect::<Vec<i64>>())
            }))
        }
        ("db", "raceclass_ids_by_attribute") | ("db", "raceformat_ids_by_attribute") => {
            let table = if method == "raceclass_ids_by_attribute" { "race_class_attribute" } else { "race_format_attribute" };
            let name = arg_str("attribute").ok_or("attribute required")?;
            let value = args.get("value").map(|v| match v {
                Value::String(t) => t.clone(),
                other => other.to_string(),
            }).unwrap_or_default();
            Ok(db::with_db(state, |conn| {
                let sql = format!("SELECT id FROM {} WHERE name = ?1 AND value = ?2 ORDER BY id", table);
                match conn.prepare(&sql) {
                    Ok(mut stmt) => {
                        let rows = stmt.query_map(rusqlite::params![name, value], |r| r.get::<_, i64>(0));
                        match rows {
                            Ok(rows) => json!(rows.filter_map(|r| r.ok()).collect::<Vec<i64>>()),
                            Err(_) => json!([]),
                        }
                    }
                    Err(_) => json!([]),
                }
            }))
        }
        ("db", "race_delete") => {
            let race_id = arg_i64("race_id").ok_or("race_id required")?;
            db::with_db(state, |conn| {
                conn.execute("DELETE FROM saved_race_lap WHERE race_id = ?1", [race_id]).ok();
                conn.execute("DELETE FROM saved_pilot_race WHERE race_id = ?1", [race_id]).ok();
                conn.execute("DELETE FROM saved_race_meta_attribute WHERE id = ?1", [race_id]).ok();
                conn.execute("DELETE FROM saved_race_meta WHERE id = ?1", [race_id]).ok();
            });
            Ok(json!(true))
        }
        ("db", "races_by_heat") => {
            let heat_id = arg_i64("heat_id").ok_or("heat_id required")?;
            Ok(db::with_db(state, |conn| {
                let mut stmt = conn.prepare(
                    "SELECT id, round_id FROM saved_race_meta WHERE heat_id = ?1 ORDER BY id").unwrap();
                let rows = stmt.query_map([heat_id], |r| Ok(json!({
                    "id": r.get::<_, i64>(0)?,
                    "round_id": r.get::<_, i64>(1)?,
                }))).unwrap();
                json!(rows.filter_map(|r| r.ok()).collect::<Vec<Value>>())
            }))
        }
        ("db", "races_by_raceclass") => {
            let class_id = arg_i64("class_id").ok_or("class_id required")?;
            Ok(db::with_db(state, |conn| {
                let mut stmt = conn.prepare(
                    "SELECT id, heat_id, round_id FROM saved_race_meta WHERE class_id = ?1 ORDER BY id").unwrap();
                let rows = stmt.query_map([class_id], |r| Ok(json!({
                    "id": r.get::<_, i64>(0)?,
                    "heat_id": r.get::<_, i64>(1)?,
                    "round_id": r.get::<_, i64>(2)?,
                }))).unwrap();
                json!(rows.filter_map(|r| r.ok()).collect::<Vec<Value>>())
            }))
        }
        ("db", "slot_alter") => {
            let slot_id = arg_i64("slot_id").ok_or("slot_id required")?;
            let pilot = args.get("pilot").cloned().unwrap_or(Value::Null);
            let pilot_val: Option<i64> = pilot.as_i64();
            db::with_db(state, |conn| {
                conn.execute("UPDATE heat_node SET pilot_id = ?1 WHERE id = ?2",
                             rusqlite::params![pilot_val, slot_id]).ok();
            });
            Ok(json!(true))
        }
        ("db", "options_reset") => {
            db::with_db(state, |conn| { conn.execute("DELETE FROM global_settings", []).ok(); });
            Ok(json!(true))
        }

        // ---- db: saved races, results ----
        ("db", "race_by_heat_round") => {
            let heat_id = arg_i64("heat_id").ok_or("heat_id required")?;
            let round_id = arg_i64("round_id").ok_or("round_id required")?;
            Ok(db::with_db(state, |conn| {
                conn.query_row(
                    "SELECT id, heat_id, round_id, class_id, format_id, CAST(start_time AS TEXT) \
                     FROM saved_race_meta WHERE heat_id = ?1 AND round_id = ?2 ORDER BY id DESC LIMIT 1",
                    rusqlite::params![heat_id, round_id],
                    |r| Ok(json!({
                        "id": r.get::<_, i64>(0)?,
                        "heat_id": r.get::<_, i64>(1)?,
                        "round_id": r.get::<_, i64>(2)?,
                        "class_id": r.get::<_, Option<i64>>(3)?,
                        "format_id": r.get::<_, Option<i64>>(4)?,
                        "start_time": r.get::<_, Option<String>>(5)?,
                    })),
                ).unwrap_or(Value::Null)
            }))
        }
        ("db", "race_results") | ("db", "heat_results") => {
            let race_id = arg_i64("race_id").ok_or("race_id required")?;
            Ok(db::with_db(state, |conn| {
                conn.query_row(
                    "SELECT results FROM saved_race_meta WHERE id = ?1",
                    [race_id],
                    |r| r.get::<_, Option<String>>(0),
                )
                .ok()
                .flatten()
                .and_then(|text| serde_json::from_str::<Value>(&text).ok())
                .unwrap_or(Value::Null)
            }))
        }
        ("db", "race_results_rebuild") => Ok(json!(true)),
        ("db", "event_results") => Ok(Value::Null),
        ("db", "races_clear") => {
            db::with_db(state, |conn| {
                for table in ["saved_race_lap", "saved_pilot_race", "saved_race_meta_attribute", "saved_race_meta"] {
                    let sql = format!("DELETE FROM {}", table);
                    conn.execute(&sql, []).ok();
                }
            });
            Ok(json!(true))
        }
        ("db", "deleted_race_by_id") => {
            let race_id = arg_i64("race_id").ok_or("race_id required")?;
            Ok(db::with_db(state, |conn| {
                conn.query_row(
                    "SELECT id, race_id, data FROM deleted_race WHERE race_id = ?1",
                    [race_id],
                    |r| Ok(json!({
                        "id": r.get::<_, i64>(0)?,
                        "race_id": r.get::<_, i64>(1)?,
                        "data": r.get::<_, Option<String>>(2)?,
                    })),
                ).unwrap_or(Value::Null)
            }))
        }

        // ---- db: classes, formats, profiles ----
        ("db", "raceclass_by_id") => {
            let class_id = arg_i64("class_id").ok_or("class_id required")?;
            Ok(db::with_db(state, |conn| {
                conn.query_row(
                    "SELECT id, name, description, format_id FROM race_class WHERE id = ?1",
                    [class_id],
                    |r| Ok(json!({
                        "id": r.get::<_, i64>(0)?,
                        "name": r.get::<_, Option<String>>(1)?,
                        "description": r.get::<_, Option<String>>(2)?,
                        "format_id": r.get::<_, Option<i64>>(3)?,
                    })),
                ).unwrap_or(Value::Null)
            }))
        }
        ("db", "raceformat_by_id") => {
            let format_id = arg_i64("format_id").ok_or("format_id required")?;
            Ok(db::with_db(state, |conn| {
                conn.query_row(
                    "SELECT id, name, race_mode, race_time_sec FROM race_format WHERE id = ?1",
                    [format_id],
                    |r| Ok(json!({
                        "id": r.get::<_, i64>(0)?,
                        "name": r.get::<_, Option<String>>(1)?,
                        "race_mode": r.get::<_, Option<i64>>(2)?,
                        "race_time_sec": r.get::<_, Option<i64>>(3)?,
                    })),
                ).unwrap_or(Value::Null)
            }))
        }
        ("db", "frequencyset_by_id") => {
            let profile_id = arg_i64("frequencyset_id").ok_or("frequencyset_id required")?;
            Ok(db::with_db(state, |conn| {
                conn.query_row(
                    "SELECT id, name, description, frequencies FROM profiles WHERE id = ?1",
                    [profile_id],
                    |r| Ok(json!({
                        "id": r.get::<_, i64>(0)?,
                        "name": r.get::<_, Option<String>>(1)?,
                        "description": r.get::<_, Option<String>>(2)?,
                        "frequencies": r.get::<_, Option<String>>(3)?
                            .and_then(|t| serde_json::from_str::<Value>(&t).ok()),
                    })),
                ).unwrap_or(Value::Null)
            }))
        }
        ("db", "raceclass_attribute_value") | ("db", "raceformat_attribute_value") => {
            let table = if method == "raceclass_attribute_value" { "race_class_attribute" } else { "race_format_attribute" };
            let id = arg_i64("raceclass_id").or_else(|| arg_i64("raceformat_id")).ok_or("id required")?;
            let name = arg_str("attribute").ok_or("attribute required")?;
            Ok(db::with_db(state, |conn| {
                let sql = format!("SELECT value FROM {} WHERE id = ?1 AND name = ?2", table);
                match conn.prepare(&sql) {
                    Ok(mut stmt) => stmt
                        .query_row(rusqlite::params![id, name], |r| r.get::<_, String>(0))
                        .ok()
                        .map(|v| json!(v))
                        .unwrap_or(Value::Null),
                    Err(_) => Value::Null,
                }
            }))
        }
        ("db", "heats_by_class") => {
            let class_id = arg_i64("class_id").ok_or("class_id required")?;
            Ok(db::with_db(state, |conn| {
                let mut stmt = conn.prepare(
                    "SELECT id FROM heat WHERE class_id = ?1 ORDER BY id").unwrap();
                let rows = stmt.query_map([class_id], |r| r.get::<_, i64>(0)).unwrap();
                json!(rows.filter_map(|r| r.ok()).collect::<Vec<i64>>())
            }))
        }
        ("db", "heats_reset") | ("db", "pilots_reset") | ("db", "raceclasses_reset")
        | ("db", "raceformats_reset") | ("db", "reset_all") => {
            let tables: &[&str] = match method {
                "heats_reset" => &["heat_node", "heat"],
                "pilots_reset" => &["pilot"],
                "raceclasses_reset" => &["race_class"],
                "raceformats_reset" => &["race_format"],
                _ => &["heat_node", "heat", "pilot", "race_class", "race_format"],
            };
            db::with_db(state, |conn| {
                for table in tables {
                    let sql = format!("DELETE FROM {}", table);
                    conn.execute(&sql, []).ok();
                }
            });
            Ok(json!(true))
        }

        // ---- db: pilot runs ----
        ("db", "pilotrun_by_id") => {
            let run_id = arg_i64("pilotrun_id").ok_or("pilotrun_id required")?;
            Ok(db::with_db(state, |conn| {
                conn.query_row(
                    "SELECT id, race_id, node_index, pilot_id, enter_at, exit_at \
                     FROM saved_pilot_race WHERE id = ?1",
                    [run_id],
                    |r| Ok(json!({
                        "id": r.get::<_, i64>(0)?,
                        "race_id": r.get::<_, i64>(1)?,
                        "node_index": r.get::<_, i64>(2)?,
                        "pilot_id": r.get::<_, i64>(3)?,
                        "enter_at": r.get::<_, Option<i64>>(4)?,
                        "exit_at": r.get::<_, Option<i64>>(5)?,
                    })),
                ).unwrap_or(Value::Null)
            }))
        }
        ("db", "pilotrun_history") => {
            let run_id = arg_i64("pilotrun_id").ok_or("pilotrun_id required")?;
            Ok(db::with_db(state, |conn| {
                conn.query_row(
                    "SELECT history_values, history_times FROM saved_pilot_race WHERE id = ?1",
                    [run_id],
                    |r| Ok(json!({
                        "history_values": r.get::<_, Option<String>>(0)?,
                        "history_times": r.get::<_, Option<String>>(1)?,
                    })),
                ).unwrap_or(Value::Null)
            }))
        }
        ("db", "lap_splits") => {
            let lap_id = arg_i64("lap_id").ok_or("lap_id required")?;
            Ok(db::with_db(state, |conn| {
                let mut stmt = conn.prepare(
                    "SELECT id, node_index, pilot_id, lap_id, split_id, split_time \
                     FROM lap_split WHERE lap_id = ?1 ORDER BY split_id").unwrap();
                let rows = stmt.query_map([lap_id], |r| Ok(json!({
                    "id": r.get::<_, i64>(0)?,
                    "node_index": r.get::<_, i64>(1)?,
                    "pilot_id": r.get::<_, Option<i64>>(2)?,
                    "lap_id": r.get::<_, i64>(3)?,
                    "split_id": r.get::<_, i64>(4)?,
                    "split_time": r.get::<_, Option<f64>>(5)?,
                }))).unwrap();
                json!(rows.filter_map(|r| r.ok()).collect::<Vec<Value>>())
            }))
        }
        ("db", "saved_lap_splits_by_race") => {
            let race_id = arg_i64("race_id").ok_or("race_id required")?;
            Ok(db::with_db(state, |conn| {
                let mut stmt = conn.prepare(
                    "SELECT id, race_id, pilotrace_id, node_index, pilot_id, lap_id, split_id, split_time \
                     FROM saved_race_lap_split WHERE race_id = ?1 ORDER BY id").unwrap();
                let rows = stmt.query_map([race_id], |r| Ok(json!({
                    "id": r.get::<_, i64>(0)?,
                    "race_id": r.get::<_, i64>(1)?,
                    "pilotrun_id": r.get::<_, Option<i64>>(2)?,
                    "node_index": r.get::<_, i64>(3)?,
                    "pilot_id": r.get::<_, Option<i64>>(4)?,
                    "lap_id": r.get::<_, i64>(5)?,
                    "split_id": r.get::<_, i64>(6)?,
                    "split_time": r.get::<_, Option<f64>>(7)?,
                }))).unwrap();
                json!(rows.filter_map(|r| r.ok()).collect::<Vec<Value>>())
            }))
        }

        // ---- race: lap edits and scheduling ----
        ("race", "lap_add") => {
            let seat = arg_i64("seat").ok_or("seat required")? as usize;
            let timestamp = args.get("timestamp").and_then(|v| v.as_f64()).unwrap_or(0.0);
            crate::race::register_lap_at(state, seat, timestamp);
            Ok(json!(true))
        }
        ("race", "lap_insert") => {
            let seat = arg_i64("seat").ok_or("seat required")? as usize;
            let stamp = args.get("lap_time_stamp").and_then(|v| v.as_f64()).ok_or("lap_time_stamp required")?;
            let source = arg_i64("source").unwrap_or(4);
            let peak = arg_i64("peak");
            Ok(json!(crate::race::insert_lap(state, seat, stamp, source, peak, true).is_some()))
        }
        ("race", "laps_replace") => {
            let seat = arg_i64("seat").ok_or("seat required")? as usize;
            let laps = args.get("laps").and_then(|v| v.as_array()).cloned().unwrap_or_default();
            {
                let mut race = state.race.lock().unwrap();
                if let Some(slot) = race.node_laps.get_mut(seat) {
                    slot.clear();
                    for lap in laps {
                        slot.push(crate::state::Lap {
                            lap_time_stamp: lap.get("lap_time_stamp").and_then(|v| v.as_f64()).unwrap_or(0.0),
                            lap_time: lap.get("lap_time").and_then(|v| v.as_f64()).unwrap_or(0.0),
                            source: lap.get("source").and_then(|v| v.as_i64()).unwrap_or(4),
                            deleted: lap.get("deleted").and_then(|v| v.as_bool()).unwrap_or(false),
                            peak_rssi: lap.get("peak_rssi").and_then(|v| v.as_i64()),
                        });
                    }
                }
            }
            Ok(json!(true))
        }
        ("race", "schedule") => {
            let when = args.get("time").and_then(|v| v.as_f64()).unwrap_or(0.0);
            {
                let mut race = state.race.lock().unwrap();
                race.scheduled = when > 0.0;
                race.scheduled_time = when;
            }
            Ok(json!(true))
        }

        // ---- db: profiles / classes / formats CRUD ----
        ("db", "frequencyset_add") => {
            let name = arg_str("name").unwrap_or_else(|| "Frequency Set".to_string());
            let id = db::with_db(state, |conn| {
                conn.execute(
                    "INSERT INTO profiles (name, description, frequencies, enter_ats, exit_ats, f_ratio)
                     VALUES (?1, '', '[]', NULL, NULL, 0)",
                    rusqlite::params![name],
                ).ok();
                conn.last_insert_rowid()
            });
            Ok(json!({"id": id}))
        }
        ("db", "frequencyset_delete") => {
            let id = arg_i64("frequencyset_id").ok_or("frequencyset_id required")?;
            db::with_db(state, |conn| { conn.execute("DELETE FROM profiles WHERE id = ?1", [id]).ok(); });
            Ok(json!(true))
        }
        ("db", "frequencyset_duplicate") => {
            let id = arg_i64("frequencyset_id").ok_or("frequencyset_id required")?;
            let new_id = db::with_db(state, |conn| {
                conn.execute(
                    "INSERT INTO profiles (name, description, frequencies, enter_ats, exit_ats, f_ratio)
                     SELECT name || ' copy', description, frequencies, enter_ats, exit_ats, f_ratio
                     FROM profiles WHERE id = ?1",
                    [id],
                ).ok();
                conn.last_insert_rowid()
            });
            Ok(json!({"id": new_id}))
        }
        ("db", "frequencysets_reset") => {
            db::with_db(state, |conn| {
                conn.execute("DELETE FROM profiles", []).ok();
                conn.execute(
                    "INSERT INTO profiles (name, description, frequencies, enter_ats, exit_ats, f_ratio)
                     VALUES ('Default', '', '[[null,null,5658],[null,null,5695],[null,null,5732],[null,null,5769],[null,null,5806],[null,null,5843],[null,null,5880],[null,null,5917]]', NULL, NULL, 0)",
                    [],
                ).ok();
            });
            Ok(json!(true))
        }
        ("db", "raceclass_add") => {
            let name = arg_str("name").unwrap_or_else(|| "Class".to_string());
            let format_id = arg_i64("format_id").unwrap_or(1);
            let id = db::with_db(state, |conn| {
                conn.execute(
                    "INSERT INTO race_class (name, description, format_id, win_condition, results, \"cacheStatus\", ranking, rounds, active)
                     VALUES (?1, '', ?2, 0, NULL, '{}', NULL, 0, 1)",
                    rusqlite::params![name, format_id],
                ).ok();
                conn.last_insert_rowid()
            });
            Ok(json!({"id": id}))
        }
        ("db", "raceclass_alter") => {
            let class_id = arg_i64("class_id").ok_or("class_id required")?;
            let name = arg_str("name");
            db::with_db(state, |conn| {
                if let Some(name) = &name {
                    conn.execute("UPDATE race_class SET name = ?1 WHERE id = ?2",
                                 rusqlite::params![name, class_id]).ok();
                }
            });
            Ok(json!(true))
        }
        ("db", "raceclass_delete") => {
            let class_id = arg_i64("class_id").ok_or("class_id required")?;
            db::with_db(state, |conn| { conn.execute("DELETE FROM race_class WHERE id = ?1", [class_id]).ok(); });
            Ok(json!(true))
        }
        ("db", "raceclass_duplicate") => {
            let class_id = arg_i64("class_id").ok_or("class_id required")?;
            let new_id = db::with_db(state, |conn| {
                conn.execute(
                    "INSERT INTO race_class (name, description, format_id, win_condition, results, \"cacheStatus\", ranking, rounds, active)
                     SELECT name || ' copy', description, format_id, win_condition, NULL, '{}', ranking, rounds, 1
                     FROM race_class WHERE id = ?1",
                    [class_id],
                ).ok();
                conn.last_insert_rowid()
            });
            Ok(json!({"id": new_id}))
        }
        ("db", "raceformat_add") => {
            let source = arg_i64("source_format_id").unwrap_or(1);
            let name = arg_str("name").unwrap_or_else(|| "Format".to_string());
            let new_id = db::with_db(state, |conn| {
                conn.execute(
                    "INSERT INTO race_format (name, race_mode, race_time_sec, lap_grace_sec,
                     staging_fixed_tones, start_delay_min_ms, start_delay_max_ms, staging_tones,
                     number_laps_win, win_condition, team_racing_mode, start_behavior)
                     SELECT ?1, race_mode, race_time_sec, lap_grace_sec, staging_fixed_tones,
                            start_delay_min_ms, start_delay_max_ms, staging_tones, number_laps_win,
                            win_condition, team_racing_mode, start_behavior
                     FROM race_format WHERE id = ?2",
                    rusqlite::params![name, source],
                ).ok();
                conn.last_insert_rowid()
            });
            Ok(json!({"id": new_id}))
        }
        ("db", "raceformat_alter") => {
            let format_id = arg_i64("format_id").ok_or("format_id required")?;
            let name = arg_str("name");
            db::with_db(state, |conn| {
                if let Some(name) = &name {
                    conn.execute("UPDATE race_format SET name = ?1 WHERE id = ?2",
                                 rusqlite::params![name, format_id]).ok();
                }
            });
            Ok(json!(true))
        }
        ("db", "raceformat_delete") => {
            let format_id = arg_i64("format_id").ok_or("format_id required")?;
            db::with_db(state, |conn| { conn.execute("DELETE FROM race_format WHERE id = ?1", [format_id]).ok(); });
            Ok(json!(true))
        }
        ("db", "raceformat_duplicate") => {
            let format_id = arg_i64("format_id").ok_or("format_id required")?;
            let new_id = db::with_db(state, |conn| {
                conn.execute(
                    "INSERT INTO race_format (name, race_mode, race_time_sec, lap_grace_sec,
                     staging_fixed_tones, start_delay_min_ms, start_delay_max_ms, staging_tones,
                     number_laps_win, win_condition, team_racing_mode, start_behavior)
                     SELECT name || ' copy', race_mode, race_time_sec, lap_grace_sec, staging_fixed_tones,
                            start_delay_min_ms, start_delay_max_ms, staging_tones, number_laps_win,
                            win_condition, team_racing_mode, start_behavior
                     FROM race_format WHERE id = ?1",
                    [format_id],
                ).ok();
                conn.last_insert_rowid()
            });
            Ok(json!({"id": new_id}))
        }
        ("db", "race_add") => {
            let heat_id = arg_i64("heat_id").ok_or("heat_id required")?;
            let round_id = arg_i64("round_id").unwrap_or(1);
            let class_id = arg_i64("class_id");
            let format_id = arg_i64("format_id");
            let id = db::with_db(state, |conn| {
                conn.execute(
                    "INSERT INTO saved_race_meta (round_id, heat_id, class_id, format_id, start_time,
                                                  start_time_formatted, results, \"cacheStatus\")
                     VALUES (?1, ?2, ?3, ?4, '', '', NULL, '{}')",
                    rusqlite::params![round_id, heat_id, class_id, format_id],
                ).ok();
                conn.last_insert_rowid()
            });
            Ok(json!({"id": id}))
        }
        ("db", "deleted_race_conflicts") => Ok(json!([])),
        ("db", "deleted_race_data") => {
            let race_id = arg_i64("race_id").ok_or("race_id required")?;
            Ok(db::with_db(state, |conn| {
                conn.query_row("SELECT data FROM deleted_race WHERE race_id = ?1", [race_id],
                    |r| r.get::<_, Option<String>>(0)).ok().flatten()
                    .and_then(|t| serde_json::from_str::<Value>(&t).ok())
                    .unwrap_or(Value::Null)
            }))
        }
        ("db", "saved_lap_splits") => Ok(db::with_db(state, |conn| {
            let mut stmt = conn.prepare(
                "SELECT id, race_id, pilotrace_id, split_time FROM saved_race_lap_split ORDER BY id").unwrap();
            let rows = stmt.query_map([], |r| Ok(json!({
                "id": r.get::<_, i64>(0)?,
                "race_id": r.get::<_, i64>(1)?,
                "pilotrun_id": r.get::<_, Option<i64>>(2)?,
                "split_time": r.get::<_, Option<f64>>(3)?,
            }))).unwrap();
            json!(rows.filter_map(|r| r.ok()).collect::<Vec<Value>>())
        })),

        // ---- hardware interface (read-only view of the seats) ----
        ("interface", "seats") => Ok(json!(state.node_count())),
        ("interface", "mock_signals") => Ok(json!(["none", "random", "carryover"])),
        ("interface", "is_mock_only") => Ok(json!(true)),
        ("interface", "seat_provider_type") => {
            let seat = arg_i64("seat").unwrap_or(0);
            Ok(json!(format!("mock:{}", seat)))
        }
        ("interface", "add") | ("interface", "register_mock_signal")
        | ("interface", "seat_crossing_end") | ("interface", "seat_enter_at_set")
        | ("interface", "seat_exit_at_set") => Ok(json!(true)),

        // ---- ui: field option refresh ----
        ("ui", "update_field_options") => {
            crate::socket::broadcast_payload(state, "ui_field_update",
                args.get("field").cloned().unwrap_or(json!({})));
            Ok(json!(true))
        }
        ("eventresults", "results") => Ok(Value::Null),

        ("db", "slots") => Ok(db::with_db(state, |conn| {
            let mut stmt = conn
                .prepare("SELECT id, heat_id, node_index, pilot_id FROM heat_node ORDER BY id")
                .unwrap();
            let rows = stmt
                .query_map([], |r| Ok(json!({
                    "id": r.get::<_, i64>(0)?,
                    "heat_id": r.get::<_, i64>(1)?,
                    "node_index": r.get::<_, i64>(2)?,
                    "pilot": r.get::<_, Option<i64>>(3)?,
                })))
                .unwrap();
            json!(rows.filter_map(|r| r.ok()).collect::<Vec<Value>>())
        })),
        ("db", "saved_lap_splits_by_pilotrun") => {
            let run_id = arg_i64("pilotrun_id").ok_or("pilotrun_id required")?;
            Ok(db::with_db(state, |conn| {
                let mut stmt = conn.prepare(
                    "SELECT id, race_id, pilotrace_id, split_id, split_time \
                     FROM saved_race_lap_split WHERE pilotrace_id = ?1 ORDER BY id").unwrap();
                let rows = stmt.query_map([run_id], |r| Ok(json!({
                    "id": r.get::<_, i64>(0)?,
                    "race_id": r.get::<_, i64>(1)?,
                    "pilotrun_id": r.get::<_, Option<i64>>(2)?,
                    "split_id": r.get::<_, i64>(3)?,
                    "split_time": r.get::<_, Option<f64>>(4)?,
                }))).unwrap();
                json!(rows.filter_map(|r| r.ok()).collect::<Vec<Value>>())
            }))
        }
        ("db", "raceclass_ranking") => {
            let class_id = arg_i64("class_id").ok_or("class_id required")?;
            Ok(db::with_db(state, |conn| {
                crate::class_rank::calc_class_ranking(conn, class_id).unwrap_or(Value::Null)
            }))
        }
        ("db", "raceclass_results") => {
            let class_id = arg_i64("class_id").ok_or("class_id required")?;
            Ok(db::with_db(state, |conn| {
                crate::class_rank::calc_class_ranking(conn, class_id).unwrap_or(Value::Null)
            }))
        }
        ("db", "pilotrun_add") => {
            let race_id = arg_i64("race_id").ok_or("race_id required")?;
            let node_index = arg_i64("node_index").unwrap_or(0);
            let pilot_id = arg_i64("pilot_id").unwrap_or(0);
            let id = db::with_db(state, |conn| {
                conn.execute(
                    "INSERT INTO saved_pilot_race (race_id, node_index, pilot_id, history_values,
                                                   history_times, enter_at, exit_at)
                     VALUES (?1, ?2, ?3, NULL, NULL, NULL, NULL)",
                    rusqlite::params![race_id, node_index, pilot_id],
                ).ok();
                conn.last_insert_rowid()
            });
            Ok(json!({"id": id}))
        }

        // ---- calibration (autoadjust + suggestions) ----
        ("race", "seat_auto_adjust") => {
            let seat = arg_i64("seat").ok_or("seat required")?;
            let result = crate::autoadjust::auto_adjust_seat(state, seat);
            Ok(json!({"result": result.0, "error": result.1.map(|r| format!("{:?}", r))}))
        }
        ("race", "seat_auto_adjust_undo") => {
            let seat = arg_i64("seat").ok_or("seat required")?;
            let out = crate::autoadjust::undo_auto_adjust_seat(state, seat);
            Ok(json!({"result": out.0, "error": out.1.map(|r| format!("{:?}", r))}))
        }
        ("race", "seat_suggest_calibration") => {
            let seat = arg_i64("seat").ok_or("seat required")?;
            let (suggestion, reason) = crate::calibration::suggest_for_seat(state, seat);
            Ok(match suggestion {
                Some(s) => json!({
                    "enter_at": s.enter_at,
                    "exit_at": s.exit_at,
                    "lap_count": s.lap_count,
                }),
                None => json!({"error": format!("{:?}", reason)}),
            })
        }
        ("db", "pilotrun_auto_adjust") => {
            let run_id = arg_i64("pilotrun_id").ok_or("pilotrun_id required")?;
            let out = crate::autoadjust::auto_adjust_pilotrun(state, run_id);
            Ok(json!({"result": out.0, "error": out.1.map(|r| format!("{:?}", r))}))
        }
        ("db", "pilotrun_auto_adjust_undo") => {
            let run_id = arg_i64("pilotrun_id").ok_or("pilotrun_id required")?;
            let out = crate::autoadjust::undo_auto_adjust_pilotrun(state, run_id);
            Ok(json!({"result": out.0, "error": out.1.map(|r| format!("{:?}", r))}))
        }
        ("db", "pilotrun_suggest_calibration") => {
            let run_id = arg_i64("pilotrun_id").ok_or("pilotrun_id required")?;
            let (suggestion, reason) = crate::calibration::suggest_for_pilotrun(state, run_id);
            Ok(match suggestion {
                Some(s) => json!({
                    "enter_at": s.enter_at,
                    "exit_at": s.exit_at,
                    "lap_count": s.lap_count,
                }),
                None => json!({"error": format!("{:?}", reason)}),
            })
        }

        ("db", "race_restore") => {
            let race_id = arg_i64("race_id").ok_or("race_id required")?;
            let new_id = db::with_db(state, |conn| {
                let row: Option<(i64, i64, Option<i64>, Option<i64>, Option<String>)> = conn
                    .query_row(
                        "SELECT round_id, heat_id, class_id, format_id, start_time_formatted \
                         FROM deleted_race WHERE race_id = ?1",
                        [race_id],
                        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
                    )
                    .ok();
                match row {
                    Some((round_id, heat_id, class_id, format_id, start)) => {
                        conn.execute(
                            "INSERT INTO saved_race_meta (round_id, heat_id, class_id, format_id,
                                                          start_time, start_time_formatted, results, \"cacheStatus\")
                             VALUES (?1, ?2, ?3, ?4, '', ?5, NULL, '{}')",
                            rusqlite::params![round_id, heat_id, class_id, format_id, start],
                        )
                        .ok();
                        let id = conn.last_insert_rowid();
                        conn.execute("DELETE FROM deleted_race WHERE race_id = ?1", [race_id]).ok();
                        id
                    }
                    None => 0,
                }
            });
            Ok(json!({"id": new_id}))
        }

        _ => Err(format!("Unknown RHAPI method: {}.{}", object, method)),
    }
}


/// Reads a stored option (RHAPI db.option) as text.
fn option_value(state: &SharedState, name: &str) -> Option<String> {
    db::with_db(state, |conn| {
        conn.query_row(
            "SELECT option_value FROM global_settings WHERE option_name = ?1",
            [name],
            |r| r.get::<_, String>(0),
        )
        .ok()
    })
}

/// Builds the `ui` payload for one page, mirroring RHUI.emit_ui: plugin
/// panels with their settings, values resolved from config or db options.
pub fn build_ui_payload(state: &SharedState, page: &str) -> Value {
    let panels = state.plugin_panels.lock().unwrap().clone();
    let fields = state.plugin_fields.lock().unwrap().clone();

    let mut out = Vec::new();
    for panel in panels.iter().filter(|p| p.get("page").and_then(|v| v.as_str()) == Some(page)) {
        let panel_name = panel.get("name").and_then(|v| v.as_str()).unwrap_or("");

        let settings: Vec<Value> = fields
            .iter()
            .filter(|entry| entry.get("panel").and_then(|v| v.as_str()) == Some(panel_name))
            .map(|entry| {
                let field = entry.get("field").cloned().unwrap_or(json!({}));
                let name = field.get("name").and_then(|v| v.as_str()).unwrap_or("");
                let section = field.get("section").and_then(|v| v.as_str());
                let field_type = field
                    .get("field_type")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");

                let stored = match section {
                    Some(section) => db::get_config_value(state, section, name),
                    None => option_value(state, name).map(Value::String),
                };

                let mut out = field.clone();
                if let Some(stored) = stored {
                    let value = if field_type == "checkbox" {
                        match &stored {
                            Value::String(text) => {
                                json!(!(text.is_empty() || text == "0" || text == "false"))
                            }
                            Value::Bool(flag) => json!(*flag),
                            Value::Null => json!(false),
                            _ => json!(true),
                        }
                    } else {
                        stored
                    };
                    out["value"] = value;
                }
                out
            })
            .collect();

        out.push(json!({
            "panel": {
                "name": panel_name,
                "label": panel.get("label").cloned().unwrap_or(json!(panel_name)),
                "order": panel.get("order").cloned().unwrap_or(Value::Null),
                "open": panel.get("open").cloned().unwrap_or(json!(false)),
            },
            "settings": settings,
            "fn_binds": [],
            "quickbuttons": [],
            "markdowns": [],
        }));
    }

    json!({"page": page, "panels": out})
}

/// Forwards a client socket event to the bridge and waits for the plugin's
/// return value, which becomes the socket ack. Null when no bridge answers.
pub async fn invoke_plugin(state: &SharedState, event: &str, args: Value) -> Value {
    use std::sync::atomic::Ordering;
    let id = state.plugin_seq.fetch_add(1, Ordering::Relaxed);
    let (tx, rx) = tokio::sync::oneshot::channel();
    state.plugin_pending.lock().unwrap().insert(id, tx);
    crate::socket::broadcast_payload(
        state,
        "plugin_invoke",
        json!({"id": id, "event": event, "args": args}),
    );
    match tokio::time::timeout(std::time::Duration::from_secs(10), rx).await {
        Ok(Ok(value)) => value,
        _ => {
            state.plugin_pending.lock().unwrap().remove(&id);
            Value::Null
        }
    }
}

/// Registers the RPC endpoint on a connected socket (called from the
/// socket.io namespace handler).
pub fn register_rpc(
    socket: &socketioxide::extract::SocketRef,
    state: SharedState,
) {
    use socketioxide::extract::{AckSender, Data};
    let state_for_result = state.clone();
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

    // Reply channel for server -> bridge invocations (plugin_invoke).
    socket.on(
        "plugin_result",
        move |data: Data<Value>| async move {
            let id = data.0.get("id").and_then(|v| v.as_u64());
            if let Some(id) = id {
                let sender = state_for_result.plugin_pending.lock().unwrap().remove(&id);
                if let Some(tx) = sender {
                    let _ = tx.send(data.0.get("result").cloned().unwrap_or(Value::Null));
                }
            }
        },
    );
}
