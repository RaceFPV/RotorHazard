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
use std::sync::Arc;

pub struct HardwareInterface {
    pub nodes: Vec<Arc<SerialNode>>,
    pub port_names: Vec<String>,
    pub lap_stats_supported: Vec<bool>,
    pub last_rssi: Vec<i64>,
    pub last_crossing: Vec<bool>,
    pub last_peak: Vec<i64>,
    pub server_crossing_active: Vec<bool>,
    pub server_crossing_start: Vec<f64>,
}

impl HardwareInterface {
    pub fn new(port_names: &[String]) -> Self {
        HardwareInterface {
            nodes: Vec::new(),
            port_names: port_names.to_vec(),
            lap_stats_supported: Vec::new(),
            last_rssi: Vec::new(),
            last_crossing: Vec::new(),
            last_peak: Vec::new(),
            server_crossing_active: Vec::new(),
            server_crossing_start: Vec::new(),
        }
    }

    pub fn discover(&mut self) {
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
        self.nodes = serial::discover_nodes(&port_paths)
            .into_iter()
            .map(Arc::new)
            .collect();
        let n = self.nodes.len();
        self.lap_stats_supported = vec![false; n];
        self.last_rssi = vec![0; n];
        self.last_crossing = vec![false; n];
        self.last_peak = vec![0; n];
        self.server_crossing_active = vec![false; n];
        self.server_crossing_start = vec![0.0; n];
    }

    pub fn num_nodes(&self) -> usize {
        self.nodes.len()
    }

    pub fn node_frequencies(&self) -> Vec<u16> {
        self.nodes.iter().map(|n| n.frequency).collect()
    }

    pub fn set_frequency(&self, node_index: usize, freq: u16) {
        if let Some(node) = self.nodes.get(node_index) {
            node.write_frequency(freq);
        }
    }

    pub fn set_enter_at_level(&self, node_index: usize, level: i64) {
        if let Some(node) = self.nodes.get(node_index) {
            node.write_enter_at_level(level as u16);
        }
    }

    pub fn set_exit_at_level(&self, node_index: usize, level: i64) {
        if let Some(node) = self.nodes.get(node_index) {
            node.write_exit_at_level(level as u16);
        }
    }

    pub fn force_end_crossing(&self, node_index: usize) {
        if let Some(node) = self.nodes.get(node_index) {
            node.force_end_crossing();
        }
    }

    /// One update pass over all hardware nodes.
    pub fn update(&mut self, state: &SharedState, now: f64) {
        let (racing, _racing_start) = {
            let race = state.race.lock().unwrap();
            (
                race.race_status == RACE_STATUS_RACING,
                race.start_time_monotonic,
            )
        };

        for idx in 0..self.nodes.len() {
            if idx >= self.lap_stats_supported.len() {
                break;
            }
            let node = self.nodes[idx].clone();

            if self.lap_stats_supported[idx] {
                // Full LAP_STATS node (STM32 etc.) — crossing detection
                // done on-board, reported in the block read
                if let Some(stats) = node.read_lap_stats() {
                    let was_crossing = self.last_crossing[idx];
                    self.last_rssi[idx] = stats.rssi_val;
                    self.last_crossing[idx] = stats.crossing_flag;

                    {
                        let mut mock = state.mock.lock().unwrap();
                        if let Some(m) = mock.get_mut(idx) {
                            m.current_rssi = stats.rssi_val;
                            m.crossing_flag = stats.crossing_flag;
                            m.node_peak_rssi = stats.node_peak_rssi;
                            m.node_nadir_rssi = stats.node_nadir_rssi;
                            m.pass_peak_rssi = stats.pass_peak_rssi;
                            m.pass_nadir_rssi = stats.pass_nadir_rssi;
                            m.debug_pass_count += 1;
                        }
                    }

                    if was_crossing && !stats.crossing_flag && racing {
                        race_ops::register_lap_at(state, idx, now);
                    }
                }
            } else {
                // Simple node — server-side crossing detection
                self.poll_simple_node(state, idx, &node, racing, now);
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
        let prev_peak = self.last_peak[idx];

        let peak = node
            .read_block(serial::READ_NODE_RSSI_PEAK, 1)
            .map(|d| d[0] as i64)
            .unwrap_or(prev_peak);

        let enter_at = node.enter_at_level;
        let exit_at = node.exit_at_level;

        // crossing state machine:
        // - start: peak rises above enter_at
        // - end: peak drops below exit_at
        if !was_crossing && peak > enter_at {
            self.server_crossing_active[idx] = true;
            self.server_crossing_start[idx] = now;
            self.last_crossing[idx] = true;
        } else if was_crossing && peak < exit_at {
            self.server_crossing_active[idx] = false;
            self.last_crossing[idx] = false;
            if racing {
                race_ops::register_lap_at(state, idx, now);
            }
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