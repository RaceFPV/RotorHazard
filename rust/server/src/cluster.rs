//! Cluster node management — port of ClusterNodeSet.py (primary side)
//! plus the secondary-side socket handlers it pairs with.
//!
//! A "primary" timer connects to secondary RotorHazard servers listed in
//! the SECONDARIES config; their passes become split/speed records, event
//! triggers, or mirrored state depending on the entry's mode.

use crate::state::{RACE_STATUS_RACING, RACE_STATUS_STAGING};
use serde_json::{json, Value};

/// Extracts the first JSON value from a socket.io payload.
fn payload_to_value(payload: rust_socketio::Payload) -> Value {
    match payload {
        rust_socketio::Payload::Text(vals) => vals.into_iter().next().unwrap_or(Value::Null),
        rust_socketio::Payload::Binary(_) => Value::Null,
        #[allow(deprecated)]
        rust_socketio::Payload::String(s) => {
            serde_json::from_str(&s).unwrap_or(json!(s))
        }
    }
}
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use tracing::{debug, info, warn};

pub const SPLIT_MODE: &str = "split";
pub const MIRROR_MODE: &str = "mirror";
pub const ACTION_MODE: &str = "action";

pub const TIME_CALLOUT: &str = "time";
pub const SPEED_CALLOUT: &str = "speed";
pub const BOTH_CALLOUT: &str = "both";
pub const NAME_ONLY_CALLOUT: &str = "nameOnly";
pub const NONE_CALLOUT: &str = "none";

pub const LATENCY_AVG_SIZE: usize = 30;
pub const TIMEDIFF_MEDIAN_SIZE: usize = 30;
pub const TIMEDIFF_WARNING_THRESH_MS: i64 = 250;
pub const JOIN_SETTLE_SECS: f64 = 3.0;

/// Mask applied to secondary passwords when echoing config.
#[allow(dead_code)]
pub const SECONDARY_PASSWORD_MASK: &str = "**********";

/// Copy of SECONDARIES with each non-empty password masked.
#[allow(dead_code)]
pub fn mask_secondary_passwords(secondaries: &Value) -> Value {
    let mut masked = secondaries.clone();
    if let Some(arr) = masked.as_array_mut() {
        for info in arr.iter_mut() {
            if info.get("password").and_then(|p| p.as_str()).map(|p| !p.is_empty()).unwrap_or(false) {
                if let Some(obj) = info.as_object_mut() {
                    obj.insert("password".to_string(), json!(SECONDARY_PASSWORD_MASK));
                }
            }
        }
    }
    masked
}

/// Restores masked passwords from the stored config (matching by
/// address, then by position).
#[allow(dead_code)]
pub fn restore_secondary_passwords(secondaries: &mut Value, stored: &Value) {
    let stored_arr: Vec<Value> = stored.as_array().cloned().unwrap_or_default();
    if let Some(arr) = secondaries.as_array_mut() {
        for (idx, info) in arr.iter_mut().enumerate() {
            if info.get("password").and_then(|p| p.as_str()) == Some(SECONDARY_PASSWORD_MASK) {
                let address = info.get("address").and_then(|a| a.as_str());
                let found = stored_arr
                    .iter()
                    .find(|s| s.get("address").and_then(|a| a.as_str()) == address)
                    .or_else(|| stored_arr.get(idx));
                if let Some(pw) = found.and_then(|s| s.get("password")).and_then(|p| p.as_str()) {
                    if let Some(obj) = info.as_object_mut() {
                        obj.insert("password".to_string(), json!(pw));
                    }
                }
            }
        }
    }
}

/// Rolling statistics (Averager in util/Averager.py).
#[derive(Default)]
struct Averager {
    vals: Vec<i64>,
    min: i64,
    max: i64,
    last: i64,
}

#[allow(dead_code)]
impl Averager {
    fn new(size: usize) -> Self {
        Averager { vals: Vec::with_capacity(size), min: 0, max: 0, last: 0 }
    }
    fn add(&mut self, val: i64) {
        if self.vals.len() >= LATENCY_AVG_SIZE {
            self.vals.remove(0);
        }
        self.vals.push(val);
        self.min = *self.vals.iter().min().unwrap();
        self.max = *self.vals.iter().max().unwrap();
        self.last = val;
    }
    fn avg(&self) -> f64 {
        if self.vals.is_empty() {
            0.0
        } else {
            self.vals.iter().sum::<i64>() as f64 / self.vals.len() as f64
        }
    }
    fn int_avg(&self) -> i64 {
        self.avg().round() as i64
    }
    fn len(&self) -> usize {
        self.vals.len()
    }
}

/// Running median over an insertion-order ring (util/RunningMedian.py).
/// The even-count median is the midpoint of the two middle values, as in
/// the Python `median()` helper.
struct RunningMedian {
    ring: std::collections::VecDeque<i64>,
}

impl RunningMedian {
    fn new(_size: usize) -> Self {
        RunningMedian {
            ring: std::collections::VecDeque::new(),
        }
    }
    fn insert(&mut self, val: i64) {
        if self.ring.len() >= TIMEDIFF_MEDIAN_SIZE {
            self.ring.pop_front();
        }
        self.ring.push_back(val);
    }
    fn median(&self) -> f64 {
        let mut sorted: Vec<i64> = self.ring.iter().copied().collect();
        sorted.sort_unstable();
        let len = sorted.len();
        if len == 0 {
            0.0
        } else if len % 2 == 1 {
            sorted[len / 2] as f64
        } else {
            (sorted[len / 2] + sorted[len / 2 - 1]) as f64 / 2.0
        }
    }
    #[allow(dead_code)]
    fn is_empty(&self) -> bool {
        self.ring.is_empty()
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Split,
    Mirror,
    Action,
}

impl Mode {
    fn as_str(&self) -> &'static str {
        match self {
            Mode::Split => SPLIT_MODE,
            Mode::Mirror => MIRROR_MODE,
            Mode::Action => ACTION_MODE,
        }
    }
}

