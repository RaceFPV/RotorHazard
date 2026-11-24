//! RotorHazard server -- Rust implementation (Phase 1 skeleton).
//!
//! Serves the existing static frontend, the /api JSON endpoints, and the
//! Socket.IO event surface, against the same database.db schema and seed
//! data as the Python server.  Behavior is gated by the protocol
//! characterization suite in src/tests/protocol/.

mod actions;
mod api;
mod heatgen;
mod imdtabler;
mod assets;
mod calibration;
mod class_rank;
mod cluster;
mod hardware;
mod db;
mod export_import;
mod fwupdate;
mod led;
mod mock;
mod vrx;
mod pages;
mod serial;
mod payloads;
mod phase3;
mod results;
mod race;
mod sensor;
mod socket;
mod state;
mod util;

use axum::extract::State;
use serde_json::{json, Value};
use socketioxide::SocketIo;
use state::{AppState, ServerInfo, SharedState};
use std::sync::{Arc, Mutex};

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info".into()),
        )
        .init();

    let port: u16 = std::env::var("RH_PORT")
        .or_else(|_| std::env::var("HTTP_PORT"))
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(5000);

    // minimal config.json support (same file the Python server writes)
    // first-run: create config.json with the Python default values
    // (admin/rotorhazard credentials, standard sections) when absent
    // Data directory resolution, mirroring the Python server's chain:
    // 1. RH_DATA_DIR env var (must exist)
    // 2. datapath.ini in the exe dir (expanduser, must exist)
    // 3. ~/rh-data if it exists
    // 4. exe dir if config.json exists there
    // 5. cwd if config.json exists there
    // 6. ~/rh-data created as needed
    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|p| p.to_path_buf()));

    let data_dir = resolve_data_dir(&exe_dir);

    let config_path = format!("{}/config.json", data_dir);
    if !std::path::Path::new(&config_path).exists() {
        std::fs::create_dir_all(&data_dir).expect("failed to create data directory");
        let secret_key: String = (0..50)
            .map(|_| {
                let idx = (std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.subsec_nanos())
                    .unwrap_or(0)
                    % 62) as usize;
                const CHARS: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";
                CHARS[idx] as char
            })
            .collect();
        let default_config = json!({
            "SECRETS": {
                "ADMIN_USERNAME": "admin",
                "ADMIN_PASSWORD": "rotorhazard",
                "SECRET_KEY": secret_key,
            },
            "GENERAL": {
                "HTTP_PORT": port,
                "SECONDARIES": [],
                "SECONDARY_TIMEOUT": 300,
                "DEBUG": false,
                "CORS_ALLOWED_HOSTS": "*",
                "ADMIN_SOCKET_AUTH": true,
                "FORCE_S32_BPILL_FLAG": false,
                "DEF_NODE_FWUPDATE_URL": "",
                "SHUTDOWN_BUTTON_GPIOPIN": 18,
                "SHUTDOWN_BUTTON_DELAYMS": 2500,
                "DB_AUTOBKP_NUM_KEEP": 30,
                "SYNC_LEADTIME_SECS": 0.9,
                "AUDIO_SINGLE_TAB": true,
                "LOG_SENSORS_DATA_RATE": 300,
                "SERIAL_PORTS": [],
                "MOCK_NODES": 0,
                // mock nodes simulate live RSSI by default so the Run
                // page graphs and auto-crossings work out of the box
                "MOCK_NODE_SIGNAL": 2,
                "MOCK_UPDATE_INTERVAL": 0.5,
                "LAST_MODIFIED_TIME": 0,
            },
            "TIMING": {
                "startThreshLowerAmount": "0",
                "startThreshLowerDuration": "0",
                "calibrationMode": "1",
                "MinLapBehavior": "0",
            },
            "UI": {
                "timerName": "RotorHazard",
                "timerLogo": "",
                "hue_0": "212", "sat_0": "55",
                "lum_0_low": "29.2", "lum_0_high": "46.7",
                "contrast_0_low": "#ffffff", "contrast_0_high": "#ffffff",
                "hue_1": "25", "sat_1": "85.3",
                "lum_1_low": "37.6", "lum_1_high": "54.5",
                "contrast_1_low": "#ffffff", "contrast_1_high": "#000000",
                "currentLanguage": "",
                "timeFormat": "{m}:{s}.{d}",
                "timeFormatPhonetic": "{m} {s}.{d}",
            },
            "USER": {},
            "HARDWARE": {"I2C_BUS": 1},
            "LED": {
                "LED_COUNT": 0, "LED_GPIO": 10, "LED_FREQ_HZ": 800000,
                "LED_DMA": 10, "LED_INVERT": false, "LED_CHANNEL": 0,
                "LED_STRIP": "GRB", "LED_ROWS": 1, "PANEL_ROTATE": 0,
                "INVERTED_PANEL_ROWS": false,
                "SERIAL_CTRLR_PORT": null, "SERIAL_CTRLR_BAUD": 115200,
                "ledEffects": "", "ledBrightness": 32, "ledColorMode": "",
                "seatColors": ["#0022ff", "#ff5500", "#00ff22", "#ff0055",
                               "#ddff00", "#7700ff", "#00ffdd", "#aaaaaa"],
            },
            "LOGGING": {"CONSOLE_LOG_LEVEL": 30, "FILE_LOG_LEVEL": 10,
                        "EVENTS": 0, "DEBUG_SENSORS": false},
            "SENSORS": {},
            "PLUGINS": {},
        });
        std::fs::write(&config_path, serde_json::to_string_pretty(&default_config).unwrap())
            .expect("failed to write default config.json");
        tracing::info!("Created default configuration file (admin/rotorhazard credentials)");
    }

    let http_port_cfg = read_config_port(&data_dir).unwrap_or(port);
    let (admin_username, admin_password) = read_admin_creds(&data_dir);

    if let Err(e) = pages::init() {
        tracing::warn!("template environment: {}", e);
    }
    crate::phase3::set_languages(crate::assets::load_languages());

    let conn = db::open_or_init(&data_dir).expect("failed to open/init database");

    // serial ports come from the config file (GENERAL.SERIAL_PORTS)
    let serial_ports: Vec<String> = std::fs::read_to_string(&config_path)
        .ok()
        .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
        .and_then(|cfg| {
            cfg.get("GENERAL")
                .and_then(|g| g.get("SERIAL_PORTS"))
                .and_then(|v| v.as_array())
                .map(|arr| arr.iter()
                    .filter_map(|v| v.as_str().map(|s| s.to_string()))
                    .collect::<Vec<_>>())
        })
        .unwrap_or_default();

    // single discovery pass: real hardware replaces the mock system
    let hardware_opt = if serial_ports.is_empty() {
        None
    } else {
        tracing::info!("Discovering serial nodes on: {:?}", serial_ports);
        let mut hw = crate::hardware::HardwareInterface::new(&serial_ports);
        hw.discover(Some(&std::path::Path::new(&data_dir)
            .join(crate::serial::baud_cache_file_name())));
        let found = hw.num_nodes();
        tracing::info!("Found {} serial node(s)", found);
        if found > 0 { Some(hw) } else { None }
    };

    // node count: serial hardware count if found, else RH_NODES (min 8)
    let num_nodes: usize = match &hardware_opt {
        Some(hw) => hw.num_nodes(),
        None => std::env::var("RH_NODES")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(0)
            .max(8),
    };

    // mock mode: give every node a frequency so no seat sits disabled.
    // Missing entries are filled with unused Raceband channels in order.
    // (skipped when real hardware drives the node count)
    if hardware_opt.is_none() && db::autofill_mock_frequencies(&conn, num_nodes) {
        tracing::info!("Auto-assigned frequencies to disabled mock nodes");
    }

    tracing::info!(
        "RotorHazard (Rust) starting: data dir {}, port {}, {} nodes",
        data_dir, http_port_cfg, num_nodes
    );
    tracing::info!("assets: templates and static are embedded (debug builds read the source tree)");


    // LED manager: strip config + saved effect assignments (config.json LED)
    let led_config: Value = std::fs::read_to_string(&config_path)
        .ok()
        .and_then(|t| serde_json::from_str::<Value>(&t).ok())
        .unwrap_or(json!({}));
    let led_manager = crate::led::LedManager::new(&led_config);
    led_manager.apply_assignments(
        led_config
            .get("LED")
            .and_then(|l| l.get("ledEffects")),
    );

    let state: SharedState = Arc::new(AppState {
        db: Arc::new(Mutex::new(conn)),
        race: Arc::new(Mutex::new(state::RaceState::new(num_nodes))),
        num_nodes: std::sync::atomic::AtomicUsize::new(num_nodes),
        io: std::sync::OnceLock::new(),
        info: ServerInfo {
            release_version: "4.6.0-dev.1-rust".to_string(),
            server_api: 50,
            node_api_best: 36,
            json_api: 3,
        },
        data_dir: data_dir.clone(),
        admin_username,
        admin_password,
        config_items: Arc::new(Mutex::new(std::collections::HashMap::new())),
        mock: Mutex::new((0..num_nodes).map(|_| state::MockNodeState::new()).collect()),
        mock_provider: Mutex::new((
            String::new(),
            crate::mock::MockProvider::NoSignal,
        )),
        hardware: Mutex::new(None),
        led_manager,
        vrx_manager: crate::vrx::VRxControlManager::new(),
        cluster: std::sync::OnceLock::new(),
        actions_engine: crate::actions::ActionEngine::new(),
        restart_required: std::sync::atomic::AtomicBool::new(false),
        cluster_joined: std::sync::atomic::AtomicBool::new(false),
        server_instance_token: state::now_epoch_ms() as f64 + (state::monotonic() * 1000.0).fract(),
    });

    let (socketio_layer, io) = SocketIo::builder()
        .with_state(state.clone())
        .build_layer();
    state.io.set(io.clone()).expect("io set once");

    // install the discovered hardware interface
    if let Some(hw) = hardware_opt {
        // sync frequencies between the database profile and the nodes:
        // DB wins when set, otherwise adopt the node's own frequency
        let db_freqs = db::with_db(&state, |conn| {
            let pid = db::current_profile_id(conn);
            db::profile_frequencies(conn, pid)
        });
        {
            for (i, node) in hw.nodes.iter().enumerate() {
                let dbf = db_freqs.get(i).map(|(_, _, f)| *f).unwrap_or(0);
                if dbf != 0 && dbf != node.frequency as i64 {
                    hw.set_frequency(i, dbf as u16);
                } else if dbf == 0 && node.frequency != 0 {
                    db::with_db(&state, |conn| {
                        let pid = db::current_profile_id(conn);
                        db::set_node_frequency_at(conn, pid, i, node.frequency as i64)
                    });
                }
            }
        }
        *state.hardware.lock().unwrap() = Some(hw);

        // hardware update loop: poll lap stats / RSSI from real nodes at
        // 50 Hz (RHInterface.update loop)
        let hw_state = state.clone();
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(std::time::Duration::from_millis(20)).await;
                let now = state::monotonic();
                if let Some(hw) = hw_state.hardware.lock().unwrap().as_mut() {
                    hw.update(&hw_state, now);
                }
            }
        });
    }

    // startup LED effect (init_LED_effects wiring)
    state.led_manager.trigger("startup", &json!({}));
    state.vrx_manager.do_startup(&json!({}));

    // serial-port hot-plug scan (check_serial_ports_appeared): every 5s
    // outside a race, detect newly-attached ttyUSB/ttyACM nodes
    {
        let hp_state = state.clone();
        let led_cfg_port = led_config
            .get("LED")
            .and_then(|l| l.get("SERIAL_CTRLR_PORT"))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(std::time::Duration::from_secs(5)).await;
                if let Some(hw) = hp_state.hardware.lock().unwrap().as_mut() {
                    hw.hotplug_scan(&hp_state, &[led_cfg_port.clone()]);
                }
            }
        });
    }

    // cluster: start connections to configured secondary timers
    if let Some(cluster) = crate::cluster::ClusterNodeSet::new(state.clone(), &led_config) {
        let _ = state.cluster.set(cluster);
    }

    // event actions (EventActions.loadActions)
    state.actions_engine.load(&data_dir);

    pages::set_state(state.clone());

    // initialize the mock signal provider from the config file (once)
    {
        let setting = db::get_config_string(&state, "GENERAL", "MOCK_NODE_SIGNAL");
        let mut holder = state.mock_provider.lock().unwrap();
        *holder = (setting.clone(), crate::mock::provider_from_setting(&setting));
        tracing::info!("Mock signal {} active", setting);
    }
    socket::register_handlers(io, state.clone());

    // mock-node update loop at MOCK_UPDATE_INTERVAL (MockInterface.update),
    // and a separate fixed 0.5s heartbeat broadcast (heartbeat_thread_function)
    {
        let mock_state = state.clone();
        tokio::spawn(async move {
            loop {
                // env RH_UPDATE_INTERVAL overrides the config setting
                let interval_secs = std::env::var("RH_UPDATE_INTERVAL")
                    .ok()
                    .and_then(|v| v.parse::<f64>().ok())
                    .unwrap_or_else(|| {
                        crate::db::get_config_number(&mock_state, "GENERAL", "MOCK_UPDATE_INTERVAL", 0.5)
                    });
                let interval = std::time::Duration::from_secs_f64(interval_secs.max(0.05));
                tokio::time::sleep(interval).await;
                socket::mock_update_tick(&mock_state);
            }
        });
    }
    {
        let hb_state = state.clone();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(std::time::Duration::from_millis(500));
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            let mut hb_tick: u64 = 0;
            loop {
                interval.tick().await;
                let payload = db::with_db(&hb_state, |conn| {
                    let profile_id = db::current_profile_id(conn);
                    let freqs = db::profile_frequencies(conn, profile_id);
                    let mock = hb_state.mock.lock().unwrap();
                    json!({
                        "current_rssi": (0..hb_state.node_count())
                            .map(|i| mock.get(i).map(|m| m.current_rssi).unwrap_or(0))
                            .collect::<Vec<_>>(),
                        "frequency": (0..hb_state.node_count())
                            .map(|i| freqs.get(i).map(|(_, _, f)| *f).unwrap_or(0))
                            .collect::<Vec<_>>(),
                        "loop_time": (0..hb_state.node_count())
                            .map(|i| mock.get(i).map(|m| m.loop_time).unwrap_or(1))
                            .collect::<Vec<_>>(),
                        "crossing_flag": (0..hb_state.node_count())
                            .map(|i| mock.get(i).map(|m| m.crossing_flag).unwrap_or(false))
                            .collect::<Vec<_>>(),
                    })
                });
                socket::broadcast(&hb_state, "heartbeat", payload);

                // periodic environmental-data and cluster-status broadcasts
                // (heartbeat_thread_function in server.py)
                hb_tick += 1;
                if hb_tick % 8 == 0 {
                    socket::broadcast(&hb_state, "environmental_data",
                        crate::sensor::environmental_data());
                }
                if hb_tick % 4 == 2 {
                    if let Some(cluster) = hb_state.cluster.get() {
                        if cluster.has_secondaries() {
                            socket::broadcast(&hb_state, "cluster_status",
                                cluster.get_cluster_status_info());
                        }
                    }
                }

                let node_payload = {
                    let mock = hb_state.mock.lock().unwrap();
                    json!({
                        "node_peak_rssi": (0..hb_state.node_count())
                            .map(|i| mock.get(i).map(|m| m.node_peak_rssi).unwrap_or(0))
                            .collect::<Vec<_>>(),
                        "node_nadir_rssi": (0..hb_state.node_count())
                            .map(|i| mock.get(i).map(|m| m.node_nadir_rssi).unwrap_or(0))
                            .collect::<Vec<_>>(),
                        "pass_peak_rssi": (0..hb_state.node_count())
                            .map(|i| mock.get(i).map(|m| m.pass_peak_rssi).unwrap_or(0))
                            .collect::<Vec<_>>(),
                        "pass_nadir_rssi": (0..hb_state.node_count())
                            .map(|i| mock.get(i).map(|m| m.pass_nadir_rssi).unwrap_or(0))
                            .collect::<Vec<_>>(),
                        "debug_pass_count": (0..hb_state.node_count())
                            .map(|i| mock.get(i).map(|m| m.debug_pass_count).unwrap_or(0))
                            .collect::<Vec<_>>(),
                    })
                };
                socket::broadcast(&hb_state, "node_data", node_payload);
            }
        });
    }

    let app = axum::Router::new()
        .route("/api/status", axum::routing::get(api::status))
        .route("/api/pilot/all", axum::routing::get(api::pilot_all))
        .route("/api/pilot/{pilot_id}", axum::routing::get(api::pilot_one))
        .route("/api/heat/all", axum::routing::get(api::heat_all))
        .route("/api/heat/{heat_id}", axum::routing::get(api::heat_one))
        .route("/api/class/all", axum::routing::get(api::class_all))
        .route("/api/class/{class_id}", axum::routing::get(api::class_one))
        .route("/api/format/all", axum::routing::get(api::format_all))
        .route("/api/format/{format_id}", axum::routing::get(api::format_one))
        .route("/api/profile/all", axum::routing::get(api::profile_all))
        .route("/api/profile/{profile_id}", axum::routing::get(api::profile_one))
        .route("/api/race/all", axum::routing::get(api::race_all))
        .route(
            "/api/race/{heat_id}/{round_id}",
            axum::routing::get(api::race_detail),
        )
        .route("/api/options", axum::routing::get(api::options))
        .route("/api", axum::routing::get(api_docs))
        .route("/", axum::routing::get(render_page))
        .route("/event", axum::routing::get(render_page))
        .route("/results", axum::routing::get(render_page))
        .route("/run", axum::routing::get(render_page))
        .route("/current", axum::routing::get(render_page))
        .route("/marshal", axum::routing::get(render_page))
        .route("/format", axum::routing::get(render_page))
        .route("/settings", axum::routing::get(render_page))
        .route("/scanner", axum::routing::get(render_page))
        .route("/streams", axum::routing::get(render_page))
        .route("/stream/results", axum::routing::get(render_page))
        .route("/imdtabler", axum::routing::get(render_page))
        .route("/database", axum::routing::get(render_page))
        .route("/vrxstatus", axum::routing::get(render_page))
        .route("/advanced-settings", axum::routing::get(render_page))
        .route("/updatenodes", axum::routing::get(render_page))
        .route("/plugins", axum::routing::get(render_page))
        .route("/hardwarelog", axum::routing::get(render_page))
        .route("/first-run", axum::routing::get(render_page))
        .route("/stream/node/{node_id}", axum::routing::get(render_page))
        .route("/stream/class/{class_id}", axum::routing::get(render_page))
        .route("/stream/heat/{heat_id}", axum::routing::get(render_page))
        .route("/static/{*path}", axum::routing::get(assets::static_handler))
        .layer(socketio_layer)
        .with_state(state);

    let listener = tokio::net::TcpListener::bind(("0.0.0.0", http_port_cfg))
        .await
        .expect("failed to bind port");
    tracing::info!("Running http server at port {}", http_port_cfg);
    axum::serve(listener, app).await.expect("server error");
}

