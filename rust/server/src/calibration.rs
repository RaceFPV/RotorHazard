//! Seat calibration — port of calibration.py (the pure algorithm).
//!
//! The `Calibration` glue class (profile/node writes) lives in the socket
//! handlers and db module; this file is the EnterAt/ExitAt suggestion
//! engine, ported line-for-line so it produces the same suggestions as
//! the Python server.  Differential tests in tests/ compare this port
//! against Python-generated expected outputs.

use serde_json::{json, Value};
use std::collections::HashMap;

pub const AUTO_CAL_MIN_SPAN: f64 = 15.0;
pub const AUTO_CAL_BAND_MARGIN: i64 = 2;
pub const AUTO_CAL_SCAN_EXIT_FRACTION: f64 = 0.25;
pub const AUTO_CAL_MIN_BAND: i64 = 3;
pub const AUTO_CAL_MIN_BAND_FRACTION: f64 = 0.05;
pub const AUTO_CAL_MIN_REGULAR_BAND_FRACTION: f64 = 0.04;
pub const AUTO_CAL_NARROW_BAND_NOISE_FRACTION: f64 = 0.05;
pub const AUTO_CAL_WIDE_FRACTION: f64 = 0.75;
pub const AUTO_CAL_BAND_POSITION: f64 = 2.0 / 3.0;
pub const AUTO_CAL_PEAK_MARGIN_FRACTION: f64 = 0.1;
pub const AUTO_CAL_PEAK_MARGIN_SPAN_FRACTION: f64 = 0.2;
pub const AUTO_CAL_MIN_EXIT_GAP: f64 = 3.0;
pub const AUTO_CAL_MIN_EXIT_GAP_FRACTION: f64 = 0.1;
pub const AUTO_CAL_EXIT_TOP_MARGIN_FRACTION: f64 = 0.05;
pub const AUTO_CAL_EXIT_GAP_FRACTION: f64 = 0.3;
pub const AUTO_CAL_EXIT_SPAN_FRACTION: f64 = 0.19;
pub const AUTO_CAL_EXIT_SPAN_CAP_FRACTION: f64 = 0.4;
pub const AUTO_CAL_MIN_EXIT_GAP_OF_ENTER: f64 = 0.1;
pub const AUTO_CAL_MIN_EXIT_GAP_OF_ENTER_FALLBACK: f64 = 0.05;
pub const AUTO_CAL_FALLBACK_MIN_SPAN: f64 = 30.0;
pub const AUTO_CAL_MAX_CROSSING_S: f64 = 20.0;
pub const AUTO_CAL_MAX_PASS_CROSSING_S: f64 = 3.0;
pub const AUTO_CAL_CUT_ROUNDS: usize = 20;
pub const AUTO_CAL_STEP_SPACING_FRACTION: f64 = 0.75;
pub const AUTO_CAL_EARLY_END_S: f64 = 1.0;
pub const AUTO_CAL_STEP_REACH_FRACTION: f64 = 0.5;
pub const AUTO_CAL_HOLESHOT_WINDOW_S: f64 = 20.0;
pub const AUTO_CAL_HOLESHOT_SEARCH: i64 = 12;
pub const AUTO_CAL_HOLESHOT_MATCH_S: f64 = 1.0;
pub const AUTO_CAL_DEFAULT_MIN_LAP_MS: f64 = 5000.0;
pub const AUTO_CAL_NOISE_PENALTY: f64 = 3.0;
pub const AUTO_CAL_OPEN_PENALTY: f64 = 8.0;
pub const AUTO_CAL_MERGED_RATIO: f64 = 1.9;
pub const AUTO_CAL_MERGED_PENALTY: f64 = 6.0;
pub const AUTO_CAL_TYPICAL_LAP_MIN_CROSSINGS: usize = 8;
pub const AUTO_CAL_TYPICAL_LAP_SPREAD: f64 = 2.0;
pub const AUTO_CAL_MIN_LAP_TOO_LONG_FRACTION: f64 = 0.75;
pub const AUTO_CAL_CLEAN_PEAK_WINDOW_S: f64 = 1.0;
pub const AUTO_CAL_BELL_DROP_FRACTION: f64 = 0.2;
pub const AUTO_CAL_BELL_SIDE_S: f64 = 2.0;
pub const AUTO_CAL_BELL_TOLERANCE_FRACTION: f64 = 0.1;
pub const AUTO_CAL_BELL_PASS_FRACTION: f64 = 0.9;
pub const AUTO_CAL_BELL_ENTER_FRACTION: f64 = 0.015;
pub const AUTO_CAL_BELL_ENTER_MIN: f64 = 2.0;
pub const AUTO_CAL_BELL_PASS_LOSS_FRACTION: f64 = 0.05;
pub const AUTO_CAL_DOUBLE_WINDOW_S: f64 = 1.0;
pub const AUTO_CAL_TRAILING_NOISE_WINDOW_S: f64 = 5.0;
pub const AUTO_CAL_LANDING_SHORT_RATIO: f64 = 0.6;
pub const AUTO_CAL_LANDING_LONG_RATIO: f64 = 2.5;
pub const AUTO_CAL_DURATION_WEIGHT: f64 = 1.0;

/// Why no EnterAt/ExitAt could be suggested (NoSuggestion in Python).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NoSuggestion {
    NotFound,
    NoHistory,
    PassPeakOnly,
    WeakSignal,
    NoGatePasses,
    NoCleanExit,
}

impl NoSuggestion {
    pub fn as_str(&self) -> &'static str {
        match self {
            NoSuggestion::NotFound => "not_found",
            NoSuggestion::NoHistory => "no_history",
            NoSuggestion::PassPeakOnly => "pass_peak_only",
            NoSuggestion::WeakSignal => "weak_signal",
            NoSuggestion::NoGatePasses => "no_gate_passes",
            NoSuggestion::NoCleanExit => "no_clean_exit",
        }
    }
}

/// One crossing/lap candidate (the lap dicts in Python).
#[derive(Debug, Clone, Default)]
pub struct CalLap {
    pub crossing_start: f64,
    pub crossing_end: f64,
    pub lap_time_stamp: f64,
    pub peak_rssi: Option<i64>,
    pub deleted: bool,
    pub noise: bool,
    pub late: bool,
    pub landing: bool,
    pub open: bool,
}

/// A counted pass with its peak and good/bad status (tuples in Python).
#[derive(Debug, Clone)]
pub struct CalPass {
    pub time: f64,
    pub peak_rssi: i64,
    pub ok: bool,
}

#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct CalibrationSuggestion {
    pub enter_at: i64,
    pub exit_at: i64,
    pub laps: Vec<CalLap>,
    pub lap_count: usize,
    pub band: (i64, i64),
    pub passes: Vec<CalPass>,
}

/// JavaScript's Math.round, which rounds halves up.
pub fn js_round(value: f64) -> i64 {
    (value + 0.5).floor() as i64
}

/// The laps the marshal-page crossing algorithm gives for the history
/// with these thresholds (computeLaps in marshal.js must match).
pub fn compute_laps(
    values: &[i64],
    times: &[f64],
    start_time: f64,
    enter_at: i64,
    exit_at: i64,
    unlimited_time: bool,
    race_time_sec: f64,
    min_lap_ms: f64,
    min_lap_behavior: bool,
    min_first_crossing_ms: f64,
) -> Vec<CalLap> {
    let _ = (unlimited_time, race_time_sec); // used only via callers' windowing
    let mut last_lap_time_stamp = f64::NEG_INFINITY;
    let mut laps: Vec<CalLap> = Vec::new();
    let mut crossing = false;
    let mut crossing_start = 0.0f64;
    let mut peak_rssi = 0i64;
    let mut peak_first = 0.0f64;
    let mut peak_last = 0.0f64;
    let mut time = 0.0f64;

    for (&rssi, &t) in values.iter().zip(times.iter()) {
        time = t;
        if !crossing && rssi > enter_at {
            crossing = true;
            crossing_start = time;
        }
        if rssi >= peak_rssi {
            peak_last = time;
            if rssi > peak_rssi {
                peak_first = time;
                peak_rssi = rssi;
            }
        }
        if crossing && rssi < exit_at {
            let lap_time_stamp = ((peak_last + peak_first) / 2.0 - start_time) * 1000.0;
            if lap_time_stamp > 0.0 {
                let mut lap = CalLap {
                    crossing_start,
                    crossing_end: time,
                    lap_time_stamp,
                    peak_rssi: Some(peak_rssi),
                    deleted: false,
                    ..Default::default()
                };
                if lap_time_stamp < min_first_crossing_ms {
                    lap.deleted = true;
                    lap.noise = true;
                } else if min_lap_behavior && lap_time_stamp < last_lap_time_stamp + min_lap_ms {
                    lap.deleted = true;
                    lap.noise = true;
                } else {
                    last_lap_time_stamp = lap_time_stamp;
                }
                laps.push(lap);
            }
            crossing = false;
            peak_rssi = 0;
        }
    }
    if crossing {
        laps.push(CalLap {
            crossing_start,
            crossing_end: time,
            lap_time_stamp: ((peak_last + peak_first) / 2.0 - start_time) * 1000.0,
            peak_rssi: None,
            deleted: false,
            open: true,
            ..Default::default()
        });
    }
    // auto-delete late laps
    let mut finished = false;
    for lap in laps.iter_mut() {
        if finished {
            lap.deleted = true;
            lap.late = true;
        } else if !unlimited_time && lap.lap_time_stamp > race_time_sec * 1000.0 {
            finished = true;
        }
    }
    laps
}