/// Mutable connection/lifecycle state for one secondary.
struct SecState {
    last_contact: f64, // <=0 means disconnected
    start_connect_time: f64,
    first_contact_time: f64,
    last_check_query_time: f64,
    secs_since_disconnect: f64,
    freqs_sent: bool,
    num_disconnects: u64,
    num_disconns_during_race: u64,
    num_contacts: u64,
    latency: Averager,
    total_up_secs: u64,
    total_down_secs: u64,
    time_diff_median: RunningMedian,
    time_diff_median_ms: f64,
    prog_start_epoch: i64,
    running: bool,
    joined: bool,
    join_connect_time: f64,
    use_auth_header: bool,
    auth_reconnect: bool,
    auth_failed: bool,
    action_pass_times: HashMap<usize, f64>,
    prev_sec_pass_tstamps: HashMap<usize, Option<f64>>,
}

pub struct SecondaryNode {
    pub id: usize,
    pub info: Value,
    pub address: String,
    mode: Mode,
    distance_m: f64,
    query_interval: f64,
    first_query_interval: f64,
    query_timeout: f64,
    rec_events: bool,
    time_callout: bool,
    speed_callout: bool,
    name_callout: bool,
    event: Option<String>,
    state: Mutex<SecState>,
    client: Mutex<Option<rust_socketio::RawClient>>,
    stop_flag: AtomicBool,
}

fn get_info_f64(info: &Value, key: &str, default: f64) -> f64 {
    info.get(key).and_then(|v| v.as_f64()).unwrap_or(default)
}
fn get_info_str<'a>(info: &'a Value, key: &str) -> Option<&'a str> {
    info.get(key).and_then(|v| v.as_str())
}

#[allow(dead_code)] // several helpers await the cluster settings-UI handlers
impl SecondaryNode {
    pub fn new(id: usize, info: Value) -> Arc<SecondaryNode> {
        let mut addr = get_info_str(&info, "address").unwrap_or("").to_string();
        if !addr.contains("://") {
            addr = format!("http://{}", addr);
        }
        let mut query_interval = get_info_f64(&info, "queryInterval", 0.0);
        if query_interval <= 0.0 {
            query_interval = 10.0;
        }
        let first_query_interval = if query_interval >= 3.0 { 3.0 } else { 1.0 };
        let query_timeout = get_info_f64(&info, "timeout", 300.0);
        let distance = get_info_f64(&info, "distance", 0.0) * 1000.0;

        let mode = match get_info_str(&info, "mode").unwrap_or(SPLIT_MODE).to_lowercase().as_str() {
            MIRROR_MODE => Mode::Mirror,
            ACTION_MODE => Mode::Action,
            _ => Mode::Split,
        };
        let rec_events = info
            .get("recEventsFlag")
            .and_then(|v| v.as_bool())
            .unwrap_or(mode == Mode::Mirror);

        let callout = get_info_str(&info, "callout").map(|s| s.to_lowercase());
        let event_name = get_info_str(&info, "event").map(|s| s.to_string());
        let (time_callout, speed_callout, name_callout) = match callout.as_deref() {
            Some(TIME_CALLOUT) => (true, false, true),
            Some(SPEED_CALLOUT) => (false, true, true),
            Some(BOTH_CALLOUT) => (true, true, true),
            Some(NAME_ONLY_CALLOUT) => (false, false, true),
            Some(NONE_CALLOUT) => (false, false, false),
            _ => {
                if distance > 0.0 {
                    (false, true, true)
                } else {
                    (true, false, true)
                }
            }
        };

        Arc::new(SecondaryNode {
            id,
            info,
            address: addr,
            mode,
            distance_m: distance,
            query_interval,
            first_query_interval,
            query_timeout,
            rec_events,
            time_callout,
            speed_callout,
            name_callout,
            event: event_name,
            state: Mutex::new(SecState {
                last_contact: -1.0,
                start_connect_time: 0.0,
                first_contact_time: 0.0,
                last_check_query_time: 0.0,
                secs_since_disconnect: 0.0,
                freqs_sent: false,
                num_disconnects: 0,
                num_disconns_during_race: 0,
                num_contacts: 0,
                latency: Averager::new(LATENCY_AVG_SIZE),
                total_up_secs: 0,
                total_down_secs: 0,
                time_diff_median: RunningMedian::new(TIMEDIFF_MEDIAN_SIZE),
                time_diff_median_ms: 0.0,
                prog_start_epoch: 0,
                running: false,
                joined: false,
                join_connect_time: 0.0,
                use_auth_header: false,
                auth_reconnect: false,
                auth_failed: false,
                action_pass_times: HashMap::new(),
                prev_sec_pass_tstamps: HashMap::new(),
            }),
            client: Mutex::new(None),
            stop_flag: AtomicBool::new(false),
        })
    }

    pub fn is_split_mode(&self) -> bool {
        self.mode == Mode::Split
    }

    /// Resets the freqs-sent flag so frequencies are re-sent on next contact.
    pub fn reset_freqs_sent(&self) {
        self.state.lock().unwrap().freqs_sent = false;
    }
    pub fn is_mirror_mode(&self) -> bool {
        self.mode == Mode::Mirror
    }
    pub fn is_action_mode(&self) -> bool {
        self.mode == Mode::Action
    }
    pub fn rec_events(&self) -> bool {
        self.rec_events
    }
    pub fn event_name(&self) -> Option<String> {
        self.event.clone()
    }
    /// Split/action timers that record events also get an event-action.
    pub fn is_split_or_action(&self) -> bool {
        self.mode != Mode::Mirror
    }

    fn auth_headers(state: &crate::state::SharedState, info: &Value) -> Option<String> {
        let (username, password) = if info.get("username").is_some() || info.get("password").is_some() {
            (
                get_info_str(info, "username").unwrap_or("").to_string(),
                get_info_str(info, "password").unwrap_or("").to_string(),
            )
        } else {
            let cfg = crate::db::get_config_string(state, "SECRETS", "ADMIN_USERNAME");
            let cfgp = crate::db::get_config_string(state, "SECRETS", "ADMIN_PASSWORD");
            (cfg, cfgp)
        };
        if username.is_empty() && password.is_empty() {
            return None;
        }
        Some(format!(
            "Basic {}",
            base64_encode(format!("{}:{}", username, password).as_bytes())
        ))
    }

