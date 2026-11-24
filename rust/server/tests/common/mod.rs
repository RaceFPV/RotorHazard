//! Shared test harness (compiled per test binary; not every binary
//! uses every item).
#![allow(dead_code)]

use serde_json::{json, Value};
use std::net::TcpListener;
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

pub const RACE_STATUS_READY: i64 = 0;
pub const RACE_STATUS_RACING: i64 = 1;
pub const RACE_STATUS_DONE: i64 = 2;
pub const RACE_STATUS_STAGING: i64 = 3;

/// Timestamp token for unique entity names within a run.
pub fn test_token() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    format!("{}", SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs())
}

struct SharedEvents {
    events: Vec<(String, Value)>,
}

/// One running server under test.
pub struct ServerHandle {
    pub base_url: String,
    pub data_dir: std::path::PathBuf,
    child: Mutex<Option<Child>>,
    log_path: std::path::PathBuf,
}

impl ServerHandle {
    pub fn stop(&mut self) {
        if let Some(child) = self.child.get_mut().ok().and_then(|c| c.as_mut()) {
            let _ = child.kill();
            let _ = child.wait();
        }
        let _ = std::fs::remove_dir_all(&self.data_dir);
    }

    pub fn log_tail(&self) -> String {
        std::fs::read_to_string(&self.log_path).unwrap_or_else(|_| "(log unavailable)".into())
    }
}

impl Drop for ServerHandle {
    fn drop(&mut self) {
        self.stop();
    }
}

pub fn free_port_pub() -> u16 { free_port() }

