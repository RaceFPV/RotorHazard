//! Race lifecycle: staging, start, lap registration, stop, save, discard.
//! Mirrors RHRace semantics that the protocol tests characterize.

use crate::db;
use crate::payloads;
use crate::state::*;
use serde_json::json;
use socketioxide::SocketIo;

fn emit_to_all(io: &SocketIo, event: &'static str, payload: serde_json::Value) {
    if let Some(ns) = io.of("/") {
        tokio::spawn(async move {
            if let Err(e) = ns.emit(event, &payload).await {
                tracing::warn!("broadcast {} failed: {:?}", event, e);
            }
        });
    }
}

fn broadcast(state: &SharedState, event: &'static str, payload: serde_json::Value) {
    emit_to_all(state.io.get().expect("io initialized"), event, payload);
}

fn emit_race_status(state: &SharedState) {
    let payload = db::with_db(state, |conn| {
        let race = state.race.lock().unwrap();
        payloads::build_race_status(conn, &race)
    });
    broadcast(&state, "race_status", payload);
}

/// Stages a race on the current heat: emits STAGING, then schedules the
/// start (staging tones + random delay) as a background task.
pub fn stage_race(state: &SharedState) {
    // read format params first (DB lock only), then lock race
    let format_id = {
        let race = state.race.lock().unwrap();
        if race.race_status != RACE_STATUS_READY {
            // Python ignores stage requests while a race is active
            return;
        }
        race.format_id
    };
    let tones = db::with_db(state, |conn| {
        conn.query_row(
            "SELECT staging_fixed_tones FROM race_format WHERE id = ?1",
            [format_id],
            |row| row.get::<_, i64>(0),
        )
        .unwrap_or(3)
    });
    let delay_ms = db::with_db(state, |conn| {
        let (lo, hi) = conn
            .query_row(
                "SELECT start_delay_min_ms, start_delay_max_ms FROM race_format WHERE id = ?1",
                [format_id],
                |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)),
            )
            .unwrap_or((500, 3500));
        if hi > lo {
            lo + (fake_rand() % (hi - lo) as u64) as i64
        } else {
            lo
        }
    });
    {
        let mut race = state.race.lock().unwrap();
        if race.race_status != RACE_STATUS_READY {
            return;
        }
        race.race_status = RACE_STATUS_STAGING;
    }

    emit_race_status(state);
    state.vrx_manager.do_race_stage(&json!({}));
    // LED staging effect with the staging-tone timing (stagingTrigger args)
    state.led_manager.trigger(
        "raceStage",
        &json!({
            "pi_staging_at_s": monotonic() + tones as f64,
            "pi_starts_at_s": monotonic() + tones as f64 + delay_ms as f64 / 1000.0,
            "staging_tones": tones,
        }),
    );

    let state = state.clone();
    // staging tones ring at 1-second intervals, then a blocking wait to start
    let start_after_ms = (tones as u64 * 1000) + delay_ms as u64;
    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(start_after_ms)).await;
        let race = state.race.clone();
        let mut race = race.lock().unwrap();
        if race.race_status != RACE_STATUS_STAGING {
            return; // race was cancelled or reset while staging
        }
        race.start_time_monotonic = monotonic();
        race.start_time_formatted = now_formatted_str();
        race.race_status = RACE_STATUS_RACING;
        for laps in &mut race.node_laps {
            laps.clear();
        }
        drop(race);
        // clear any stuck crossing flags on real hardware (Python sends
        // FORCE_END_CROSSING when a race starts)
        if let Some(hw) = state.hardware.lock().unwrap().as_ref() {
            for i in 0..hw.num_nodes() {
                hw.force_end_crossing(i);
            }
        }
        emit_race_status(&state);
        state.led_manager.trigger("raceStart", &json!({}));
        state.led_manager.trigger_idle(RACE_STATUS_RACING);
        state.vrx_manager.do_race_start(&json!({}));
    });

}

/// Simulates a lap crossing on the given node (mock-node debug path).
pub fn simulate_lap(state: &SharedState, node_index: usize) {
    register_lap_at(state, node_index, monotonic());
}

/// Registers a lap on the given node at the given monotonic time, if the
/// race is running (the shared crossing-to-lap path).
pub fn register_lap_at(state: &SharedState, node_index: usize, now: f64) {
    let mut race = state.race.lock().unwrap();
    if race.race_status != RACE_STATUS_RACING {
        return;
    }
    if now < race.start_time_monotonic {
        return; // crossing before race start is rejected
    }
    let start = race.start_time_monotonic;
    if let Some(laps) = race.node_laps.get_mut(node_index) {
        let lap_time = match laps.last() {
            Some(prev) => (now - prev.lap_time_stamp) * 1000.0,
            None => (now - start) * 1000.0,
        };
        laps.push(Lap {
            lap_time_stamp: now,
            lap_time,
            source: 1, // real-time source
            deleted: false,
        });
    }
    drop(race);

    let node_data = payloads::build_node_data(&state.race.lock().unwrap());
    broadcast(&state, "node_data", node_data);
    let laps_payload = db::with_db(state, |conn| {
        let race = state.race.lock().unwrap();
        payloads::build_current_laps(conn, &race)
    });
    broadcast(&state, "current_laps", laps_payload);
    state.led_manager.trigger("raceLapRecorded", &json!({"node_index": node_index}));
    state.vrx_manager.do_race_lap_recorded(&json!({"node_index": node_index}));
}