/// Routes that require admin auth when credentials are configured
/// (mirrors the Python @requires_auth set).
const AUTHED_PAGES: &[&str] = &[
    "/first-run", "/run", "/marshal", "/format", "/settings", "/advanced-settings",
    "/scanner", "/updatenodes", "/plugins", "/hardwarelog", "/database", "/vrxstatus",
];

/// Renders the page for the request path (page map in pages::PAGES).
async fn render_page(
    uri: axum::http::Uri,
    headers: axum::http::HeaderMap,
    State(state): State<SharedState>,
) -> impl axum::response::IntoResponse {
    let path = uri.path().to_string();

    // page auth: when admin credentials are configured, authed pages require
    // HTTP basic auth (401 challenge otherwise), matching requires_auth
    if AUTHED_PAGES.contains(&path.as_str())
        && !state.admin_username.is_empty()
        && !state.admin_password.is_empty()
    {
        let authed = headers
            .get(axum::http::header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.strip_prefix("Basic "))
            .and_then(|b| {
                use base64::Engine;
                base64::engine::general_purpose::STANDARD
                    .decode(b)
                    .ok()
            })
            .and_then(|decoded| String::from_utf8(decoded).ok())
            .map(|creds| {
                let (user, pass) = creds.split_once(':').unwrap_or(("", ""));
                user == state.admin_username && pass == state.admin_password
            })
            .unwrap_or(false);
        if !authed {
            let mut resp = axum::http::Response::new(axum::body::Body::empty());
            *resp.status_mut() = axum::http::StatusCode::UNAUTHORIZED;
            resp.headers_mut().insert(
                axum::http::header::WWW_AUTHENTICATE,
                axum::http::HeaderValue::from_static("Basic realm=\"Login Required\""),
            );
            return resp;
        }
    }

    let template = pages::PAGES
        .iter()
        .find(|(p, _)| *p == path)
        .map(|(_, t)| *t)
        .unwrap_or("home.html");
    pages::render(template, state.node_count(), pages::page_context(&path, &state))
}

