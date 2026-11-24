//! Event actions — port of EventActions.py + the rh_actions_builtin
//! plugin (Speak / Message / Alert / Clear Messages / Schedule Race).
//!
//! Actions are stored as JSON in config.json USER.actions:
//! `[{"event": "...", "effect": "speak", "text": "..."}]`.  On each
//! hooked event every action matching the event name runs its effect,
//! with %-token substitution from live race data (doReplace in RHData).

use serde_json::{json, Value};
use std::sync::Mutex;

/// Actions loaded from USER.actions (EventActions.loadActions).
pub struct ActionEngine {
    actions: Mutex<Vec<Value>>,
}

impl ActionEngine {
    pub fn new() -> Self {
        ActionEngine { actions: Mutex::new(Vec::new()) }
    }

    /// Reloads the action list from config.json (CONFIG_SET hook).
    pub fn load(&self, data_dir: &str) {
        let setting = std::fs::read_to_string(format!("{}/config.json", data_dir))
            .ok()
            .and_then(|t| serde_json::from_str::<Value>(&t).ok())
            .and_then(|c| c.get("USER").and_then(|u| u.get("actions")).cloned());
        let mut actions = self.actions.lock().unwrap();
        actions.clear();
        match setting {
            Some(v) if v.is_array() => {
                actions.extend(v.as_array().unwrap().iter().cloned());
            }
            Some(v) if v.is_string() => {
                // Python may store the list as a JSON string
                if let Ok(parsed) = serde_json::from_str::<Vec<Value>>(v.as_str().unwrap_or("")) {
                    actions.extend(parsed);
                }
            }
            _ => {
                tracing::debug!("No actions to load");
            }
        }
    }

    /// Runs every action bound to the event (EventActions.doActions).
    pub fn do_actions(&self, state: &crate::state::SharedState, event: &str, args: &Value) {
        let actions: Vec<Value> = self.actions.lock().unwrap().clone();
        for action in actions {
            let action_event = action.get("event").and_then(|v| v.as_str()).unwrap_or("");
            if action_event != event {
                continue;
            }
            let effect = action.get("effect").and_then(|v| v.as_str()).unwrap_or("");
            let result = match effect {
                "speak" => self.effect_speak(state, &action, args),
                "message" => self.effect_message(state, &action, args, false),
                "alert" => self.effect_message(state, &action, args, true),
                "clearMessages" => {
                    crate::socket::broadcast(state, "clear_priority_messages", json!({}));
                    Ok(())
                }
                "schedule" => self.effect_schedule(state, &action),
                other => {
                    tracing::debug!("No action effect registered for '{}'", other);
                    Ok(())
                }
            };
            if let Err(e) = result {
                tracing::error!("Error running action effect '{}': {}", effect, e);
            }
        }
    }

    fn effect_speak(
        &self,
        state: &crate::state::SharedState,
        action: &Value,
        args: &Value,
    ) -> Result<(), String> {
        let raw_text = action.get("text").and_then(|v| v.as_str()).unwrap_or("");
        let mut delay_holder: Option<f64> = None;
        let text = replace_tokens(state, raw_text, args, true, &mut delay_holder)?;
        if text.is_empty() {
            return Ok(());
        }
        let payload = |t: &str| {
            json!({
                "text": t,
                "domain": false,
                "winner_flag": false,
                "priority": false,
                "resume_queue": false,
            })
        };
        match delay_holder {
            None => {
                crate::socket::broadcast(state, "phonetic_text", payload(&text));
            }
            Some(secs) => {
                // %DELAY_N_SECS%: emit the (possibly multi-part) callout
                // after the delay
                let pieces: Vec<String> = if text.contains("%PILOTS_INTERVAL") {
                    text.split("%PILOTS_INTERVAL_SECS%").map(|s| s.to_string()).collect()
                } else {
                    vec![text]
                };
                let state = state.clone();
                tokio::spawn(async move {
                    tokio::time::sleep(std::time::Duration::from_secs_f64(secs.max(0.0))).await;
                    for piece in pieces {
                        crate::socket::broadcast(&state, "phonetic_text", payload(piece.trim()));
                    }
                });
            }
        }
        Ok(())
    }

    fn effect_message(
        &self,
        state: &crate::state::SharedState,
        action: &Value,
        args: &Value,
        interrupt: bool,
    ) -> Result<(), String> {
        let raw_text = action.get("text").and_then(|v| v.as_str()).unwrap_or("");
        let mut no_delay = None;
        let text = replace_tokens(state, raw_text, args, false, &mut no_delay)?;
        if text.is_empty() {
            return Ok(());
        }
        crate::socket::emit_priority_message(state, &text, interrupt);
        Ok(())
    }

