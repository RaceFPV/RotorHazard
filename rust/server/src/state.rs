//! Shared application state and domain types for the RotorHazard server.

use rusqlite::Connection;
use socketioxide::SocketIo;
use std::sync::{Arc, Mutex};

pub const RACE_STATUS_READY: i64 = 0;
pub const RACE_STATUS_RACING: i64 = 1;
pub const RACE_STATUS_DONE: i64 = 2;
pub const RACE_STATUS_STAGING: i64 = 3;

/// A single lap crossing in the current (unsaved) race.
/// `lap_time_stamp` and `lap_time` are milliseconds since race start
/// (the Python convention).
#[derive(Debug, Clone)]
pub struct Lap {
    pub lap_time_stamp: f64,
    pub lap_time: f64,
    pub source: i64,
    pub deleted: bool,
    pub peak_rssi: Option<i64>,
}

/// Stored undo state for the last auto adjustment of a seat or saved pilot
/// run (Calibration._undo in Python), keyed "seat:<idx>" or "run:<id>".
#[derive(Debug, Clone)]
pub enum CalUndo {
    Run {
        enter_at: i64,
        exit_at: i64,
        laps: Vec<UndoLap>,
        node: Option<(usize, i64, i64)>, // (node_index, node enter_at, node exit_at)
    },
    Seat {
        start_time: f64,
        enter_at: i64,
        exit_at: i64,
        /// Recovered lap time stamps (recover mode) when present.
        stamps: Option<Vec<f64>>,
        /// Lap snapshot (replace mode) when present.
        laps: Option<Vec<UndoLap>>,
    },
}

/// A lap snapshot as stored for undo.
#[derive(Debug, Clone)]
pub struct UndoLap {
    pub lap_time_stamp: f64,
    pub lap_time: f64,
    pub peak_rssi: Option<i64>,
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
    pub start_time_epoch_ms: f64,  // epoch ms when the race started (0 before)
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
            start_time_epoch_ms: 0.0,
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
        self.start_time_epoch_ms = 0.0;
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

#[cfg(test)]
mod history_tests {
    use super::*;

    fn node() -> MockNodeState {
        MockNodeState::new()
    }

    #[test]
    fn add_entry_extends_when_last_two_equal() {
        let mut n = node();
        n.history_add_entry(50, 1.0);
        n.history_add_entry(50, 2.0);
        // third equal value extends the second entry's time instead of appending
        n.history_add_entry(50, 3.0);
        assert_eq!(n.history_values, vec![50, 50]);
        assert_eq!(n.history_times, vec![1.0, 3.0]);
        // a different value appends normally
        n.history_add_entry(60, 4.0);
        assert_eq!(n.history_values, vec![50, 50, 60]);
    }

    #[test]
    fn add_pn_peak_and_nadir_both_ordering() {
        let mut n = node();
        // times are ms SINCE the event: peak ended 200ms ago (started 300ms
        // ago), nadir started 100ms ago -> peakLast > nadirFirst -> peak first
        n.history_add_pn(10.0, 200, 300, 200, 40, 100, 50);
        // peak: 10-0.3=9.7, 10-0.2=9.8; nadir: 10-0.1=9.9, 10-0.05=9.95
        assert_eq!(n.history_values, vec![200, 200, 40, 40]);
        assert_eq!(n.history_times, vec![9.7, 9.8, 9.9, 9.95]);
    }

    #[test]
    fn add_pn_nadir_first_when_nadir_happened_first() {
        let mut n = node();
        // nadir happened before the peak: nadirLast(250) vs peakLast(50):
        // peakLast(50) > nadirFirst(300)? no -> nadir processed first
        n.history_add_pn(10.0, 200, 100, 50, 40, 300, 250);
        // nadir: 10-0.3=9.7, 10-0.25=9.75; peak: 10-0.1=9.9, 10-0.05=9.95
        assert_eq!(n.history_values, vec![40, 40, 200, 200]);
        assert_eq!(n.history_times, vec![9.7, 9.75, 9.9, 9.95]);
    }

    #[test]
    fn add_pn_peak_only_and_nadir_only() {
        let mut n = node();
        n.history_add_pn(5.0, 200, 100, 50, 0, 0, 0);
        assert_eq!(n.history_values, vec![200, 200]);
        let mut n = node();
        n.history_add_pn(5.0, 0, 0, 0, 30, 100, 50);
        assert_eq!(n.history_values, vec![30, 30]);
        // equal first/last times produce a single entry
        let mut n = node();
        n.history_add_pn(5.0, 200, 100, 100, 0, 0, 0);
        assert_eq!(n.history_values, vec![200]);
        assert_eq!(n.history_times, vec![4.9]);
    }

    #[test]
    fn add_pn_zero_rssi_records_nothing() {
        let mut n = node();
        n.history_add_pn(5.0, 0, 0, 0, 0, 0, 0);
        assert!(n.history_values.is_empty());
    }