async fn api_docs() -> impl axum::response::IntoResponse {
    let body = json!({
        "endpoints": [
            "/api/status", "/api/pilot/all", "/api/pilot/<id>",
            "/api/heat/all", "/api/heat/<id>",
            "/api/class/all", "/api/class/<id>",
            "/api/format/all", "/api/format/<id>",
            "/api/profile/all", "/api/profile/<id>",
            "/api/race/current", "/api/race/all", "/api/race/<heat_id>/<round_id>",
            "/api/options"
        ]
    });
    (
        axum::http::StatusCode::CREATED,
        [(axum::http::header::CONTENT_TYPE, "application/json")],
        axum::Json(body),
    )
}

fn read_admin_creds(data_dir: &str) -> (String, String) {
    let path = format!("{}/config.json", data_dir);
    let default = (String::new(), String::new());
    let Ok(text) = std::fs::read_to_string(&path) else {
        return default;
    };
    let Ok(cfg) = serde_json::from_str::<serde_json::Value>(&text) else {
        return default;
    };
    let secrets = cfg.get("SECRETS");
    let user = secrets
        .and_then(|s| s.get("ADMIN_USERNAME"))
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let pass = secrets
        .and_then(|s| s.get("ADMIN_PASSWORD"))
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    (user, pass)
}

