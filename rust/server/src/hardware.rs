//! Real hardware interface — wires serial nodes into the server's race
//! state.  Replaces the mock node system when SERIAL_PORTS is configured.
//!
//! Supports two node types:
//! - Full RH LAP_STATS nodes (STM32): the node does crossing detection
//!   on-board and reports everything in a single block read
//! - Simple nodes (ESP32 etc.): no LAP_STATS; the server polls RSSI
//!   peak/nadir and does threshold-based crossing detection itself

use crate::race as race_ops;
use crate::serial::{self, SerialNode};
use crate::state::*;
use serde_json::json;
use std::sync::Arc;
use tracing::info;

pub struct HardwareInterface {
    pub nodes: Vec<Arc<SerialNode>>,
    pub port_names: Vec<String>,
    pub lap_stats_supported: Vec<bool>,
    pub last_rssi: Vec<i64>,
    pub last_crossing: Vec<bool>,
    pub last_peak: Vec<i64>,
    pub last_lap_id: Vec<i32>,
    pub server_crossing_active: Vec<bool>,
    pub server_crossing_start: Vec<f64>,
    /// current enter/exit-at levels (cache; nodes are behind Arc)
    pub cur_enter_at: Vec<i64>,
    pub cur_exit_at: Vec<i64>,
    /// consecutive failed read cycles per node (drives port reopen)
    pub read_fail_streak: Vec<u32>,
    /// set after a port reopen: next lap read syncs the lap counter
    /// without recording a pass (rh_lap_check_pending)
    pub lap_check_pending: Vec<bool>,
}

/// consecutive failed update passes before attempting a port reopen
const REOPEN_AFTER_FAILS: u32 = 10;

impl HardwareInterface {
    pub fn new(port_names: &[String]) -> Self {
        HardwareInterface {
            nodes: Vec::new(),
            port_names: port_names.to_vec(),
            lap_stats_supported: Vec::new(),
            last_rssi: Vec::new(),
            last_crossing: Vec::new(),
            last_peak: Vec::new(),
            last_lap_id: Vec::new(),
            server_crossing_active: Vec::new(),
            server_crossing_start: Vec::new(),
            cur_enter_at: Vec::new(),
            cur_exit_at: Vec::new(),
            read_fail_streak: Vec::new(),
            lap_check_pending: Vec::new(),
        }
    }

    pub fn discover(&mut self, cache_path: Option<&std::path::Path>) {
        let port_paths: Vec<String> = self
            .port_names
            .iter()
            .map(|p| {
                if p.starts_with("/dev/") {
                    p.clone()
                } else {
                    format!("/dev/{}", p)
                }
            })
            .collect();
        self.nodes = serial::discover_nodes(&port_paths, cache_path)
            .into_iter()
            .map(Arc::new)
            .collect();
        let n = self.nodes.len();
        // nodes with API level >= 13 do on-board crossing detection and
        // report full lap stats; older nodes need server-side detection
        self.lap_stats_supported = self.nodes.iter().map(|nd| nd.api_level >= 13).collect();
        self.last_rssi = vec![0; n];
        self.last_crossing = vec![false; n];
        self.last_peak = vec![0; n];
        self.last_lap_id = vec![-1; n];
        self.server_crossing_active = vec![false; n];
        self.server_crossing_start = vec![0.0; n];
        self.cur_enter_at = self.nodes.iter().map(|nd| nd.enter_at_level).collect();
        self.cur_exit_at = self.nodes.iter().map(|nd| nd.exit_at_level).collect();
        self.read_fail_streak = vec![0; n];
        self.lap_check_pending = vec![false; n];
    }

    pub fn num_nodes(&self) -> usize {
        self.nodes.len()
    }

    #[allow(dead_code)]
    pub fn node_frequencies(&self) -> Vec<u16> {
        self.nodes.iter().map(|n| n.frequency).collect()
    }

    pub fn set_frequency(&self, node_index: usize, freq: u16) {
        if let Some(node) = self.nodes.get(node_index) {
            node.write_frequency(freq);
        }
    }

