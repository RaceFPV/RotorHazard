//! Heartbeat and mock-node coverage.
//!
//! The frequency-test scanner (src/server/static/scanner.js) draws its graph
//! from the `heartbeat` broadcast: one point per node from `current_rssi[i]`
//! at `frequency[i]`. A missing key, a short array or a value that never
//! changes shows up on that page as a broken or flat trace, which is exactly
//! the flakiness reported around mock nodes and the graphing.
//!
//! The Python suite has narrower versions of some of these
//! (test_protocol_phase3.py::test_signal_none_freezes_rssi and
//! test_mock_rssi_varies_and_laps_register); these pin the wire contract.

mod common;

use common::*;
use serde_json::{json, Value};
use std::time::Duration;

const T: Duration = Duration::from_secs(10);

/// Same config shape the Python harness uses to exercise simulated mocks.
fn spawn_simulated_server() -> ServerHandle {
    common::spawn_server_with_config(&json!({
        "SECRETS": {"ADMIN_USERNAME": "", "ADMIN_PASSWORD": ""},
        "GENERAL": {"DEBUG": false, "MOCK_NODE_SIGNAL": "2"},
    }))
}

fn heartbeat(client: &SocketClient) -> Value {
    client.wait_for("heartbeat", T)
}

/// The scanner reads exactly these four arrays, index-matched to the seats.
#[test]
fn test_heartbeat_payload_shape_matches_scanner() {
    let mut server = spawn_server();
    let mut client = SocketClient::connect(&server.base_url);
    std::thread::sleep(Duration::from_millis(300));
    client.drain();

    let hb = heartbeat(&client);
    for key in ["current_rssi", "frequency", "loop_time", "crossing_flag"] {
        let arr = hb
            .get(key)
            .and_then(|v| v.as_array())
            .unwrap_or_else(|| panic!("heartbeat missing array \"{}\": {}", key, hb));
        assert!(
            arr.len() >= 8,
            "heartbeat.{} has {} entries, expected one per seat: {}",
            key,
            arr.len(),
            hb
        );
    }

    let rssi = hb["current_rssi"].as_array().unwrap();
    let freq = hb["frequency"].as_array().unwrap();
    let loop_time = hb["loop_time"].as_array().unwrap();
    let crossing = hb["crossing_flag"].as_array().unwrap();
    assert_eq!(rssi.len(), freq.len(), "rssi/frequency arity differ");
    assert_eq!(rssi.len(), loop_time.len(), "rssi/loop_time arity differ");
    assert_eq!(rssi.len(), crossing.len(), "rssi/crossing arity differ");

    for v in rssi {
        assert!(v.is_i64() || v.is_u64(), "current_rssi must be numeric: {}", v);
    }
    for v in freq {
        assert!(v.is_i64() || v.is_u64(), "frequency must be numeric: {}", v);
    }
    for v in crossing {
        assert!(v.is_boolean(), "crossing_flag must be boolean: {}", v);
    }

    client.disconnect();
    server.stop();
}

/// Without a signal provider every seat reports 0: the graph should be flat,
/// not garbage. Pins the MOCK_NODE_SIGNAL=0 default.
#[test]
fn test_heartbeat_rssi_frozen_without_signal_provider() {
    let mut server = spawn_server();
    let mut client = SocketClient::connect(&server.base_url);
    std::thread::sleep(Duration::from_millis(300));
    client.drain();

    for _ in 0..3 {
        let hb = heartbeat(&client);
        for (i, v) in hb["current_rssi"].as_array().unwrap().iter().enumerate() {
            assert_eq!(
                v.as_i64().unwrap_or(-1),
                0,
                "seat {} reported RSSI {} with no signal provider: {}",
                i,
                v,
                hb
            );
        }
    }

    client.disconnect();
    server.stop();
}

