//! Auto Adjust: applies calibration suggestions to saved pilot runs and to
//! seats of the current race, with undo. Ports the auto-adjust section of
//! Python calibration.py (upstream 4.6).

use crate::calibration::{self, NoSuggestion};
use crate::db;
use crate::state::{CalUndo, SharedState, UndoLap, RACE_STATUS_DONE, RACE_STATUS_RACING};
use serde_json::{json, Value};

/// In a running race, crossings ending this close to the end of the history
/// are left to the node (AUTO_CAL LIVE_SETTLE_S).
pub const AUTO_CAL_LIVE_SETTLE_S: f64 = 3.0;

pub const LAP_SOURCE_MANUAL: i64 = 1;
pub const LAP_SOURCE_RECALC: i64 = 2;
pub const LAP_SOURCE_API: i64 = 4;

/// (result, None) or (None, NoSuggestion reason), as JSON payloads.
pub type Adjust = (Option<Value>, Option<NoSuggestion>);

fn count_active(laps: &[UndoLap]) -> i64 {
    laps.iter().filter(|l| !l.deleted).count() as i64
}

/// Live laps of a seat converted to the undo snapshot form.
fn live_snapshot(live: &[crate::state::Lap]) -> Vec<UndoLap> {
    live.iter()
        .map(|l| UndoLap {
            lap_time_stamp: l.lap_time_stamp,
            lap_time: l.lap_time,
            peak_rssi: l.peak_rssi,
            source: l.source,
            deleted: l.deleted,
        })
        .collect()
}

/// A minimal view of a suggestion lap for merging (from CalLap).
pub struct CalLapLike {
    pub lap_time_stamp: f64,
    pub peak_rssi: Option<i64>,
    pub deleted: bool,
}

/// Merges the suggestion's recalculated laps with the current manual and API
/// laps, sorted by time stamp with recomputed lap times (_replacement_laps).
/// All stamps are in milliseconds since race start (the Python convention);
/// lap times are in ms.
fn replacement_laps(suggested: &[CalLapLike], current: &[UndoLap]) -> Vec<UndoLap> {
    let mut laps: Vec<UndoLap> = suggested
        .iter()
        .map(|l| UndoLap {
            lap_time_stamp: l.lap_time_stamp,
            lap_time: 0.0,
            peak_rssi: l.peak_rssi,
            source: LAP_SOURCE_RECALC,
            deleted: l.deleted,
        })
        .collect();
    for l in current {
        if l.source == LAP_SOURCE_MANUAL || l.source == LAP_SOURCE_API {
            laps.push(UndoLap {
                lap_time_stamp: l.lap_time_stamp,
                lap_time: 0.0,
                peak_rssi: l.peak_rssi,
                source: l.source,
                deleted: l.deleted,
            });
        }
    }
    with_lap_times(&mut laps);
    laps
}

/// Sorts laps by stamp and recomputes lap times in ms (_with_lap_times).
fn with_lap_times(laps: &mut [UndoLap]) {
    laps.sort_by(|a, b| a.lap_time_stamp.partial_cmp(&b.lap_time_stamp).unwrap());
    let mut last: Option<f64> = None;
    for lap in laps.iter_mut() {
        if lap.deleted {
            lap.lap_time = 0.0;
        } else {
            lap.lap_time = match last {
                Some(prev) => lap.lap_time_stamp - prev,
                None => lap.lap_time_stamp,
            };
            last = Some(lap.lap_time_stamp);
        }
    }
}

fn adjust_result(
    enter_at: i64,
    exit_at: i64,
    mode: &str,
    laps: &[UndoLap],
    previous: &[UndoLap],
) -> Value {
    json!({
        "enter_at": enter_at,
        "exit_at": exit_at,
        "mode": mode,
        "lap_count": count_active(laps),
        "laps_added": count_active(laps) - count_active(previous),
        "recovered_laps": [],
    })
}