/// Stops the race (registers no more laps) and moves to DONE.
pub fn stop_race(state: &SharedState) {
    {
        let mut race = state.race.lock().unwrap();
        if race.race_status == RACE_STATUS_RACING {
            race.end_time_monotonic = monotonic();
            race.race_status = RACE_STATUS_DONE;
        }
    }
    emit_race_status(state);
    state.led_manager.trigger("raceStop", &json!({}));
    state.led_manager.trigger_idle(RACE_STATUS_DONE);
    state.vrx_manager.do_race_stop(&json!({}));
}

/// Saves the current race to the database and returns to READY.
pub fn save_laps(state: &SharedState) -> bool {
    // snapshot race data first, then release the race lock before DB work
    let snapshot = {
        let race = state.race.lock().unwrap();
        if race.race_status != RACE_STATUS_DONE || race.current_heat.is_none() {
            return false;
        }
        Some((
            race.current_heat.unwrap(),
            race.format_id,
            race.start_time_monotonic,
            race.start_time_formatted.clone(),
            race.node_laps.clone(),
            race.node_pilots.clone(),
        ))
    };
    let Some((heat_id, format_id, start_ms, start_formatted, laps_snap, pilots_snap)) = snapshot
    else {
        return false;
    };
    let saved = {
        let _ = &laps_snap;

        db::with_db(state, |conn| {
            let max_round = db::max_round(conn, heat_id);
            let class_id: Option<i64> = conn
                .query_row("SELECT class_id FROM heat WHERE id = ?1", [heat_id], |row| row.get(0))
                .ok()
                .flatten();
            conn.execute(
                "INSERT INTO saved_race_meta (round_id, heat_id, class_id, format_id, start_time,
                                             start_time_formatted, results, \"cacheStatus\")
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, NULL, ?7)",
                rusqlite::params![
                    max_round + 1,
                    heat_id,
                    class_id,
                    format_id,
                    start_ms,
                    start_formatted,
                    json!({"data_ver": 0.0, "build_ver": 0.0}).to_string(),
                ],
            )
            .unwrap();
            let race_id = conn.last_insert_rowid();

            let profile_id = db::current_profile_id(conn);
            let freqs = db::profile_frequencies(conn, profile_id);
            let enter_levels: Vec<i64> = {
                let (enter, _): (Option<String>, Option<String>) = conn
                    .query_row(
                        "SELECT enter_ats, exit_ats FROM profiles WHERE id = ?1",
                        [profile_id],
                        |row| Ok((row.get(0)?, row.get(1)?)),
                    )
                    .unwrap_or((None, None));
                enter
                    .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
                    .and_then(|v| v.get("v").cloned())
                    .and_then(|v| v.as_array().cloned())
                    .map(|arr| {
                        arr.iter()
                            .map(|x| x.as_i64().unwrap_or(90))
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_else(|| vec![90; state.num_nodes])
            };

            // snapshot the RSSI history and marshal type for calibration
            // suggestions on saved runs (Python saves node.history_*)
            let history_snap: Vec<(Vec<i64>, Vec<f64>)> = {
                let mock = state.mock.lock().unwrap();
                (0..state.num_nodes)
                    .map(|i| {
                        mock.get(i)
                            .map(|m| (m.history_values.clone(), m.history_times.clone()))
                            .unwrap_or_default()
                    })
                    .collect()
            };
            let marshal_type = {
                let holder = state.mock_provider.lock().unwrap();
                holder.1.marshal_type()
            };

            for node_index in 0..state.num_nodes {
                let freq = freqs.get(node_index).map(|(_, _, f)| *f).unwrap_or(0);
                if freq == 0 {
                    continue; // disabled node: no pilot race
                }
                let pilot_id = pilots_snap[node_index];
                let (hist_values, hist_times) = history_snap
                    .get(node_index)
                    .cloned()
                    .unwrap_or_default();
                conn.execute(
                    "INSERT INTO saved_pilot_race (race_id, node_index, pilot_id, history_values,
                                                   history_times, penalty_time, penalty_desc,
                                                   enter_at, exit_at, frequency, marshal_type)
                     VALUES (?1, ?2, ?3, ?7, ?8, 0, NULL, ?4, ?5, ?6, ?9)",
                    rusqlite::params![
                        race_id,
                        node_index,
                        pilot_id,
                        enter_levels.get(node_index).copied().unwrap_or(90),
                        enter_levels.get(node_index).copied().unwrap_or(80),
                        freq,
                        json!(hist_values).to_string(),
                        json!(hist_times).to_string(),
                        marshal_type,
                    ],
                )
                .unwrap();
                let pilotrace_id = conn.last_insert_rowid();

                let mut lap_id = 0i64;
                for lap in &laps_snap[node_index] {
                    conn.execute(
                        "INSERT INTO saved_race_lap (race_id, pilotrace_id, node_index, pilot_id,
                                                     lap_time_stamp, lap_time, lap_time_formatted,
                                                     source, deleted, peak_rssi)
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, NULL)",
                        rusqlite::params![
                            race_id,
                            pilotrace_id,
                            node_index,
                            pilot_id,
                            lap.lap_time_stamp,
                            lap.lap_time,
                            db::format_time_to_str(lap.lap_time),
                            lap.source,
                            lap.deleted
                        ],
                    )
                    .unwrap();
                    lap_id += 1;
                }
                let _ = lap_id;
            }

            // compute and cache the race leaderboard (matching the Python
            // results cache in saved_race_meta.results)
            let displayname: String = conn
                .query_row(
                    "SELECT COALESCE(note, 'Heat ' || id) FROM heat WHERE id = ?1",
                    [heat_id],
                    |row| row.get(0),
                )
                .unwrap_or_else(|_| format!("Heat {}", heat_id));
            let fmt = crate::results::FormatInfo::from_conn(conn, format_id);
            let consec = db::get_option_int(conn, "consecutivesCount", 3);
            let mut pilots_in = Vec::new();
            for node_index in 0..state.num_nodes {
                let pilot_id = pilots_snap[node_index];
                if pilot_id.is_none() {
                    continue;
                }
                let callsign = pilot_id
                    .and_then(|pid| {
                        conn.query_row("SELECT callsign FROM pilot WHERE id = ?1", [pid], |r| r.get::<_, String>(0))
                            .ok()
                    })
                    .unwrap_or_default();
                let team = pilot_id
                    .and_then(|pid| {
                        conn.query_row("SELECT team FROM pilot WHERE id = ?1", [pid], |r| r.get::<_, String>(0))
                            .ok()
                    });
                let crossings: Vec<(f64, f64)> = laps_snap[node_index]
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
            let leaderboard = crate::results::calc_leaderboard(
                &pilots_in, &fmt, max_round + 1, Some(heat_id), &displayname, consec);
            conn.execute(
                "UPDATE saved_race_meta SET results = ?1, \"cacheStatus\" = ?2 WHERE id = ?3",
                rusqlite::params![
                    leaderboard.to_string(),
                    json!({"data_ver": 1.0, "build_ver": 1.0}).to_string(),
                    race_id
                ],
            )
            .unwrap();
        });
        // release DB before taking the race lock again (lock order)
        true
    };
    if saved {
        let mut race = state.race.lock().unwrap();
        race.clear_laps();
        race.race_status = RACE_STATUS_READY;
    }
    if saved {
        broadcast(&state, "race_saved", json!({}));
        emit_race_status(state);
        // race list broadcast lives in socket.rs to reuse its builder
        crate::socket::broadcast_race_list(state);
        state.led_manager.trigger("lapsClear", &json!({}));
        state.led_manager.trigger_idle(RACE_STATUS_READY);
        state.vrx_manager.do_laps_clear(&json!({}));
    }
    saved
}

/// Discards the current unsaved race and returns to READY.
pub fn discard_laps(state: &SharedState) {
    {
        let mut race = state.race.lock().unwrap();
        race.clear_laps();
        race.race_status = RACE_STATUS_READY;
    }
    emit_race_status(state);
    state.led_manager.trigger("lapsClear", &json!({}));
    state.led_manager.trigger_idle(RACE_STATUS_READY);
    state.vrx_manager.do_laps_clear(&json!({}));
}

/// 'YYYY-MM-DD HH:MM:SS.mmm' of the current instant, matching
/// RHTimeFns.datetimeToFormattedStr over datetime.now().
pub fn now_formatted_str() -> String {
    let now = chrono::Local::now();
    now.format("%Y-%m-%d %H:%M:%S.").to_string()
        + &format!("{:03}", now.timestamp_subsec_millis())
}

/// Tiny non-crypto pseudo-random for start delays (deterministic enough here).
fn fake_rand() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.subsec_nanos() as u64)
        .unwrap_or(0);
    nanos.wrapping_mul(2654435761) >> 16
}