fn read_config_port(data_dir: &str) -> Option<u16> {
    let text = std::fs::read_to_string(format!("{}/config.json", data_dir)).ok()?;
    let cfg = serde_json::from_str::<serde_json::Value>(&text).ok()?;
    cfg.get("GENERAL")?
        .get("HTTP_PORT")?
        .as_u64()
        .map(|p| p as u16)
}

fn resolve_data_dir(exe_dir: &Option<std::path::PathBuf>) -> String {
    // 1: RH_DATA_DIR env var (must exist)
    if let Ok(env_path) = std::env::var("RH_DATA_DIR") {
        let expanded = expanduser(&env_path);
        if std::path::Path::new(&expanded).exists() {
            return expanded;
        }
        panic!(
            "Unable to find data location given by RH_DATA_DIR: {}",
            env_path
        );
    }

    // 2: datapath.ini in the exe dir
    if let Some(exe) = exe_dir {
        let ini = exe.join("datapath.ini");
        if let Ok(line) = std::fs::read_to_string(&ini) {
            let expanded = expanduser(line.trim());
            if !expanded.is_empty() {
                if std::path::Path::new(&expanded).exists() {
                    return expanded;
                }
                panic!(
                    "datapath.ini points to an invalid location: {}",
                    expanded
                );
            }
        }
    }

    // 3: exe dir with a data/ subdirectory (standard RotorHazard install
    //    layout: drop the binary next to an existing data/ dir)
    if let Some(exe) = exe_dir {
        let data = exe.join("data");
        if data.join("config.json").exists() || data.join("database.db").exists() {
            return data.to_string_lossy().to_string();
        }
    }

    // 4: exe dir if config.json exists directly (data stored flat)
    if let Some(exe) = exe_dir {
        if exe.join("config.json").exists() {
            return exe.to_string_lossy().to_string();
        }
    }

    // 5: ~/rh-data if it exists
    let home = std::env::var("HOME").unwrap_or_default();
    let rh_data = format!("{}/rh-data", home);
    if std::path::Path::new(&rh_data).is_dir() {
        return rh_data;
    }

    // 6: cwd if config.json exists there
    if let Ok(cwd) = std::env::current_dir() {
        if cwd.join("config.json").exists() {
            return cwd.to_string_lossy().to_string();
        }
    }

    // 7: ~/rh-data created as needed
    std::fs::create_dir_all(&rh_data)
        .expect("Unable to create ~/rh-data and no alternate path specified");
    rh_data
}

/// Expands a leading '~' to the user home directory.
fn expanduser(path: &str) -> String {
    if path == "~" {
        return std::env::var("HOME").unwrap_or_default();
    }
    if let Some(rest) = path.strip_prefix("~/") {
        return format!(
            "{}/{}",
            std::env::var("HOME").unwrap_or_default(),
            rest
        );
    }
    path.to_string()
}
