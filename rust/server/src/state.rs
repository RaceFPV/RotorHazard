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
}

pub type SharedState = Arc<AppState>;