    /// Starts (or restarts) the worker thread (start_connection).
    pub fn start_connection(self: &Arc<Self>, state: crate::state::SharedState) {
        {
            let mut st = self.state.lock().unwrap();
            st.auth_failed = false;
            st.start_connect_time = 0.0;
            st.last_contact = -1.0;
            st.first_contact_time = 0.0;
            st.last_check_query_time = 0.0;
            st.secs_since_disconnect = 0.0;
            st.freqs_sent = false;
            st.num_disconnects = 0;
            st.num_disconns_during_race = 0;
            st.num_contacts = 0;
            st.latency = Averager::new(LATENCY_AVG_SIZE);
            st.total_up_secs = 0;
            st.total_down_secs = 0;
            st.time_diff_median = RunningMedian::new(TIMEDIFF_MEDIAN_SIZE);
            st.time_diff_median_ms = 0.0;
            st.prog_start_epoch = 0;
            st.running = true;
        }
        self.stop_flag.store(false, Ordering::Relaxed);
        let node = self.clone();
        let state = state.clone();
        std::thread::Builder::new()
            .name(format!("secondary-{}", self.id + 1))
            .spawn(move || node.worker_thread(state))
            .expect("spawn secondary worker");
    }

    fn stop(&self) {
        self.stop_flag.store(true, Ordering::Relaxed);
        self.state.lock().unwrap().running = false;
    }

    pub fn is_running(&self) -> bool {
        self.state.lock().unwrap().running
    }

    pub fn last_contact_time(&self) -> f64 {
        self.state.lock().unwrap().last_contact
    }
    pub fn num_disconnects(&self) -> u64 {
        self.state.lock().unwrap().num_disconnects
    }
    pub fn auth_failed(&self) -> bool {
        self.state.lock().unwrap().auth_failed
    }

    /// The worker loop: connect/retry/backoff, join settle, frequency
    /// push, heartbeat queries, disconnect-on-stall (worker thread).
    fn worker_thread(self: Arc<Self>, state: crate::state::SharedState) {
        {
            let mut st = self.state.lock().unwrap();
            st.start_connect_time = crate::state::monotonic();
        }
        debug!("Started worker thread for secondary timer {}", self.id + 1);
        std::thread::sleep(std::time::Duration::from_millis(100));
        loop {
            if self.stop_flag.load(Ordering::Relaxed) {
                break;
            }
            std::thread::sleep(std::time::Duration::from_secs(1));
            if self.stop_flag.load(Ordering::Relaxed) {
                break;
            }
            let now = crate::state::monotonic();
            let (last_contact, num_disconnects, _secs_since, start_connect, joined, join_connect, freqs_sent) = {
                let st = self.state.lock().unwrap();
                (
                    st.last_contact,
                    st.num_disconnects,
                    st.secs_since_disconnect,
                    st.start_connect_time,
                    st.joined,
                    st.join_connect_time,
                    st.freqs_sent,
                )
            };

            if last_contact <= 0.0 {
                // not connected: track disconnect time and retry
                let new_secs = now - start_connect;
                {
                    self.state.lock().unwrap().secs_since_disconnect = new_secs;
                }
                if new_secs >= 1.0 {
                    let race_status = state.race.lock().unwrap().race_status;
                    if num_disconnects > 0
                        || (race_status != RACE_STATUS_STAGING && race_status != RACE_STATUS_RACING)
                    {
                        info!("Attempting to connect to secondary {} at {}...", self.id + 1, self.address);
                        self.connect_once(&state, now);
                    }
                }
            } else {
                // connected
                if !joined {
                    if now - join_connect < JOIN_SETTLE_SECS {
                        continue;
                    }
                    debug!("No join response from secondary {} at {}; proceeding", self.id + 1, self.address);
                    self.settle_join(&state);
                }
                if !freqs_sent {
                    self.send_frequencies(&state);
                } else {
                    // heartbeat query cadence
                    let first_query_due = now > last_contact + self.first_query_interval;
                    let query_due = now > last_contact + self.query_interval
                        && now > last_contact + self.query_interval;
                    if now > last_contact + self.query_interval && (first_query_due || query_due) {
                        {
                            self.state.lock().unwrap().last_check_query_time = now;
                        }
                        let payload = json!({"timestamp": 0});
                        let _ = self.emit("check_secondary_query", payload);
                    } else if last_contact > 0.0 {
                        let last_check = self.state.lock().unwrap().last_check_query_time;
                        if last_check > last_contact {
                            if last_check - last_contact > 3.9 {
                                warn!("Disconnecting after no response for 'check_secondary_query' received for secondary {} at {}",
                                    self.id + 1, self.address);
                                self.on_disconnect();
                            }
                        }
                    }
                }
            }
            let _ = freqs_sent;
        }
        debug!("Exiting worker thread for secondary timer {}", self.id + 1);
    }

