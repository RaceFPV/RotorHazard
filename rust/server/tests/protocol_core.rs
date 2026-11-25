//! Rust port of src/tests/protocol/test_protocol.py — core protocol
//! characterization (19 tests): data loads, HTTP APIs, pilot/heat/class/
//! format CRUD, frequencies, and the race flow.

mod common;

use common::*;
use serde_json::{json, Value};
use std::time::Duration;

const T: Duration = Duration::from_secs(15);
const LONG: Duration = Duration::from_secs(45);

#[allow(dead_code)]
fn wait_race_status(client: &SocketClient, target: i64, timeout: Duration, allow_ready: bool) -> Value {
    let deadline = std::time::Instant::now() + timeout;
    loop {
        let v = client.wait_for("race_status", deadline.saturating_duration_since(std::time::Instant::now()));
        let status = v.get("race_status").and_then(|x| x.as_i64()).unwrap_or(-1);
        if status == target || (allow_ready && status == RACE_STATUS_READY) {
            return v;
        }
        if std::time::Instant::now() > deadline {
            panic!("Timed out waiting for race_status=={}", target);
        }
    }
}

// ---- DataLoadTest ----

#[test]
fn test_load_data_payloads() {
    with_server(|_server, client, _| {
        client.emit("load_data", &json!({"load_types": [
            "pilot_data", "heat_data", "class_data", "format_data",
            "frequency_data", "node_data", "race_status"]}));
        let pilots = client.wait_for("pilot_data", T);
        assert!(pilots.get("pilots").and_then(|p| p.as_array()).is_some());

        let heats = client.wait_for("heat_data", T);
        assert!(heats.get("heats").and_then(|p| p.as_array()).is_some());

        let classes = client.wait_for("class_data", T);
        assert!(classes.get("classes").is_some());

        let formats = client.wait_for("format_data", T);
        let fmts = formats.get("formats").and_then(|f| f.as_array()).expect("formats");
        assert!(!fmts.is_empty());
        let fmt = &fmts[0];
        for key in ["id", "name", "unlimited_time", "race_time_sec",
                    "lap_grace_sec", "staging_fixed_tones", "win_condition"] {
            assert!(fmt.get(key).is_some(), "missing key {}", key);
        }

        let freqs = client.wait_for("frequency_data", T);
        let fdata = freqs.get("fdata").and_then(|f| f.as_array()).expect("fdata");
        assert_eq!(fdata.len(), 8);

        let node_data = client.wait_for("node_data", T);
        for key in ["node_peak_rssi", "node_nadir_rssi", "pass_peak_rssi",
                    "pass_nadir_rssi", "debug_pass_count"] {
            let arr = node_data.get(key).and_then(|x| x.as_array()).expect(key);
            assert_eq!(arr.len(), 8);
        }

        let status = client.wait_for("race_status", T);
        assert!(status.get("race_status").is_some());
    });
}

#[test]
fn test_http_api_status() {
    with_server(|_server, client, _| {
        let status = client.http_get_json("/api/status");
        let info = &status["status"]["server_info"];
        assert!(info.get("server_api").is_some());
        assert!(info.get("release_version").is_some());
    });
}

#[test]
fn test_http_api_pilot_roundtrip() {
    with_server(|_server, client, _| {
        let resp = client.emit_and_wait("load_data", &json!({"load_types": ["pilot_data"]}), "pilot_data", T);
        let prev_max = resp["pilots"].as_array().unwrap()
            .iter().filter_map(|p| p["pilot_id"].as_i64()).max().unwrap_or(0);
        let new_pilot = client.wait_for_new_id("add_pilot", &json!(null), "pilot_data", "pilot_id", "pilots", prev_max, T);

        // the HTTP API serializes the model directly (key 'id'), while the
        // socket payloads use 'pilot_id' -- protocol asymmetry preserved
        let api_pilot = client.http_get_json(&format!("/api/pilot/{}", new_pilot));
        assert_eq!(api_pilot["pilot"]["id"], new_pilot);
        let api_all = client.http_get_json("/api/pilot/all");
        assert!(api_all["pilots"].as_array().unwrap().iter().any(|p| p["id"] == new_pilot));
    });
}

// ---- PilotCrudTest ----