fn regular_lap_times(stamps: &[f64]) -> (Option<f64>, bool) {
    let lap_times: Vec<f64> = stamps.windows(2).map(|w| w[1] - w[0]).collect();
    if lap_times.len() < (AUTO_CAL_TYPICAL_LAP_MIN_CROSSINGS as usize - 1) {
        return (None, false);
    }
    (
        Some(lap_times[lap_times.len() / 2]),
        lap_times[lap_times.len() * 3 / 4]
            <= lap_times[lap_times.len() / 4] * AUTO_CAL_TYPICAL_LAP_SPREAD,
    )
}

fn after_race(lap: &CalLap) -> bool {
    lap.late
        || (lap.open && lap.crossing_end - lap.crossing_start <= AUTO_CAL_MAX_CROSSING_S)
}

fn ruled_late(lap: &CalLap) -> bool {
    lap.late && !lap.landing
}

/// A crossing that never closes, or that lasts far longer than a gate
/// pass, makes the scan unusable; laps after the race can be left out.
fn crossings_usable(laps: &[CalLap], skip_late: bool) -> bool {
    !laps.iter().any(|lap| {
        (lap.open || lap.crossing_end - lap.crossing_start > AUTO_CAL_MAX_CROSSING_S)
            && !(ruled_late(lap) || (skip_late && after_race(lap)))
    })
}

fn count_noise_crossings(laps: &[CalLap], min_lap_ms: f64) -> i64 {
    let min_gap = if min_lap_ms != 0.0 {
        min_lap_ms
    } else {
        AUTO_CAL_DEFAULT_MIN_LAP_MS
    };
    let mut noise = 0i64;
    let mut last_stamp: Option<f64> = None;
    for lap in laps {
        if lap.late {
            continue;
        }
        if lap.noise {
            noise += 1;
        } else if !lap.deleted {
            if let Some(st) = last_stamp {
                if lap.lap_time_stamp - st < min_gap {
                    noise += 1;
                }
            }
            last_stamp = Some(lap.lap_time_stamp);
        }
    }
    noise
}

fn has_early_crossing(laps: &[CalLap]) -> bool {
    laps.iter()
        .any(|lap| !lap.deleted && lap.lap_time_stamp <= AUTO_CAL_HOLESHOT_WINDOW_S * 1000.0)
}

fn adds_holeshot_only(laps: &[CalLap], base_laps: &[CalLap]) -> bool {
    let stamps: Vec<f64> = laps
        .iter()
        .filter(|l| !l.deleted)
        .map(|l| l.lap_time_stamp)
        .collect();
    let base: Vec<f64> = base_laps
        .iter()
        .filter(|l| !l.deleted)
        .map(|l| l.lap_time_stamp)
        .collect();
    stamps.len() == base.len() + 1
        && stamps[0] <= AUTO_CAL_HOLESHOT_WINDOW_S * 1000.0
        && stamps
            .iter()
            .skip(1)
            .zip(base.iter())
            .all(|(a, b)| (a - b).abs() <= AUTO_CAL_HOLESHOT_MATCH_S * 1000.0)
}

fn score_calibration(laps: &[CalLap], min_lap_ms: f64, with_duration: bool) -> f64 {
    let mut score = AUTO_CAL_NOISE_PENALTY * count_noise_crossings(laps, min_lap_ms) as f64;
    let mut stamps: Vec<f64> = Vec::new();
    for lap in laps {
        if lap.late {
            continue;
        }
        if lap.open || lap.crossing_end - lap.crossing_start > AUTO_CAL_MAX_CROSSING_S {
            score += AUTO_CAL_OPEN_PENALTY;
        } else if !lap.deleted || lap.noise {
            if !lap.deleted {
                stamps.push(lap.lap_time_stamp);
            }
            if with_duration {
                score += AUTO_CAL_DURATION_WEIGHT * (lap.crossing_end - lap.crossing_start);
            }
        }
    }
    let lap_times: Vec<f64> = stamps.windows(2).map(|w| w[1] - w[0]).collect();
    if lap_times.len() >= 3 {
        let mut sorted_times = lap_times.clone();
        sorted_times.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let median = sorted_times[sorted_times.len() / 2];
        score += AUTO_CAL_MERGED_PENALTY
            * lap_times
                .iter()
                .filter(|&&t| t > median * AUTO_CAL_MERGED_RATIO)
                .count() as f64;
    }
    score
}

/// A gate pass rises past halfway (or three-quarters) from the trace
/// minimum to its peak once within the window around the crossing.
fn clean_peak(values: &[i64], times: &[f64], lo: i64, lap: &CalLap) -> bool {
    let start = lap.crossing_start - AUTO_CAL_CLEAN_PEAK_WINDOW_S;
    let end = lap.crossing_end + AUTO_CAL_CLEAN_PEAK_WINDOW_S;
    let segment: Vec<(i64, f64)> = values
        .iter()
        .copied()
        .zip(times.iter().copied())
        .filter(|&(_, t)| start <= t && t <= end)
        .collect();
    if segment.is_empty() {
        return false;
    }
    let rises = |level: f64| -> usize {
        let mut count = 0usize;
        let mut below: Vec<f64> = Vec::new();
        for &(value, time) in &segment {
            if (value as f64) >= level {
                if count == 0 || below.len() > 1 {
                    count += 1;
                }
                below.clear();
            } else {
                below.push(time);
            }
        }
        count
    };
    let peak = segment.iter().map(|&(v, _)| v).max().unwrap();
    [0.5, 0.75].iter().any(|&fraction| {
        rises(lo as f64 + (peak as f64 - lo as f64) * fraction) == 1
    })
}

/// (value, time) of the peaks above halfway up the trace that fall
/// steadily on both sides.
fn clean_peaks(values: &[i64], times: &[f64]) -> Vec<(i64, f64)> {
    let lo = *values.iter().min().unwrap();
    let hi = *values.iter().max().unwrap();
    let window = AUTO_CAL_CLEAN_PEAK_WINDOW_S;
    let mut peaks = Vec::new();
    let mut last_time = f64::NEG_INFINITY;
    for i in 0..values.len() {
        let value = values[i];
        let time = times[i];
        if (value as f64) < (lo as f64 + hi as f64) / 2.0 || time - last_time <= window {
            continue;
        }
        // bisect_left(times, time - window) .. bisect_right(times, time + window)
        let first = times.partition_point(|&t| t < time - window);
        let last = times.partition_point(|&t| t <= time + window);
        if last <= first {
            continue;
        }
        let window_max = values[first..last].iter().max().unwrap();
        let index_in_window = values[first..last]
            .iter()
            .position(|&v| v == value)
            .map(|p| p + first);
        if value < *window_max || index_in_window != Some(i) {
            continue;
        }
        let base = value as f64 - (value as f64 - lo as f64) * AUTO_CAL_BELL_DROP_FRACTION;
        let tolerance = (value as f64 - lo as f64) * AUTO_CAL_BELL_TOLERANCE_FRACTION;
        let mut sides = 0;
        for step in [-1i64, 1i64] {
            let mut j = i as i64;
            let mut lowest = value;
            while j + step >= 0 && (j + step) < values.len() as i64
                && (times[(j + step) as usize] - time).abs() <= AUTO_CAL_BELL_SIDE_S
            {
                j += step;
                let v = values[j as usize];
                if v as f64 > lowest as f64 + tolerance {
                    break;
                }
                lowest = lowest.min(v);
                if v as f64 <= base {
                    sides += 1;
                    break;
                }
            }
        }
        if sides == 2 {
            peaks.push((value, time));
            last_time = time;
        }
    }
    peaks
}

fn without_doubles(laps: &[CalLap]) -> Vec<usize> {
    // indices of laps that are not second bumps of a stronger nearby pass
    let window = AUTO_CAL_DOUBLE_WINDOW_S * 1000.0;
    let candidates: Vec<usize> = laps
        .iter()
        .enumerate()
        .filter(|(_, lap)| (!lap.deleted || lap.noise) && !after_race(lap))
        .map(|(i, _)| i)
        .collect();
    let rank = |i: usize| -> (bool, i64, f64) {
        let lap = &laps[i];
        (
            !lap.deleted,
            lap.peak_rssi.unwrap_or(0),
            -lap.lap_time_stamp,
        )
    };
    let mut doubles = std::collections::HashSet::new();
    for &i in &candidates {
        for &other in &candidates {
            if other == i {
                continue;
            }
            if (laps[other].lap_time_stamp - laps[i].lap_time_stamp).abs() <= window
                && rank(other) > rank(i)
            {
                doubles.insert(i);
            }
        }
    }
    (0..laps.len()).filter(|i| !doubles.contains(i)).collect()
}

fn mark_after_lap_limit(laps: &mut [CalLap], lap_limit: i64) {
    let mut count = 0i64;
    for lap in laps.iter_mut() {
        if count >= lap_limit {
            lap.deleted = true;
            lap.late = true;
        } else if !lap.deleted {
            count += 1;
        }
    }
}

fn mark_trailing_noise(laps: &mut [CalLap], min_lap_ms: f64, end_ms: f64) {
    let min_gap = min_lap_ms.max(AUTO_CAL_DEFAULT_MIN_LAP_MS);
    let mut flagged: Vec<(usize, bool)> = Vec::new();
    let mut last_stamp: Option<f64> = None;
    for (i, lap) in laps.iter().enumerate() {
        if lap.late || (lap.deleted && !lap.noise) {
            continue;
        }
        let flag = lap.noise
            || match last_stamp {
                Some(st) => lap.lap_time_stamp - st < min_gap,
                None => false,
            };
        flagged.push((i, flag));
        last_stamp = Some(lap.lap_time_stamp);
    }
    let window_start = end_ms - AUTO_CAL_TRAILING_NOISE_WINDOW_S * 1000.0;
    while let Some((i, flag)) = flagged.pop() {
        if !(flag && laps[i].lap_time_stamp >= window_start) {
            break;
        }
        laps[i].deleted = true;
        laps[i].late = true;
        laps[i].landing = true;
    }
}