    fn connect_once(self: &Arc<Self>, state: &crate::state::SharedState, now: f64) {
        let mut st = self.state.lock().unwrap();
        // beyond timeout with no prior connection: give up
        if now > st.start_connect_time + self.query_timeout && st.num_disconnects == 0 {
            warn!("Reached timeout; no longer trying to connect to secondary {} at {}", self.id + 1, self.address);
            st.running = false;
            drop(st);
            self.stop_flag.store(true, Ordering::Relaxed);
            return;
        }
        drop(st);

        let use_auth = self.state.lock().unwrap().use_auth_header;
        let mut builder = rust_socketio::ClientBuilder::new(&self.address).reconnect(false);
        if use_auth {
            if let Some(header) = Self::auth_headers(state, &self.info) {
                builder = builder.opening_header("Authorization", header);
            }
        }

        let node = Arc::clone(self);
        let node_disc = Arc::clone(self);
        let node_pass = Arc::clone(self);
        let state_pass = state.clone();
        let node_check = Arc::clone(self);
        let node_join = Arc::clone(self);
        let client = builder
            .on("connect", move |_payload, _| {
                node.on_connect();
            })
            .on("disconnect", move |_, _| {
                node_disc.on_disconnect();
            })
            .on("pass_record", move |payload, _| {
                node_pass.on_pass_record(&state_pass, payload_to_value(payload));
            })
            .on("check_secondary_response", move |payload, _| {
                node_check.on_check_secondary_response(payload_to_value(payload));
            })
            .on("join_cluster_response", move |payload, _| {
                node_join.on_join_cluster_response(payload_to_value(payload));
            })
            .on("error", |err, _| {
                tracing::debug!("Secondary socket error: {:?}", err);
            })
            .connect_raw();

        match client {
            Ok(client) => {
                *self.client.lock().unwrap() = Some(client);
            }
            Err(e) => {
                let st = self.state.lock().unwrap();
                if st.last_contact > 0.0 {
                    info!("Error connecting to secondary {} at {}: {}", self.id + 1, self.address, e);
                    drop(st);
                    self.on_disconnect();
                } else if st.num_disconnects > 0 {
                    info!("Unable to connect to secondary {} at {}: {}", self.id + 1, self.address, e);
                }
            }
        }
    }

    fn settle_join(&self, state: &crate::state::SharedState) {
        {
            self.state.lock().unwrap().joined = true;
        }
        let race_status = state.race.lock().unwrap().race_status;
        if !self.is_mirror_mode() && (race_status == RACE_STATUS_STAGING || race_status == RACE_STATUS_RACING) {
            let _ = self.emit("stage_race", Value::Null);
        }
    }