#[test]
fn test_add_alter_delete_pilot() {
    with_server(|_server, client, _| {
        let resp = client.emit_and_wait("load_data", &json!({"load_types": ["pilot_data"]}), "pilot_data", T);
        let prev_max = resp["pilots"].as_array().unwrap()
            .iter().filter_map(|p| p["pilot_id"].as_i64()).max().unwrap_or(0);
        let new_pilot = client.wait_for_new_id("add_pilot", &json!(null), "pilot_data", "pilot_id", "pilots", prev_max, T);

        let resp = client.emit_and_wait("alter_pilot", &json!({
            "pilot_id": new_pilot,
            "callsign": "TST",
            "team_name": "team T",
            "phonetic": "testy",
            "name": "Test Pilot",
        }), "pilot_data", T);
        let pilots = resp["pilots"].as_array().unwrap();
        let pilot = pilots.iter().find(|p| p["pilot_id"] == new_pilot).expect("pilot present");
        assert_eq!(pilot["callsign"], "TST");
        assert_eq!(pilot["team"], "team T");
        assert_eq!(pilot["phonetic"], "testy");

        client.emit("delete_pilot", &json!({"pilot": new_pilot}));
        let resp = client.wait_for("pilot_data", T);
        let pilots = resp["pilots"].as_array().unwrap();
        assert!(!pilots.iter().any(|p| p["pilot_id"] == new_pilot));
    });
}

// ---- ClassCrudTest ----

#[test]
fn test_add_alter_delete_class() {
    with_server(|_server, client, _| {
        let resp = client.emit_and_wait("load_data", &json!({"load_types": ["class_data"]}), "class_data", T);
        let prev_max = resp["classes"].as_array().unwrap()
            .iter().filter_map(|c| c["id"].as_i64()).max().unwrap_or(0);
        let new_class = client.wait_for_new_id("add_race_class",
            &json!({"name": format!("TestClass {}", test_token())}),
            "class_data", "id", "classes", prev_max, T);

        // one token per assertion pair: the token embeds the epoch second and
        // must not be re-evaluated across the request and the assertion
        let alter_name = format!("TestClassAltered {}", test_token());
        let resp = client.emit_and_wait("alter_race_class", &json!({
            "class_id": new_class,
            "class_name": alter_name,
            "class_description": "renamed",
        }), "class_data", T);
        let classes = resp["classes"].as_array().unwrap();
        let cls = classes.iter().find(|c| c["id"] == new_class).expect("class present");
        assert_eq!(cls["name"], alter_name);

        client.emit("delete_class", &json!({"class": new_class}));
        let resp = client.wait_for("class_data", T);
        assert!(!resp["classes"].as_array().unwrap().iter().any(|c| c["id"] == new_class));
    });
}

#[test]
fn test_http_api_class_roundtrip() {
    with_server(|_server, client, _| {
        let resp = client.emit_and_wait("load_data", &json!({"load_types": ["class_data"]}), "class_data", T);
        let prev_max = resp["classes"].as_array().unwrap()
            .iter().filter_map(|c| c["id"].as_i64()).max().unwrap_or(0);
        let new_class = client.wait_for_new_id("add_race_class",
            &json!({"name": format!("TestClassAPI {}", test_token())}),
            "class_data", "id", "classes", prev_max, T);

        let api = client.http_get_json(&format!("/api/class/{}", new_class));
        assert_eq!(api["class"]["id"], new_class);
    });
}

// ---- HeatCrudTest ----

fn max_heat_id(client: &SocketClient) -> i64 {
    let resp = client.emit_and_wait("load_data", &json!({"load_types": ["heat_data"]}), "heat_data", T);
    resp["heats"].as_array().unwrap()
        .iter().filter_map(|h| h["id"].as_i64()).max().unwrap_or(0)
}

fn new_pilot(client: &SocketClient) -> i64 {
    let resp = client.emit_and_wait("load_data", &json!({"load_types": ["pilot_data"]}), "pilot_data", T);
    let prev_max = resp["pilots"].as_array().unwrap()
        .iter().filter_map(|p| p["pilot_id"].as_i64()).max().unwrap_or(0);
    client.wait_for_new_id("add_pilot", &json!(null), "pilot_data", "pilot_id", "pilots", prev_max, T)
}

#[test]
fn test_add_alter_heat_and_assign_pilot() {
    with_server(|_server, client, _| {
        let token = test_token();
        let prev_max = max_heat_id(client);
        let new_heat = client.wait_for_new_id("add_heat", &json!(null), "heat_data", "id", "heats", prev_max, T);
        let resp = client.emit_and_wait("load_data", &json!({"load_types": ["heat_data"]}), "heat_data", T);
        let heat = resp["heats"].as_array().unwrap().iter().find(|h| h["id"] == new_heat).unwrap();
        let slots = heat["slots"].as_array().unwrap();
        assert!(slots.len() >= 8);
        let slot_id = slots[0]["id"].clone();

        let pilot_id = new_pilot(client);
        let resp = client.emit_and_wait("alter_heat", &json!({
            "heat": new_heat,
            "name": format!("TestHeat {}", token),
        }), "heat_data", T);
        let heat = resp["heats"].as_array().unwrap().iter().find(|h| h["id"] == new_heat).unwrap();
        assert_eq!(heat["name"], format!("TestHeat {}", token));

        let resp = client.emit_and_wait("alter_heat", &json!({
            "heat": new_heat,
            "slot_id": slot_id,
            "pilot": pilot_id,
        }), "heat_data", T);
        let heat = resp["heats"].as_array().unwrap().iter().find(|h| h["id"] == new_heat).unwrap();
        assert_eq!(heat["slots"].as_array().unwrap()[0]["pilot_id"], pilot_id);
    });
}

