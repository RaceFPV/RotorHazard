//! Protocol tests for the Auto Adjust feature (upstream calibration.py
//! auto-adjust section): socket handlers 'auto_adjust_laps' and
//! 'undo_auto_adjust', plus the 'set_profile' handler that re-sends cluster
//! frequencies.

mod common;

use common::*;
use serde_json::{json, Value};
use std::time::Duration;

const T: Duration = Duration::from_secs(15);

fn spawn_default() -> ServerHandle {
    spawn_server()
}

fn laps_auto_adjusted(client: &SocketClient, body: Value) -> Value {
    client.emit("auto_adjust_laps", &body);
    client.wait_for("laps_auto_adjusted", T)
}

fn undo_auto_adjusted(client: &SocketClient, body: Value) -> Value {
    client.emit("undo_auto_adjust", &body);
    client.wait_for("laps_auto_adjusted", T)
}

#[test]
fn test_auto_adjust_seat_rejected_when_not_racing() {
    let mut server = spawn_default();
    let mut client = SocketClient::connect(&server.base_url);
    std::thread::sleep(Duration::from_millis(300));
    client.drain();
    let resp = laps_auto_adjusted(&client, json!({"node": 0}));
    assert_eq!(resp["action"], "adjusted");
    assert!(resp["enter_at"].is_null(), "enter_at must be null on failure");
    assert_eq!(resp["reason"], "not_racing");
    assert_eq!(resp["node_index"], 0);
    client.disconnect();
}

#[test]
fn test_undo_auto_adjust_nothing_to_undo() {
    let mut server = spawn_default();
    let mut client = SocketClient::connect(&server.base_url);
    std::thread::sleep(Duration::from_millis(300));
    client.drain();
    // seat path with no prior adjustment
    let resp = undo_auto_adjusted(&client, json!({"node": 0}));
    assert_eq!(resp["action"], "undone");
    assert!(resp["enter_at"].is_null());
    assert_eq!(resp["reason"], "nothing_to_undo");
    // pilot run path with a nonexistent id
    let resp = undo_auto_adjusted(&client, json!({"pilotrace_id": 99999}));
    assert_eq!(resp["action"], "undone");
    assert!(resp["enter_at"].is_null());
    assert_eq!(resp["reason"], "nothing_to_undo");
    client.disconnect();
}

#[test]
fn test_auto_adjust_pilotrun_unknown_id() {
    let mut server = spawn_default();
    let mut client = SocketClient::connect(&server.base_url);
    std::thread::sleep(Duration::from_millis(300));
    client.drain();
    let resp = laps_auto_adjusted(&client, json!({"pilotrace_id": 99999}));
    assert_eq!(resp["action"], "adjusted");
    assert!(resp["enter_at"].is_null());
    assert!(resp["pilotrace_id"].is_number());
    // reason is a known NoSuggestion value
    let reason = resp["reason"].as_str().unwrap_or("");
    assert!(
        ["not_found", "no_history", "pass_peak_only", "weak_signal", "no_gate_passes", "no_clean_exit"]
            .contains(&reason),
        "unexpected reason: {}",
        reason
    );
    client.disconnect();
}

#[test]
fn test_auto_adjust_bad_seat_index() {
    let mut server = spawn_default();
    let mut client = SocketClient::connect(&server.base_url);
    std::thread::sleep(Duration::from_millis(300));
    client.drain();
    let resp = laps_auto_adjusted(&client, json!({"node": 999}));
    assert!(resp["enter_at"].is_null());
    assert_eq!(resp["reason"], "not_found");
    client.disconnect();
}

#[test]
fn test_auto_adjust_seat_with_no_history() {
    // a stopped race with no RSSI history yields a suggestion failure with a
    // known reason (no_history), not a crash or a bogus success
    let mut server = spawn_default();
    let mut client = SocketClient::connect(&server.base_url);
    std::thread::sleep(Duration::from_millis(300));
    client.drain();
    client.emit("stage_race", &json!(null));
    // wait for RACING (status 1) before stopping
    let deadline = std::time::Instant::now() + T;
    loop {
        let v = client.wait_for("race_status", T);
        if v.get("race_status").and_then(|x| x.as_i64()) == Some(1) {
            break;
        }
        if std::time::Instant::now() > deadline {
            panic!("timed out waiting for RACING status");
        }
    }
    client.emit("stop_race", &json!(null));
    // wait for DONE (status 2)
    let deadline = std::time::Instant::now() + T;
    loop {
        let v = client.wait_for("race_status", T);
        if v.get("race_status").and_then(|x| x.as_i64()) == Some(2) {
            break;
        }
        if std::time::Instant::now() > deadline {
            panic!("timed out waiting for DONE status");
        }
    }
    let resp = laps_auto_adjusted(&client, json!({"node": 0}));
    assert_eq!(resp["action"], "adjusted");
    assert!(resp["enter_at"].is_null());
    let reason = resp["reason"].as_str().unwrap_or("");
    assert!(
        ["no_history", "pass_peak_only", "weak_signal", "no_gate_passes", "no_clean_exit"]
            .contains(&reason),
        "unexpected reason: {}",
        reason
    );
    client.disconnect();
}

// ---- set_profile ----

#[test]
fn test_set_profile_updates_current_profile() {
    let mut server = spawn_default();
    let mut client = SocketClient::connect(&server.base_url);
    std::thread::sleep(Duration::from_millis(300));
    client.drain();

    // find another profile to switch to
    client.emit("load_data", &json!({"load_types": ["node_tuning"]}));
    let resp = client.wait_for("node_tuning", T);
    let ids = resp["profile_ids"].as_array().cloned().unwrap_or_default();
    assert!(!ids.is_empty(), "there must be at least one profile");
    let current = resp["current_profile"].as_i64().unwrap();
    let other = ids.iter().map(|v| v.as_i64().unwrap_or(0)).find(|i| *i != current);
    let Some(other) = other else {
        // only one profile seeded: switching to the same profile is still valid
        client.emit("set_profile", &json!({"profile": current}));
        std::thread::sleep(Duration::from_millis(300));
        client.emit("load_data", &json!({"load_types": ["node_tuning"]}));
        let resp = client.wait_for("node_tuning", T);
        assert_eq!(resp["current_profile"], json!(current));
        client.disconnect();
        return;
    };

    client.emit("set_profile", &json!({"profile": other}));
    std::thread::sleep(Duration::from_millis(500));
    client.emit("load_data", &json!({"load_types": ["node_tuning"]}));
    let resp = client.wait_for("node_tuning", T);
    assert_eq!(resp["current_profile"], json!(other));

    // switching back must work too
    client.emit("set_profile", &json!({"profile": current}));
    std::thread::sleep(Duration::from_millis(500));
    client.emit("load_data", &json!({"load_types": ["node_tuning"]}));
    let resp = client.wait_for("node_tuning", T);
    assert_eq!(resp["current_profile"], json!(current));
    client.disconnect();
}

#[test]
fn test_set_profile_emits_frequency_data() {
    let mut server = spawn_default();
    let mut client = SocketClient::connect(&server.base_url);
    std::thread::sleep(Duration::from_millis(300));
    client.drain();
    client.emit("load_data", &json!({"load_types": ["node_tuning"]}));
    let resp = client.wait_for("node_tuning", T);
    let current = resp["current_profile"].as_i64().unwrap_or(1);

    client.emit("set_profile", &json!({"profile": current}));
    // the server broadcasts frequency_data in response to a profile change
    let resp = client.wait_for("frequency_data", T);
    assert!(resp.is_object(), "frequency_data must be an object payload");
    client.disconnect();
}