    fn send_frequencies(&self, state: &crate::state::SharedState) {
        {
            self.state.lock().unwrap().freqs_sent = true;
        }
        if self.is_mirror_mode() {
            return;
        }
        info!("Sending node frequencies to secondary {} at {}", self.id + 1, self.address);
        crate::db::with_db(state, |conn| {
            let pid = crate::db::current_profile_id(conn);
            for (idx, (_, _, freq)) in crate::db::profile_frequencies(conn, pid).iter().enumerate() {
                let _ = self.emit("set_frequency", json!({"node": idx, "frequency": freq}));
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
        });
    }

    /// Emit through the connected client; updates contact stats.
    fn emit(&self, event: &str, data: Value) -> bool {
        let mut st = self.state.lock().unwrap();
        if st.last_contact > 0.0 {
            if let Some(client) = self.client.lock().unwrap().as_ref() {
                let ok = client.emit(event, data).is_ok();
                if ok {
                    st.last_contact = crate::state::monotonic();
                    st.num_contacts += 1;
                }
                return ok;
            }
        }
        false
    }

    fn on_connect(&self) {
        let mut st = self.state.lock().unwrap();
        if st.last_contact <= 0.0 {
            st.last_contact = crate::state::monotonic();
            st.first_contact_time = st.last_contact;
            st.joined = false;
            st.join_connect_time = st.last_contact;
            if st.auth_reconnect {
                info!("Reconnected to secondary {} at {} with admin credentials", self.id + 1, self.address);
            } else if st.num_disconnects == 0 {
                info!("Connected to secondary {} at {} (mode: {})", self.id + 1, self.address, self.mode.as_str());
            } else {
                info!("Reconnected to secondary {} at {}", self.id + 1, self.address);
            }
            let _ = self.emit("join_cluster_ex", json!({"mode": self.mode.as_str()}));
            if st.auth_reconnect {
                st.auth_reconnect = false;
            }
        } else {
            st.last_contact = crate::state::monotonic();
            debug!("Received extra 'on_connect' event for secondary {} at {}", self.id + 1, self.address);
        }
    }

    pub fn on_disconnect(&self) {
        let mut st = self.state.lock().unwrap();
        if st.last_contact > 0.0 {
            st.start_connect_time = crate::state::monotonic();
            st.last_contact = -1.0;
            st.num_disconnects += 1;
            if st.auth_reconnect {
                debug!("Disconnected from secondary {} at {} to reconnect with admin credentials", self.id + 1, self.address);
                return;
            }
            if st.auth_failed {
                debug!("Disconnected from secondary {} at {} after its admin credentials were refused", self.id + 1, self.address);
                return;
            }
            st.num_disconns_during_race += 1;
            warn!("Disconnected from secondary {} at {}", self.id + 1, self.address);
            *self.client.lock().unwrap() = None;
        } else {
            debug!("Received extra 'on_disconnect' event for secondary {} at {}", self.id + 1, self.address);
        }
    }

    fn on_check_secondary_response(&self, payload: Value) {
        let mut st = self.state.lock().unwrap();
        if st.last_contact > 0.0 {
            let now = crate::state::monotonic();
            st.last_contact = now;
            st.num_contacts += 1;
            let transit = if st.last_check_query_time > 0.0 { now - st.last_check_query_time } else { 0.0 };
            if transit > 0.0 {
                st.latency.add((transit * 1000.0).round() as i64);
                if !payload.is_null() {
                    let ts = payload.get("timestamp").and_then(|v| v.as_i64()).unwrap_or(0);
                    if ts != 0 {
                        let local = epoch_millis_from_monotonic(st.last_check_query_time + transit / 2.0);
                        st.time_diff_median.insert(ts - local);
                        st.time_diff_median_ms = st.time_diff_median.median();
                        return;
                    }
                }
                debug!("Received check_secondary_response with no timestamp from secondary {} at {}",
                    self.id + 1, self.address);
            }
        } else {
            debug!("Received check_secondary_response while disconnected from secondary {} at {}",
                self.id + 1, self.address);
        }
    }

    fn on_join_cluster_response(&self, payload: Value) {
        let mut st = self.state.lock().unwrap();
        let auth_ok = payload.get("auth_ok").and_then(|v| v.as_bool());
        if auth_ok != Some(true) && !st.use_auth_header && Self::auth_headers_for(&self.info).is_some() {
            info!("Secondary {} at {} requires admin credentials; reconnecting with admin credentials",
                self.id + 1, self.address);
            st.use_auth_header = true;
            st.auth_reconnect = true;
            drop(st);
            *self.client.lock().unwrap() = None;
            return;
        }
        if auth_ok == Some(false) {
            warn!("Secondary {} at {} did not accept this timer's admin credentials; disconnecting", self.id + 1, self.address);
            st.auth_failed = true;
            st.running = false;
            self.stop_flag.store(true, Ordering::Relaxed);
            drop(st);
            *self.client.lock().unwrap() = None;
            return;
        }
        if !st.joined {
            st.joined = true;
        }
        let info_str = payload.get("server_info").and_then(|v| v.as_str()).unwrap_or("{}");
        let info_dict: Value = serde_json::from_str(info_str).unwrap_or(json!({}));
        let prg_str = info_dict.get("prog_start_epoch").and_then(|v| v.as_str());
        let mut new_prg_start = false;
        if let Some(s) = prg_str {
            if let Ok(v) = s.parse::<f64>() {
                let prg_epoch = v as i64;
                if st.prog_start_epoch == 0 {
                    st.prog_start_epoch = prg_epoch;
                    new_prg_start = true;
                } else if prg_epoch != st.prog_start_epoch {
                    st.prog_start_epoch = prg_epoch;
                    new_prg_start = true;
                    info!("New 'prog_start_epoch' value for secondary {}; resetting 'timeDiff' median", self.id + 1);
                    st.time_diff_median = RunningMedian::new(TIMEDIFF_MEDIAN_SIZE);
                    st.time_diff_median_ms = 0.0;
                }
            }
        }
        let version = info_dict.get("release_version").and_then(|v| v.as_str());
        if new_prg_start || st.num_disconnects == 0 {
            match version {
                Some(v) if v != crate::state::RELEASE_VERSION => warn!(
                    "Different program version ('{}') running on secondary {} at {}",
                    v, self.id + 1, self.address
                ),
                Some(_) => {}
                None => warn!("Unable to parse 'release_version' from secondary {} at {}", self.id + 1, self.address),
            }
        }
        drop(st);
        // tell the secondary to expect message-acks for pass_record
        let _ = self.emit("cluster_message_ack", json!({"messageType": "join_cluster_response"}));
    }

    fn auth_headers_for(info: &Value) -> Option<String> {
        let username = get_info_str(info, "username").unwrap_or("");
        let password = get_info_str(info, "password").unwrap_or("");
        if username.is_empty() && password.is_empty() {
            None
        } else {
            Some(format!("Basic {}", base64_encode(format!("{}:{}", username, password).as_bytes())))
        }
    }

    /// Pass record from a secondary: split/speed/action handling
    /// (on_pass_record).
    fn on_pass_record(&self, state: &crate::state::SharedState, payload: Value) {
        {
            let mut st = self.state.lock().unwrap();
            st.last_contact = crate::state::monotonic();
            st.num_contacts += 1;
        }
        let node_index = payload.get("node").and_then(|v| v.as_i64()).unwrap_or(0) as usize;
        let racing = state.race.lock().unwrap().race_status == RACE_STATUS_RACING;
        if !racing {
            info!("Ignoring split {} for node {} because race not running", self.id + 1, node_index + 1);
            self.ack_pass_record(&payload);
            return;
        }

        let current_heat = state.race.lock().unwrap().current_heat;
        let pilot_id = current_heat.and_then(|heat| {
            crate::db::with_db(state, |conn| {
                conn.query_row(
                    "SELECT pilot_id FROM heat_node WHERE heat_id = ?1 AND node_index = ?2",
                    rusqlite::params![heat, node_index as i64],
                    |r| r.get::<_, Option<i64>>(0),
                )
                .ok()
                .flatten()
            })
        });

        let Some(pilot_id) = pilot_id else {
            info!("Split pass record dismissed: Node {}, no pilot on node", node_index + 1);
            self.ack_pass_record(&payload);
            return;
        };

        let split_ts_epoch_ms = payload.get("timestamp").and_then(|v| v.as_i64()).unwrap_or(0);
        let time_diff = self.state.lock().unwrap().time_diff_median_ms;
        let split_ts_epoch_ms = split_ts_epoch_ms - time_diff as i64;

        if self.is_action_mode() {
            // action mode: rate-limited event trigger
            let min_repeat = get_info_f64(&self.info, "minRepeatSecs", 10.0);
            let now = crate::state::monotonic();
            let mut st = self.state.lock().unwrap();
            let last = st.action_pass_times.get(&node_index).copied().unwrap_or(0.0);
            if now - last >= min_repeat {
                st.action_pass_times.insert(node_index, now);
                drop(st);
                info!("Secondary 'action' timer pass record: Node {}, pilot_id {}", node_index + 1, pilot_id);
                self.play_tone(state);
                if let Some(event) = &self.event {
                    crate::socket::trigger_custom_event(state, event, &json!({
                        "node_index": node_index, "pilot_id": pilot_id
                    }));
                }
            } else {
                info!("Ignoring secondary 'action' timer pass record too soon after previous (limit={} secs): Node {}",
                    min_repeat, node_index + 1);
            }
            self.ack_pass_record(&payload);
            return;
        }

        // split mode
        let split_ts = (split_ts_epoch_ms as f64) - start_time_epoch_ms(state);
        let lap_count = {
            let race = state.race.lock().unwrap();
            race.node_laps
                .get(node_index)
                .map(|laps| laps.len().saturating_sub(1))
                .unwrap_or(0)
        };
        let split_id = self.id;

        // find the timestamp of the previous split/last lap for this node
        let last_split_ts: Option<f64> = crate::db::with_db(state, |conn| {
            let rows: Vec<(i64, f64)> = {
                // (split ids are i64 in the schema; lap ids likewise)
                let mut stmt = conn
                    .prepare(
                        "SELECT split_id, split_time_stamp FROM lap_split \
                         WHERE node_index = ?1 AND lap_id = ?2 ORDER BY split_id",
                    )
                    .ok()?;
                let rows = stmt
                    .query_map(rusqlite::params![node_index as i64, lap_count as i64], |r| {
                        Ok((r.get::<_, i64>(0)?, r.get::<_, f64>(1)?))
                    })
                    .ok()?;
                rows.filter_map(|r| r.ok()).collect()
            };
            if rows.is_empty() {
                if split_id == 0 {
                    // timestamp of the last lap pass (including lap 0);
                    // lap stamps are monotonic seconds, matching split_ts
                    let race = state.race.lock().unwrap();
                    race.node_laps
                        .get(node_index)
                        .and_then(|laps| laps.last())
                        .map(|lap| lap.lap_time_stamp)
                } else {
                    None // out-of-order first split
                }
            } else {
                let last_split_id = rows[rows.len() - 1].0;
                let split_id = split_id as i64;
                if split_id > last_split_id {
                    if split_id == last_split_id + 1 {
                        Some(rows[rows.len() - 1].1)
                    } else {
                        None // missing splits in between
                    }
                } else {
                    None // out-of-order
                }
            }
        });

        let split_time = last_split_ts.map(|last| ((split_ts - last) * 1000.0).round() / 1000.0);

        match split_time {
            Some(t) if t <= 0.0 => {
                info!("Ignoring split {} with non-positive time {} for node {}",
                    split_id + 1, t, node_index + 1);
            }
            Some(t) => {
                let split_speed = if self.distance_m > 0.0 {
                    Some((self.distance_m / t * 100.0).round() / 100.0)
                } else {
                    None
                };
                info!(
                    "Split pass record: Node {}, lap {}, split {}, time={}s, speed={}",
                    node_index + 1,
                    lap_count + 1,
                    split_id + 1,
                    t,
                    split_speed.map(|s| s.to_string()).unwrap_or_else(|| "None".into())
                );
                let split_data = json!({
                    "node_index": node_index,
                    "pilot_id": pilot_id,
                    "lap_id": lap_count,
                    "split_id": split_id,
                    "split_time_stamp": split_ts,
                    "split_time": t,
                    "split_speed": split_speed,
                    "time_callout_flag": self.time_callout,
                    "speed_callout_flag": self.speed_callout,
                    "name_callout_flag": self.name_callout,
                });
                crate::db::with_db(state, |conn| {
                    let _ = conn.execute(
                        "INSERT INTO lap_split (node_index, pilot_id, lap_id, split_id, \
                                                split_time_stamp, split_time, split_time_formatted, split_speed) \
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                        rusqlite::params![
                            node_index as i64,
                            pilot_id,
                            lap_count as i64,
                            split_id as i64,
                            split_ts_epoch_ms,
                            (t * 1000.0) as i64,
                            format_split_time(t),
                            split_speed,
                        ],
                    );
                });
                crate::socket::broadcast(
                    state,
                    "split_pass_info",
                    json!({"split_pass_info": split_data}),
                );
                if let Some(event) = &self.event {
                    crate::socket::trigger_custom_event(state, event, &split_data);
                }
            }
            None => {
                debug!("Ignoring out-of-order or early split {} for node {}", split_id + 1, node_index + 1);
            }
        }

        // hand the timestamp to the next split timer for speed chaining
        {
            let mut st = self.state.lock().unwrap();
            if !st.prev_sec_pass_tstamps.is_empty() {
                st.prev_sec_pass_tstamps.insert(node_index, None);
            }
        }
        self.ack_pass_record(&payload);
    }