#[test]
fn test_set_current_heat() {
    with_server(|_server, client, _| {
        let prev_max = max_heat_id(client);
        let new_heat = client.wait_for_new_id("add_heat", &json!(null), "heat_data", "id", "heats", prev_max, T);
        let resp = client.emit_and_wait("set_current_heat", &json!({"heat": new_heat}), "current_heat", T);
        assert_eq!(resp["current_heat"], new_heat);
    });
}

#[test]
fn test_delete_heat() {
    with_server(|_server, client, _| {
        let prev_max = max_heat_id(client);
        let new_heat = client.wait_for_new_id("add_heat", &json!(null), "heat_data", "id", "heats", prev_max, T);
        client.emit("delete_heat", &json!({"heat": new_heat}));
        let resp = client.wait_for("heat_data", T);
        assert!(!resp["heats"].as_array().unwrap().iter().any(|h| h["id"] == new_heat));
    });
}

// ---- RaceFormatTest ----

#[test]
fn test_add_alter_set_format() {
    with_server(|_server, client, _| {
        let fmt_token = test_token();
        let resp = client.emit_and_wait("load_data", &json!({"load_types": ["format_data"]}), "format_data", T);
        let prev_max = resp["formats"].as_array().unwrap()
            .iter().filter_map(|f| f["id"].as_i64()).max().unwrap_or(0);
        let new_fmt = client.wait_for_new_id("add_race_format",
            &json!({"source_format_id": 1}), "format_data", "id", "formats", prev_max, T);

        let resp = client.emit_and_wait("alter_race_format", &json!({
            "format_id": new_fmt,
            "format_name": format!("TestFmtR {}", fmt_token),
            "race_time_sec": 45,
            "staging_fixed_tones": 1,
        }), "format_data", T);
        let altered = resp["formats"].as_array().unwrap().iter().find(|f| f["id"] == new_fmt).unwrap();
        assert_eq!(altered["name"], format!("TestFmtR {}", fmt_token));
        assert_eq!(altered["race_time_sec"], 45);

        let resp = client.emit_and_wait("set_race_format", &json!({"race_format": new_fmt}), "race_status", T);
        assert_eq!(resp["race_format_id"], new_fmt);
    });
}

#[test]
fn test_http_api_formats() {
    with_server(|_server, client, _| {
        let api = client.http_get_json("/api/format/all");
        assert!(api["formats"].as_array().expect("formats").len() >= 1);
    });
}

// ---- FrequencyTest ----

#[test]
fn test_set_frequency() {
    with_server(|_server, client, _| {
        let resp = client.emit_and_wait("load_data", &json!({"load_types": ["frequency_data"]}), "frequency_data", T);
        let fdata = resp["fdata"].as_array().unwrap();
        let node = fdata.iter().position(|n| n["f"].as_i64().unwrap_or(0) != 0).unwrap_or(0);
        let resp = client.emit_and_wait("set_frequency", &json!({
            "node": node, "band": "F", "channel": 4, "frequency": 5800,
        }), "frequency_data", T);
        let fdata = resp["fdata"].as_array().unwrap();
        assert_eq!(fdata[node]["frequency"], 5800);
        assert_eq!(fdata[node]["band"], "F");
    });
}

    #[test]
    fn test_frequency_preset() {
        with_server(|_server, client, _| {
            let resp = client.emit_and_wait("set_frequency_preset", &json!({
                "profile": Value::Null,
                "preset": "RB",
            }), "frequency_data", T);
            // RB preset assigns factory band/channels to the first 6 nodes
            let first6 = resp["fdata"].as_array().unwrap()[..6]
                .iter()
                .map(|f| f["frequency"].as_i64().unwrap_or(0))
                .collect::<Vec<_>>();
            assert!(first6.iter().all(|&f| f > 0));
        });
    }

    #[test]
    fn test_add_profile() {
        with_server(|_server, client, _| {
            // a new profile must not disturb the per-node frequency shape
            let resp = client.emit_and_wait("add_profile", &json!({
                "name": format!("TestProfile {}", test_token()),
            }), "frequency_data", T);
            assert_eq!(resp["fdata"].as_array().unwrap().len(), 8);
        });
    }

    // ---- RaceFlowTest ----