    pub fn set_enter_at_level(&mut self, node_index: usize, level: i64) {
        if let Some(node) = self.nodes.get(node_index) {
            node.write_enter_at_level(level as u16);
        }
        if let Some(v) = self.cur_enter_at.get_mut(node_index) {
            *v = level;
        }
    }

    pub fn set_exit_at_level(&mut self, node_index: usize, level: i64) {
        if let Some(node) = self.nodes.get(node_index) {
            node.write_exit_at_level(level as u16);
        }
        if let Some(v) = self.cur_exit_at.get_mut(node_index) {
            *v = level;
        }
    }

    pub fn force_end_crossing(&self, node_index: usize) {
        if let Some(node) = self.nodes.get(node_index) {
            node.force_end_crossing();
        }
    }

    /// One update pass over all hardware nodes.
    pub fn update(&mut self, state: &SharedState, now: f64) {
        let (racing, race_status) = {
            let race = state.race.lock().unwrap();
            (race.race_status == RACE_STATUS_RACING, race.race_status)
        };

        for idx in 0..self.nodes.len() {
            if idx >= self.lap_stats_supported.len() {
                break;
            }
            let node = self.nodes[idx].clone();

            if self.lap_stats_supported[idx] {
                // Full LAP_STATS node (STM32 etc.) — crossing detection
                // done on-board, reported in the block read
                match node.read_lap_stats() {
                    Some(stats) => {
                        self.process_lap_stats(state, idx, &node, stats, racing, race_status, now)
                    }
                    None => self.handle_read_failure(idx, &node),
                }
            } else {
                // Simple node — server-side crossing detection
                self.poll_simple_node(state, idx, &node, racing, now);
            }
        }
    }