    fn play_tone(&self, state: &crate::state::SharedState) {
        let duration = get_info_f64(&self.info, "toneDuration", 0.0) as i64;
        if duration <= 0 {
            return;
        }
        let frequency = get_info_f64(&self.info, "toneFrequency", 0.0) as i64;
        let volume = get_info_f64(&self.info, "toneVolume", 100.0) as i64;
        if frequency > 0 && volume > 0 {
            crate::socket::broadcast(
                state,
                "play_beep_tone",
                json!({
                    "duration": duration,
                    "frequency": frequency,
                    "volume": volume,
                    "type": get_info_str(&self.info, "toneType").unwrap_or("square"),
                }),
            );
        }
    }

    fn ack_pass_record(&self, payload: &Value) {
        let _ = self.emit(
            "cluster_message_ack",
            json!({"messageType": "pass_record", "messagePayload": payload}),
        );
    }

    /// Cluster status row for the UI (getClusterStatusInfo entry).
    pub fn status_entry(&self) -> Value {
        let now = crate::state::monotonic();
        let st = self.state.lock().unwrap();
        let (up_secs, down_secs) = if st.last_contact > 0.0 {
            ((now - st.first_contact_time).round() as u64, 0)
        } else {
            (0, st.secs_since_disconnect.round() as u64)
        };
        let total_up = st.total_up_secs + up_secs;
        let total_down = st.total_down_secs + down_secs;
        let availability = if total_up + total_down > 0 {
            (100.0 * total_up as f64 / (total_up + total_down) as f64 * 10.0).round() / 10.0
        } else {
            0.0
        };
        let last_contact_str = if st.last_contact >= 0.0 {
            format!("{}s", (now - st.last_contact) as i64)
        } else if st.auth_failed {
            "Auth failed".to_string()
        } else if st.num_disconnects > 0 {
            "connection lost".to_string()
        } else if st.running {
            "never connected".to_string()
        } else {
            "Not found".to_string()
        };
        json!({
            "address": self.address,
            "modeIndicator": match self.mode {
                Mode::Mirror => "Mirror",
                Mode::Split => "Split",
                Mode::Action => "Action",
            },
            "minLatencyMs": st.latency.min,
            "avgLatencyMs": st.latency.int_avg(),
            "maxLatencyMs": st.latency.max,
            "lastLatencyMs": st.latency.last,
            "numDisconnects": st.num_disconnects,
            "numContacts": st.num_contacts,
            "timeDiffMs": st.time_diff_median_ms,
            "upTimeSecs": up_secs,
            "downTimeSecs": down_secs,
            "availability": availability,
            "last_contact": last_contact_str,
        })
    }

