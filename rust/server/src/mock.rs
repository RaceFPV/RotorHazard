//! Mock signal providers, ported from src/interface/MockSignal.py.
//!
//! The 'MOCK_NODE_SIGNAL' setting names a provider: '0' none, '1' CSV
//! replay (mock_data_<n>.csv in the data directory), '2' simulated random
//! crossings, '3'/'4' pass-peak-only variants.  The setting may hold the
//! number or the provider name; an unknown name falls back to 'none'.

use crate::state::MockNodeState;
use serde_json::json;
use serde_json::Value;

#[derive(Debug, Clone)]
pub struct SignalSample {
    pub lap_id: i64,
    pub rssi: i64,
    pub cross_flag: bool,
    pub node_peak_rssi: i64,
    pub pass_peak_rssi: i64,
    pub pass_nadir_rssi: i64,
    pub node_nadir_rssi: i64,
    pub loop_time: i64,
    pub history_flag: bool,
}

#[derive(Debug, Clone)]
pub struct ReplayRecord {
    pub read_time: Option<f64>,
    pub sample: SignalSample,
}

#[derive(Debug, Clone)]
pub struct ReplayState {
    pub records: Vec<ReplayRecord>,
    pub timed: bool,
    pub pos: usize,
    pub base_now: Option<f64>,
    pub base_time: f64,
}

#[derive(Debug, Clone)]
pub struct SimulatedState {
    pub pass_peak_only: bool,
    pub history_flag: bool,
    /// Per-seat crossing state, held by the provider across readings
    /// (as in MockSignal.SimulatedSignal._state).
    pub state: std::collections::HashMap<usize, SimState>,
}

#[derive(Debug, Clone, Copy)]
pub struct SimState {
    pub lap_number: i64,
    pub is_crossing: bool,
    pub pass_peak_rssi: i64,
    pub pass_nadir_rssi: i64,
}



#[derive(Debug, Clone)]
pub enum MockProvider {
    NoSignal,
    ReplayCsv(Vec<Option<ReplayState>>),
    Simulated(SimulatedState),
}

pub const MAX_READINGS_PER_UPDATE: usize = 200;

/// Parses one CSV line in the hardware-interface log format:
/// read_time, lap_id, ms_val, rssi, node_peak, pass_peak, loop_time,
/// cross_flag(T/F), pass_nadir, node_nadir, peak, peakFirst, peakLast,
/// nadir, nadirFirst, nadirLast
pub fn parse_csv_line(line: &str) -> Option<ReplayRecord> {
    let cols: Vec<&str> = line.trim().split(',').collect();
    if cols.len() < 16 {
        return None;
    }
    let num = |i: usize| -> Option<i64> { cols[i].trim().parse::<i64>().ok() };
    let sample = SignalSample {
        lap_id: num(1)?,
        rssi: num(3)?,
        cross_flag: cols[7].trim() == "T",
        node_peak_rssi: num(4)?,
        pass_peak_rssi: num(5)?,
        pass_nadir_rssi: num(8)?,
        node_nadir_rssi: num(9)?,
        loop_time: num(6)?,
        history_flag: true,
    };
    let read_time = cols[0].trim().parse::<f64>().ok();
    Some(ReplayRecord {
        read_time,
        sample,
    })
}

/// Loads a replay file; None when missing or empty.
pub fn load_replay(path: &str) -> Option<ReplayState> {
    let text = std::fs::read_to_string(path).ok()?;
    let mut records = Vec::new();
    let mut bad = 0usize;
    for line in text.lines() {
        if line.trim().is_empty() {
            continue;
        }
        match parse_csv_line(line) {
            Some(rec) => records.push(rec),
            None => bad += 1,
        }
    }
    if bad > 0 {
        tracing::warn!("Skipped {} malformed line(s) in {}", bad, path);
    }
    if records.is_empty() {
        return None;
    }
    let times: Vec<Option<f64>> = records.iter().map(|r| r.read_time).collect();
    let timed = times.iter().all(|t| t.is_some())
        && times
            .windows(2)
            .all(|w| w[1].unwrap() > w[0].unwrap());
    tracing::info!(
        "Loaded {} ({} readings{})",
        path,
        records.len(),
        if timed { ", at recorded times" } else { "" }
    );
    Some(ReplayState {
        records,
        timed,
        pos: 0,
        base_now: None,
        base_time: 0.0,
    })
}