    /// Scans /dev for newly-attached serial nodes and adds them at
    /// runtime (check_serial_ports_appeared in server.py).  Candidate
    /// ports are ttyUSB*/ttyACM* devices not already managed; `exclude`
    /// filters configured/other-service ports.
    pub fn hotplug_scan(&mut self, state: &SharedState, exclude: &[String]) {
        let dir = match std::fs::read_dir("/dev") {
            Ok(d) => d,
            Err(_) => return,
        };
        let managed: std::collections::HashSet<String> = self
            .port_names
            .iter()
            .chain(self.nodes.iter().map(|n| &n.port))
            .map(|p| p.trim_start_matches("/dev/").to_string())
            .collect();
        let mut candidates: Vec<String> = Vec::new();
        for entry in dir.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if !(name.starts_with("ttyUSB") || name.starts_with("ttyACM")) {
                continue;
            }
            if managed.contains(&name) || exclude.iter().any(|e| e.contains(&name)) {
                continue;
            }
            candidates.push(name);
        }
        if candidates.is_empty() {
            return;
        }
        let race_status = state.race.lock().unwrap().race_status;
        if race_status == RACE_STATUS_RACING || race_status == RACE_STATUS_STAGING {
            return; // no discovery while a race is live (Python parity)
        }
        let port_paths: Vec<String> = candidates
            .iter()
            .map(|c| format!("/dev/{}", c))
            .collect();
        tracing::info!("New serial port(s) detected: {:?}", port_paths);
        let found = serial::discover_nodes(&port_paths, None);
        for node in found {
            let idx = self.nodes.len();
            tracing::info!("Hot-plugged serial node {} at {} (API {}, fw {})",
                idx + 1, node.port, node.api_level, node.firmware_version);
            self.lap_stats_supported.push(node.api_level >= 13);
            self.last_rssi.push(0);
            self.last_crossing.push(false);
            self.last_peak.push(0);
            self.last_lap_id.push(-1);
            self.server_crossing_active.push(false);
            self.server_crossing_start.push(0.0);
            self.cur_enter_at.push(node.enter_at_level);
            self.cur_exit_at.push(node.exit_at_level);
            self.read_fail_streak.push(0);
            self.lap_check_pending.push(false);
            self.port_names.push(node.port.clone());
            let arc = Arc::new(node);
            // adopt the DB profile's frequency for the new seat
            let dbf = crate::db::with_db(state, |conn| {
                let pid = crate::db::current_profile_id(conn);
                crate::db::profile_frequencies(conn, pid)
                    .get(idx)
                    .map(|(_, _, f)| *f)
                    .unwrap_or(0)
            });
            if dbf != 0 && dbf != arc.frequency as i64 {
                arc.write_frequency(dbf as u16);
            }
            self.nodes.push(arc);
        }
        if self.nodes.len() > self.lap_stats_supported.len() {
            // keep the vectors consistent even on partial discovery
            self.pad_state_vectors();
        }
        // grow the mock-seat state so the UI/heartbeat cover the new seats
        {
            let mut mock = state.mock.lock().unwrap();
            while mock.len() < self.nodes.len() {
                mock.push(MockNodeState::new());
            }
        }
        // grow the seat count so the UI shows the new seats
        state.set_node_count(self.nodes.len());
        crate::socket::broadcast(state, "node_data", crate::payloads::build_node_data(&state.race.lock().unwrap()));
    }

    fn pad_state_vectors(&mut self) {
        let n = self.nodes.len();
        let pad = |v: &mut Vec<i64>| while v.len() < n { v.push(0) };
        pad(&mut self.last_rssi);
        pad(&mut self.last_peak);
        while self.last_crossing.len() < n { self.last_crossing.push(false); }
        while self.lap_stats_supported.len() < n { self.lap_stats_supported.push(false); }
        while self.last_lap_id.len() < n { self.last_lap_id.push(-1); }
        while self.server_crossing_active.len() < n { self.server_crossing_active.push(false); }
        while self.server_crossing_start.len() < n { self.server_crossing_start.push(0.0); }
        while self.cur_enter_at.len() < n { self.cur_enter_at.push(0); }
        while self.cur_exit_at.len() < n { self.cur_exit_at.push(0); }
        while self.read_fail_streak.len() < n { self.read_fail_streak.push(0); }
        while self.lap_check_pending.len() < n { self.lap_check_pending.push(false); }
    }

    /// Records a failed read pass and reopens the port + restores node
    /// settings after a sustained failure (_check_restore_node_settings).
    fn handle_read_failure(&mut self, idx: usize, node: &SerialNode) {
        self.read_fail_streak[idx] += 1;
        if self.read_fail_streak[idx] >= REOPEN_AFTER_FAILS {
            self.read_fail_streak[idx] = 0;
            info!("Node {} read failures; attempting port reopen at {}", idx + 1, node.port);
            if node.reopen() {
                self.lap_check_pending[idx] = true;
                // restore settings the node lost across its reset
                if node.write_frequency(node.frequency) {
                    node.write_enter_at_level(node.enter_at_level as u16);
                    node.write_exit_at_level(node.exit_at_level as u16);
                    info!("Node {} port reopened; frequency and levels restored", idx + 1);
                }
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn process_lap_stats(
        &mut self,
        state: &SharedState,
        idx: usize,
        _node: &SerialNode,
        stats: crate::serial::LapStats,
        racing: bool,
        race_status: i64,
        now: f64,
    ) {
        self.read_fail_streak[idx] = 0;

        // node restarted while the port was down: resync the lap counter
        // without recording a pass (rh_lap_check_pending in Python)
        if self.lap_check_pending[idx] {
            self.lap_check_pending[idx] = false;
            if self.last_lap_id[idx] != -1 && stats.lap_id < self.last_lap_id[idx] {
                self.last_lap_id[idx] = -1;
            }
        }

        self.last_rssi[idx] = stats.rssi_val;
        let cross = stats.crossing_flag.unwrap_or(false);
        if cross != self.last_crossing[idx] {
            // crossing status changed: LED gate enter/exit events
            let event = if cross { "crossingEnter" } else { "crossingExit" };
            state.led_manager.trigger(event, &json!({"node_index": idx}));
            let args = json!({
                "node_index": idx,
                "pilot_id": state.race.lock().unwrap().node_pilots.get(idx).copied().flatten(),
            });
            state.actions_engine.do_actions(state, event, &args);
            crate::socket::emit_plugin_event(state, event, &args);
        }
        self.last_crossing[idx] = cross;

        // new lap detected: register it (process_updates in
        // BaseHardwareInterface.py); the first read only
        // syncs the lap counter, it does not record a pass
        if stats.lap_id != self.last_lap_id[idx] {
            let prev_lap_id = self.last_lap_id[idx];
            self.last_lap_id[idx] = stats.lap_id;
            if prev_lap_id != -1 && racing {
                // lap timestamp is node-relative: now minus ms_val
                let lap_time = now - (stats.ms_val as f64 / 1000.0);
                race_ops::register_lap_at(state, idx, lap_time.max(0.0));
            }
        }

        let mut mock = state.mock.lock().unwrap();
        if let Some(m) = mock.get_mut(idx) {
            m.current_rssi = stats.rssi_val;
            m.crossing_flag = cross;
            m.node_peak_rssi = stats.node_peak_rssi;
            m.node_nadir_rssi = stats.node_nadir_rssi;
            m.pass_peak_rssi = stats.pass_peak_rssi;
            m.pass_nadir_rssi = stats.pass_nadir_rssi;
            m.loop_time = stats.loop_time.max(1);
            m.debug_pass_count += 1;
            // peak/nadir history events feed the calibration engine
            // (api < 18 nodes report no history; DONE races record none)
            if _node.api_level >= 18 && race_status != RACE_STATUS_DONE {
                m.history_add_pn(
                    now,
                    stats.pn_peak_rssi,
                    stats.pn_peak_first_ms,
                    stats.pn_peak_last_ms,
                    stats.pn_nadir_rssi,
                    stats.pn_nadir_first_ms,
                    stats.pn_nadir_last_ms,
                );
            }
            if race_status == RACE_STATUS_READY {
                m.prune_history(now);
            }
        }
    }

    fn poll_simple_node(
        &mut self,
        state: &SharedState,
        idx: usize,
        node: &SerialNode,
        racing: bool,
        now: f64,
    ) {
        let was_crossing = self.server_crossing_active[idx];

        let peak_read = node.read_block(serial::READ_NODE_RSSI_PEAK, 1);
        let peak = match peak_read {
            Some(d) => d[0] as i64,
            None => {
                self.handle_read_failure(idx, node);
                return;
            }
        };
        self.read_fail_streak[idx] = 0;

        let enter_at = self.cur_enter_at.get(idx).copied().unwrap_or(node.enter_at_level);
        let exit_at = self.cur_exit_at.get(idx).copied().unwrap_or(node.exit_at_level);

        // crossing state machine:
        // - start: peak rises above enter_at
        // - end: peak drops below exit_at
        if !was_crossing && peak > enter_at {
            self.server_crossing_active[idx] = true;
            self.server_crossing_start[idx] = now;
            self.last_crossing[idx] = true;
            state.led_manager.trigger("crossingEnter", &json!({"node_index": idx}));
        } else if was_crossing && peak < exit_at {
            self.server_crossing_active[idx] = false;
            self.last_crossing[idx] = false;
            if racing {
                race_ops::register_lap_at(state, idx, now);
            }
            state.led_manager.trigger("crossingExit", &json!({"node_index": idx}));
        }

        let rssi = if self.server_crossing_active[idx] {
            peak
        } else {
            peak.min(enter_at)
        };
        self.last_rssi[idx] = rssi;
        self.last_peak[idx] = peak;

        {
            let mut mock = state.mock.lock().unwrap();
            if let Some(m) = mock.get_mut(idx) {
                m.current_rssi = rssi;
                m.crossing_flag = self.server_crossing_active[idx];
                m.node_peak_rssi = peak.max(m.node_peak_rssi);
                m.pass_peak_rssi = if self.server_crossing_active[idx] { peak } else { m.pass_peak_rssi };
                m.pass_nadir_rssi = if !self.server_crossing_active[idx] { rssi } else { m.pass_nadir_rssi };
            }
        }
    }
}