    fn total_up_secs(&self) -> u64 {
        self.state.lock().unwrap().total_up_secs
    }
    fn time_diff_median_ms(&self) -> f64 {
        self.state.lock().unwrap().time_diff_median_ms
    }
    fn add_up_time(&self, secs: u64) {
        self.state.lock().unwrap().total_up_secs += secs;
    }
    fn clear_prev_pass_tstamps(&self) {
        self.state.lock().unwrap().prev_sec_pass_tstamps.clear();
    }
    fn reset_disconns_during_race(&self) {
        self.state.lock().unwrap().num_disconns_during_race = 0;
    }
}

/// monotonic seconds -> epoch milliseconds, using the same program-start
/// epoch the rest of the server uses.
pub fn epoch_millis_from_monotonic(monotonic_secs: f64) -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    let now_epoch = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0);
    now_epoch - ((crate::state::monotonic() - monotonic_secs) * 1000.0) as i64
}

fn start_time_epoch_ms(state: &crate::state::SharedState) -> f64 {
    state.race.lock().unwrap().start_time_epoch_ms
}

fn format_split_time(t: f64) -> String {
    let secs = t.floor() as i64;
    let millis = ((t - t.floor()) * 1000.0).round() as i64;
    format!("{:02}:{:02}.{:03}", secs / 60, secs % 60, millis)
}