fn mark_landing(laps: &mut [CalLap]) {
    let counted: Vec<usize> = laps
        .iter()
        .enumerate()
        .filter(|(_, l)| !l.deleted)
        .map(|(i, _)| i)
        .collect();
    if counted.len() < 2 {
        return;
    }
    let lap_times: Vec<f64> = counted
        .windows(2)
        .map(|w| laps[w[1]].lap_time_stamp - laps[w[0]].lap_time_stamp)
        .collect();
    if lap_times.len() < 3 {
        return;
    }
    let mut sorted_times = lap_times.clone();
    sorted_times.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let median = sorted_times[sorted_times.len() / 2];
    let mut last = counted.len() - 1;
    while last > 0
        && !(median * AUTO_CAL_LANDING_SHORT_RATIO <= lap_times[last - 1]
            && lap_times[last - 1] <= median * AUTO_CAL_LANDING_LONG_RATIO)
    {
        last -= 1;
    }
    if last == 0 {
        return;
    }
    let end = laps[counted[last]].lap_time_stamp;
    for lap in laps.iter_mut() {
        if lap.lap_time_stamp > end && !lap.deleted {
            lap.deleted = true;
            lap.late = true;
            lap.landing = true;
        }
    }
}

/// Non-deleted, non-after-race laps (the `counted` helper in Python).
fn counted_fn(laps: &[CalLap]) -> Vec<&CalLap> {
    laps.iter().filter(|l| !l.deleted && !after_race(l)).collect()
}

#[derive(Clone)]
struct Plateau {
    count: i64,
    noise: i64,
    high: i64,
    low: i64,
}

#[derive(Clone)]
struct Run {
    count: usize,
    high: i64,
    low: i64,
    stamps: Vec<f64>,
}

struct Found {
    enter_at: i64,
    exit_at: i64,
    laps: Vec<CalLap>,
    lap_count: usize,
    band: (i64, i64),
    passes: Vec<CalPass>,
}