fn setup_heat_with_pilots(client: &SocketClient, num_pilots: usize) -> i64 {
    let mut pilot_ids = Vec::new();
    for _ in 0..num_pilots {
        pilot_ids.push(new_pilot(client));
    }
    let prev_max_heat = max_heat_id(client);
    let new_heat = client.wait_for_new_id("add_heat", &json!(null), "heat_data", "id", "heats", prev_max_heat, T);
    let resp = client.emit_and_wait("load_data", &json!({"load_types": ["heat_data"]}), "heat_data", T);
    let heat = resp["heats"].as_array().unwrap().iter().find(|h| h["id"] == new_heat).unwrap().clone();

    for (slot, pilot_id) in heat["slots"].as_array().unwrap().iter().take(num_pilots).zip(&pilot_ids) {
        client.emit_and_wait("alter_heat", &json!({
            "heat": new_heat,
            "slot_id": slot["id"],
            "pilot": pilot_id,
        }), "heat_data", T);
    }
    client.emit_and_wait("set_current_heat", &json!({"heat": new_heat}), "current_heat", T);
    new_heat
}

fn wait_race_status_any(client: &SocketClient, observer: &SocketClient, target: i64, timeout: Duration, allow_ready: bool) -> Value {
    let deadline = std::time::Instant::now() + timeout;
    loop {
        let v = SocketClient::wait_any([client, observer], "race_status",
            deadline.saturating_duration_since(std::time::Instant::now()));
        let status = v.get("race_status").and_then(|x| x.as_i64()).unwrap_or(-1);
        if status == target || (allow_ready && status == RACE_STATUS_READY) {
            return v;
        }
        if std::time::Instant::now() > deadline {
            panic!("Timed out waiting for race_status=={}", target);
        }
    }
}

#[test]
fn test_stage_race_simulate_laps_save() {
    with_server(|_server, client, observer| {
        let new_heat = setup_heat_with_pilots(client, 2);

        client.emit("stage_race", &json!(null));
        wait_race_status_any(client, observer, RACE_STATUS_RACING, LONG, false);

        // simulate two laps on node 0, one on node 1
        let resp = client.emit_and_wait("simulate_lap", &json!({"node": 0}), "current_laps", T);
        let node0_laps = resp["current"]["node_index"][0]["laps"].as_array().unwrap();
        assert!(node0_laps.len() >= 1, "expected >=1 lap on node 0");

        std::thread::sleep(Duration::from_millis(600));
        let resp = client.emit_and_wait("simulate_lap", &json!({"node": 0}), "current_laps", T);
        let node0_laps = resp["current"]["node_index"][0]["laps"].as_array().unwrap();
        assert!(node0_laps.len() >= 2);

        let resp = client.emit_and_wait("simulate_lap", &json!({"node": 1}), "current_laps", T);
        let node1_laps = resp["current"]["node_index"][1]["laps"].as_array().unwrap();
        assert!(node1_laps.len() >= 1);

        // stop and save (status may pass through DONE or go straight to
        // READY when there is nothing to save)
        client.emit("stop_race", &json!(null));
        wait_race_status_any(client, observer, RACE_STATUS_DONE, Duration::from_secs(20), true);
        client.emit("save_laps", &json!(null));
        SocketClient::wait_any([client, observer], "race_saved", Duration::from_secs(20));

        // saved race must be queryable over HTTP with rounds recorded
        let races = client.http_get_json("/api/race/all");
        let heats = races["races"]["heats"].as_array().unwrap();
        let saved = heats.iter().find(|h| h["id"] == new_heat)
            .unwrap_or_else(|| panic!("heat {} missing from /api/race/all", new_heat));
        assert!(saved["rounds"].as_i64().unwrap_or(0) >= 1,
            "no rounds recorded for heat {}", new_heat);
    });
}

#[test]
fn test_stage_and_stop_without_save() {
    with_server(|_server, client, observer| {
        setup_heat_with_pilots(client, 1);
        client.emit("stage_race", &json!(null));
        wait_race_status_any(client, observer, RACE_STATUS_RACING, LONG, false);
        client.emit("stop_race", &json!(null));
        wait_race_status_any(client, observer, RACE_STATUS_DONE, Duration::from_secs(20), false);
        client.emit("discard_laps", &json!(null));
        wait_race_status_any(client, observer, RACE_STATUS_READY, Duration::from_secs(20), false);
    });
}

// ---- ConfigTest ----