pub fn base64_encode(data: &[u8]) -> String {
    const CHARS: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for chunk in data.chunks(3) {
        let b = [chunk[0], *chunk.get(1).unwrap_or(&0), *chunk.get(2).unwrap_or(&0)];
        let n = ((b[0] as u32) << 16) | ((b[1] as u32) << 8) | b[2] as u32;
        out.push(CHARS[(n >> 18) as usize & 63] as char);
        out.push(CHARS[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 { CHARS[(n >> 6) as usize & 63] as char } else { '=' });
        out.push(if chunk.len() > 2 { CHARS[n as usize & 63] as char } else { '=' });
    }
    out
}

/// The primary-side manager (ClusterNodeSet).
pub struct ClusterNodeSet {
    pub secondaries: Vec<Arc<SecondaryNode>>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_matches_standard_vectors() {
        assert_eq!(base64_encode(b""), "");
        assert_eq!(base64_encode(b"f"), "Zg==");
        assert_eq!(base64_encode(b"fo"), "Zm8=");
        assert_eq!(base64_encode(b"foo"), "Zm9v");
        assert_eq!(base64_encode(b"foob"), "Zm9vYg==");
        assert_eq!(base64_encode(b"fooba"), "Zm9vYmE=");
        assert_eq!(base64_encode(b"foobar"), "Zm9vYmFy");
        // auth header style
        assert_eq!(base64_encode(b"admin:pass"), "YWRtaW46cGFzcw==");
    }

    #[test]
    fn password_masking_roundtrip() {
        let secondaries = json!([
            {"address": "a", "password": "secret1"},
            {"address": "b", "password": ""},
            {"address": "c"},
        ]);
        let masked = mask_secondary_passwords(&secondaries);
        assert_eq!(masked[0]["password"], SECONDARY_PASSWORD_MASK);
        assert_eq!(masked[1]["password"], ""); // empty passwords are not masked
        assert!(masked[2].get("password").is_none());
        // restore puts the real password back, matching by address
        let mut restored = masked;
        restore_secondary_passwords(&mut restored, &secondaries);
        assert_eq!(restored[0]["password"], "secret1");
        // matching falls back to position when the address changed
        let mut changed = json!([
            {"address": "new-address", "password": SECONDARY_PASSWORD_MASK},
        ]);
        restore_secondary_passwords(&mut changed, &secondaries);
        assert_eq!(changed[0]["password"], "secret1");
    }

    #[test]
    fn averager_tracks_min_max_last() {
        let mut avg = Averager::new(LATENCY_AVG_SIZE);
        assert_eq!(avg.avg(), 0.0);
        for v in [100i64, 50, 200, 150] {
            avg.add(v);
        }
        assert_eq!(avg.min, 50);
        assert_eq!(avg.max, 200);
        assert_eq!(avg.last, 150);
        assert_eq!(avg.avg(), 125.0);
        assert_eq!(avg.int_avg(), 125);
        // fill to capacity, then confirm ring behavior: the oldest drops
        for v in 0..LATENCY_AVG_SIZE as i64 {
            avg.add(v);
        }
        assert_eq!(avg.len(), LATENCY_AVG_SIZE);
        assert_eq!(avg.min, 0); // the original 50 dropped off long ago
        avg.add(-5);
        assert_eq!(avg.min, -5);
        assert_eq!(avg.max, 29);
    }

    #[test]
    fn running_median_matches_python_semantics() {
        let mut med = RunningMedian::new(TIMEDIFF_MEDIAN_SIZE);
        assert_eq!(med.median(), 0.0);
        assert!(med.is_empty());
        for v in [5i64, 1, 9] {
            med.insert(v);
        }
        assert_eq!(med.median(), 5.0); // sorted [1,5,9], odd count
        med.insert(3);
        assert_eq!(med.median(), 4.0); // sorted [1,3,5,9]: midpoint of 3,5
        // (window is 30 deep; nothing drops yet)
        // fill past capacity and confirm the ring drops the OLDEST value,
        // not the smallest
        let mut med2 = RunningMedian::new(TIMEDIFF_MEDIAN_SIZE);
        for v in 0..TIMEDIFF_MEDIAN_SIZE as i64 {
            med2.insert(v);
        }
        assert_eq!(med2.median(), 14.5); // sorted 0..29, midpoint 14,15
        med2.insert(1000); // drops 0
        med2.insert(-1000); // drops 1
        // sorted [-1000, 2..29 (28 values), 1000]: midpoint of 15,16
        assert_eq!(med2.median(), 15.5);
    }

    #[test]
    fn split_time_formatting() {
        assert_eq!(format_split_time(0.0), "00:00.000");
        assert_eq!(format_split_time(1.5), "00:01.500");
        assert_eq!(format_split_time(65.25), "01:05.250");
        assert_eq!(format_split_time(600.0), "10:00.000");
    }

    #[test]
    fn epoch_conversion_is_recent() {
        // the monotonic->epoch conversion must land near the current epoch
        let now_epoch = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as i64;
        let converted = epoch_millis_from_monotonic(crate::state::monotonic());
        assert!((converted - now_epoch).abs() < 5000, "conversion drifted: {} vs {}", converted, now_epoch);
        // 10 seconds in the past converts to ~10s earlier
        let past = epoch_millis_from_monotonic(crate::state::monotonic() - 10.0);
        assert!((now_epoch - past) >= 9500 && (now_epoch - past) <= 10500);
    }

    #[test]
    fn payload_text_extracts_first_value() {
        let v = payload_to_value(rust_socketio::Payload::Text(vec![json!({"node": 1})]));
        assert_eq!(v["node"], 1);
        let empty = payload_to_value(rust_socketio::Payload::Text(vec![]));
        assert!(empty.is_null());
    }
}

#[allow(dead_code)] // settings-UI/retry/status handlers land with the UI pass
impl ClusterNodeSet {
    /// Builds secondaries from the SECONDARIES config entries and starts
    /// their worker threads; None when no secondaries are configured.
    pub fn new(state: crate::state::SharedState, config: &Value) -> Option<ClusterNodeSet> {
        let entries = config
            .get("GENERAL")
            .and_then(|g| g.get("SECONDARIES"))
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();
        if entries.is_empty() {
            return None;
        }
        let mut set = ClusterNodeSet { secondaries: Vec::new() };
        for (idx, entry) in entries.into_iter().enumerate() {
            let node = SecondaryNode::new(idx, entry.clone());
            node.start_connection(state.clone());
            let _ = entry;
            set.secondaries.push(node);
        }
        info!("Cluster: managing {} secondary timer(s)", set.secondaries.len());
        Some(set)
    }

    pub fn has_secondaries(&self) -> bool {
        !self.secondaries.is_empty()
    }

    pub fn has_rec_events_secondaries(&self) -> bool {
        self.secondaries.iter().any(|s| s.rec_events())
    }

    pub fn is_split_secondary_available(&self, idx: usize) -> bool {
        self.secondaries.get(idx).map_or(false, |s| {
            s.is_split_mode() && (s.last_contact_time() > 0.0 || s.num_disconnects() > 0)
        })
    }

    /// Forces split secondaries to re-send frequencies on next contact
    /// (resendFrequencies in Python).
    pub fn resend_frequencies(&self) {
        for secondary in &self.secondaries {
            if secondary.is_split_mode() {
                secondary.reset_freqs_sent();
            }
        }
    }

    /// Emits an event to all secondaries (emit()).
    pub fn emit(&self, event: &str, data: Value) {
        for secondary in &self.secondaries {
            let _ = secondary.emit(event, data.clone());
        }
    }

    /// Emits to split/action secondaries, with optional clock correction
    /// (emitToSplits).
    pub fn emit_to_splits(&self, event: &str, data: Value, add_time_corr: bool) {
        for secondary in &self.secondaries {
            if secondary.is_split_or_action() {
                let mut payload = data.clone();
                if add_time_corr {
                    if let Some(obj) = payload.as_object_mut() {
                        obj.insert("correction_ms".into(), json!(secondary.time_diff_median_ms()));
                    }
                }
                let _ = secondary.emit(event, payload);
            }
        }
    }

    /// Forwards a server event to event-recording secondaries
    /// (emitEventTrigger / the event repeater).
    pub fn emit_event_trigger(&self, evt_name: &str, evt_args: &Value) {
        if !self.has_rec_events_secondaries() {
            return;
        }
        let payload = json!({
            "evt_name": evt_name,
            "evt_args": serde_json::to_string(evt_args).unwrap_or_default(),
        });
        for secondary in &self.secondaries {
            if secondary.rec_events() {
                let _ = secondary.emit("cluster_event_trigger", payload.clone());
            }
        }
    }

    pub fn shutdown(&self) {
        for secondary in &self.secondaries {
            info!("Setting 'runningFlag' to False on secondary timer {}", secondary.id + 1);
            secondary.stop();
        }
    }

    /// Retries a stopped secondary (retrySecondary from the UI).
    pub fn retry_secondary(&self, state: &crate::state::SharedState, secondary_id: usize) {
        if let Some(secondary) = self.secondaries.get(secondary_id) {
            if !secondary.is_running() {
                info!("Retrying connection to secondary {} at {}", secondary.id + 1, secondary.address);
                secondary.start_connection(state.clone());
            } else {
                warn!("Attempted retry of running secondary {}", secondary_id + 1);
            }
        } else {
            warn!("Secondary ID value ({}) out of bounds in retrySecondary", secondary_id + 1);
        }
    }

    pub fn get_cluster_status_info(&self) -> Value {
        json!({ "secondaries": self.secondaries.iter().map(|s| s.status_entry()).collect::<Vec<_>>() })
    }

    /// Per-race bookkeeping at race start (doClusterRaceStart).
    pub fn do_cluster_race_start(&self) {
        for secondary in &self.secondaries {
            secondary.reset_disconns_during_race();
            secondary.clear_prev_pass_tstamps();
            if secondary.last_contact_time() > 0.0 {
                let td = secondary.time_diff_median_ms();
                if td.abs() > TIMEDIFF_WARNING_THRESH_MS as f64 {
                    info!("Secondary {} clock not synchronized with primary, timeDiff={}ms", secondary.id + 1, td);
                } else {
                    debug!("Secondary {} clock synchronized OK with primary, timeDiff={}ms", secondary.id + 1, td);
                }
            } else if secondary.num_disconnects() > 0 {
                warn!("Secondary {} not connected at race start", secondary.id + 1);
            }
        }
    }

    /// Per-race bookkeeping at race stop (doClusterRaceStop).
    pub fn do_cluster_race_stop(&self) {
        for secondary in &self.secondaries {
            let up = secondary.total_up_secs();
            let _ = up;
            if secondary.last_contact_time() > 0.0 {
                info!("Connected at race stop to secondary {} at {}", secondary.id + 1, secondary.address);
            } else if secondary.num_disconnects() > 0 {
                warn!("Not connected at race stop to secondary {} at {}", secondary.id + 1, secondary.address);
            }
        }
    }
}