/// Reads a node's current enter/exit levels from the active profile.
pub fn node_levels(state: &SharedState, seat: usize) -> (i64, i64) {
    db::with_db(state, |conn| {
        let profile_id = db::current_profile_id(conn);
        let read_arr = |col: &str| -> Vec<i64> {
            let raw: Option<String> = conn
                .query_row(
                    &format!("SELECT {} FROM profiles WHERE id = ?1", col),
                    [profile_id],
                    |r| r.get(0),
                )
                .ok();
            raw.and_then(|s| serde_json::from_str::<Value>(&s).ok())
                .and_then(|v| v.get("v").cloned())
                .and_then(|v| v.as_array().cloned())
                .map(|arr| arr.iter().map(|x| x.as_i64().unwrap_or(0)).collect())
                .unwrap_or_default()
        };
        let enter = read_arr("enter_ats");
        let exit = read_arr("exit_ats");
        (enter.get(seat).copied().unwrap_or(90), exit.get(seat).copied().unwrap_or(80))
    })
}

/// Writes a node's levels to the active profile, pushes them to hardware and
/// broadcasts the levels payload (Calibration.set_enter_at_level/_set_levels).
fn set_levels(state: &SharedState, seat: usize, enter_at: i64, exit_at: i64) {
    // ExitAt first when lowering, so ExitAt stays below EnterAt
    let (_, cur_exit) = node_levels(state, seat);
    let mut ops = vec![("exit_ats", exit_at), ("enter_ats", enter_at)];
    if enter_at >= cur_exit {
        ops.reverse();
    }
    db::with_db(state, |conn| {
        let profile_id = db::current_profile_id(conn);
        for (col, val) in ops {
            let raw: Option<String> = conn
                .query_row(
                    &format!("SELECT {} FROM profiles WHERE id = ?1", col),
                    [profile_id],
                    |r| r.get(0),
                )
                .ok();
            let mut arr = raw
                .and_then(|s| serde_json::from_str::<Value>(&s).ok())
                .unwrap_or(json!({"v": []}));
            let v = arr.as_object_mut().unwrap().entry("v").or_insert(json!([]));
            while v.as_array().unwrap().len() <= seat {
                v.as_array_mut().unwrap().push(Value::Null);
            }
            v.as_array_mut().unwrap()[seat] = json!(val);
            let _ = conn.execute(
                &format!("UPDATE profiles SET {} = ?1 WHERE id = ?2", col),
                rusqlite::params![arr.to_string(), profile_id],
            );
        }
    });
    if let Some(hw) = state.hardware.lock().unwrap().as_mut() {
        hw.set_enter_at_level(seat, enter_at);
        hw.set_exit_at_level(seat, exit_at);
    }
    let payload = db::with_db(state, crate::payloads::build_enter_exit_levels);
    crate::socket::broadcast_payload(state, "enter_and_exit_at_levels", payload);
}

/// Sets the levels now, or when the node's current crossing ends
/// (_set_levels_after_crossing). The delayed path polls the crossing flag.
fn set_levels_after_crossing(state: &SharedState, seat: usize, enter_at: i64, exit_at: i64) {
    let crossing = state
        .mock
        .lock()
        .unwrap()
        .get(seat)
        .map(|m| m.crossing_flag)
        .unwrap_or(false);
    if !crossing {
        set_levels(state, seat, enter_at, exit_at);
        return;
    }
    let state = state.clone();
    tokio::spawn(async move {
        let deadline = std::time::Instant::now()
            + std::time::Duration::from_secs_f64(calibration::AUTO_CAL_MAX_CROSSING_S);
        loop {
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            let still = state
                .mock
                .lock()
                .unwrap()
                .get(seat)
                .map(|m| m.crossing_flag)
                .unwrap_or(false);
            if !still || std::time::Instant::now() >= deadline {
                break;
            }
        }
        set_levels(&state, seat, enter_at, exit_at);
    });
}