/// The display entries for the Advanced Settings signal selector.
pub fn signal_entries() -> Value {
    json!([
        {"name": "0", "label": "None", "provider": {"label_detail": ""}},
        {"name": "1", "label": "Replay", "provider": {"label_detail": "mock_data_[n].csv"}},
        {"name": "2", "label": "Simulated", "provider": {"label_detail": ""}},
        {"name": "3", "label": "Simulated - Pass Peak Only", "provider": {"label_detail": ""}},
        {"name": "4", "label": "Simulated - Pass Peak Only (+RSSI)", "provider": {"label_detail": ""}},
    ])
}

/// Resolves the 'MOCK_NODE_SIGNAL' setting to a provider; unknown names
/// fall back to none (matching MockInterface.get_signal_provider).
pub fn provider_from_setting(value: &str) -> MockProvider {
    match value {
        "1" | "replay" | "Replay" => MockProvider::ReplayCsv(Vec::new()),
        "2" | "simulated" | "Simulated" => MockProvider::Simulated(SimulatedState {
            pass_peak_only: false,
            history_flag: true,
            state: std::collections::HashMap::new(),
        }),
        "3" => MockProvider::Simulated(SimulatedState {
            pass_peak_only: true,
            history_flag: false,
            state: std::collections::HashMap::new(),
        }),
        "4" => MockProvider::Simulated(SimulatedState {
            pass_peak_only: true,
            history_flag: true,
            state: std::collections::HashMap::new(),
        }),
        _ => MockProvider::NoSignal,
    }
}

impl MockProvider {
    /// The marshal type this provider presents (MarshalType values from
    /// BaseHardwareInterface).
    pub fn marshal_type(&self) -> i64 {
        match self {
            MockProvider::NoSignal => 0,
            MockProvider::ReplayCsv(_) => 0,
            MockProvider::Simulated(sim) => {
                if sim.pass_peak_only && sim.history_flag {
                    1 // HYBRID_PASS_PEAK_RSSI
                } else if sim.pass_peak_only {
                    2 // PASS_PEAK_ONLY
                } else {
                    0 // FULL_RSSI
                }
            }
        }
    }