fn free_port() -> u16 {
    for _ in 0..50 {
        if let Ok(listener) = TcpListener::bind(("127.0.0.1", 0)) {
            let port = listener.local_addr().unwrap().port();
            drop(listener);
            return port;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    panic!("Unable to find a free port");
}

/// Spawns the server binary (the one cargo built for this test run) with
/// mock nodes and an empty database — spawn_server in the Python harness.
pub fn spawn_server() -> ServerHandle {
    spawn_server_with_config(&json!({
        "SECRETS": {"ADMIN_USERNAME": "", "ADMIN_PASSWORD": ""},
        "GENERAL": {"HTTP_PORT": 0, "DEBUG": false, "MOCK_NODE_SIGNAL": "0"},
    }))
}

/// Spawns a server with a custom config (spawn_custom_server in Python).
/// The HTTP_PORT in the config is overridden with a free port.
pub fn spawn_server_with_config(config: &Value) -> ServerHandle {
    let bin = env!("CARGO_BIN_EXE_rh-server");
    let port = free_port();
    let data_dir = std::env::temp_dir().join(format!(
        "rh_rs_proto_{}_{}",
        std::process::id(),
        port
    ));
    std::fs::create_dir_all(&data_dir).unwrap();
    let mut cfg = config.clone();
    if let Some(g) = cfg.get_mut("GENERAL") {
        if let Some(obj) = g.as_object_mut() {
            obj.insert("HTTP_PORT".into(), json!(port));
        }
    }
    std::fs::write(data_dir.join("config.json"), cfg.to_string()).unwrap();
    build_handle(bin, port, data_dir)
}

fn build_handle(bin: &str, port: u16, data_dir: std::path::PathBuf) -> ServerHandle {
    let log_path = data_dir.join("server_test.log");
    let data_dir = std::env::temp_dir().join(format!(
        "rh_rs_proto_{}_{}",
        std::process::id(),
        port
    ));
    std::fs::create_dir_all(&data_dir).unwrap();
    std::fs::write(
        data_dir.join("config.json"),
        json!({
            "SECRETS": {"ADMIN_USERNAME": "", "ADMIN_PASSWORD": ""},
            "GENERAL": {"HTTP_PORT": port, "DEBUG": false, "MOCK_NODE_SIGNAL": "0"},
        })
        .to_string(),
    )
    .unwrap();

    let log_file = std::fs::File::create(&log_path).unwrap();
    let child = Command::new(bin)
        .env("RH_DATA_DIR", &data_dir)
        .env("RH_NODES", "8")
        .stdout(Stdio::from(log_file.try_clone().unwrap()))
        .stderr(Stdio::from(log_file))
        .spawn()
        .expect("failed to spawn server");

    let mut handle = ServerHandle {
        base_url: format!("http://127.0.0.1:{}", port),
        data_dir,
        child: Mutex::new(Some(child)),
        log_path,
    };

    // wait for readiness
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let exited = {
            let mut child_guard = handle.child.lock().unwrap();
            match child_guard.as_mut() {
                Some(child) => child.try_wait().map(|s| s.is_some()).unwrap_or(false),
                None => true,
            }
        };
        if exited {
            panic!("server exited -- log:\n{}", handle.log_tail());
        }
        if http_get_json(&format!("{}/api/status", handle.base_url)).is_ok() {
            break;
        }
        if Instant::now() > deadline {
            handle.stop();
            panic!("server not ready in 30s -- log:\n{}", handle.log_tail());
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    handle
}

/// Minimal HTTP GET returning parsed JSON (http_get_json in Python).
pub fn http_get_json(url: &str) -> Result<Value, String> {
    let mut resp = ureq::get(url).call().map_err(|e| e.to_string())?;
    let text = resp
        .body_mut()
        .read_to_string()
        .map_err(|e| e.to_string())?;
    serde_json::from_str(&text).map_err(|e| e.to_string())
}

/// Socket.IO test client wrapper (the Client class in the Python
/// harness): collects broadcasts, waits for events, emits with acks.
pub struct SocketClient {
    client: rust_socketio::client::Client,
    shared: Arc<(Mutex<Vec<(String, Value)>>, Condvar)>,
    ack_seq: std::sync::atomic::AtomicU64,
    ack_results: Arc<(Mutex<std::collections::HashMap<u64, Value>>, Condvar)>,
    pub base_url: String,
}

impl SocketClient {
    pub fn connect(base_url: &str) -> SocketClient {
        let shared = Arc::new((Mutex::new(Vec::new()), Condvar::new()));
        let ack_shared = Arc::new((
            Mutex::new(std::collections::HashMap::new()),
            Condvar::new(),
        ));

        let ev_shared = shared.clone();
        let client = rust_socketio::ClientBuilder::new(base_url)
            .reconnect(false)
            .on_any(move |event, payload, _| {
                if let rust_socketio::Payload::Text(vals) = payload {
                    if let Some(v) = vals.into_iter().next() {
                        let (lock, cvar) = &*ev_shared;
                        let mut events = lock.lock().unwrap();
                        events.push((event.to_string(), v));
                        cvar.notify_all();
                    }
                }
            })
            .connect()
            .expect("socket connect failed");

        SocketClient {
            client,
            shared,
            ack_seq: std::sync::atomic::AtomicU64::new(0),
            ack_results: ack_shared,
            base_url: base_url.to_string(),
        }
    }

    pub fn disconnect(&mut self) {
        let _ = self.client.disconnect();
    }

    /// Emits an event (fire and forget).
    pub fn emit(&self, event: &str, data: &Value) {
        self.client.emit(event, data.clone()).expect("emit failed");
    }

    /// Drains all collected broadcasts.
    pub fn drain(&self) {
        let (lock, _) = &*self.shared;
        lock.lock().unwrap().clear();
    }

    /// Waits for the next broadcast with the given event name
    /// (wait_for in Python).  Returns its payload.
    pub fn wait_for(&self, event: &str, timeout: Duration) -> Value {
        let (lock, cvar) = &*self.shared;
        let deadline = Instant::now() + timeout;
        let mut events = lock.lock().unwrap();
        loop {
            if let Some(pos) = events.iter().position(|(e, _)| e == event) {
                let (_, v) = events.remove(pos);
                // remove everything before it too (stale broadcasts)
                let keep_from = pos;
                events.drain(..keep_from);
                return v;
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                let received: Vec<String> = events.iter().map(|(e, _)| e.clone()).collect();
                panic!(
                    "Timed out waiting for event \"{}\"; received: {:?}",
                    event, received
                );
            }
            let (guard, result) = cvar
                .wait_timeout(events, remaining)
                .unwrap();
            events = guard;
            let _ = result;
        }
    }

    /// Emits and waits for a response event (emit_and_wait in Python),
    /// draining stale broadcasts first.
    pub fn emit_and_wait(&self, event: &str, data: &Value, resp_event: &str, timeout: Duration) -> Value {
        self.drain();
        std::thread::sleep(Duration::from_millis(150));
        self.drain();
        self.emit(event, data);
        self.wait_for(resp_event, timeout)
    }

    /// Waits for a broadcast on either of two clients (wait_any in
    /// Python) — used by the two-client tests.
    pub fn wait_any(
        clients: [&SocketClient; 2],
        event: &str,
        timeout: Duration,
    ) -> Value {
        let deadline = Instant::now() + timeout;
        loop {
            for c in clients {
                {
                    let (lock, _) = &*c.shared;
                    let events = lock.lock().unwrap();
                    if let Some(pos) = events.iter().position(|(e, _)| e == event) {
                        let (_, v) = events[pos].clone();
                        drop(events);
                        let mut ev = c.shared.0.lock().unwrap();
                        ev.drain(..pos + 1);
                        return v;
                    }
                }
            }
            if Instant::now() > deadline {
                panic!("Timed out waiting for event \"{}\"", event);
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    /// Two-client variant with pre-drain (emit_and_wait semantics).
    pub fn emit_and_wait_any(
        emitter: &SocketClient,
        other: &SocketClient,
        event: &str,
        data: &Value,
        resp_event: &str,
        timeout: Duration,
    ) -> Value {
        emitter.drain();
        other.drain();
        std::thread::sleep(Duration::from_millis(150));
        emitter.drain();
        other.drain();
        emitter.emit(event, data);
        SocketClient::wait_any([emitter, other], resp_event, timeout)
    }

    /// Waits for a new entity id in a collection broadcast greater than
    /// prev_max (wait_for_new_id in Python).
    pub fn wait_for_new_id(
        &self,
        emit_event: &str,
        emit_data: &Value,
        resp_event: &str,
        id_field: &str,
        collection_field: &str,
        prev_max: i64,
        timeout: Duration,
    ) -> i64 {
        self.drain();
        self.emit(emit_event, emit_data);
        let deadline = Instant::now() + timeout;
        loop {
            let v = self.wait_for(resp_event, deadline.saturating_duration_since(Instant::now()));
            let ids: Vec<i64> = v
                .get(collection_field)
                .and_then(|c| c.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|item| item.get(id_field).and_then(|x| x.as_i64()))
                        .collect()
                })
                .unwrap_or_default();
            if let Some(&m) = ids.iter().max() {
                if m > prev_max {
                    return m;
                }
            }
            if Instant::now() > deadline {
                panic!("Timed out waiting for new id via {}", resp_event);
            }
        }
    }

    /// HTTP GET returning parsed JSON.
    pub fn http_get_json(&self, path: &str) -> Value {
        http_get_json(&format!("{}{}", self.base_url, path)).expect("HTTP GET failed")
    }

    /// Connects with HTTP basic auth on the socket connection request
    /// (the authenticated-client path of the auth tests).
    pub fn connect_with_auth(base_url: &str, user: &str, pass: &str) -> SocketClient {
        let creds = base64_encode(format!("{}:{}", user, pass).as_bytes());
        let shared = Arc::new((Mutex::new(Vec::new()), Condvar::new()));
        let ev_shared = shared.clone();
        let client = rust_socketio::ClientBuilder::new(base_url)
            .reconnect(false)
            .opening_header("Authorization", format!("Basic {}", creds))
            .on_any(move |event, payload, _| {
                if let rust_socketio::Payload::Text(vals) = payload {
                    if let Some(v) = vals.into_iter().next() {
                        let (lock, cvar) = &*ev_shared;
                        let mut events = lock.lock().unwrap();
                        events.push((event.to_string(), v));
                        cvar.notify_all();
                    }
                }
            })
            .connect()
            .expect("authed socket connect failed");
        SocketClient {
            client,
            shared,
            ack_seq: std::sync::atomic::AtomicU64::new(0),
            ack_results: Arc::new((Mutex::new(std::collections::HashMap::new()), Condvar::new())),
            base_url: base_url.to_string(),
        }
    }

    /// Socket.io ack round-trip: emits with an ack callback and waits
    /// for the reply (used by the auth tests).
    pub fn emit_ack(&self, event: &str, data: &Value, timeout: Duration) -> Value {
        let id = self
            .ack_seq
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let ack_shared = self.ack_results.clone();
        let _ = self.client.emit_with_ack(
            event,
            json!({"data": data, "ack_id": id}),
            timeout,
            move |payload, _| {
                if let rust_socketio::Payload::Text(vals) = payload {
                    if let Some(v) = vals.into_iter().next() {
                        let (lock, cvar) = &*ack_shared;
                        let mut acks = lock.lock().unwrap();
                        acks.insert(id, v);
                        cvar.notify_all();
                    }
                }
            },
        );
        let (lock, cvar) = &*self.ack_results;
        let mut acks = lock.lock().unwrap();
        let deadline = Instant::now() + timeout;
        loop {
            if let Some(v) = acks.remove(&id) {
                return v;
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                panic!("Timed out waiting for ack on \"{}\"", event);
            }
            let (guard, _) = cvar.wait_timeout(acks, remaining).unwrap();
            acks = guard;
        }
    }
}

/// Runs `f` with a spawned server and connected clients, stopping the
/// server afterwards (ProtocolTestCase setUp/tearDown).
pub fn with_server<T>(
    f: impl FnOnce(&ServerHandle, &SocketClient, &SocketClient) -> T,
) -> T {
    let mut server = spawn_server();
    let mut client = SocketClient::connect(&server.base_url);
    let mut observer = SocketClient::connect(&server.base_url);
    std::thread::sleep(Duration::from_millis(300));
    client.drain();
    observer.drain();
    let result = f(&server, &client, &observer);
    client.disconnect();
    observer.disconnect();
    server.stop();
    result
}

/// Reads the server log tail for diagnostics.
pub fn server_log(server: &ServerHandle) -> String {
    server.log_tail()
}

/// Base64 encoding for basic-auth headers.
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
