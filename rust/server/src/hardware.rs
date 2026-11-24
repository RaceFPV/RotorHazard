//! Real hardware interface — wires serial nodes into the server\x27s race
//! state.  Replaces the mock node system when SERIAL_PORTS is configured.
//!
//! Supports nodes that implement the full RH LAP_STATS block (STM32
//! nodes) and falls back to server-side crossing detection for simpler
//! nodes (e.g. ESP32) that expose RSSI peak/nadir but not the full stats
//! block.

use crate::serial::{self, SerialNode};
use crate::state::*;
use std::sync::Arc;

pub struct HardwareInterface {
    pub nodes: Vec<Arc<SerialNode>>,
    pub port_names: Vec<String>,
    pub lap_stats_supported: Vec<bool>,
    pub last_rssi: Vec<i64>,
    pub last_crossing: Vec<bool>,
}

impl HardwareInterface {
    pub fn new(port_names: &[String]) -> Self {
        HardwareInterface {
            nodes: Vec::new(),
            port_names: port_names.to_vec(),
            lap_stats_supported: Vec::new(),
            last_rssi: Vec::new(),
            last_crossing: Vec::new(),
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

    /// One update pass: poll each node for lap stats (if supported) or
    /// RSSI peak/nadir changes (server-side crossing detection).
    pub fn update(&mut self, state: &SharedState, now: f64) {
        let racing_start = {
            let race = state.race.lock().unwrap();
            if race.race_status == RACE_STATUS_RACING {
                Some(race.start_time_monotonic)
            } else {
                None
            }
        };

        let node_count = self.nodes.len().min(self.lap_stats_supported.len());
        for idx in 0..node_count {
            let node = &self.nodes[idx];

            if self.lap_stats_supported[idx] {
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
                        }
                    }

                    if was_crossing && !stats.crossing_flag && racing_start.is_some() {
                        crate::race::register_lap_at(state, idx, now);
                    }
                }
            } else {
                // server-side crossing detection via RSSI peak/nadir
                let was_crossing = self.last_crossing[idx];
                let prev_rssi = self.last_rssi[idx];

                let peak = node
                    .read_block(serial::READ_NODE_RSSI_PEAK, 1)
                    .and_then(|d| Some(d[0] as i64))
                    .unwrap_or(prev_rssi);

                let enter_at = node.enter_at_level;
                let exit_at = node.exit_at_level;
                let crossing = peak > exit_at && peak >= enter_at.saturating_sub(10);
                let rssi = if crossing { peak } else { enter_at.min(peak) };

                self.last_rssi[idx] = rssi;
                self.last_crossing[idx] = crossing;

                {
                    let mut mock = state.mock.lock().unwrap();
                    if let Some(m) = mock.get_mut(idx) {
                        m.current_rssi = rssi;
                        m.crossing_flag = crossing;
                        m.node_peak_rssi = peak;
                        m.pass_peak_rssi = if crossing { peak } else { m.pass_peak_rssi };
                    }
                }

                if was_crossing && !crossing && racing_start.is_some() {
                    crate::race::register_lap_at(state, idx, now);
                }
            }
        }
    }
}