/// The suggestion from the readings given (Python _suggest_from).
#[allow(clippy::too_many_arguments)]
fn suggest_from(
    values: &[i64],
    times: &[f64],
    start_time: f64,
    unlimited_time: bool,
    race_time_sec: f64,
    min_lap_ms: f64,
    min_lap_behavior: bool,
    min_first_crossing_ms: f64,
    lap_limit: i64,
    fixed: Option<(i64, i64)>,
) -> (Option<Found>, Option<NoSuggestion>) {
    let lo = *values.iter().min().unwrap();
    let hi = *values.iter().max().unwrap();
    if ((hi - lo) as f64) < AUTO_CAL_MIN_SPAN {
        return (None, Some(NoSuggestion::WeakSignal));
    }

    // Python rebinds min_lap_ms mid-function; the closures below capture
    // f64 by copy, so the live value lives in a Cell
    let min_lap_cell = std::cell::Cell::new(min_lap_ms);
    let laps_for = |enter: i64, exit_at: i64| -> Vec<CalLap> {
        let mut laps = compute_laps(
            values, times, start_time, enter, exit_at, unlimited_time, race_time_sec,
            min_lap_cell.get(), min_lap_behavior, min_first_crossing_ms,
        );
        if lap_limit != 0 {
            mark_after_lap_limit(&mut laps, lap_limit);
        }
        mark_trailing_noise(&mut laps, min_lap_cell.get(), (times[times.len() - 1] - start_time) * 1000.0);
        if unlimited_time {
            mark_landing(&mut laps);
        }
        laps
    };
    let pass_count = |laps: &[CalLap]| -> i64 {
        laps.iter().filter(|l| !after_race(l)).count() as i64
            - count_noise_crossings(laps, min_lap_cell.get())
    };

    // a minimum lap close to or longer than the laps flown would class
    // real passes as noise; the widest run of EnterAt values with a
    // steady number of crossings gives the typical lap
    let mut runs: Vec<Run> = Vec::new();
    let mut enter = hi - 1;
    while enter > lo + AUTO_CAL_BAND_MARGIN {
        let laps = laps_for(enter, enter - (1i64).max(js_round((enter - lo) as f64 * AUTO_CAL_SCAN_EXIT_FRACTION)));
        let crossing_stamps: Vec<f64> = laps
            .iter()
            .filter(|l| (!l.deleted || l.noise) && !after_race(l))
            .map(|l| l.lap_time_stamp)
            .collect();
        if let Some(last) = runs.last_mut() {
            if last.count == crossing_stamps.len() {
                // Python only updates 'low' here; stamps stay from the
                // run's first (highest) level
                last.low = enter;
                enter -= 1;
                continue;
            }
        }
        runs.push(Run {
            count: crossing_stamps.len(),
            high: enter,
            low: enter,
            stamps: crossing_stamps,
        });
        enter -= 1;
    }
    runs.retain(|run| run.count >= AUTO_CAL_TYPICAL_LAP_MIN_CROSSINGS);
    if !runs.is_empty() {
        // Python max() returns the FIRST maximum on ties
        let widest = runs
            .iter()
            .enumerate()
            .fold((0usize, 0i64), |(bi, bw), (i, r)| {
                let w = r.high - r.low;
                if w > bw { (i, w) } else { (bi, bw) }
            });
        let widest = &runs[widest.0];
        let (typical_lap, regular) = regular_lap_times(&widest.stamps);
        if let Some(tl) = typical_lap {
            let min_lap_ms = min_lap_cell.get();
            let effective = if min_lap_ms != 0.0 { min_lap_ms } else { AUTO_CAL_DEFAULT_MIN_LAP_MS };
            if regular && effective > tl * AUTO_CAL_MIN_LAP_TOO_LONG_FRACTION {
                min_lap_cell.set(tl / 2.0);
            }
        }
    }

    let mut scan: HashMap<i64, Vec<CalLap>> = HashMap::new();
    let mut plateaus: Vec<Plateau> = Vec::new();
    // Python tracks `run` (the current plateau); an unusable level resets
    // it to None so the next usable level always starts a new plateau
    let mut run_active = false;
    let mut enter = hi - 1;
    while enter > lo + AUTO_CAL_BAND_MARGIN {
        let exit_gap = (1i64).max(js_round((enter - lo) as f64 * AUTO_CAL_SCAN_EXIT_FRACTION));
        let laps = laps_for(enter, enter - exit_gap);
        scan.insert(enter, laps.clone());
        if laps.is_empty() || !crossings_usable(&laps, true) {
            run_active = false; // run = None
            enter -= 1;
            continue;
        }
        let noise = count_noise_crossings(&laps, min_lap_cell.get());
        let count = laps.iter().filter(|l| !after_race(l)).count() as i64 - noise;
        if run_active {
            if let Some(last) = plateaus.last_mut() {
                if last.count == count && last.noise == noise {
                    last.low = enter;
                    enter -= 1;
                    continue;
                }
            }
        }
        plateaus.push(Plateau { count, noise, high: enter, low: enter });
        run_active = true;
        enter -= 1;
    }

    let widest = plateaus
        .iter()
        .map(|p| p.high - p.low)
        .max()
        .unwrap_or(0);
    let mut band: Option<Plateau> = None;
    let stamps_of = |laps: &[CalLap]| -> Vec<f64> {
        laps.iter()
            .filter(|l| !l.deleted && !after_race(l))
            .map(|l| l.lap_time_stamp)
            .collect()
    };
    let trailing_only = |lower: &[CalLap], upper: &[CalLap]| -> bool {
        let upper_stamps = stamps_of(upper);
        let extra: Vec<f64> = stamps_of(lower)
            .into_iter()
            .filter(|s| upper_stamps.iter().all(|u| (s - u).abs() > AUTO_CAL_HOLESHOT_MATCH_S * 1000.0))
            .collect();
        let extra_min = extra.iter().copied().fold(f64::INFINITY, f64::min);
        let upper_max = upper_stamps
            .iter()
            .copied()
            .fold(f64::NEG_INFINITY, f64::max);
        unlimited_time
            && !extra.is_empty()
            && extra.len() * 2 < upper_stamps.len()
            && extra_min > upper_max
    };
    let messy_only = |lower: &[CalLap], upper: &[CalLap]| -> bool {
        let upper_stamps = stamps_of(upper);
        let extra: Vec<&CalLap> = lower
            .iter()
            .filter(|l| !l.deleted && !after_race(l)
                && upper_stamps.iter().all(|u| (l.lap_time_stamp - u).abs() > AUTO_CAL_HOLESHOT_MATCH_S * 1000.0))
            .collect();
        !extra.is_empty() && !extra.iter().any(|l| clean_peak(values, times, lo, l))
    };
    for p in &plateaus {
        let width_ok = (p.high - p.low) as f64 >= widest as f64 * AUTO_CAL_WIDE_FRACTION;
        if !width_ok {
            continue;
        }
        let better = match &band {
            None => true,
            Some(b) => {
                p.count > b.count
                    && !trailing_only(&scan[&p.high], &scan[&b.high])
                    && !messy_only(&scan[&p.high], &scan[&b.high])
            }
        };
        if better {
            band = Some(p.clone());
        }
    }

    let regular_stamps = |laps: &[CalLap]| -> (Option<f64>, bool) {
        regular_lap_times(&stamps_of(laps))
    };
    let too_narrow = |band: &Option<Plateau>| -> bool {
        match band {
            None => true,
            Some(b) => {
                let narrow1 = ((b.high - b.low) as f64)
                    < (AUTO_CAL_MIN_BAND as f64).max((hi - lo) as f64 * AUTO_CAL_MIN_BAND_FRACTION);
                let narrow2 = ((b.high - b.low) as f64)
                    < (AUTO_CAL_MIN_BAND as f64)
                        .max((b.high - lo) as f64 * AUTO_CAL_MIN_REGULAR_BAND_FRACTION);
                let noisy = b.noise as f64 > b.count as f64 * AUTO_CAL_NARROW_BAND_NOISE_FRACTION;
                let (_, regular) = regular_stamps(&scan[&b.high]);
                narrow1 && (narrow2 || noisy || !regular)
            }
        }
    };

    if too_narrow(&band) {
        // join adjacent bands with the same number of crossings, noise
        // crossings included, and take the highest joined band wide enough
        #[derive(Clone)]
        struct Joined {
            count: i64,
            noise: i64,
            high: i64,
            low: i64,
            crossings: i64,
        }
        let mut joined: Vec<Joined> = Vec::new();
        for p in &plateaus {
            match joined.last_mut() {
                Some(last)
                    if last.crossings == p.count + p.noise && last.low == p.high + 1 =>
                {
                    last.low = p.low;
                    last.count = last.count.min(p.count);
                    last.noise = last.noise.max(p.noise);
                }
                _ => joined.push(Joined {
                    count: p.count,
                    noise: p.noise,
                    high: p.high,
                    low: p.low,
                    crossings: p.count + p.noise,
                }),
            }
        }
        let wide: Vec<Joined> = joined
            .into_iter()
            .filter(|p| {
                (p.high - p.low) as f64
                    >= (AUTO_CAL_MIN_BAND as f64).max((hi - lo) as f64 * AUTO_CAL_MIN_BAND_FRACTION)
            })
            .collect();
        band = wide.first().map(|w| Plateau {
            count: w.count,
            noise: w.noise,
            high: w.high,
            low: w.low,
        });
        if band.is_none() {
            match fixed {
                None => return (None, Some(NoSuggestion::NoGatePasses)),
                Some((f_enter, f_count)) => {
                    if !scan.contains_key(&f_enter) {
                        return (None, Some(NoSuggestion::NoGatePasses));
                    }
                    band = Some(Plateau { count: f_count, noise: 0, high: f_enter, low: f_enter });
                }
            }
        }
    }
    let mut band = band.unwrap();

    // below a band of good passes, narrow steady steps that each add a
    // good pass, with no noise other than second bumps, hold weaker real
    // passes; the lowest such step is taken as the band
    let good = |lap: &CalLap| -> bool {
        clean_peak(values, times, lo, lap)
            && lap.crossing_end - lap.crossing_start <= AUTO_CAL_MAX_PASS_CROSSING_S
    };
    let counted = counted_fn;
    let median_gap = |pass_stamps: &[f64]| -> f64 {
        let mut gaps: Vec<f64> = pass_stamps.windows(2).map(|w| w[1] - w[0]).collect();
        gaps.sort_by(|a, b| a.partial_cmp(b).unwrap());
        if gaps.is_empty() { 0.0 } else { gaps[gaps.len() / 2] }
    };
    let band_is_in_plateaus = plateaus
        .iter()
        .any(|p| p.high == band.high && p.low == band.low && p.count == band.count);
    let band_stamps_all = if band_is_in_plateaus {
        stamps_of(&scan[&band.low])
    } else {
        Vec::new()
    };
    let band_cadence = median_gap(&band_stamps_all);
    let mut cadence = band_cadence;
    let band_index = plateaus
        .iter()
        .position(|p| p.high == band.high && p.low == band.low && p.count == band.count);

    if let Some(bi) = band_index {
        if counted(&scan[&band.high]).iter().all(|l| good(l)) {
            let mut base = stamps_of(&scan[&band.low]);
            let mut low = band.low;
            let mut count = band.count;
            let mut step: Option<Plateau> = None;
            let mut gained = false;
            let ended_early = unlimited_time
                || times[times.len() - 1] - start_time < race_time_sec - AUTO_CAL_EARLY_END_S;
            for p in &plateaus[bi + 1..] {
                if p.high != low - 1 || p.count < count {
                    break;
                }
                let laps = &scan[&p.low];
                // noise other than second bumps ends the step-down
                let kept: Vec<&CalLap> =
                    laps.iter().filter(|l| !l.deleted || l.noise).collect();
                let has_other_noise = laps.iter().any(|lap| {
                    lap.noise
                        && !kept.iter().any(|other| {
                            !std::ptr::eq(*other, lap)
                                && !other.noise
                                && (other.lap_time_stamp - lap.lap_time_stamp).abs()
                                    <= AUTO_CAL_DOUBLE_WINDOW_S * 1000.0
                        })
                });
                if has_other_noise {
                    break;
                }
                let added: Vec<&CalLap> = counted(laps)
                    .into_iter()
                    .filter(|lap| {
                        base.iter().all(|b| (lap.lap_time_stamp - b).abs() > AUTO_CAL_HOLESHOT_MATCH_S * 1000.0)
                    })
                    .collect();
                if band_cadence == 0.0 {
                    cadence = if counted(laps).len() >= 3 {
                        median_gap(&stamps_of(laps))
                    } else {
                        0.0
                    };
                }
                let below = scan.get(&(p.low - 1)).unwrap_or(laps);
                let split = |lap: &CalLap| -> bool {
                    below.iter().any(|other| {
                        (!other.deleted || other.noise)
                            && (other.lap_time_stamp - lap.lap_time_stamp).abs() > 0.0
                            && (other.lap_time_stamp - lap.lap_time_stamp).abs()
                                <= AUTO_CAL_DOUBLE_WINDOW_S * 1000.0
                    })
                };
                let last_lap = counted(laps).last().copied();
                if !added.is_empty()
                    && ended_early
                    && added.iter().all(|&lap| {
                        match last_lap {
                            Some(l) if std::ptr::eq(l, lap) => {
                                (times[times.len() - 1] - start_time) * 1000.0
                                    - lap.lap_time_stamp
                                    < cadence
                            }
                            _ => false,
                        }
                    })
                {
                    break;
                }
                if !added.is_empty() && band_stamps_all.len() == 2 && cadence == band_cadence {
                    for lap in &added {
                        let t = lap.lap_time_stamp;
                        let before = base
                            .iter()
                            .copied()
                            .filter(|&b| b < t)
                            .fold(f64::INFINITY, f64::min);
                        let after = base
                            .iter()
                            .copied()
                            .filter(|&b| b > t)
                            .fold(f64::INFINITY, f64::min);
                        if before.is_finite() && after.is_finite() {
                            if good(lap)
                                && !split(lap)
                                && (t - before).min(after - t)
                                    >= (t - before).max(after - t) * AUTO_CAL_STEP_SPACING_FRACTION
                            {
                                cadence = (after - before) / 2.0;
                            }
                        }
                    }
                }
                // Python: checked = added if band_cadence else counted(laps) if cadence else []
                let checked: Vec<&CalLap> = if band_cadence != 0.0 {
                    added.iter().copied().collect()
                } else if cadence != 0.0 {
                    counted(laps)
                } else {
                    Vec::new()
                };
                if !added.is_empty() && !added.iter().any(|lap| good(lap) && !split(lap)) {
                    // a step that adds only messy passes is passed over
                    low = p.low;
                    count = p.count;
                    base = stamps_of(laps);
                    continue;
                }
                let spaced = |lap: &CalLap| -> bool {
                    counted(laps).iter().all(|other| {
                        std::ptr::eq(*other, lap)
                            || (other.lap_time_stamp - lap.lap_time_stamp).abs()
                                >= cadence * AUTO_CAL_STEP_SPACING_FRACTION
                    })
                };
                if !added.is_empty() && !checked.iter().all(|&lap| spaced(lap)) {
                    break;
                }
                gained = gained || !added.is_empty();
                low = p.low;
                count = p.count;
                base = stamps_of(laps);
                let steady = |p: &Plateau| -> bool {
                    let ref_stamps = stamps_of(&scan[&p.low]);
                    (p.low..=p.high).all(|e| {
                        let s = stamps_of(&scan[&e]);
                        s.len() == ref_stamps.len()
                            && s.iter().all(|st| {
                                ref_stamps
                                    .iter()
                                    .any(|r| (st - r).abs() <= AUTO_CAL_HOLESHOT_MATCH_S * 1000.0)
                            })
                    })
                };
                if gained
                    && cadence != 0.0
                    && steady(p)
                    && p.noise == 0
                    && p.count > band.count
                    && ((p.high - p.low + 1) as f64) >= AUTO_CAL_MIN_BAND as f64
                {
                    step = Some(p.clone());
                }
            }
            if let Some(step_p) = step {
                if (band.low - step_p.low) as f64
                    >= (band.high - band.low) as f64 * AUTO_CAL_STEP_REACH_FRACTION
                {
                    band = step_p;
                }
            }
        }
    }

    let mut enter_val = band.low + js_round((band.high - band.low) as f64 * AUTO_CAL_BAND_POSITION);
    let band_scan = scan.get(&band.high).cloned().unwrap_or_default();
    let peaks: Vec<i64> = band_scan
        .iter()
        .filter(|l| !l.deleted && !after_race(l))
        .filter_map(|l| l.peak_rssi)
        .collect();
    if !peaks.is_empty() {
        let weakest = peaks.iter().copied().min().unwrap();
        let margin = (weakest as f64 * AUTO_CAL_PEAK_MARGIN_FRACTION)
            .min((weakest - lo) as f64 * AUTO_CAL_PEAK_MARGIN_SPAN_FRACTION);
        let target = weakest - js_round(margin);
        let mut lowest = band.low;
        let band_stamps = stamps_of(&band_scan);
        let target_laps = scan.get(&target.max(*scan.keys().min().unwrap())).cloned().unwrap_or_default();
        let bad = |lap: &CalLap| -> bool {
            !good(lap)
                || target_laps.iter().any(|other| {
                    (!other.deleted || other.noise)
                        && (other.lap_time_stamp - lap.lap_time_stamp).abs() > 0.0
                        && (other.lap_time_stamp - lap.lap_time_stamp).abs()
                            <= AUTO_CAL_DOUBLE_WINDOW_S * 1000.0
                })
        };
        loop {
            if !(lowest - 1 > target && scan.contains_key(&(lowest - 1))
                && crossings_usable(&scan[&(lowest - 1)], true))
            {
                break;
            }
            let prev = &scan[&(lowest - 1)];
            let core: Vec<&CalLap> = without_doubles(prev)
                .into_iter()
                .map(|i| &prev[i])
                .filter(|l| (!l.deleted || l.noise) && !after_race(l))
                .collect();
            let extra: Vec<&&CalLap> = core
                .iter()
                .filter(|lap| {
                    band_stamps
                        .iter()
                        .map(|s| (lap.lap_time_stamp - s).abs())
                        .fold(f64::INFINITY, f64::min)
                        > AUTO_CAL_DOUBLE_WINDOW_S * 1000.0
                })
                .collect();
            let doubles_removed = core.len() - extra.len();
            if doubles_removed != band_stamps.len() || !extra.iter().all(|l| bad(l)) {
                break;
            }
            lowest -= 1;
        }
        if lowest == band.low {
            lowest = js_round((band.low + band.high) as f64 / 2.0);
        }
        enter_val = lowest.max(enter_val.min(target));
    }

    // a weak holeshot peaks at the top of the ripple; step down to the
    // first EnterAt that adds exactly one early crossing cleanly
    if !has_early_crossing(&scan[&enter_val]) {
        let base_score = score_calibration(&scan[&enter_val], min_lap_cell.get(), true);
        let mut e = band.low - 1;
        loop {
            if e < band.low - AUTO_CAL_HOLESHOT_SEARCH {
                break;
            }
            match scan.get(&e) {
                None => break,
                Some(laps_e) => {
                    if pass_count(laps_e) == band.count + 1
                        && adds_holeshot_only(laps_e, &scan[&enter_val])
                        && crossings_usable(laps_e, false)
                        && score_calibration(laps_e, min_lap_cell.get(), true) <= base_score
                    {
                        enter_val = e;
                        break;
                    }
                }
            }
            e -= 1;
        }
    }

    // ExitAt: the lowest score marks the clean range
    let mut structural: HashMap<i64, f64> = HashMap::new();
    let mut best_exit: Option<i64> = None;
    let mut best_score = f64::INFINITY;
    let min_gap = (AUTO_CAL_MIN_EXIT_GAP)
        .max(js_round((enter_val - lo) as f64 * AUTO_CAL_MIN_EXIT_GAP_FRACTION) as f64);
    let window = AUTO_CAL_DOUBLE_WINDOW_S * 1000.0;
    let first_gap = ((enter_val - lo) as f64 * AUTO_CAL_EXIT_GAP_FRACTION)
        .max(
            ((hi - lo) as f64 * AUTO_CAL_EXIT_SPAN_FRACTION)
                .min((enter_val - lo) as f64 * AUTO_CAL_EXIT_SPAN_CAP_FRACTION),
        );
    let first_stamps_unsorted: Vec<f64> = stamps_of(&laps_for(
        enter_val,
        enter_val - js_round(first_gap) as i64,
    ));
    let mut first_stamps = first_stamps_unsorted.clone();
    first_stamps.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let mut doubles: Vec<(f64, f64)> = Vec::new();
    for w in first_stamps.windows(2) {
        let (a, b) = (w[0], w[1]);
        if b - a <= window {
            match doubles.last_mut() {
                Some(last) if last.1 == a => last.1 = b,
                _ => doubles.push((a, b)),
            }
        }
    }
    if first_stamps
        .iter()
        .all(|&s| doubles.iter().any(|&(a, b)| a <= s && s <= b))
    {
        doubles.clear();
    }
    let scored = |laps: &[CalLap]| -> Vec<CalLap> {
        laps.iter()
            .filter(|lap| {
                !doubles
                    .iter()
                    .any(|&(a, b)| a - window <= lap.lap_time_stamp && lap.lap_time_stamp <= b + window)
            })
            .cloned()
            .collect()
    };

    for &skip_late in [false, true].iter() {
        let mut exit_at = enter_val - min_gap as i64;
        while exit_at > lo {
            let laps = scored(&laps_for(enter_val, exit_at));
            if !crossings_usable(&laps, skip_late)
                || pass_count(&laps) < band.count - doubles.len() as i64
            {
                exit_at -= 1;
                continue;
            }
            structural.insert(exit_at, score_calibration(&laps, min_lap_cell.get(), false));
            let score = score_calibration(&laps, min_lap_cell.get(), true);
            if score < best_score {
                best_score = score;
                best_exit = Some(exit_at);
            }
            exit_at -= 1;
        }
        if best_exit.is_some() {
            break;
        }
    }
    let best_exit = match best_exit {
        Some(e) => e,
        None => return (None, Some(NoSuggestion::NoCleanExit)),
    };
    let best_score_structural = structural[&best_exit];
    let mut top = best_exit;
    while structural.get(&(top + 1)) == Some(&best_score_structural) {
        top += 1;
    }
    let mut bottom = best_exit;
    while structural.get(&(bottom - 1)) == Some(&best_score_structural) {
        bottom -= 1;
    }

    // floor from the low points between passes
    let counted_laps: Vec<CalLap> = laps_for(enter_val, best_exit)
        .into_iter()
        .filter(|l| !l.deleted && !after_race(l))
        .collect();
    let mut lows: Vec<i64> = Vec::new();
    for w in counted_laps.windows(2) {
        let (prev, lap) = (&w[0], &w[1]);
        let between: Vec<usize> = (0..times.len())
            .filter(|&i| prev.crossing_end < times[i] && times[i] < lap.crossing_start)
            .collect();
        if !between.is_empty() {
            let low = between.iter().map(|&i| values[i]).min().unwrap();
            lows.push(low);
        }
    }
    lows.sort();
    let floor = if lows.len() >= AUTO_CAL_TYPICAL_LAP_MIN_CROSSINGS {
        lo.max(lows[lows.len() / 4])
    } else {
        lo
    };
    let gap = ((enter_val - floor) as f64 * AUTO_CAL_EXIT_GAP_FRACTION)
        .max(
            ((hi - floor) as f64 * AUTO_CAL_EXIT_SPAN_FRACTION)
                .min((enter_val - floor) as f64 * AUTO_CAL_EXIT_SPAN_CAP_FRACTION),
        );
    let mut exit_at = bottom.max(
        (top - js_round((enter_val - lo) as f64 * AUTO_CAL_EXIT_TOP_MARGIN_FRACTION))
            .min(enter_val - js_round(gap)),
    );

    // keep the smallest gap, taking the highest ExitAt at or below it
    // that is within the clean range or as clean
    let keep = if fixed.is_some() && band.low == fixed.unwrap().0 {
        pass_count(&scored(&laps_for(enter_val, best_exit))) as f64
            * (1.0 - AUTO_CAL_BELL_PASS_LOSS_FRACTION)
    } else {
        0.0
    };
    let highest_clean = |limit: i64| -> Option<i64> {
        let mut exit_value = limit;
        while exit_value > lo {
            let in_range = bottom <= exit_value && exit_value <= top;
            let structural_ok = structural
                .get(&exit_value)
                .map(|&s| s <= best_score_structural)
                .unwrap_or(false);
            let keep_ok = keep == 0.0
                || pass_count(&scored(&laps_for(enter_val, exit_value))) as f64 >= keep;
            if (in_range || structural_ok) && keep_ok {
                return Some(exit_value);
            }
            exit_value -= 1;
        }
        None
    };
    let wanted = highest_clean(
        exit_at.min(enter_val - ((enter_val as f64) * AUTO_CAL_MIN_EXIT_GAP_OF_ENTER).ceil() as i64),
    );
    let fallback = highest_clean(
        exit_at.min(
            enter_val
                - ((enter_val as f64) * AUTO_CAL_MIN_EXIT_GAP_OF_ENTER_FALLBACK).ceil() as i64,
        ),
    );
    if let Some(w) = wanted {
        exit_at = w;
    } else if (hi - lo) as f64 >= AUTO_CAL_FALLBACK_MIN_SPAN
        && fallback.is_some()
        && (fallback.unwrap() - lo) as f64 >= AUTO_CAL_MIN_EXIT_GAP
    {
        exit_at = fallback.unwrap();
    } else {
        return (None, Some(NoSuggestion::NoCleanExit));
    }

    let laps = laps_for(enter_val, exit_at);
    // double-peak / overlong validation
    let split_stamps: Vec<f64> = laps_for(enter_val, enter_val - js_round(gap) as i64)
        .iter()
        .filter(|l| (!l.deleted || l.noise) && !after_race(l))
        .map(|l| start_time + l.lap_time_stamp / 1000.0)
        .collect();
    let double = |lap: &CalLap| -> bool {
        let mut inside: Vec<f64> = split_stamps
            .iter()
            .copied()
            .filter(|&t| lap.crossing_start <= t && t <= lap.crossing_end)
            .collect();
        inside.sort_by(|a, b| a.partial_cmp(b).unwrap());
        inside.windows(2).any(|w| w[1] - w[0] <= AUTO_CAL_DOUBLE_WINDOW_S)
    };
    let final_counted: Vec<&CalLap> = laps
        .iter()
        .filter(|l| !l.deleted && !after_race(l))
        .collect();
    if final_counted.iter().all(|lap| {
        double(lap) || lap.crossing_end - lap.crossing_start > AUTO_CAL_MAX_PASS_CROSSING_S
    }) {
        return (None, Some(NoSuggestion::NoGatePasses));
    }

    let passes: Vec<CalPass> = final_counted
        .iter()
        .filter_map(|lap| {
            lap.peak_rssi.map(|peak| CalPass {
                time: start_time + lap.lap_time_stamp / 1000.0,
                peak_rssi: peak,
                ok: !double(lap)
                    && lap.crossing_end - lap.crossing_start <= AUTO_CAL_MAX_PASS_CROSSING_S
                    && clean_peak(values, times, lo, lap),
            })
        })
        .collect();

    (
        Some(Found {
            enter_at: enter_val,
            exit_at,
            laps: laps.clone(),
            // Python: len([lap for lap in laps if not lap['deleted']])
            lap_count: laps.iter().filter(|l| !l.deleted).count(),
            band: (band.low, band.high),
            passes,
        }),
        None,
    )
}