    fn effect_schedule(
        &self,
        state: &crate::state::SharedState,
        action: &Value,
    ) -> Result<(), String> {
        let secs = action.get("sec").and_then(|v| v.as_i64()).unwrap_or(0);
        let mins = action.get("min").and_then(|v| v.as_i64()).unwrap_or(0);
        if secs != 0 || mins != 0 {
            let delay = (mins * 60 + secs) as f64;
            {
                let mut race = state.race.lock().unwrap();
                race.scheduled = true;
                race.scheduled_time = crate::state::now_epoch_ms() + delay * 1000.0;
            }
            let payload = {
                let race = state.race.lock().unwrap();
                json!({"scheduled": race.scheduled, "scheduled_at": race.scheduled_time})
            };
            crate::socket::broadcast(state, "race_scheduled", payload);
        }
        Ok(())
    }
}

fn heat_display_name(conn: &rusqlite::Connection, heat_id: Option<i64>, short: bool) -> Option<String> {
    heat_id.and_then(|id| {
        conn.query_row(
            "SELECT COALESCE(NULLIF(note, ''), NULL), auto_name FROM heat WHERE id = ?1",
            [id],
            |r| {
                let note: Option<String> = r.get(0)?;
                let auto_name: Option<String> = r.get(1)?;
                let name = note
                    .clone()
                    .or(auto_name)
                    .unwrap_or_else(|| format!("Heat {}", id));
                if short {
                    // spoken form shortens to the note or bare heat number
                    Ok(note.unwrap_or_else(|| format!("{}", id)))
                } else {
                    Ok(name)
                }
            },
        )
        .ok()
    })
}

/// Pilot display/spoken callsign for the pilot tied to the event args
/// (pilot_id direct, else via the seat in the current heat).
fn pilot_for_args(
    conn: &rusqlite::Connection,
    state: &crate::state::SharedState,
    args: &Value,
    spoken: bool,
) -> Option<(String, String)> {
    let pilot_id = if let Some(pid) = args.get("pilot_id").and_then(|v| v.as_i64()) {
        Some(pid)
    } else {
        let node_index = args.get("node_index").and_then(|v| v.as_i64())? as usize;
        state
            .race
            .lock()
            .unwrap()
            .node_pilots
            .get(node_index)
            .copied()
            .flatten()
    }?;
    conn.query_row(
        "SELECT callsign, COALESCE(NULLIF(phonetic, ''), callsign) FROM pilot WHERE id = ?1",
        [pilot_id],
        |r| {
            let callsign: String = r.get(0)?;
            let phonetic: String = r.get(1)?;
            Ok(if spoken { phonetic } else { callsign.clone() }.pipe(|name| (callsign, name)))
        },
    )
    .ok()
}

trait Pipe: Sized {
    fn pipe<T, F: FnOnce(Self) -> T>(self, f: F) -> T {
        f(self)
    }
}
impl<T> Pipe for T {}

/// Fastest-lap row from the most recent saved race's cached results
/// (callout_results in Python falls back to the last race the same way).
fn last_race_results(conn: &rusqlite::Connection) -> Option<Value> {
    conn.query_row(
        "SELECT results FROM saved_race_meta WHERE results IS NOT NULL ORDER BY id DESC LIMIT 1",
        [],
        |r| r.get::<_, String>(0),
    )
    .ok()
    .and_then(|s| serde_json::from_str(&s).ok())
}

fn fastest_lap_row(results: &Value) -> Option<(String, String, f64)> {
    let board = results.get("by_race_time").and_then(|v| v.as_array())?;
    let mut best: Option<(String, String, f64)> = None;
    for entry in board {
        let laps = entry.get("laps").and_then(|v| v.as_i64()).unwrap_or(0);
        if laps < 2 {
            continue; // no completed-lap timing yet
        }
        if let (Some(cs), Some(fl)) = (
            entry.get("callsign").and_then(|v| v.as_str()),
            entry.get("fastest_lap").and_then(|v| v.as_f64()),
        ) {
            if best.as_ref().map(|(_, _, t)| fl < *t).unwrap_or(true) {
                best = Some((cs.to_string(), format_lap_time(fl), fl));
            }
        }
    }
    best
}

fn format_lap_time(secs: f64) -> String {
    let total_ms = (secs * 1000.0).round() as i64;
    let m = total_ms / 60000;
    let s = (total_ms % 60000) as f64 / 1000.0;
    format!("{}:{:06.3}", m, s)
}