/// Saves the levels and laps to a saved pilot run, clears the affected result
/// caches and rebuilds the race's cache (_resave_pilotrun). Returns the
/// (race_id, heat_id, class_id) when the pilot run exists, else None.
fn resave_pilotrun(
    state: &SharedState,
    pilotrace_id: i64,
    enter_at: i64,
    exit_at: i64,
    laps: &[UndoLap],
) -> Option<(i64, Option<i64>, Option<i64>)> {
    db::with_db(state, |conn| {
        let row: Option<(i64, Option<i64>, Option<i64>)> = conn
            .query_row(
                "SELECT pr.race_id, m.heat_id, m.class_id
                 FROM saved_pilot_race pr
                 LEFT JOIN saved_race_meta m ON m.id = pr.race_id
                 WHERE pr.id = ?1",
                [pilotrace_id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .ok();
        let Some((race_id, heat_id, class_id)) = row else {
            return None;
        };
        let _ = conn.execute(
            "UPDATE saved_pilot_race SET enter_at = ?1, exit_at = ?2 WHERE id = ?3",
            rusqlite::params![enter_at, exit_at, pilotrace_id],
        );
        let _ = conn.execute(
            "DELETE FROM saved_race_lap WHERE pilotrace_id = ?1",
            [pilotrace_id],
        );
        let (node_index, pilot_id): (i64, Option<i64>) = conn
            .query_row(
                "SELECT node_index, pilot_id FROM saved_pilot_race WHERE id = ?1",
                [pilotrace_id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap_or((0, None));
        for lap in laps {
            let _ = conn.execute(
                "INSERT INTO saved_race_lap (race_id, pilotrace_id, node_index, pilot_id,
                                             lap_time_stamp, lap_time, lap_time_formatted,
                                             source, deleted, peak_rssi)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                rusqlite::params![
                    race_id,
                    pilotrace_id,
                    node_index,
                    pilot_id,
                    lap.lap_time_stamp,
                    lap.lap_time,
                    db::format_time_to_str(lap.lap_time),
                    lap.source,
                    lap.deleted,
                    lap.peak_rssi,
                ],
            );
        }
        // clear the caches that depend on this run
        let _ = conn.execute(
            "UPDATE saved_race_meta SET results = NULL, \"cacheStatus\" = NULL WHERE id = ?1",
            [race_id],
        );
        if let Some(h) = heat_id {
            let _ = conn.execute(
                "UPDATE heat SET results = NULL, \"cacheStatus\" = NULL WHERE id = ?1",
                [h],
            );
        }
        if let Some(c) = class_id {
            let _ = conn.execute(
                "UPDATE race_class SET results = NULL, \"cacheStatus\" = NULL WHERE id = ?1",
                [c],
            );
        }
        Some((race_id, heat_id, class_id))
    })
}

/// Rebuilds the saved race's results cache and emits result data.
fn rebuild_results(state: &SharedState, race_id: i64) {
    db::with_db(state, |conn| {
        let row: Option<(i64, Option<i64>, Option<i64>, i64)> = conn
            .query_row(
                "SELECT format_id, heat_id, class_id, round_id FROM saved_race_meta WHERE id = ?1",
                [race_id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .ok();
        let Some((format_id, heat_id, _class_id, round_id)) = row else {
            return;
        };
        let displayname: String = heat_id
            .and_then(|h| {
                conn.query_row(
                    "SELECT COALESCE(note, 'Heat ' || id) FROM heat WHERE id = ?1",
                    [h],
                    |r| r.get(0),
                )
                .ok()
            })
            .unwrap_or_else(|| "Practice".to_string());
        let fmt = crate::results::FormatInfo::from_conn(conn, format_id);
        let consec = db::get_option_int(conn, "consecutivesCount", 3);
        let mut pilots_in = Vec::new();
        let mut pr_stmt = conn
            .prepare(
                "SELECT node_index, pilot_id FROM saved_pilot_race
                 WHERE race_id = ?1 ORDER BY node_index",
            )
            .unwrap();
        let mut prs = pr_stmt.query([race_id]).unwrap();
        while let Some(pr) = prs.next().unwrap() {
            let node_index: i64 = pr.get(0).unwrap();
            let pilot_id: Option<i64> = pr.get(1).unwrap();
            let Some(pilot_id) = pilot_id else { continue };
            let Ok((callsign, team)) = conn.query_row(
                "SELECT callsign, team FROM pilot WHERE id = ?1",
                [pilot_id],
                |r| Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?)),
            ) else {
                continue;
            };
            let mut crossings = Vec::new();
            if let Ok(mut lstmt) = conn.prepare(
                "SELECT lap_time, lap_time_stamp FROM saved_race_lap
                 WHERE pilotrace_id = (SELECT id FROM saved_pilot_race
                                       WHERE race_id = ?1 AND node_index = ?2)
                 AND deleted != 1",
            ) {
                if let Ok(mut lrows) = lstmt.query(rusqlite::params![race_id, node_index]) {
                    while let Some(lr) = lrows.next().unwrap() {
                        crossings.push((lr.get::<_, f64>(0).unwrap(), lr.get::<_, f64>(1).unwrap()));
                    }
                }
            }
            pilots_in.push(crate::results::PilotLaps {
                pilot_id: Some(pilot_id),
                callsign,
                team_name: team,
                node: node_index as usize,
                crossings,
            });
        }
        let leaderboard = crate::results::calc_leaderboard(
            &pilots_in,
            &fmt,
            round_id,
            heat_id,
            &displayname,
            consec,
        );
        let _ = conn.execute(
            "UPDATE saved_race_meta SET results = ?1, \"cacheStatus\" = ?2 WHERE id = ?3",
            rusqlite::params![
                leaderboard.to_string(),
                json!({"data_ver": 1.0, "build_ver": 1.0}).to_string(),
                race_id
            ],
        );
    });
    crate::socket::broadcast_race_list(state);
}

/// The heat display name, round and callsign for the follow-up message.
fn run_label_parts(state: &SharedState, pilotrace_id: i64) -> (String, i64, String) {
    db::with_db(state, |conn| {
        let row: Option<(Option<i64>, i64, Option<i64>)> = conn
            .query_row(
                "SELECT m.heat_id, m.round_id, pr.pilot_id
                 FROM saved_pilot_race pr LEFT JOIN saved_race_meta m ON m.id = pr.race_id
                 WHERE pr.id = ?1",
                [pilotrace_id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .ok();
        match row {
            Some((heat_id, round_id, pilot_id)) => {
                let heat_name = heat_id
                    .and_then(|h| {
                        conn.query_row(
                            "SELECT COALESCE(note, 'Heat ' || id) FROM heat WHERE id = ?1",
                            [h],
                            |r| r.get::<_, String>(0),
                        )
                        .ok()
                    })
                    .unwrap_or_else(|| "Practice".to_string());
                let callsign = pilot_id
                    .and_then(|p| {
                        conn.query_row(
                            "SELECT callsign FROM pilot WHERE id = ?1",
                            [p],
                            |r| r.get::<_, String>(0),
                        )
                        .ok()
                    })
                    .unwrap_or_default();
                (heat_name, round_id, callsign)
            }
            None => ("Practice".to_string(), 0, String::new()),
        }
    })
}

fn read_saved_laps(conn: &rusqlite::Connection, pilotrace_id: i64) -> Vec<UndoLap> {
    let mut laps = Vec::new();
    if let Ok(mut stmt) = conn.prepare(
        "SELECT lap_time_stamp, lap_time, peak_rssi, source, deleted
         FROM saved_race_lap WHERE pilotrace_id = ?1 ORDER BY lap_time_stamp",
    ) {
        if let Ok(mut rows) = stmt.query([pilotrace_id]) {
            while let Some(r) = rows.next().unwrap() {
                laps.push(UndoLap {
                    lap_time_stamp: r.get::<_, f64>(0).unwrap_or(0.0),
                    lap_time: r.get::<_, f64>(1).unwrap_or(0.0),
                    peak_rssi: r.get::<_, Option<i64>>(2).unwrap_or(None),
                    source: r.get::<_, i64>(3).unwrap_or(0),
                    deleted: r.get::<_, bool>(4).unwrap_or(false),
                });
            }
        }
    }
    laps
}

// ---------------------------------------------------------------- pilot runs

/// Applies the suggested EnterAt/ExitAt and recalculated laps to a saved pilot
/// run, keeping its manual and API laps (auto_adjust_pilotrun).
pub fn auto_adjust_pilotrun(state: &SharedState, pilotrace_id: i64) -> Adjust {
    let (suggestion, reason) = calibration::suggest_for_pilotrun(state, pilotrace_id);
    let Some(suggestion) = suggestion else {
        return (None, reason);
    };
    let (current_laps, node_index) = db::with_db(state, |conn| {
        let ni: Option<i64> = conn
            .query_row(
                "SELECT node_index FROM saved_pilot_race WHERE id = ?1",
                [pilotrace_id],
                |r| r.get(0),
            )
            .ok();
        (read_saved_laps(conn, pilotrace_id), ni)
    });
    let undo_laps = current_laps.clone();
    let suggested: Vec<CalLapLike> = suggestion
        .laps
        .iter()
        .map(|l| CalLapLike {
            lap_time_stamp: l.lap_time_stamp,
            peak_rssi: l.peak_rssi,
            deleted: l.deleted,
        })
        .collect();
    let new_laps = replacement_laps(&suggested, &current_laps);
    let enter_at = suggestion.enter_at;
    let exit_at = suggestion.exit_at;

    // node levels to undo / restore, when the node is in range
    let node_undo = node_index
        .filter(|ni| *ni >= 0 && (*ni as usize) < state.node_count())
        .map(|ni| {
            let (e, x) = node_levels(state, ni as usize);
            (ni as usize, e, x)
        });

    let Some((race_id, _, _)) = resave_pilotrun(state, pilotrace_id, enter_at, exit_at, &new_laps)
    else {
        return (None, Some(NoSuggestion::NotFound));
    };
    if let Some((ni, _, _)) = node_undo {
        set_levels_after_crossing(state, ni, enter_at, exit_at);
        tracing::info!(
            "Auto adjusted node {}: EnterAt {}, ExitAt {} (from saved run {})",
            ni + 1, enter_at, exit_at, pilotrace_id
        );
    }
    state.cal_undo.lock().unwrap().insert(
        format!("run:{}", pilotrace_id),
        CalUndo::Run { enter_at, exit_at, laps: undo_laps.clone(), node: node_undo },
    );
    rebuild_results(state, race_id);

    let (heat_name, round_id, callsign) = run_label_parts(state, pilotrace_id);
    let message = format!(
        "EnterAt/ExitAt adjusted for {} Round {} / {}: EnterAt {}, ExitAt {}",
        heat_name, round_id, callsign, enter_at, exit_at
    );
    crate::socket::emit_priority_message(state, &message, false);
    if db::get_config_number(state, "TIMING", "calibrationMode", 0.0) != 0.0 {
        calibration::auto_calibrate(state);
    }
    (Some(adjust_result(enter_at, exit_at, "replace", &new_laps, &undo_laps)), None)
}

/// Restores a saved pilot run to before its last auto adjustment
/// (undo_auto_adjust_pilotrun).
pub fn undo_auto_adjust_pilotrun(state: &SharedState, pilotrace_id: i64) -> Adjust {
    let undo = state
        .cal_undo
        .lock()
        .unwrap()
        .remove(&format!("run:{}", pilotrace_id));
    let Some(CalUndo::Run { enter_at, exit_at, laps, node }) = undo else {
        return (None, Some(NoSuggestion::NothingToDo));
    };
    let previous_count = db::with_db(state, |conn| {
        count_active(&read_saved_laps(conn, pilotrace_id))
    });
    let node_in_range = node.map(|(ni, _, _)| ni < state.node_count()).unwrap_or(false);
    if node.is_some() && !node_in_range {
        tracing::warn!(
            "Undo of auto adjust of saved run {}: node out of range; node levels not restored",
            pilotrace_id
        );
    }
    let Some((race_id, _, _)) = resave_pilotrun(state, pilotrace_id, enter_at, exit_at, &laps)
    else {
        return (None, Some(NoSuggestion::NotFound));
    };
    if let Some((ni, e, x)) = node {
        if node_in_range {
            set_levels_after_crossing(state, ni, e, x);
            tracing::info!(
                "Undid auto adjustment of node {}: EnterAt {}, ExitAt {} (from saved run {})",
                ni + 1, e, x, pilotrace_id
            );
        }
    }
    rebuild_results(state, race_id);

    let (heat_name, round_id, callsign) = run_label_parts(state, pilotrace_id);
    let (msg_e, msg_x) = node
        .filter(|_| node_in_range)
        .map(|(_, e, x)| (e, x))
        .unwrap_or((enter_at, exit_at));
    let message = format!(
        "EnterAt/ExitAt restored for {} Round {} / {}: EnterAt {}, ExitAt {}",
        heat_name, round_id, callsign, msg_e, msg_x
    );
    crate::socket::emit_priority_message(state, &message, false);

    (Some(json!({
        "enter_at": enter_at,
        "exit_at": exit_at,
        "mode": "replace",
        "lap_count": count_active(&laps),
        "laps_added": count_active(&laps) - previous_count,
        "recovered_laps": [],
    })), None)
}

// ------------------------------------------------------------- current seats

/// Applies the suggested EnterAt/ExitAt to a seat of the current race
/// (auto_adjust_seat). In a running race only passes the seat has no lap for
/// are added; in a stopped race the laps are replaced.
pub fn auto_adjust_seat(state: &SharedState, seat_index: i64) -> Adjust {
    let status = state.race.lock().unwrap().race_status;
    if status != RACE_STATUS_RACING && status != RACE_STATUS_DONE {
        return (None, Some(NoSuggestion::NotRacing));
    }
    let (suggestion, reason) = calibration::suggest_for_seat(state, seat_index);
    let Some(suggestion) = suggestion else {
        return (None, reason);
    };
    let seat = seat_index as usize;
    let (enter_at, exit_at) = (suggestion.enter_at, suggestion.exit_at);
    let (start_time, live_laps) = {
        let race = state.race.lock().unwrap();
        (race.start_time_monotonic, race.node_laps.get(seat).cloned().unwrap_or_default())
    };
    let snapshot = live_snapshot(&live_laps);

    if status == RACE_STATUS_RACING {
        // recover mode: insert the passes the seat has no lap for
        let history_times = state
            .mock
            .lock()
            .unwrap()
            .get(seat)
            .map(|m| m.history_times.clone())
            .unwrap_or_default();
        let stamps = recover_laps(state, seat, &suggestion.laps, &history_times);
        if stamps.is_empty() {
            tracing::info!(
                "Auto adjust of node {}: no missed lap to recover; levels unchanged",
                seat + 1
            );
            return (None, Some(NoSuggestion::NoMissedLap));
        }
        set_levels_after_crossing(state, seat, enter_at, exit_at);
        let (now_active, previous_count) = {
            let race = state.race.lock().unwrap();
            let active = race
                .node_laps
                .get(seat)
                .map(|l| l.iter().filter(|x| !x.deleted).count())
                .unwrap_or(0) as i64;
            (active, count_active(&snapshot))
        };
        let recovered = recovered_laps(state, seat, &stamps);
        state.cal_undo.lock().unwrap().insert(
            format!("seat:{}", seat),
            CalUndo::Seat {
                start_time,
                enter_at,
                exit_at,
                stamps: Some(stamps),
                laps: None,
            },
        );
        tracing::info!(
            "Auto adjusted node {}: EnterAt {}, ExitAt {}, {} laps added (recover)",
            seat + 1, enter_at, exit_at, now_active - previous_count
        );
        (Some(json!({
            "enter_at": enter_at,
            "exit_at": exit_at,
            "mode": "recover",
            "lap_count": now_active,
            "laps_added": now_active - previous_count,
            "recovered_laps": recovered,
        })), None)
    } else {
        // stopped race: replace the laps
        let suggested: Vec<CalLapLike> = suggestion
            .laps
            .iter()
            .map(|l| CalLapLike {
                lap_time_stamp: l.lap_time_stamp,
                peak_rssi: l.peak_rssi,
                deleted: l.deleted,
            })
            .collect();
        let new_laps = replacement_laps(&suggested, &snapshot);
        set_levels(state, seat, enter_at, exit_at);
        replace_live_laps(state, seat, &new_laps);
        state.cal_undo.lock().unwrap().insert(
            format!("seat:{}", seat),
            CalUndo::Seat {
                start_time,
                enter_at,
                exit_at,
                stamps: None,
                laps: Some(snapshot.clone()),
            },
        );
        let result = adjust_result(enter_at, exit_at, "replace", &new_laps, &snapshot);
        tracing::info!(
            "Auto adjusted node {}: EnterAt {}, ExitAt {}, {} laps added (replace)",
            seat + 1, enter_at, exit_at, result["laps_added"]
        );
        (Some(result), None)
    }
}

/// Restores a seat of the current race to before its last auto adjustment,
/// keeping laps recorded since (undo_auto_adjust_seat).
pub fn undo_auto_adjust_seat(state: &SharedState, seat_index: i64) -> Adjust {
    let key = format!("seat:{}", seat_index);
    let undo = state.cal_undo.lock().unwrap().get(&key).cloned();
    let Some(CalUndo::Seat { start_time, enter_at, exit_at, stamps, laps }) = undo else {
        return (None, Some(NoSuggestion::NothingToDo));
    };
    let (status, now_start) = {
        let race = state.race.lock().unwrap();
        (race.race_status, race.start_time_monotonic)
    };
    if now_start != start_time || (status != RACE_STATUS_RACING && status != RACE_STATUS_DONE) {
        return (None, Some(NoSuggestion::NothingToDo));
    }
    state.cal_undo.lock().unwrap().remove(&key);
    let seat = seat_index as usize;
    let previous_count = state
        .race
        .lock()
        .unwrap()
        .node_laps
        .get(seat)
        .map(|l| l.iter().filter(|x| !x.deleted).count())
        .unwrap_or(0) as i64;
    let mode;
    if let Some(stamps) = &stamps {
        // recover mode: delete the recovered laps by stamp, keep later laps
        mode = "recover";
        let indexes: Vec<usize> = state
            .race
            .lock()
            .unwrap()
            .node_laps
            .get(seat)
            .map(|laps| {
                laps.iter()
                    .enumerate()
                    .filter(|(_, l)| {
                        l.source == LAP_SOURCE_RECALC
                            && !l.deleted
                            && stamps.contains(&l.lap_time_stamp)
                    })
                    .map(|(i, _)| i)
                    .collect()
            })
            .unwrap_or_default();
        for idx in indexes.iter().rev() {
            delete_live_lap(state, seat, *idx);
        }
        set_levels_after_crossing(state, seat, enter_at, exit_at);
    } else {
        mode = "replace";
        set_levels(state, seat, enter_at, exit_at);
        if let Some(laps) = &laps {
            replace_live_laps(state, seat, laps);
        }
    }
    let now_count = state
        .race
        .lock()
        .unwrap()
        .node_laps
        .get(seat)
        .map(|l| l.iter().filter(|x| !x.deleted).count())
        .unwrap_or(0) as i64;
    tracing::info!(
        "Undid auto adjustment of node {}: EnterAt {}, ExitAt {}",
        seat + 1, enter_at, exit_at
    );
    (Some(json!({
        "enter_at": enter_at,
        "exit_at": exit_at,
        "mode": mode,
        "lap_count": now_count,
        "laps_added": now_count - previous_count,
        "recovered_laps": [],
    })), None)
}

/// Inserts the suggested passes the seat has no lap for; returns their time
/// stamps in ms (_recover_laps). All stamps are milliseconds since race
/// start, matching the Python convention.
fn recover_laps(
    state: &SharedState,
    seat: usize,
    suggested: &[calibration::CalLap],
    history_times: &[f64],
) -> Vec<f64> {
    let min_lap_ms =
        db::with_db(state, |conn| db::get_option_int(conn, "MinLapSec", 10)) as f64 * 1000.0;
    let min_lap_behavior = db::get_config_number(state, "TIMING", "MinLapBehavior", 1.0) != 0.0;
    let settled_end = history_times
        .last()
        .map(|t| t - AUTO_CAL_LIVE_SETTLE_S)
        .unwrap_or(f64::NEG_INFINITY);
    let match_ms = calibration::AUTO_CAL_HOLESHOT_MATCH_S * 1000.0;
    let live: Vec<(f64, bool)> = state
        .race
        .lock()
        .unwrap()
        .node_laps
        .get(seat)
        .map(|laps| laps.iter().map(|l| (l.lap_time_stamp, l.deleted)).collect())
        .unwrap_or_default();
    let mut missed: Vec<&calibration::CalLap> = Vec::new();
    for lap in suggested {
        let stamp = lap.lap_time_stamp;
        if lap.deleted || lap.open || lap.crossing_end > settled_end {
            continue;
        }
        // a lap near it, deleted or not, is the same pass
        if live.iter().any(|(s, _)| (s - stamp).abs() <= match_ms) {
            continue;
        }
        if min_lap_behavior
            && live.iter().any(|(s, del)| !del && (s - stamp).abs() < min_lap_ms)
        {
            continue;
        }
        if min_lap_behavior
            && missed.iter().any(|m| (m.lap_time_stamp - stamp).abs() < min_lap_ms)
        {
            continue;
        }
        missed.push(lap);
    }
    let mut inserted = Vec::new();
    for (count, lap) in missed.iter().enumerate() {
        let last = count + 1 == missed.len();
        if crate::race::insert_lap(
            state,
            seat,
            lap.lap_time_stamp,
            LAP_SOURCE_RECALC,
            lap.peak_rssi,
            last,
        )
        .is_some()
        {
            inserted.push(lap.lap_time_stamp);
        }
    }
    inserted
}

/// The inserted laps still present, in time stamp order, numbered as the Run
/// page shows them (_recovered_laps).
fn recovered_laps(state: &SharedState, seat: usize, stamps_ms: &[f64]) -> Vec<Value> {
    let start_behavior_first = db::with_db(state, |conn| {
        let fid = state.race.lock().unwrap().format_id;
        conn.query_row(
            "SELECT start_behavior FROM race_format WHERE id = ?1",
            [fid],
            |r| r.get::<_, i64>(0),
        )
        .map(|v| v == 1)
        .unwrap_or(false)
    });
    let mut laps = {
        let race = state.race.lock().unwrap();
        race.node_laps
                .get(seat)
                .map(|laps| {
                    laps.iter()
                        .filter(|l| !l.deleted && stamps_ms.contains(&l.lap_time_stamp))
                        .cloned()
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default()
    };
    laps.sort_by(|a, b| a.lap_time_stamp.partial_cmp(&b.lap_time_stamp).unwrap());
    laps.iter()
        .enumerate()
        .map(|(idx, lap)| {
            let lap_number = if start_behavior_first { idx + 1 } else { idx };
            json!({
                "lap_number": lap_number,
                "lap_time": lap.lap_time,
                "lap_time_formatted": db::format_time_to_str(lap.lap_time),
                "lap_time_stamp": lap.lap_time_stamp,
            })
        })
        .collect()
}

/// Replaces a seat's live laps with the given set and recomputes lap times
/// (race.replace_laps). Stamps and lap times in ms since race start.
fn replace_live_laps(state: &SharedState, seat: usize, laps: &[UndoLap]) {
    let mut new_laps: Vec<crate::state::Lap> = laps
        .iter()
        .map(|l| crate::state::Lap {
            lap_time_stamp: l.lap_time_stamp,
            lap_time: 0.0,
            source: l.source,
            deleted: l.deleted,
            peak_rssi: l.peak_rssi,
        })
        .collect();
    let mut last: Option<f64> = None;
    for lap in new_laps.iter_mut() {
        if lap.deleted {
            lap.lap_time = 0.0;
        } else {
            lap.lap_time = match last {
                Some(prev) => lap.lap_time_stamp - prev,
                None => lap.lap_time_stamp,
            };
            last = Some(lap.lap_time_stamp);
        }
    }
    {
        let mut race = state.race.lock().unwrap();
        if let Some(slot) = race.node_laps.get_mut(seat) {
            *slot = new_laps;
        }
    }
    refresh_seat_clients(state, seat);
}

/// Deletes a live lap by index and refreshes clients (race.delete_lap).
fn delete_live_lap(state: &SharedState, seat: usize, lap_index: usize) {
    {
        let mut race = state.race.lock().unwrap();
        if let Some(laps) = race.node_laps.get_mut(seat) {
            if let Some(lap) = laps.get_mut(lap_index) {
                lap.deleted = true;
            }
        }
    }
    refresh_seat_clients(state, seat);
}

/// Pushes node/lap/leaderboard data after a live-lap change.
fn refresh_seat_clients(state: &SharedState, _seat: usize) {
    let node_data = crate::payloads::build_node_data(&state.race.lock().unwrap());
    crate::socket::broadcast_payload(state, "node_data", node_data);
    let laps_payload = db::with_db(state, |conn| {
        let race = state.race.lock().unwrap();
        crate::payloads::build_current_laps(conn, &race)
    });
    crate::socket::broadcast_payload(state, "current_laps", laps_payload);
    let leaderboard = crate::socket::build_current_leaderboard_for(state);
    crate::socket::broadcast_payload(state, "leaderboard", leaderboard);
}