#[test]
fn test_config_load_limiting() {
    with_server(|_server, client, _| {
        // Upstream restricts which config sections a page may fetch
        // (RHUI.CONFIG_LOAD_ITEMS): GENERAL is limited to SECONDARIES and
        // SERIAL_PORTS, SENSORS allows the whole section, and SECRETS is
        // never fetchable.
        client.emit("set_config", &json!({
            "section": "GENERAL",
            "key": "RACE_START_DELAY_EXTRA_SECS",
            "value": 0.5,
        }));
        std::thread::sleep(Duration::from_millis(500));
        // an item outside the GENERAL allowlist is not served
        client.emit("load_data", &json!({"load_types": [
            {"type": "config", "value": {"GENERAL": ["RACE_START_DELAY_EXTRA_SECS"]}}]}));
        let resp = client.wait_for("config_update", T);
        let config = &resp["config"];
        assert!(config["GENERAL"]["RACE_START_DELAY_EXTRA_SECS"].is_null());
        // allowed GENERAL items are served
        client.emit("load_data", &json!({"load_types": [
            {"type": "config", "value": {"GENERAL": ["SERIAL_PORTS"]}}]}));
        let resp = client.wait_for("config_update", T);
        assert!(resp["config"]["GENERAL"]["SERIAL_PORTS"].is_object()
            || resp["config"]["GENERAL"]["SERIAL_PORTS"].is_array()
            || resp["config"]["GENERAL"]["SERIAL_PORTS"].is_null());
        assert!(resp["config"]["GENERAL"].is_object());
        // a reserved section is never served
        client.emit("load_data", &json!({"load_types": [
            {"type": "config", "value": {"SECRETS": ["adminPassword"]}}]}));
        let resp = client.wait_for("config_update", T);
        assert!(resp["config"]["SECRETS"].is_null());
    });
}

#[test]
fn test_set_option_visible_via_http() {
    with_server(|_server, client, _| {
        let token = test_token();
        client.emit("set_option", &json!({
            "option": "eventDescription",
            "value": format!("protocol-test {}", token),
        }));
        std::thread::sleep(Duration::from_millis(500));
        let opts = client.http_get_json("/api/options");
        assert_eq!(
            opts["options"]["eventDescription"].as_str().unwrap_or(""),
            format!("protocol-test {}", token),
        );
    });
}

#[test]
fn test_http_api_options() {
    with_server(|_server, client, _| {
        let opts = client.http_get_json("/api/options");
        assert!(opts.get("options").is_some());
    });
}

// ---- Marshal lap editing and scheduling (test_protocol_lifecycle.py) ----
// Ported from the Python suite: these flows go through the same marshal and
// management events the UI uses, and none of them had Rust coverage.

#[test]
fn test_delete_and_restore_lap() {
    with_server(|_server, client, observer| {
        setup_heat_with_pilots(client, 1);
        client.emit("stage_race", &json!(null));
        wait_race_status_any(client, observer, RACE_STATUS_RACING, LONG, false);

        let mut laps: Vec<Value> = Vec::new();
        for _ in 0..3 {
            let resp = client.emit_and_wait("simulate_lap", &json!({"node": 0}), "current_laps", T);
            laps = resp["current"]["node_index"][0]["laps"]
                .as_array()
                .cloned()
                .unwrap_or_default();
            std::thread::sleep(Duration::from_millis(1100));
        }
        assert_eq!(laps.len(), 3, "expected three laps, got {:?}", laps);
        assert_eq!(
            laps.iter().filter(|l| !l["deleted"].as_bool().unwrap_or(false)).count(),
            3,
            "all three laps should be live before deletion: {:?}",
            laps
        );

        let target = laps
            .iter()
            .filter(|l| !l["deleted"].as_bool().unwrap_or(false))
            .nth(1)
            .and_then(|l| l["lap_index"].as_i64())
            .expect("middle lap has a lap_index");

        // A deleted lap stays in the list but leaves the live count; the UI
        // renders it struck through rather than removing the row.
        let resp = client.emit_and_wait(
            "delete_lap",
            &json!({"node": 0, "lap_index": target}),
            "current_laps",
            T,
        );
        let laps = resp["current"]["node_index"][0]["laps"].as_array().unwrap();
        let deleted: Vec<&Value> = laps
            .iter()
            .filter(|l| l["deleted"].as_bool().unwrap_or(false))
            .collect();
        assert_eq!(deleted.len(), 1, "expected exactly one deleted lap: {:?}", laps);
        assert_eq!(deleted[0]["lap_index"].as_i64().unwrap(), target);
        assert_eq!(
            laps.iter().filter(|l| !l["deleted"].as_bool().unwrap_or(false)).count(),
            2,
            "a deleted lap must not count as live"
        );

        let resp = client.emit_and_wait(
            "restore_deleted_lap",
            &json!({"node": 0, "lap_index": target}),
            "current_laps",
            T,
        );
        let laps = resp["current"]["node_index"][0]["laps"].as_array().unwrap();
        assert_eq!(
            laps.iter().filter(|l| !l["deleted"].as_bool().unwrap_or(false)).count(),
            3,
            "restored lap should be live again"
        );

        client.emit("stop_race", &json!(null));
        wait_race_status_any(client, observer, RACE_STATUS_DONE, Duration::from_secs(20), true);
        client.emit("discard_laps", &json!(null));
    });
}