#[allow(clippy::too_many_arguments)]
fn cut_bad_crossings(
    mut found: Found,
    values: &[i64],
    times: &[f64],
    start_time: f64,
    unlimited_time: bool,
    race_time_sec: f64,
    min_lap_ms: f64,
    min_lap_behavior: bool,
    min_first_crossing_ms: f64,
    lap_limit: i64,
) -> Found {
    let lo = *values.iter().min().unwrap();
    let min_gap = (if min_lap_ms != 0.0 {
        min_lap_ms
    } else {
        AUTO_CAL_DEFAULT_MIN_LAP_MS
    })
    .min(AUTO_CAL_DEFAULT_MIN_LAP_MS)
        / 1000.0;
    let mut values = values.to_vec();
    for _ in 0..AUTO_CAL_CUT_ROUNDS {
        let good_passes: Vec<f64> = found
            .passes
            .iter()
            .filter(|p| p.ok)
            .map(|p| p.peak_rssi as f64)
            .collect();
        if good_passes.is_empty() {
            break;
        }
        let weakest = good_passes
            .iter()
            .map(|&p| p as i64)
            .min()
            .unwrap();
        let target = weakest
            - js_round(
                ((weakest as f64) * AUTO_CAL_PEAK_MARGIN_FRACTION)
                    .min((weakest - lo) as f64 * AUTO_CAL_PEAK_MARGIN_SPAN_FRACTION),
            );
        if found.enter_at <= target {
            break;
        }
        let pass_times: Vec<f64> = found.passes.iter().map(|p| p.time).collect();
        let mut cut: Vec<CalLap> = Vec::new();
        let mut enter = found.band.0 - 1;
        while enter > target - 1 {
            let exit_at =
                enter - (1i64).max(js_round((enter - lo) as f64 * AUTO_CAL_SCAN_EXIT_FRACTION));
            let mut laps: Vec<CalLap> = compute_laps(
                &values, times, start_time, enter, exit_at, unlimited_time, race_time_sec,
                0.0, false, min_first_crossing_ms,
            )
            .into_iter()
            .filter(|l| !l.deleted && !l.late && !l.open)
            .collect();
            laps.retain(|lap| {
                !pass_times
                    .iter()
                    .any(|&t| lap.crossing_start <= t && t <= lap.crossing_end)
            });
            if laps.is_empty() {
                enter -= 1;
                continue;
            }
            let near_pass = laps.iter().any(|lap| {
                pass_times.iter().any(|&t| {
                    (start_time + lap.lap_time_stamp / 1000.0 - t).abs() < min_gap
                })
            }) || laps.iter().any(|lap| {
                clean_peak(&values, times, lo, lap)
                    && lap.crossing_end - lap.crossing_start <= AUTO_CAL_MAX_PASS_CROSSING_S
            });
            if near_pass {
                break;
            }
            cut = laps;
            break;
        }
        if cut.is_empty() {
            break;
        }
        for lap in &cut {
            let span: Vec<usize> = (0..times.len())
                .filter(|&i| {
                    lap.crossing_start - min_gap <= times[i]
                        && times[i] <= lap.crossing_end + min_gap
                        && pass_times.iter().all(|&t| (times[i] - t).abs() > min_gap / 2.0)
                })
                .collect();
            if span.is_empty() {
                continue;
            }
            let low = span.iter().map(|&i| values[i]).min().unwrap();
            for i in span {
                values[i] = low;
            }
        }
        match suggest_from(
            &values, times, start_time, unlimited_time, race_time_sec, min_lap_ms,
            min_lap_behavior, min_first_crossing_ms, lap_limit, None,
        ) {
            (Some(again), _) => found = again,
            (None, _) => break,
        }
    }
    found
}