    #[test]
    fn prune_history_drops_old_entries() {
        let mut n = node();
        n.history_add_entry(10, 0.0);
        n.history_add_entry(20, 30.0);
        n.history_add_entry(30, 59.5);
        n.history_add_entry(40, 59.9);
        // now = 60: cutoff = 0; entries strictly below 0 are pruned
        n.prune_history(60.0);
        assert_eq!(n.history_values, vec![10, 20, 30, 40]);
        // now = 70: cutoff 10; the 0.0 entry goes
        n.prune_history(70.0);
        assert_eq!(n.history_values, vec![20, 30, 40]);
        // now = 100: cutoff 40; the 30.0 entry goes too
        n.prune_history(100.0);
        assert_eq!(n.history_values, vec![30, 40]);
    }
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

/// Per-seat frequency scan (Python Node.set_scan_interval + the sweep in
/// get_heartbeat_json). A scanner sweeps its node across a band by adding
/// `interval` to the frequency on every heartbeat, wrapping at the band edges
/// and narrowing the step by `zoom` each pass.
#[derive(Clone, Debug, Default)]
pub struct ScanState {
    pub enabled: bool,
    pub min_freq: f64,
    pub max_freq: f64,
    pub max_interval: f64,
    pub min_interval: f64,
    pub zoom: f64,
    pub interval: f64,
    pub current_freq: f64,
}

pub struct AppState {
    pub db: Arc<Mutex<Connection>>,
    pub race: Arc<Mutex<RaceState>>,
    /// Number of seats; grows when nodes hot-plug at runtime
    pub num_nodes: std::sync::atomic::AtomicUsize,
    /// SocketIo handle; set once after the layer is built with state
    pub io: std::sync::OnceLock<SocketIo>,
    pub info: ServerInfo,
    pub data_dir: String,
    pub admin_username: String,
    pub admin_password: String,
    /// GENERAL-section config items (Python keeps these in config.json; the
    /// Rust build holds runtime-set values here for Phase 1)
    pub config_items: Arc<Mutex<std::collections::HashMap<String, serde_json::Value>>>,
    /// Per-seat scan state for the frequency-test page (one entry per node)
    pub scan: Mutex<Vec<ScanState>>,
    /// Mock-node simulation state (one entry per node)
    pub mock: Mutex<Vec<crate::state::MockNodeState>>,
    /// Active mock signal provider (name + state), per MockSignal.py
    pub mock_provider: Mutex<(String, crate::mock::MockProvider)>,
    pub hardware: Mutex<Option<crate::hardware::HardwareInterface>>,
    /// LED event manager (led_event_manager.py port)
    pub led_manager: crate::led::LedManager,
    /// VRx Control manager (VRxControl.py port; controllers come from plugins)
    pub vrx_manager: crate::vrx::VRxControlManager,
    /// Cluster node set (ClusterNodeSet.py port); built after AppState
    pub cluster: std::sync::OnceLock<crate::cluster::ClusterNodeSet>,
    /// Event actions engine (EventActions.py + rh_actions_builtin port)
    pub actions_engine: crate::actions::ActionEngine,
    /// Set when a change requires a server restart (node fw update)
    pub restart_required: std::sync::atomic::AtomicBool,
    /// True when this timer has joined a primary as a secondary
    pub cluster_joined: std::sync::atomic::AtomicBool,
    /// Instance token emitted in load_all; clients reload when it changes
    pub server_instance_token: f64,
    /// Undo states for the last auto adjustments (calibration module)
    pub cal_undo: Mutex<std::collections::HashMap<String, CalUndo>>,
    /// Plugin-registered fetchable config sections (ui.register_config_section):
    /// section -> None for the whole section, Some(items) for an allowlist.
    pub config_load_items: Mutex<std::collections::HashMap<String, Option<Vec<String>>>>,
    /// Socket events plugins subscribed to (ui.socket_listen). The connect
    /// handler registers one forwarding handler per name, so a client emit of
    /// that event reaches the plugin and its return value becomes the ack.
    pub plugin_listeners: Mutex<Vec<String>>,
    /// domain -> load error (None = loaded). Reported by the bridge.
    pub plugin_load_status: Mutex<std::collections::HashMap<String, Option<String>>>,
    /// Invocations awaiting the bridge's reply, keyed by call id.
    pub plugin_pending:
        Mutex<std::collections::HashMap<u64, tokio::sync::oneshot::Sender<serde_json::Value>>>,
    /// Monotonic id for server -> bridge invocations.
    pub plugin_seq: std::sync::atomic::AtomicU64,
    /// Panels plugins registered (ui.register_panel): name, label, page, order.
    pub plugin_panels: Mutex<Vec<serde_json::Value>>,
    /// Fields plugins added to those panels (fields.register_option).
    pub plugin_fields: Mutex<Vec<serde_json::Value>>,
    /// Markdown blocks plugins added to those panels (ui.register_markdown).
    pub plugin_markdowns: Mutex<Vec<serde_json::Value>>,
    /// Buttons plugins added to those panels (ui.register_quickbutton).
    pub plugin_quickbuttons: Mutex<Vec<serde_json::Value>>,
    /// HTTP routes plugins registered (ui.blueprint_add): domain, rule, methods.
    pub plugin_routes: Mutex<Vec<serde_json::Value>>,
}

impl AppState {
    /// Current seat count (num_nodes is runtime-growable).
    pub fn node_count(&self) -> usize {
        self.num_nodes.load(std::sync::atomic::Ordering::Relaxed)
    }

    #[allow(dead_code)] // hot-plug updates via set_node_count on add
    pub fn set_node_count(&self, n: usize) {
        self.num_nodes.store(n, std::sync::atomic::Ordering::Relaxed);
    }
}

pub type SharedState = Arc<AppState>;

/// The release version string reported to clients and cluster peers.
pub const RELEASE_VERSION: &str = "4.6.0-dev.7-rust";