#[test]
fn test_delete_last_lap_and_undo() {
    with_server(|_server, client, observer| {
        setup_heat_with_pilots(client, 1);
        client.emit("stage_race", &json!(null));
        wait_race_status_any(client, observer, RACE_STATUS_RACING, LONG, false);

        for _ in 0..2 {
            client.emit_and_wait("simulate_lap", &json!({"node": 0}), "current_laps", T);
            std::thread::sleep(Duration::from_millis(1100));
        }

        let resp = client.emit_and_wait("delete_last_lap", &json!({"node": 0}), "current_laps", T);
        let live = resp["current"]["node_index"][0]["laps"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|l| !l["deleted"].as_bool().unwrap_or(false))
            .count();
        assert_eq!(live, 1, "delete_last_lap should remove exactly one live lap");

        let resp = client.emit_and_wait(
            "restore_last_deleted_lap",
            &json!({"node": 0}),
            "current_laps",
            T,
        );
        let live = resp["current"]["node_index"][0]["laps"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|l| !l["deleted"].as_bool().unwrap_or(false))
            .count();
        assert_eq!(live, 2, "undo should restore the lap");

        client.emit("stop_race", &json!(null));
        wait_race_status_any(client, observer, RACE_STATUS_DONE, Duration::from_secs(20), true);
        client.emit("discard_laps", &json!(null));
    });
}

#[test]
fn test_min_lap_roundtrip() {
    with_server(|_server, client, observer| {
        // min_lap is broadcast to every socket EXCEPT the one that set it: the
        // requester already knows, the rest of the pit needs telling.
        client.emit("set_min_lap", &json!({"min_lap": 7}));
        let resp = observer.wait_for("min_lap", T);
        assert_eq!(resp["min_lap"].as_i64().unwrap_or(-1), 7, "bare payload: {}", resp);
        assert!(resp.get("min_first_crossing").is_some(), "payload: {}", resp);
        assert!(resp.get("min_lap_behavior").is_some(), "payload: {}", resp);

        observer.drain();
        client.emit("set_min_lap", &json!({"min_lap": 0}));
        let resp = observer.wait_for("min_lap", T);
        assert_eq!(resp["min_lap"].as_i64().unwrap_or(-1), 0);
    });
}

#[test]
fn test_set_enter_exit_at_levels() {
    with_server(|_server, client, _| {
        // Responses go only to the requesting socket and carry the full
        // per-node level lists from the active profile.
        let resp = client.emit_and_wait(
            "set_enter_at_level",
            &json!({"node": 0, "enter_at_level": 95}),
            "enter_and_exit_at_levels",
            T,
        );
        assert_eq!(resp["enter_at_levels"][0].as_i64().unwrap_or(-1), 95, "payload: {}", resp);

        let resp = client.emit_and_wait(
            "set_exit_at_level",
            &json!({"node": 0, "exit_at_level": 85}),
            "enter_and_exit_at_levels",
            T,
        );
        assert_eq!(resp["exit_at_levels"][0].as_i64().unwrap_or(-1), 85, "payload: {}", resp);
    });
}

#[test]
fn test_schedule_and_cancel_race() {
    with_server(|_server, client, _| {
        let resp = client.emit_and_wait("schedule_race", &json!({"m": 30, "s": 0}), "race_scheduled", T);
        assert_eq!(resp["scheduled"].as_bool(), Some(true), "payload: {}", resp);
        assert!(
            resp["scheduled_at"].as_f64().unwrap_or(0.0) > 0.0,
            "scheduled_at should be a timestamp: {}",
            resp
        );

        let resp = client.emit_and_wait("cancel_schedule_race", &json!(null), "race_scheduled", T);
        assert_eq!(resp["scheduled"].as_bool(), Some(false), "payload: {}", resp);

        // get_race_scheduled answers the requester directly (no broadcast).
        let resp = client.emit_and_wait("get_race_scheduled", &json!(null), "race_scheduled", T);
        assert!(resp.get("scheduled").is_some(), "payload: {}", resp);
        assert!(resp.get("scheduled_at").is_some(), "payload: {}", resp);
    });
}

#[test]
fn test_seat_data_shape() {
    with_server(|_server, client, _| {
        let resp = client.emit_and_wait(
            "load_data",
            &json!({"load_types": ["seat_data"]}),
            "seat_data",
            T,
        );
        let seats = resp["seats"].as_array().expect("seat_data.seats is an array");
        assert_eq!(seats.len(), 8, "expected one seat per node: {:?}", seats);
        for seat in seats {
            assert!(seat.get("seat_id").is_some(), "seat missing seat_id: {:?}", seat);
            let color = seat["color"].as_str().unwrap_or("");
            assert!(color.starts_with('#'), "seat color is not a hex string: {:?}", seat);
        }
    });
}