/// EnterAt/ExitAt from the RSSI history — the public entry point
/// (suggest_calibration_with_reason in Python).  Returns the suggestion
/// or the NoSuggestion reason.
#[allow(clippy::too_many_arguments)]
pub fn suggest_calibration_with_reason(
    values: &[i64],
    times: &[f64],
    start_time: f64,
    unlimited_time: bool,
    race_time_sec: f64,
    min_lap_ms: f64,
    min_lap_behavior: bool,
    min_first_crossing_ms: f64,
    lap_limit: i64,
) -> (Option<CalibrationSuggestion>, Option<NoSuggestion>) {
    if values.len() != times.len() {
        return (None, Some(NoSuggestion::NoHistory));
    }
    let race_end = if !unlimited_time && race_time_sec != 0.0 {
        start_time + race_time_sec
    } else {
        f64::INFINITY
    };
    let kept: Vec<(i64, f64)> = values
        .iter()
        .copied()
        .zip(times.iter().copied())
        .filter(|&(_, t)| start_time <= t && t <= race_end)
        .collect();
    let values: Vec<i64> = kept.iter().map(|&(v, _)| v).collect();
    let times: Vec<f64> = kept.iter().map(|&(_, t)| t).collect();
    if values.is_empty() || values.len() < 3 {
        return (None, Some(NoSuggestion::NoHistory));
    }
    let (mut found, mut reason) = suggest_from(
        &values, &times, start_time, unlimited_time, race_time_sec, min_lap_ms,
        min_lap_behavior, min_first_crossing_ms, lap_limit, None,
    );

    if reason == Some(NoSuggestion::NoGatePasses) {
        // clean-peak fallback
        let peaks = clean_peaks(&values, &times);
        let lo = *values.iter().min().unwrap();
        if !peaks.is_empty() {
            let mut peak_values: Vec<i64> = peaks.iter().map(|&(v, _)| v).collect();
            peak_values.sort();
            let median = peak_values[peak_values.len() / 2];
            let passes: Vec<(i64, f64)> = peaks
                .iter()
                .copied()
                .filter(|&(v, _)| {
                    (v as f64) >= lo as f64 + (median - lo) as f64 * AUTO_CAL_BELL_PASS_FRACTION
                })
                .collect();
            if passes.len() >= AUTO_CAL_TYPICAL_LAP_MIN_CROSSINGS {
                let weakest = passes.iter().map(|&(v, _)| v).min().unwrap();
                let mut gaps: Vec<f64> = passes
                    .windows(2)
                    .map(|w| w[1].1 - w[0].1)
                    .collect();
                gaps.sort_by(|a, b| a.partial_cmp(b).unwrap());
                let typical_lap = gaps[(passes.len() - 1) / 2] * 1000.0;
                let mut min_lap_ms = min_lap_ms;
                if (if min_lap_ms != 0.0 { min_lap_ms } else { AUTO_CAL_DEFAULT_MIN_LAP_MS })
                    > typical_lap * AUTO_CAL_MIN_LAP_TOO_LONG_FRACTION
                {
                    min_lap_ms = typical_lap / 2.0;
                }
                let window = AUTO_CAL_CLEAN_PEAK_WINDOW_S * 1000.0;
                let clean_stamps: Vec<f64> =
                    peaks.iter().map(|&(_, t)| (t - start_time) * 1000.0).collect();
                let crossings = |level: i64| -> Vec<f64> {
                    let exit_level =
                        level - (AUTO_CAL_MIN_EXIT_GAP.max(js_round(level as f64 * AUTO_CAL_MIN_EXIT_GAP_OF_ENTER) as f64)) as i64;
                    compute_laps(
                        &values, &times, start_time, level, exit_level, unlimited_time,
                        race_time_sec, min_lap_ms, false, min_first_crossing_ms,
                    )
                    .into_iter()
                    .filter(|lap| !lap.open)
                    .map(|lap| lap.lap_time_stamp)
                    .collect()
                };
                let mut floor = weakest;
                let mut above = crossings(floor);
                while floor - 1 > lo {
                    let below = crossings(floor - 1);
                    let added: Vec<f64> = below
                        .iter()
                        .filter(|&&stamp| above.iter().all(|&o| (stamp - o).abs() > window))
                        .copied()
                        .collect();
                    let break_out = added.iter().any(|&stamp| {
                        clean_stamps.iter().all(|&c| (stamp - c).abs() > window)
                            || above
                                .iter()
                                .map(|&o| (stamp - o).abs())
                                .fold(f64::INFINITY, f64::min)
                                < typical_lap / 2.0
                    });
                    if break_out {
                        break;
                    }
                    floor -= 1;
                    above = below;
                }
                let weakest_new = passes
                    .iter()
                    .filter(|&&(v, _)| v >= floor)
                    .map(|&(v, _)| v)
                    .chain(std::iter::once(weakest))
                    .min()
                    .unwrap();
                let enter_val = weakest_new
                    - (AUTO_CAL_BELL_ENTER_MIN.max(js_round(weakest_new as f64 * AUTO_CAL_BELL_ENTER_FRACTION) as f64))
                        as i64;
                match suggest_from(
                    &values, &times, start_time, unlimited_time, race_time_sec, min_lap_ms,
                    min_lap_behavior, min_first_crossing_ms, lap_limit,
                    Some((enter_val, passes.len() as i64)),
                ) {
                    (Some(f), _) => {
                        found = Some(f);
                        reason = None;
                    }
                    (None, _) => {}
                }
            }
        }
    }

    match found {
        Some(f) => {
            let cut = cut_bad_crossings(
                f, &values, &times, start_time, unlimited_time, race_time_sec, min_lap_ms,
                min_lap_behavior, min_first_crossing_ms, lap_limit,
            );
            (
                Some(CalibrationSuggestion {
                    enter_at: cut.enter_at,
                    exit_at: cut.exit_at,
                    laps: cut.laps,
                    lap_count: cut.lap_count,
                    band: cut.band,
                    passes: cut.passes,
                }),
                None,
            )
        }
        None => (None, reason),
    }
}