/// The simulated provider must actually vary, otherwise the scanner draws a
/// single flat line and the page looks broken.
#[test]
fn test_heartbeat_rssi_varies_with_simulated_provider() {
    let mut server = spawn_simulated_server();
    let mut client = SocketClient::connect(&server.base_url);
    std::thread::sleep(Duration::from_millis(300));
    client.drain();

    let mut samples: Vec<Vec<i64>> = Vec::new();
    for _ in 0..8 {
        let hb = heartbeat(&client);
        samples.push(
            hb["current_rssi"]
                .as_array()
                .unwrap()
                .iter()
                .map(|v| v.as_i64().unwrap_or(0))
                .collect(),
        );
    }

    let seats = samples[0].len();
    let mut moved = 0;
    let mut seen_nonzero = false;
    for seat in 0..seats {
        let values: Vec<i64> = samples.iter().map(|s| s[seat]).collect();
        if values.iter().any(|&v| v != 0) {
            seen_nonzero = true;
        }
        if values.windows(2).any(|w| w[0] != w[1]) {
            moved += 1;
        }
    }

    assert!(
        seen_nonzero,
        "simulated provider produced only zeros across {} heartbeats: {:?}",
        samples.len(),
        samples
    );
    assert!(
        moved > 0,
        "simulated provider produced a static trace on every seat ({:?}); \
         the scanner graph would be a flat line",
        samples
    );

    client.disconnect();
    server.stop();
}

/// Setting a seat's frequency must reach the scanner's `frequency[i]` within a
/// heartbeat, or the plot keeps drawing at the old channel regardless of what
/// the UI shows.
#[test]
fn test_heartbeat_frequency_follows_seat_frequency() {
    let mut server = spawn_server();
    let mut client = SocketClient::connect(&server.base_url);
    std::thread::sleep(Duration::from_millis(300));
    client.drain();

    // Pick a seat the profile actually populates, then retune it.
    let resp = client.emit_and_wait(
        "load_data",
        &json!({"load_types": ["frequency_data"]}),
        "frequency_data",
        T,
    );
    let fdata = resp["fdata"].as_array().unwrap();
    let seat = fdata
        .iter()
        .position(|n| n["f"].as_i64().unwrap_or(0) != 0)
        .unwrap_or(0);

    client.emit_and_wait(
        "set_frequency",
        &json!({"node": seat, "band": "F", "channel": 4, "frequency": 5800}),
        "frequency_data",
        T,
    );

    // Frequency changes are applied asynchronously to the node; give the
    // heartbeat loop a couple of ticks before demanding it.
    let deadline = std::time::Instant::now() + T;
    loop {
        let hb = heartbeat(&client);
        let freq = hb["frequency"][seat].as_i64().unwrap_or(-1);
        if freq == 5800 {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "heartbeat never reported 5800 on seat {} (last {}): {}",
            seat,
            freq,
            hb
        );
    }

    client.disconnect();
    server.stop();
}

/// The frequency-test page sweeps by enabling a scan on the seat: the server
/// advances that node's frequency itself on each heartbeat. The Rust server has
/// no scan support yet -- no `set_scan` handler and no scan fields on the node
/// -- so `scanner.html` cannot produce a spectrogram against it.
///
/// Left as an ignored test so it becomes the acceptance check when the feature
/// is ported: run with `cargo test --release -- --ignored`.
#[test]
#[ignore = "scan/sweep is not implemented in the Rust server (no set_scan handler)"]
fn test_heartbeat_sweeps_frequency_when_scan_enabled() {
    let mut server = spawn_server();
    let mut client = SocketClient::connect(&server.base_url);
    std::thread::sleep(Duration::from_millis(300));
    client.drain();

    client.emit(
        "set_scan",
        &json!({
            "node": 0,
            "min_scan_frequency": 5645,
            "max_scan_frequency": 5945,
            "max_scan_interval": 80,
            "min_scan_interval": 5,
            "scan_zoom": 2,
        }),
    );

    let first = heartbeat(&client);
    let start = first["frequency"][0].as_i64().unwrap_or(-1);

    let mut advanced = false;
    for _ in 0..10 {
        let hb = heartbeat(&client);
        let freq = hb["frequency"][0].as_i64().unwrap_or(-1);
        if freq != start {
            advanced = true;
            break;
        }
    }
    assert!(
        advanced,
        "frequency never advanced after set_scan (stuck at {}); the scan sweep \
         the frequency-test page depends on is not implemented",
        start
    );

    client.disconnect();
    server.stop();
}