// ---- Config-backed payloads ----
// These pin behaviour that used to be hardcoded: a saved seat palette, the
// min-lap behaviour flag, and where the start-threshold value is stored.

#[test]
fn test_min_lap_behavior_reflects_config() {
    with_server(|_server, client, observer| {
        client.emit("set_config", &json!({
            "section": "TIMING", "key": "MinLapBehavior", "value": 1,
        }));
        std::thread::sleep(Duration::from_millis(500));

        client.emit("set_min_lap", &json!({"min_lap": 5}));
        let resp = observer.wait_for("min_lap", T);
        assert_eq!(resp["min_lap"].as_i64().unwrap_or(-1), 5, "payload: {}", resp);
        assert_eq!(
            resp["min_lap_behavior"].as_i64().unwrap_or(-1),
            1,
            "min_lap_behavior must come from TIMING.MinLapBehavior, not a constant: {}",
            resp
        );
    });
}

#[test]
fn test_seat_color_persists_and_broadcasts() {
    let mut server = spawn_server();
    let mut client = SocketClient::connect(&server.base_url);
    std::thread::sleep(Duration::from_millis(300));
    client.drain();

    client.emit("set_seat_color", &json!({"seat_id": 2, "color": "#123456"}));
    let resp = client.wait_for("seat_data", T);
    assert_eq!(
        resp["seats"][2]["color"].as_str(),
        Some("#123456"),
        "seat_data broadcast did not carry the new colour: {}",
        resp
    );

    // A reload must read it back from config.json, not the hardcoded defaults.
    let resp = client.emit_and_wait(
        "load_data",
        &json!({"load_types": ["seat_data"]}),
        "seat_data",
        T,
    );
    assert_eq!(resp["seats"][2]["color"].as_str(), Some("#123456"), "payload: {}", resp);

    // Resetting restores the default palette everywhere.
    client.emit("reset_seat_color", &json!(null));
    let resp = client.wait_for("seat_data", T);
    assert_eq!(
        resp["seats"][2]["color"].as_str(),
        Some("#00ff22"),
        "reset did not restore the default colour: {}",
        resp
    );

    let config_path = server.data_dir.join("config.json");
    let cfg: Value = serde_json::from_str(&std::fs::read_to_string(&config_path).unwrap())
        .unwrap_or(json!({}));
    assert_eq!(
        cfg["LED"]["seatColors"][2].as_str(),
        Some("#00ff22"),
        "config.json did not persist the palette: {}",
        cfg
    );

    client.disconnect();
    server.stop();
}

#[test]
fn test_start_thresh_lower_amount_persists_to_config() {
    let mut server = spawn_server();
    let mut client = SocketClient::connect(&server.base_url);
    std::thread::sleep(Duration::from_millis(300));
    client.drain();

    let resp = client.emit_and_wait(
        "set_start_thresh_lower_amount",
        &json!({"start_thresh_lower_amount": 5}),
        "start_thresh_lower_amount",
        T,
    );
    assert_eq!(
        resp["start_thresh_lower_amount"].as_i64(),
        Some(5),
        "payload: {}",
        resp
    );

    // Python keeps this in config.json TIMING; a DB-only copy is invisible to
    // every page that reads the config.
    let config_path = server.data_dir.join("config.json");
    let deadline = std::time::Instant::now() + T;
    loop {
        let cfg: Value = std::fs::read_to_string(&config_path)
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or(json!({}));
        if cfg["TIMING"]["startThreshLowerAmount"].as_i64() == Some(5) {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "config.json TIMING.startThreshLowerAmount was never written: {}",
            cfg
        );
        std::thread::sleep(Duration::from_millis(100));
    }

    client.disconnect();
    server.stop();
}

// ---- Lap statistics in the current_laps payload ----