/// Suggested EnterAt/ExitAt for a seat in the current (stopped, unsaved)
/// race — suggest_for_seat in Python.
pub fn suggest_for_seat(
    state: &crate::state::SharedState,
    seat_index: i64,
) -> (Option<CalibrationSuggestion>, Option<NoSuggestion>) {
    if seat_index < 0 || seat_index >= state.node_count() as i64 {
        return (None, Some(NoSuggestion::NotFound));
    }
    // pass-peak-only interfaces record no RSSI trace
    let hw_nodes = state
        .hardware
        .lock()
        .unwrap()
        .as_ref()
        .map(|hw| hw.num_nodes())
        .unwrap_or(0);
    let marshal_type = if (seat_index as usize) < hw_nodes {
        0 // FULL_RSSI
    } else {
        let holder = state.mock_provider.lock().unwrap();
        holder.1.marshal_type()
    };
    if marshal_type == 2 {
        return (None, Some(NoSuggestion::PassPeakOnly));
    }
    let (values, times, start_time, format_id) = {
        let mock = state.mock.lock().unwrap();
        let race = state.race.lock().unwrap();
        (
            mock.get(seat_index as usize)
                .map(|m| m.history_values.clone())
                .unwrap_or_default(),
            mock.get(seat_index as usize)
                .map(|m| m.history_times.clone())
                .unwrap_or_default(),
            race.start_time_monotonic,
            race.format_id,
        )
    };
    suggest_common(
        state, &values, &times, start_time, Some(format_id),
    )
}

