//! Shared application state and domain types for the RotorHazard Rust server.

use rusqlite::Connection;
use socketioxide::SocketIo;
use std::sync::{Arc, Mutex};

pub const RACE_STATUS_READY: i64 = 0;
pub const RACE_STATUS_RACING: i64 = 1;
pub const RACE_STATUS_DONE: i64 = 2;
pub const RACE_STATUS_STAGING: i64 = 3;

/// A single lap crossing in the current (unsaved) race.
/// `lap_time_stamp` is monotonic-based seconds; `lap_time` is milliseconds.
#[derive(Debug, Clone)]
pub struct Lap {
    pub lap_time_stamp: f64,
    pub lap_time: f64,
    pub source: i64,
    pub deleted: bool,
}

/// Live race state, mirroring RHRace fields the protocol depends on.
#[derive(Debug)]
pub struct RaceState {
    pub race_status: i64,
    pub current_heat: Option<i64>,
    pub format_id: i64,
    pub start_time_monotonic: f64, // 0.0 when not started
    pub start_time_formatted: String,
    pub end_time_monotonic: f64,   // 0.0 when not stopped
    pub node_laps: Vec<Vec<Lap>>,
    pub node_pilots: Vec<Option<i64>>, // pilot per node for current heat
    pub scheduled: bool,
    pub scheduled_time: f64,
}

impl RaceState {
    pub fn new(num_nodes: usize) -> Self {
        RaceState {
            race_status: RACE_STATUS_READY,
            current_heat: None,
            format_id: 1,
            start_time_monotonic: 0.0,
            start_time_formatted: String::new(),
            end_time_monotonic: 0.0,
            node_laps: vec![Vec::new(); num_nodes],
            node_pilots: vec![None; num_nodes],
            scheduled: false,
            scheduled_time: 0.0,
        }
    }

    pub fn clear_laps(&mut self) {
        for laps in &mut self.node_laps {
            laps.clear();
        }
        self.start_time_monotonic = 0.0;
        self.start_time_formatted = String::new();
        self.end_time_monotonic = 0.0;
    }
}

/// Per-node mock simulation state (MockInterface equivalent).
#[derive(Debug, Clone)]
pub struct MockNodeState {
    pub current_rssi: i64,
    pub crossing_flag: bool,
    pub lap_number: i64,
    pub pass_peak_rssi: i64,
    pub pass_nadir_rssi: i64,
    pub node_peak_rssi: i64,
    pub node_nadir_rssi: i64,
    pub debug_pass_count: i64,
    pub loop_time: i64,
    /// Peak/nadir RSSI history events (node.history_values/history_times
    /// in Python): the input to the calibration suggestion engine
    pub history_values: Vec<i64>,
    pub history_times: Vec<f64>,
}

impl MockNodeState {
    pub fn new() -> Self {
        MockNodeState {
            current_rssi: 0,
            crossing_flag: false,
            lap_number: 0,
            pass_peak_rssi: 0,
            pass_nadir_rssi: 100,
            node_peak_rssi: 100,
            node_nadir_rssi: 20,
            debug_pass_count: 0,
            loop_time: 1,
            history_values: Vec::new(),
            history_times: Vec::new(),
        }
    }

    /// One history entry; extends the last entry's time when the previous
    /// two values are equal (PeakNadirHistory._addEntry).
    fn history_add_entry(&mut self, value: i64, time: f64) {
        let n = self.history_values.len();
        if n >= 2 && self.history_values[n - 1] == value && self.history_values[n - 2] == value {
            self.history_times[n - 1] = time;
        } else {
            self.history_values.push(value);
            self.history_times.push(time);
        }
    }

    /// Adds one peak/nadir history event (PeakNadirHistory.addTo).
    /// Times are ms *since* the event (entry time = readtime - ms/1000).
    #[allow(clippy::too_many_arguments)]
    pub fn history_add_pn(
        &mut self,
        readtime: f64,
        peak_rssi: i64,
        peak_first_ms: u16,
        peak_last_ms: u16,
        nadir_rssi: i64,
        nadir_first_ms: u16,
        nadir_last_ms: u16,
    ) {
        let add_peak = |s: &mut Self| {
            if peak_first_ms > peak_last_ms {
                s.history_add_entry(peak_rssi, readtime - peak_first_ms as f64 / 1000.0);
                s.history_add_entry(peak_rssi, readtime - peak_last_ms as f64 / 1000.0);
            } else if peak_first_ms == peak_last_ms {
                s.history_add_entry(peak_rssi, readtime - peak_last_ms as f64 / 1000.0);
            }
            // (first < last is corrupted; Python logs and skips)
        };
        let add_nadir = |s: &mut Self| {
            if nadir_first_ms > nadir_last_ms {
                s.history_add_entry(nadir_rssi, readtime - nadir_first_ms as f64 / 1000.0);
                s.history_add_entry(nadir_rssi, readtime - nadir_last_ms as f64 / 1000.0);
            } else if nadir_first_ms == nadir_last_ms {
                s.history_add_entry(nadir_rssi, readtime - nadir_last_ms as f64 / 1000.0);
            }
        };
        if peak_rssi > 0 {
            if nadir_rssi > 0 {
                // both: order by which event ended most recently
                if peak_last_ms > nadir_first_ms {
                    add_peak(self);
                    add_nadir(self);
                } else {
                    add_nadir(self);
                    add_peak(self);
                }
            } else {
                add_peak(self);
            }
        } else if nadir_rssi > 0 {
            add_nadir(self);
        }
    }

    /// Prunes history older than 60 s; only called while the race is not
    /// running (process_history in Python prunes when READY).
    pub fn prune_history(&mut self, now: f64) {
        let cutoff = now - 60.0;
        let prune_count = self
            .history_times
            .iter()
            .take_while(|&&t| t < cutoff)
            .count();
        if prune_count > 0 {
            self.history_values.drain(..prune_count);
            self.history_times.drain(..prune_count);
        }
    }
}

/// Monotonic clock in seconds (comparable to Python's time.monotonic()).
pub fn monotonic() -> f64 {
    use std::time::Instant;
    static START: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
    let start = START.get_or_init(Instant::now);
    start.elapsed().as_secs_f64()
}

pub fn now_epoch_ms() -> f64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as f64)
        .unwrap_or(0.0)
}

pub struct ServerInfo {
    pub release_version: String,
    pub server_api: i64,
    pub node_api_best: i64,
    pub json_api: i64,
}

pub struct AppState {
    pub db: Arc<Mutex<Connection>>,
    pub race: Arc<Mutex<RaceState>>,
    pub num_nodes: usize,
    /// SocketIo handle; set once after the layer is built with state
    pub io: std::sync::OnceLock<SocketIo>,
    pub info: ServerInfo,
    pub data_dir: String,
    pub admin_username: String,
    pub admin_password: String,
    /// GENERAL-section config items (Python keeps these in config.json; the
    /// Rust build holds runtime-set values here for Phase 1)
    pub config_items: Arc<Mutex<std::collections::HashMap<String, serde_json::Value>>>,
    /// Mock-node simulation state (one entry per node)
    pub mock: Mutex<Vec<crate::state::MockNodeState>>,
    /// Active mock signal provider (name + state), per MockSignal.py
    pub mock_provider: Mutex<(String, crate::mock::MockProvider)>,
    pub hardware: Mutex<Option<crate::hardware::HardwareInterface>>,
    /// Instance token emitted in load_all; clients reload when it changes
    pub server_instance_token: f64,
}

pub type SharedState = Arc<AppState>;