    /// Produces the next sample for a seat, advancing provider state.
    /// `data_dir` locates mock_data_<n>.csv files for the replay provider.
    pub fn read(&mut self, node: &mut MockNodeState, node_index: usize, now: f64,
                data_dir: &str) -> Option<SignalSample> {
        match self {
            MockProvider::NoSignal => None,
            MockProvider::Simulated(sim) => {
                let crossing_pct: i64 = std::env::var("RH_MOCK_CROSSING_PCT")
                    .ok()
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(5);
                let sim_state = sim
                    .state
                    .entry(node_index)
                    .or_insert(SimState {
                        lap_number: node.lap_number,
                        is_crossing: node.crossing_flag,
                        pass_peak_rssi: node.pass_peak_rssi,
                        pass_nadir_rssi: node.pass_nadir_rssi,
                    });
                let mut seed = seed_from_now(now);
                // mix the node index in so seats diverge
                seed = seed.wrapping_add((node_index as u64 + 1).wrapping_mul(0x9E3779B9));
                let sample = simulated_read(sim_state, &mut seed, crossing_pct, sim.history_flag);
                node.lap_number = sim_state.lap_number;
                Some(sample)
            }
            MockProvider::ReplayCsv(replays) => {
                while replays.len() <= node_index {
                    replays.push(None);
                }
                if replays[node_index].is_none() {
                    replays[node_index] = load_replay(&format!(
                        "{}/mock_data_{}.csv", data_dir, node_index + 1));
                }
                let replay = replays[node_index].as_mut()?;
                if !replay.timed {
                    let fields = replay.records[replay.pos].sample.clone();
                    replay.pos = (replay.pos + 1) % replay.records.len();
                    return Some(fields);
                }
                // timed replay: emit readings at their recorded times
                let mut samples: Vec<SignalSample> = Vec::new();
                let mut last_sample = None;
                while samples.len() < MAX_READINGS_PER_UPDATE {
                    if replay.pos >= replay.records.len() {
                        replay.pos = 0;
                        replay.base_now = None;
                    }
                    let (read_time, fields) = {
                        let rec = &replay.records[replay.pos];
                        (rec.read_time, rec.sample.clone())
                    };
                    if replay.base_now.is_none() {
                        replay.base_now = Some(now);
                        replay.base_time = read_time.unwrap_or(0.0);
                    }
                    let due = replay.base_now.unwrap() + read_time.unwrap_or(0.0) - replay.base_time;
                    if due > now {
                        break;
                    }
                    last_sample = Some(fields.clone());
                    samples.push(fields);
                    replay.pos += 1;
                }
                samples.last().cloned().or(last_sample)
            }
        }
    }
}

fn seed_from_now(now: f64) -> u64 {
    (now * 1000.0) as u64 | 1
}

/// SimulatedSignal.read: random crossings; a pass starts on about 5% of
/// readings and ends on about half.
fn simulated_read(state: &mut SimState, seed: &mut u64, crossing_pct: i64,
                  history_flag: bool) -> SignalSample {
    if state.is_crossing {
        let new_rssi = mock_rand(seed, 60, 150);
        let pass_peak = std::cmp::max(state.pass_peak_rssi, new_rssi);
        state.pass_peak_rssi = pass_peak;
        let sample = SignalSample {
            lap_id: state.lap_number,
            rssi: new_rssi,
            cross_flag: true,
            node_peak_rssi: 100,
            pass_peak_rssi: pass_peak,
            pass_nadir_rssi: state.pass_nadir_rssi,
            node_nadir_rssi: 20,
            loop_time: 1,
            history_flag: true,
        };
        if mock_rand_pct(seed) < 50 {
            state.is_crossing = false;
            state.pass_nadir_rssi = 100;
        }
        let mut s = sample;
        s.history_flag = history_flag;
        s
    } else {
        let new_rssi = mock_rand(seed, 20, 40);
        if new_rssi < state.pass_nadir_rssi {
            state.pass_nadir_rssi = new_rssi;
        }
        let sample = SignalSample {
            lap_id: state.lap_number,
            rssi: new_rssi,
            cross_flag: false,
            node_peak_rssi: 100,
            pass_peak_rssi: state.pass_peak_rssi,
            pass_nadir_rssi: state.pass_nadir_rssi,
            node_nadir_rssi: 20,
            loop_time: 1,
            history_flag: true,
        };
        if mock_rand_pct(seed) < crossing_pct {
            state.lap_number += 1;
            state.is_crossing = true;
            state.pass_peak_rssi = 0;
        }
        let mut s = sample;
        s.history_flag = history_flag;
        s
    }
}

pub fn mock_rand(seed: &mut u64, lo: i64, hi: i64) -> i64 {
    *seed = seed
        .wrapping_mul(6364136223846793005)
        .wrapping_add(1442695040888963407);
    lo + ((*seed >> 33) as i64).rem_euclid(hi - lo)
}

pub fn mock_rand_pct(seed: &mut u64) -> i64 {
    *seed = seed
        .wrapping_mul(6364136223846793005)
        .wrapping_add(1442695040888963407);
    ((*seed >> 33) as i64).rem_euclid(100)
}