/// Suggested EnterAt/ExitAt for a saved pilot run — suggest_for_pilotrun
/// in Python.  Reads the history recorded at race save.
pub fn suggest_for_pilotrun(
    state: &crate::state::SharedState,
    pilotrace_id: i64,
) -> (Option<CalibrationSuggestion>, Option<NoSuggestion>) {
    let row: Option<(i64, i64, String, String)> = db_bridge(state, |conn| {
        conn.query_row(
            "SELECT marshal_type, race_id, history_values, history_times \
             FROM saved_pilot_race WHERE id = ?1",
            [pilotrace_id],
            |r| Ok((r.get::<_, i64>(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .ok()
    });
    let (marshal_type, race_id, history_values_json, history_times_json) = match row {
        Some(r) => r,
        None => return (None, Some(NoSuggestion::NotFound)),
    };
    let race_meta: Option<(f64, Option<i64>)> = db_bridge(state, |conn| {
        conn.query_row(
            "SELECT start_time, format_id FROM saved_race_meta WHERE id = ?1",
            [race_id],
            |r| Ok((r.get::<_, i64>(0)? as f64 / 1000.0, r.get(1)?)),
        )
        .ok()
    });
    let (start_time, format_id) = match race_meta {
        Some(r) => r,
        None => return (None, Some(NoSuggestion::NotFound)),
    };
    if marshal_type == 2 {
        return (None, Some(NoSuggestion::PassPeakOnly));
    }
    let values: Vec<i64> = serde_json::from_str(&history_values_json).unwrap_or_default();
    let times: Vec<f64> = serde_json::from_str(&history_times_json).unwrap_or_default();
    if values.is_empty() {
        return (None, Some(NoSuggestion::NoHistory));
    }
    suggest_common(state, &values, &times, start_time, format_id)
}

fn db_bridge<T>(
    state: &crate::state::SharedState,
    f: impl FnOnce(&rusqlite::Connection) -> T,
) -> T {
    crate::db::with_db(state, f)
}

#[allow(clippy::too_many_arguments)]
fn suggest_common(
    state: &crate::state::SharedState,
    values: &[i64],
    times: &[f64],
    start_time: f64,
    format_id: Option<i64>,
) -> (Option<CalibrationSuggestion>, Option<NoSuggestion>) {
    let (unlimited_time, race_time_sec, min_lap_ms, min_first_crossing_ms, lap_limit) =
        crate::db::with_db(state, |conn| {
            let (unlimited_time, race_time_sec, team_racing_mode, win_condition, number_laps_win, start_behavior) =
                format_id
                    .and_then(|fid| {
                        conn.query_row(
                            "SELECT race_mode, race_time_sec, team_racing_mode, win_condition, \
                             number_laps_win, start_behavior FROM race_format WHERE id = ?1",
                            [fid],
                            |r| {
                                Ok((
                                    r.get::<_, i64>(0)?,
                                    r.get::<_, i64>(1)?,
                                    r.get::<_, i64>(2)?,
                                    r.get::<_, i64>(3)?,
                                    r.get::<_, i64>(4)?,
                                    r.get::<_, i64>(5)?,
                                ))
                            },
                        )
                        .ok()
                    })
                    .unwrap_or((1, 0, 0, 0, 0, 0));
            let min_lap_ms =
                crate::db::get_option_int(conn, "MinLapSec", 10) as f64 * 1000.0;
            let min_first_crossing_ms =
                crate::db::get_option_int(conn, "MinFirstCrossingSec", 0) as f64 * 1000.0;
            // _lap_limit: individual first-to-laps races only
            let lap_limit = if team_racing_mode == 0 && win_condition == 2 && number_laps_win > 0 {
                number_laps_win + if start_behavior == 1 { 0 } else { 1 }
            } else {
                0
            };
            (
                unlimited_time != 0,
                race_time_sec as f64,
                min_lap_ms,
                min_first_crossing_ms,
                lap_limit,
            )
        });
    // MinLapBehavior (server config, default 1) gates min-lap enforcement
    let min_lap_behavior =
        crate::db::get_config_number(state, "TIMING", "MinLapBehavior", 1.0) != 0.0;
    suggest_calibration_with_reason(
        values,
        times,
        start_time,
        unlimited_time,
        race_time_sec,
        min_lap_ms,
        min_lap_behavior,
        min_first_crossing_ms,
        lap_limit,
    )
}

/// Search race history for the best tuning values for a seat
/// (find_best_calibration_values in Python): same heat/node, then same
/// class/pilot/node, then pilot/node, then node — all requiring laps.
pub fn find_best_calibration_values(
    state: &crate::state::SharedState,
    seat_index: usize,
) -> (i64, i64) {
    // the node's current levels and frequency
    let (cur_enter, cur_exit, node_freq) = current_levels(state, seat_index);

    crate::db::with_db(state, |conn| {
        let Some(current_heat) = state.race.lock().unwrap().current_heat else {
            // practice mode: no auto calibration
            return (cur_enter, cur_exit);
        };
        let heat_class: Option<i64> = conn
            .query_row("SELECT class_id FROM heat WHERE id = ?1", [current_heat], |r| r.get(0))
            .ok()
            .flatten();
        let pilot: Option<i64> = conn
            .query_row(
                "SELECT pilot_id FROM heat_node WHERE heat_id = ?1 AND node_index = ?2",
                rusqlite::params![current_heat, seat_index as i64],
                |r| r.get(0),
            )
            .ok()
            .flatten();

        // disabled node: no calibration
        if pilot.is_none() || node_freq == 0 {
            tracing::debug!("Node {} calibration: skipping disabled node", seat_index + 1);
            return (cur_enter, cur_exit);
        }

        let races: Vec<(i64, i64, Option<i64>)> = {
            let mut stmt = conn
                .prepare("SELECT id, heat_id, class_id FROM saved_race_meta ORDER BY id DESC")
                .unwrap();
            let rows = stmt
                .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
                .unwrap();
            rows.filter_map(|r| r.ok()).collect()
        };
        let pilot_races: Vec<(i64, i64, i64, Option<i64>, i64, i64, i64)> = {
            let mut stmt = conn
                .prepare(
                    "SELECT id, race_id, node_index, pilot_id, frequency, enter_at, exit_at \
                     FROM saved_pilot_race ORDER BY id DESC",
                )
                .unwrap();
            let rows = stmt
                .query_map([], |r| {
                    Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?, r.get(6)?))
                })
                .unwrap();
            rows.filter_map(|r| r.ok()).collect()
        };
        let lap_pilotrace_ids: std::collections::HashSet<i64> = {
            let mut stmt = conn
                .prepare(
                    "SELECT DISTINCT pilotrace_id FROM saved_race_lap WHERE deleted = 0",
                )
                .unwrap();
            let rows = stmt.query_map([], |r| r.get::<_, i64>(0)).unwrap();
            rows.filter_map(|r| r.ok()).collect()
        };

        // same heat, same node
        for race in &races {
            if race.1 == current_heat {
                for pr in &pilot_races {
                    if pr.1 == race.0
                        && pr.2 == seat_index as i64
                        && pr.4 == node_freq
                        && lap_pilotrace_ids.contains(&pr.0)
                    {
                        return (pr.5, pr.6);
                    }
                }
                break;
            }
        }

        // same class, same pilot, same node
        for race in &races {
            if race.2 == heat_class {
                for pr in &pilot_races {
                    if pr.1 == race.0
                        && pr.2 == seat_index as i64
                        && pr.3 == pilot
                        && pr.4 == node_freq
                        && lap_pilotrace_ids.contains(&pr.0)
                    {
                        return (pr.5, pr.6);
                    }
                }
                break;
            }
        }

        // same pilot, same node
        for pr in &pilot_races {
            if pr.2 == seat_index as i64 && pr.3 == pilot && pr.4 == node_freq
                && lap_pilotrace_ids.contains(&pr.0)
            {
                return (pr.5, pr.6);
            }
        }

        // same node
        for pr in &pilot_races {
            if pr.2 == seat_index as i64 && pr.4 == node_freq
                && lap_pilotrace_ids.contains(&pr.0)
            {
                return (pr.5, pr.6);
            }
        }

        // fallback: no change
        tracing::debug!("Node {} calibration: no calibration hints found, no change", seat_index + 1);
        (cur_enter, cur_exit)
    })
}

/// The seat's current enter/exit levels and frequency (hardware cache for
/// real nodes, profile arrays for mock seats).
fn current_levels(state: &crate::state::SharedState, seat_index: usize) -> (i64, i64, i64) {
    let hw_nodes = state
        .hardware
        .lock()
        .unwrap()
        .as_ref()
        .map(|hw| hw.num_nodes())
        .unwrap_or(0);
    if seat_index < hw_nodes {
        let hw = state.hardware.lock().unwrap();
        let hw = hw.as_ref().unwrap();
        let freq = hw.nodes.get(seat_index).map(|n| n.frequency as i64).unwrap_or(0);
        return (
            hw.cur_enter_at.get(seat_index).copied().unwrap_or(0),
            hw.cur_exit_at.get(seat_index).copied().unwrap_or(0),
            freq,
        );
    }
    crate::db::with_db(state, |conn| {
        let pid = crate::db::current_profile_id(conn);
        let enter: Option<String> = conn
            .query_row("SELECT enter_ats FROM profiles WHERE id = ?1", [pid], |r| r.get(0))
            .ok();
        let exit: Option<String> = conn
            .query_row("SELECT exit_ats FROM profiles WHERE id = ?1", [pid], |r| r.get(0))
            .ok();
        let get_at = |json_str: Option<String>, key: &str| -> i64 {
            json_str
                .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
                .and_then(|v| v.get(key).cloned())
                .and_then(|v| v.get("v").cloned())
                .and_then(|v| v.as_array().cloned())
                .and_then(|arr| arr.get(seat_index).cloned())
                .and_then(|v| v.as_i64())
                .unwrap_or(0)
        };
        let freqs = crate::db::profile_frequencies(conn, pid);
        (
            get_at(enter, "v"),
            get_at(exit, "v"),
            freqs.get(seat_index).map(|(_, _, f)| *f).unwrap_or(0),
        )
    })
}

/// Apply best tuning values to all nodes (auto_calibrate in Python):
/// triggered after a marshal-page lap resave when calibrationMode is on.
pub fn auto_calibrate(state: &crate::state::SharedState) {
    if state.race.lock().unwrap().current_heat.is_none() {
        tracing::debug!("Skipping auto calibration; server in practice mode");
        return;
    }
    for seat_index in 0..state.node_count() {
        let (enter, exit_at) = find_best_calibration_values(state, seat_index);
        let (cur_enter, cur_exit, _) = current_levels(state, seat_index);
        if cur_enter != enter {
            set_level_db_and_hardware(state, seat_index, true, enter);
        }
        if cur_exit != exit_at {
            set_level_db_and_hardware(state, seat_index, false, exit_at);
        }
    }
    tracing::info!("Updated calibration with best discovered values");
    let payload = crate::db::with_db(state, |conn| crate::payloads::build_enter_exit_levels(conn));
    crate::socket::broadcast(state, "enter_and_exit_at_levels", payload);
}

/// Writes a level to the profile DB and the hardware (the shared body of
/// the set_enter_at_level / set_exit_at_level handlers).
pub fn set_level_db_and_hardware(
    state: &crate::state::SharedState,
    seat_index: usize,
    is_enter: bool,
    level: i64,
) {
    crate::db::with_db(state, |conn| {
        let pid = crate::db::current_profile_id(conn);
        let column = if is_enter { "enter_ats" } else { "exit_ats" };
        let existing: Option<String> = conn
            .query_row(
                &format!("SELECT {} FROM profiles WHERE id = ?1", column),
                [pid],
                |r| r.get(0),
            )
            .ok();
        let mut arr = existing
            .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
            .unwrap_or(json!({"v": []}));
        let v = arr
            .as_object_mut()
            .unwrap()
            .entry("v")
            .or_insert(json!([]));
        while v.as_array().unwrap().len() <= seat_index {
            v.as_array_mut().unwrap().push(Value::Null);
        }
        v.as_array_mut().unwrap()[seat_index] = json!(level);
        let _ = conn.execute(
            &format!("UPDATE profiles SET {} = ?1 WHERE id = ?2", column),
            rusqlite::params![arr.to_string(), pid],
        );
    });
    if is_enter {
        if let Some(hw) = state.hardware.lock().unwrap().as_mut() {
            hw.set_enter_at_level(seat_index, level);
        }
    } else if let Some(hw) = state.hardware.lock().unwrap().as_mut() {
        hw.set_exit_at_level(seat_index, level);
    }
}

/// Convenience: serialize a suggestion payload for the
/// 'calibration_suggestion' socket event.
pub fn suggestion_payload(
    result: &Option<CalibrationSuggestion>,
    reason: Option<NoSuggestion>,
) -> serde_json::Value {
    json!({
        "enter_at": result.as_ref().map(|r| r.enter_at),
        "exit_at": result.as_ref().map(|r| r.exit_at),
        "lap_count": result.as_ref().map(|r| r.lap_count),
        "reason": reason.map(|r| r.as_str()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Differential test: fixtures generated by running the real Python
    /// calibration.suggest_calibration_with_reason on synthetic traces
    /// (tests/fixtures/calibration_cases.json; regenerate with
    /// PYTHONPATH=src/server python3 src/tests/rust_fixtures/gen_calibration_cases.py).
    /// The Rust port must produce identical enter/exit/lap-count/band/reason.
    #[test]
    fn differential_against_python() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/calibration_cases.json"
        );
        let text = match std::fs::read_to_string(path) {
            Ok(t) => t,
            Err(_) => {
                eprintln!("fixture missing ({}); regenerate with the Python generator", path);
                return; // skip rather than fail when fixtures are absent
            }
        };
        let cases: Vec<serde_json::Value> = serde_json::from_str(&text).unwrap();
        assert!(!cases.is_empty(), "fixture must contain cases");
        let mut checked = 0;
        for case in &cases {
            let values: Vec<i64> = case["values"]
                .as_array()
                .unwrap()
                .iter()
                .map(|v| v.as_i64().unwrap())
                .collect();
            let times: Vec<f64> = case["times"]
                .as_array()
                .unwrap()
                .iter()
                .map(|v| v.as_f64().unwrap())
                .collect();
            let p = &case["params"];
            let (result, reason) = suggest_calibration_with_reason(
                &values,
                &times,
                p["start_time"].as_f64().unwrap(),
                p["unlimited_time"].as_bool().unwrap(),
                p["race_time_sec"].as_f64().unwrap(),
                p["min_lap_ms"].as_f64().unwrap(),
                p["min_lap_behavior"].as_i64().unwrap() != 0,
                p["min_first_crossing_ms"].as_f64().unwrap(),
                p["lap_limit"].as_i64().unwrap(),
            );
            let expected = &case["expected"];
            let got_reason = reason.map(|r| r.as_str().to_string());
            let exp_reason: Option<String> = if expected["reason"].is_null() {
                None
            } else {
                expected["reason"].as_str().map(String::from)
            };
            assert_eq!(got_reason, exp_reason, "reason mismatch on case {}", case["name"]);
            assert_eq!(
                result.as_ref().map(|r| r.enter_at),
                expected["enter_at"].as_i64(),
                "enter_at mismatch on case {}",
                case["name"]
            );
            assert_eq!(
                result.as_ref().map(|r| r.exit_at),
                expected["exit_at"].as_i64(),
                "exit_at mismatch on case {}",
                case["name"]
            );
            assert_eq!(
                result.as_ref().map(|r| r.lap_count),
                expected["lap_count"].as_i64().map(|v| v as usize),
                "lap_count mismatch on case {} (rust laps: {:?})",
                case["name"],
                result
                    .as_ref()
                    .map(|r| r
                        .laps
                        .iter()
                        .map(|l| (l.lap_time_stamp, l.deleted, l.noise, l.late, l.open))
                        .collect::<Vec<_>>())
            );
            assert_eq!(
                result.as_ref().map(|r| vec![r.band.0, r.band.1]),
                expected["band"]
                    .as_array()
                    .map(|a| a.iter().map(|v| v.as_i64().unwrap()).collect::<Vec<i64>>()),
                "band mismatch on case {}",
                case["name"]
            );
            checked += 1;
        }
        assert!(checked >= 8, "expected at least 8 differential cases");
    }
}