#[test]
fn test_fastest_lap_index_and_finished_flag() {
    with_server(|_server, client, observer| {
        setup_heat_with_pilots(client, 1);
        client.emit("stage_race", &json!(null));
        wait_race_status_any(client, observer, RACE_STATUS_RACING, LONG, false);

        let mut resp = json!(null);
        for _ in 0..3 {
            resp = client.emit_and_wait("simulate_lap", &json!({"node": 0}), "current_laps", T);
            std::thread::sleep(Duration::from_millis(1100));
        }

        let node = &resp["current"]["node_index"][0];
        let laps = node["laps"].as_array().expect("laps array");

        // Python picks the quickest completed lap, skipping index 0 (the first
        // crossing is not a lap time).
        let mut expected: Option<(usize, f64)> = None;
        for (i, lap) in laps.iter().enumerate().skip(1) {
            let t = lap["lap_time"].as_f64().unwrap_or(0.0);
            if t > 0.0 && expected.map(|(_, b)| t < b).unwrap_or(true) {
                expected = Some((i, t));
            }
        }

        let fastest = node["fastest_lap_index"].as_i64();
        assert!(
            fastest.is_some(),
            "fastest_lap_index stayed null with laps recorded: {}",
            node
        );
        assert_eq!(
            Some(fastest.unwrap() as usize),
            expected.map(|(i, _)| i),
            "fastest_lap_index does not point at the quickest lap: {}",
            node
        );

        // The default format has no lap limit, so nothing is finished yet.
        assert_eq!(
            node["finished_flag"].as_bool(),
            Some(false),
            "finished_flag should be false without a lap limit: {}",
            node
        );

        client.emit("stop_race", &json!(null));
        wait_race_status_any(client, observer, RACE_STATUS_DONE, Duration::from_secs(20), true);
        client.emit("discard_laps", &json!(null));
    });
}

// ---- Heat management, backups, time (test_protocol_lifecycle.py) ----

#[test]
fn test_duplicate_heat_copies_slots() {
    with_server(|_server, client, _| {
        let source = setup_heat_with_pilots(client, 2);
        let before = max_heat_id(client);

        let duplicated = client.wait_for_new_id(
            "duplicate_heat",
            &json!({"heat": source}),
            "heat_data",
            "id",
            "heats",
            before,
            T,
        );
        assert_ne!(duplicated, source, "duplicate must get a fresh id");

        let resp = client.emit_and_wait(
            "load_data",
            &json!({"load_types": ["heat_data"]}),
            "heat_data",
            T,
        );
        let heats = resp["heats"].as_array().unwrap();
        let pilot_ids = |heat: &Value| -> Vec<Value> {
            heat["slots"]
                .as_array()
                .unwrap()
                .iter()
                .map(|s| s["pilot"].clone())
                .collect()
        };
        let original = heats.iter().find(|h| h["id"] == source).unwrap();
        let copy = heats.iter().find(|h| h["id"] == duplicated).unwrap();
        assert_eq!(
            pilot_ids(original),
            pilot_ids(&copy),
            "duplicated heat should carry the same slot pilots"
        );
    });
}

#[test]
fn test_activate_and_deactivate_heat() {
    with_server(|_server, client, _| {
        let heat = setup_heat_with_pilots(client, 1);

        let resp = client.emit_and_wait(
            "deactivate_heat",
            &json!({"heat": heat}),
            "heat_data",
            T,
        );
        let h = resp["heats"]
            .as_array()
            .unwrap()
            .iter()
            .find(|h| h["id"] == heat)
            .cloned()
            .expect("heat present after deactivate");
        assert_eq!(h["active"].as_bool(), Some(false), "heat still active: {}", h);

        let resp = client.emit_and_wait(
            "activate_heat",
            &json!({"heat": heat}),
            "heat_data",
            T,
        );
        let h = resp["heats"]
            .as_array()
            .unwrap()
            .iter()
            .find(|h| h["id"] == heat)
            .cloned()
            .expect("heat present after activate");
        assert_eq!(h["active"].as_bool(), Some(true), "heat still inactive: {}", h);
    });
}

#[test]
fn test_database_backup_and_list() {
    let mut server = spawn_server();
    let mut client = SocketClient::connect(&server.base_url);
    std::thread::sleep(Duration::from_millis(300));
    client.drain();

    client.emit("backup_database", &json!(null));
    client.wait_for("database_bkp_done", Duration::from_secs(20));

    let resp = client.emit_and_wait("list_backups", &json!(null), "backups_list", T);
    let files = resp["backup_files"]
        .as_array()
        .expect("backups_list.backup_files is an array");
    assert!(!files.is_empty(), "no backup listed after backup_database: {}", resp);

    // The listing must name real files, else a restore picks a phantom.
    for name in files {
        let name = name.as_str().unwrap_or("");
        let path = server.data_dir.join("db_bkp").join(name);
        assert!(path.exists(), "listed backup {} does not exist at {:?}", name, path);
    }

    client.disconnect();
    server.stop();
}

#[test]
fn test_server_time_via_ack() {
    with_server(|_server, client, _| {
        let resp = client.emit_ack("get_server_time", &json!(null), T);
        // An ack arrives as the argument list, so the payload is element 0.
        let payload = resp.get(0).cloned().unwrap_or_else(|| resp.clone());
        assert!(
            payload["server_time_s"].as_f64().unwrap_or(0.0) > 0.0,
            "get_server_time ack carried no timestamp: {}",
            resp
        );
    });
}
