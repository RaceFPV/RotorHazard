//! RotorHazard server (Rust implementation).
//!
//! Serves the existing static frontend, the /api JSON endpoints, and the
//! Socket.IO event surface, against the same database.db schema and seed
//! data as the Python server.  Behavior is gated by the protocol
//! characterization suite in src/tests/protocol/.

mod actions;
mod api;
mod autoadjust;
mod heatgen;
mod imdtabler;
mod assets;
mod calibration;
mod class_rank;
mod cluster;
mod cors;
mod hardware;
mod db;
mod export_import;
mod fwupdate;
mod led;
mod mock;
mod vrx;
mod pages;
mod pluginrpc;
mod plugin_support;
mod pluginrepo;
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
        let secret_key = generate_secret_key_str();
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
                "CORS_ALLOWED_HOSTS": "", // empty allows same-origin only
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

    // Opening the database runs the schema init/migrations. Under heavy parallel
    // load (the protocol suite spawns one server per test) SQLite can transiently
    // refuse, and a panic here looks like "server not ready" ten seconds later in
    // whatever test happened to grab that port.
    let conn = {
        let mut attempt = 0u32;
        loop {
            match db::open_or_init(&data_dir) {
                Ok(conn) => break conn,
                Err(err) if attempt < 5 => {
                    attempt += 1;
                    tracing::warn!(
                        "database open failed ({}); retry {}/5",
                        err,
                        attempt
                    );
                    std::thread::sleep(std::time::Duration::from_millis(100 * attempt as u64));
                }
                Err(err) => panic!("failed to open/init database: {}", err),
            }
        }
    };

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
        "RotorHazard starting: data dir {}, port {}, {} nodes",
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
            release_version: state::RELEASE_VERSION.to_string(),
            server_api: 50,
            node_api_best: 36,
            json_api: 3,
        },
        data_dir: data_dir.clone(),
        admin_username,
        admin_password,
        config_items: Arc::new(Mutex::new(std::collections::HashMap::new())),
        mock: Mutex::new((0..num_nodes).map(|_| state::MockNodeState::new()).collect()),
        scan: Mutex::new((0..num_nodes).map(|_| state::ScanState::default()).collect()),
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
        cal_undo: std::sync::Mutex::new(std::collections::HashMap::new()),
        config_load_items: std::sync::Mutex::new(std::collections::HashMap::new()),
        plugin_listeners: std::sync::Mutex::new(Vec::new()),
        plugin_load_status: std::sync::Mutex::new(std::collections::HashMap::new()),
        plugin_pending: std::sync::Mutex::new(std::collections::HashMap::new()),
        plugin_seq: std::sync::atomic::AtomicU64::new(1),
        plugin_panels: std::sync::Mutex::new(Vec::new()),
        plugin_markdowns: std::sync::Mutex::new(Vec::new()),
        plugin_quickbuttons: std::sync::Mutex::new(Vec::new()),
        plugin_fields: std::sync::Mutex::new(Vec::new()),
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
                tracing::debug!(
                    "seat sync {}: profile={} node={} (port {})",
                    i + 1, dbf, node.frequency, node.port
                );
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
                    // Advance any enabled scan before reporting, exactly like
                    // the sweep in RHRace.get_heartbeat_json: step by interval,
                    // wrap to min at the top of the band, narrow by zoom, and
                    // reset the step to max when it gets finer than min.
                    let mut scan = hb_state.scan.lock().unwrap();
                    for i in 0..hb_state.node_count() {
                        if let Some(s) = scan.get_mut(i) {
                            if s.enabled {
                                let mut next = s.current_freq + s.interval;
                                if next < s.min_freq || next > s.max_freq {
                                    next = s.min_freq;
                                    s.interval /= s.zoom;
                                    if s.interval < s.min_interval {
                                        s.interval = s.max_interval;
                                    }
                                }
                                s.current_freq = next;
                            }
                        }
                    }
                    let frequencies: Vec<i64> = (0..hb_state.node_count())
                        .map(|i| match scan.get(i) {
                            Some(s) if s.enabled => s.current_freq.round() as i64,
                            _ => freqs.get(i).map(|(_, _, f)| *f).unwrap_or(0),
                        })
                        .collect();
                    let mock = hb_state.mock.lock().unwrap();
                    json!({
                        "current_rssi": (0..hb_state.node_count())
                            .map(|i| mock.get(i).map(|m| m.current_rssi).unwrap_or(0))
                            .collect::<Vec<_>>(),
                        "frequency": frequencies,
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

    // CORS is decided once, like the reference server: server.py builds its
    // SocketIO service with this value at import time, so a change needs a
    // restart. Empty means same-origin only.
    let cors_policy = cors::CorsPolicy::parse(&db::get_config_string(
        &state,
        "GENERAL",
        "CORS_ALLOWED_HOSTS",
    ));
    println!(
        "CORS allowed hosts: {}",
        match &cors_policy {
            cors::CorsPolicy::SameOrigin => "same-origin only".to_string(),
            cors::CorsPolicy::Any => "* (any site can read responses)".to_string(),
            cors::CorsPolicy::Origins(list) => list.join(", "),
        }
    );
    let hardening_state = HardeningState {
        cors: std::sync::Arc::new(cors_policy),
    };

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
        .layer(axum::middleware::from_fn_with_state(
            hardening_state,
            hardening_middleware,
        ))
        .with_state(state);

    let listener = tokio::net::TcpListener::bind(("0.0.0.0", http_port_cfg))
        .await
        .expect("failed to bind port");

    // Plugins run in a Python child process whose source is embedded in this
    // binary. Start it after the socket is bound so its first connect succeeds.
    plugin_support::autostart(&data_dir, http_port_cfg);
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

/// Generates a 50-character secret key of ASCII letters
/// (RHUtils.generateSecretKeyStr): OS entropy when available, with a
/// time-derived fallback (mirroring the Python try/except path).
fn generate_secret_key_str() -> String {
    const NUM_CHARS: usize = 50;
    const LETTERS: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";
    let mut out = String::with_capacity(NUM_CHARS);
    let entropy: Option<Vec<u8>> = std::fs::File::open("/dev/urandom").ok().and_then(|mut f| {
        use std::io::Read;
        let mut buf = vec![0u8; NUM_CHARS];
        f.read_exact(&mut buf).ok().map(|_| buf)
    });
    match entropy {
        Some(bytes) => {
            for b in bytes {
                out.push(LETTERS[(b as usize) % LETTERS.len()] as char);
            }
        }
        None => {
            for i in 0..NUM_CHARS {
                let nanos = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_nanos() as u64)
                    .unwrap_or(0);
                let idx = (nanos.wrapping_mul(2654435761).wrapping_add(i as u64)) as usize
                    % LETTERS.len();
                out.push(LETTERS[idx] as char);
            }
        }
    }
    out
}

/// Adds hardening response headers to every response (server.py
/// add_security_headers) and applies `GENERAL.CORS_ALLOWED_HOSTS`.
///
/// The Socket.IO endpoint is guarded the way engineio guards it: a request
/// carrying a foreign `Origin` is refused with `400 Not an accepted origin.`,
/// not merely served without a CORS header - browsers never apply CORS to a
/// WebSocket upgrade, so an unchecked upgrade would let any web page drive a
/// race. Other paths answer with CORS headers when the origin is allowed and
/// omit them otherwise, which is what the reference server does for its plain
/// HTTP routes.
#[derive(Clone)]
struct HardeningState {
    cors: std::sync::Arc<cors::CorsPolicy>,
}

async fn hardening_middleware(
    axum::extract::State(st): axum::extract::State<HardeningState>,
    request: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    use axum::http::header;

    let path = request.uri().path().to_string();
    let is_socket_io = path.starts_with("/socket.io");
    let is_api = path.starts_with("/api");
    // Read everything up front into owned strings: a closure borrowing the
    // request would still be alive when the request is handed to `next`, and
    // the resulting future would not satisfy the layer's `Service` bound.
    let (origin, host, forwarded_proto, forwarded_host, requested_headers) = {
        let headers = request.headers();
        let get = |name: &str| {
            headers
                .get(name)
                .and_then(|v| v.to_str().ok())
                .map(str::to_string)
        };
        (
            get("origin"),
            get("host"),
            get("x-forwarded-proto"),
            get("x-forwarded-host"),
            get("access-control-request-headers"),
        )
    };
    let scheme = request
        .uri()
        .scheme_str()
        .unwrap_or("http")
        .to_string();

    let verdict = st.cors.check(
        origin.as_deref(),
        &scheme,
        host.as_deref(),
        forwarded_proto.as_deref(),
        forwarded_host.as_deref(),
    );
    let preflight =
        request.method() == axum::http::Method::OPTIONS && (is_socket_io || is_api);

    let mut response = if verdict.is_denied() && is_socket_io {
        let mut resp =
            axum::response::Response::new(axum::body::Body::from("Not an accepted origin."));
        *resp.status_mut() = axum::http::StatusCode::BAD_REQUEST;
        resp
    } else if preflight {
        // A preflight has no route of its own; answering it here keeps the
        // browser from seeing the 405 the router would otherwise produce.
        let mut resp = axum::response::Response::new(axum::body::Body::empty());
        *resp.status_mut() = axum::http::StatusCode::NO_CONTENT;
        resp
    } else {
        next.run(request).await
    };

    let headers = response.headers_mut();
    headers
        .entry(header::HeaderName::from_static("x-content-type-options"))
        .or_insert(header::HeaderValue::from_static("nosniff"));
    headers
        .entry(header::HeaderName::from_static("referrer-policy"))
        .or_insert(header::HeaderValue::from_static("same-origin"));

    if matches!(verdict, cors::Verdict::NoOrigin) {
        return response;
    }
    // Anything that varies with Origin must say so, or a cache can hand a
    // foreign page a header meant for the timer.
    headers.append(header::VARY, header::HeaderValue::from_static("Origin"));

    if let cors::Verdict::Allowed(value) = &verdict {
        if let Ok(value) = header::HeaderValue::from_str(value) {
            headers.insert(header::ACCESS_CONTROL_ALLOW_ORIGIN, value);
            if is_socket_io {
                // engineio's cors_credentials default. Browsers reject a
                // literal "*" together with credentials, which is why the
                // policy echoes the caller's origin instead.
                headers.insert(
                    header::ACCESS_CONTROL_ALLOW_CREDENTIALS,
                    header::HeaderValue::from_static("true"),
                );
            }
        }
        if preflight {
            headers.insert(
                header::ACCESS_CONTROL_ALLOW_METHODS,
                header::HeaderValue::from_static("OPTIONS, GET, POST"),
            );
            let allow_headers = requested_headers
                .as_deref()
                .and_then(|h| header::HeaderValue::from_str(h).ok())
                .unwrap_or_else(|| header::HeaderValue::from_static("content-type"));
            headers.insert(header::ACCESS_CONTROL_ALLOW_HEADERS, allow_headers);
        }
    }
    response
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

    // 3: exe dir with a data/ subdirectory (standard install
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
    let home = home_dir();
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
/// The user's home directory, on either platform.
fn home_dir() -> String {
    std::env::var("USERPROFILE")
        .or_else(|_| std::env::var("HOME"))
        .unwrap_or_default()
}

fn expanduser(path: &str) -> String {
    if path == "~" {
        return home_dir();
    }
    if let Some(rest) = path.strip_prefix("~/") {
        return format!(
            "{}/{}",
            home_dir(),
            rest
        );
    }
    path.to_string()
}