/// %-token substitution (doReplace in RHData.py) for the core token set.
/// Tokens requiring unported subsystems (event-speed, co-op) are left
/// verbatim, as Python leaves unknown tokens.
pub fn replace_tokens(
    state: &crate::state::SharedState,
    text: &str,
    args: &Value,
    spoken: bool,
    delay_holder: &mut Option<f64>,
) -> Result<String, String> {
    if !text.contains('%') {
        return Ok(text.to_string());
    }
    let mut out = text.to_string();

    let replace_result: Result<(), String> = crate::db::with_db(state, |conn| -> Result<(), String> {
        // %HEAT% : current heat display name (short form when spoken)
        if out.contains("%HEAT%") {
            let heat_id = args
                .get("heat_id")
                .and_then(|v| v.as_i64())
                .or(state.race.lock().unwrap().current_heat);
            let name = heat_display_name(conn, heat_id, spoken)
                .unwrap_or_else(|| "None".to_string());
            out = out.replace("%HEAT%", &name);
        }

        // %PILOT% : pilot tied to the event
        if out.contains("%PILOT%") {
            if let Some((callsign, spoken_name)) = pilot_for_args(conn, state, args, spoken) {
                let _ = callsign;
                out = out.replace("%PILOT%", &spoken_name);
            }
        }

        // %ROUND% / %ROUND_CALL% : current round for the heat
        if out.contains("%ROUND") {
            let heat_id = state.race.lock().unwrap().current_heat;
            let round_id = heat_id.map(|h| crate::db::max_round(conn, h));
            let round_str = match round_id {
                Some(r) if r > 0 => Some(r.to_string()),
                _ => None,
            };
            out = out.replace("%ROUND_CALL%", round_str.as_deref().map(|s| format!("Round {}", s)).as_deref().unwrap_or(""));
            out = out.replace("%ROUND%", round_str.as_deref().unwrap_or(""));
        }

        // %RACE_FORMAT% : current race format name
        if out.contains("%RACE_FORMAT%") {
            let format_id = state.race.lock().unwrap().format_id;
            let fmt_name: String = conn
                .query_row("SELECT name FROM race_format WHERE id = ?1", [format_id], |r| r.get(0))
                .unwrap_or_default();
            out = out.replace("%RACE_FORMAT%", &fmt_name);
        }

        // %FASTEST_RACE_LAP% / %FASTEST_RACE_LAP_CALL%
        if out.contains("%FASTEST_RACE_LAP") {
            let fastest = last_race_results(conn).and_then(|r| fastest_lap_row(&r));
            let fastest_str = fastest
                .as_ref()
                .map(|(callsign, lap_time, _)| {
                    if spoken {
                        format!("{}, {}", callsign, lap_time)
                    } else {
                        format!("{} {}", callsign, lap_time)
                    }
                })
                .unwrap_or_default();
            out = out.replace("%FASTEST_RACE_LAP%", &fastest_str);
            let call_str = if fastest_str.is_empty() {
                String::new()
            } else {
                format!("Fastest lap time {}", fastest_str)
            };
            out = out.replace("%FASTEST_RACE_LAP_CALL%", &call_str);
        }

        // %WINNER% / %WINNER_CALL% : winner of the last race
        if out.contains("%WINNER") {
            let winner = last_race_results(conn).and_then(|r| {
                r.get("by_race_time")
                    .and_then(|b| b.as_array())
                    .and_then(|b| b.first())
                    .and_then(|first| first.get("callsign").and_then(|v| v.as_str()).map(String::from))
            });
            let winner_str = winner.unwrap_or_default();
            out = out.replace("%WINNER%", &winner_str);
            let call_str = if winner_str.is_empty() {
                String::new()
            } else {
                format!("Winner is {}", winner_str)
            };
            out = out.replace("%WINNER_CALL%", &call_str);
        }

        // %LEADER_CALL% : current leading pilot, "NAME is leading"
        if out.contains("%LEADER_CALL%") {
            let leader = {
                let race = state.race.lock().unwrap();
                let mut best: Option<(usize, usize)> = None; // (idx, laps)
                for (idx, laps) in race.node_laps.iter().enumerate() {
                    if best.map(|(_, bl)| laps.len() > bl).unwrap_or(true) && !laps.is_empty() {
                        best = Some((idx, laps.len()));
                    }
                }
                best.and_then(|(idx, _)| {
                    race.node_pilots.get(idx).copied().flatten()
                })
            };
            let leader_str = leader.and_then(|pid| {
                conn.query_row(
                    "SELECT callsign FROM pilot WHERE id = ?1",
                    [pid],
                    |r| r.get::<_, String>(0),
                )
                .ok()
            });
            let name_str = leader_str.unwrap_or_default();
            out = out.replace("%LEADER_CALL%", &format!("{} is leading", name_str));
        }

        // %DELAY_#_SECS% : delay the callout by the given seconds
        if out.contains("%DELAY_") {
            if let Some(stripped) = out.strip_prefix("%DELAY_") {
                if let Some(pos) = stripped.find("_SECS%").or_else(|| stripped.find("_SEC%")) {
                    let num_str = &stripped[..pos];
                    if num_str.replace('.', "").chars().all(|c| c.is_ascii_digit()) {
                        *delay_holder = num_str.parse::<f64>().ok();
                        let vlen = if stripped[pos..].starts_with("_SECS%") { 6 } else { 5 };
                        out = stripped[pos + vlen..].trim_start().to_string();
                    }
                }
            }
        }
        Ok(())
    });
    replace_result?;
    Ok(out)
}


