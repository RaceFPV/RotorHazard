//! VRx Control framework — port of VRxControl.py.
//!
//! The actual controllers (HDZero, rapidFIRE, ...) are provided by
//! plugins; with the embedded Python plugin host not yet active no
//! controllers register, so the manager is disabled exactly as the
//! Python server is without its VRx plugins.  The event dispatch
//! surface is wired so controllers light up when registered.

use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

/// Consider devices lost if no response within this many seconds.
#[allow(dead_code)]
pub const DEVICE_TIMEOUT: f64 = 30.0;

#[allow(dead_code)]
pub struct VRxDeviceMethod;
#[allow(dead_code)]
impl VRxDeviceMethod {
    pub const ALL: i64 = 0; // receive all events
    pub const PILOT: i64 = 1; // receive events for the assigned pilot
    pub const SEAT: i64 = 2; // receive events for the assigned seat number
}

/// Per-device seat/pilot mapping (VRxDeviceMap).
#[allow(dead_code)]
#[derive(Clone, Debug, Default)]
pub struct VRxDeviceMap {
    pub method: i64,
    pub pilot_id: Option<i64>,
    pub seat: Option<i64>,
}

/// One VRx device (goggle/receiver) known to a controller.
#[allow(dead_code)]
#[derive(Clone, Debug, Default)]
pub struct VRxDevice {
    pub id: String,
    pub ready: bool,
    pub connected: bool,
    pub last_request: Option<f64>,
    pub last_response: Option<f64>,
    pub video_lock: bool,
    pub kind: Option<String>,
    pub name: Option<String>,
    pub address: Option<String>,
    pub map: VRxDeviceMap,
    pub extended_properties: Value,
}

#[allow(dead_code)]
impl VRxDevice {
    /// Device status dict (VRxDeviceStatus).
    pub fn get_status(&self, now: f64) -> Value {
        let mut response_time = Value::Null;
        let mut connected = self.connected;
        if let (Some(resp), Some(req)) = (self.last_response, self.last_request) {
            response_time = json!(resp - req);
            if req > resp && now > req + DEVICE_TIMEOUT {
                connected = false;
            }
        }
        let ready = if connected { self.ready } else { false };
        json!({
            "id": self.id,
            "ready": ready,
            "connected": connected,
            "response_time": response_time,
            "video_lock": self.video_lock,
            "type": self.kind,
            "name": self.name,
            "address": self.address,
            "map": {
                "method": self.map.method,
                "pilot_id": self.map.pilot_id,
                "seat": self.map.seat,
            },
            "extended_properties": self.extended_properties,
            "last_request": self.last_request,
            "last_response": self.last_response,
        })
    }
}

/// Controller base (VRxController): a named collection of devices with
/// overridable event hooks.
#[allow(dead_code)] // hooks are completed by plugin controllers
pub trait VRxController: Send + Sync {
    fn name(&self) -> &str;
    fn label(&self) -> &str;
    fn ready(&self) -> bool;

    fn devices(&self) -> Vec<VRxDevice> {
        Vec::new()
    }

    fn update_status(&self) {}
    fn device_status(&self, _device: &VRxDevice) -> Value {
        json!({})
    }
    fn get_all_device_status(&self) -> Vec<Value> {
        self.devices().iter().map(|d| self.device_status(d)).collect()
    }

    fn set_device_method(&self, _device_id: &str, _method: i64) -> bool {
        false
    }
    fn set_device_seat(&self, _device_id: &str, _seat: i64) -> bool {
        false
    }
    fn set_device_pilot(&self, _device_id: &str, _pilot_id: i64) -> bool {
        false
    }

    // event hooks (all default no-ops, as in Python)
    fn on_startup(&self, _args: &Value) {}
    fn on_heat_set(&self, _args: &Value) {}
    fn on_race_stage(&self, _args: &Value) {}
    fn on_race_start(&self, _args: &Value) {}
    fn on_race_finish(&self, _args: &Value) {}
    fn on_race_stop(&self, _args: &Value) {}
    fn on_race_lap_recorded(&self, _args: &Value) {}
    fn on_race_pilot_done(&self, _args: &Value) {}
    fn on_laps_clear(&self, _args: &Value) {}
    fn on_lap_delete(&self, args: &Value) {
        self.on_race_lap_recorded(args)
    }
    fn on_frequency_set(&self, _args: &Value) {}
    fn on_send_message(&self, _args: &Value) {}
    fn on_option_set(&self, _args: &Value) {}
    fn on_shutdown(&self, _args: &Value) {}
}

/// VRxControlManager: dispatches server events to registered controllers.
pub struct VRxControlManager {
    controllers: Mutex<HashMap<String, Arc<dyn VRxController>>>,
    enabled: Mutex<bool>,
}

#[allow(dead_code)] // full surface activates when controllers register
impl VRxControlManager {
    pub fn new() -> Self {
        // Evt.VRX_INITIALIZE would register plugin controllers here; the
        // plugin host is not yet active, so none register (manager
        // disabled, matching the Python server without VRx plugins)
        let mgr = VRxControlManager {
            controllers: Mutex::new(HashMap::new()),
            enabled: Mutex::new(false),
        };
        if mgr.controllers.lock().unwrap().is_empty() {
            tracing::debug!("VRx Control disabled: no registered controllers");
        } else {
            tracing::info!("VRx Control enabled");
            *mgr.enabled.lock().unwrap() = true;
        }
        mgr
    }

    /// Registers a controller at runtime (the plugin-host entry point).
    pub fn register_controller(&self, controller: Arc<dyn VRxController>) {
        let name = controller.name().to_string();
        if self.controllers.lock().unwrap().insert(name.clone(), controller).is_some() {
            tracing::warn!("Overwriting VRx Controller \"{}\"", name);
        }
        *self.enabled.lock().unwrap() = true;
        tracing::info!("Importing VRx Controller {}", name);
    }

    pub fn is_enabled(&self) -> bool {
        if !*self.enabled.lock().unwrap() {
            return false;
        }
        self.controllers
            .lock()
            .unwrap()
            .values()
            .any(|c| c.ready())
    }

    /// Disconnects all controllers and stops dispatching (kill()).
    pub fn kill(&self) {
        if *self.enabled.lock().unwrap() {
            tracing::info!("Killing active VRx Control");
            *self.enabled.lock().unwrap() = false;
            self.controllers.lock().unwrap().clear();
        } else {
            tracing::debug!("VRx Control already disabled");
        }
    }

    pub fn update_status(&self) {
        for c in self.controllers.lock().unwrap().values() {
            c.update_status();
        }
    }

    pub fn get_controller_status(&self) -> Value {
        let mut status = serde_json::Map::new();
        for c in self.controllers.lock().unwrap().values() {
            status.insert(
                c.name().to_string(),
                json!({"ready": c.ready(), "devices": c.devices().len()}),
            );
        }
        Value::Object(status)
    }

    pub fn get_all_device_status(&self) -> Value {
        let now = crate::state::monotonic();
        let mut devices = serde_json::Map::new();
        for c in self.controllers.lock().unwrap().values() {
            for (device, status) in c
                .devices()
                .into_iter()
                .zip(c.get_all_device_status().into_iter())
            {
                devices.insert(format!("{}:{}", c.name(), device.id), status);
            }
        }
        let _ = now;
        Value::Object(devices)
    }

    /// Devices mapped to a seat/pilot for the given seat index
    /// (getActiveDevices).
    pub fn get_active_devices(&self, seat: i64, pilot_id: Option<i64>) -> Vec<(String, VRxDevice)> {
        let mut out = Vec::new();
        for c in self.controllers.lock().unwrap().values() {
            for device in c.devices() {
                let active = device.map.method == VRxDeviceMethod::ALL
                    || (device.map.method == VRxDeviceMethod::PILOT
                        && device.map.pilot_id == pilot_id)
                    || (device.map.method == VRxDeviceMethod::SEAT
                        && device.map.seat == Some(seat));
                if active {
                    out.push((format!("{}:{}", c.name(), device.id), device));
                }
            }
        }
        out
    }

    pub fn set_device_method(&self, device_id: &str, method: i64) -> bool {
        for c in self.controllers.lock().unwrap().values() {
            if device_id.starts_with(&format!("{}:", c.name()))
                && c.set_device_method(device_id, method)
            {
                return true;
            }
        }
        false
    }

    pub fn set_device_seat(&self, device_id: &str, seat: i64) -> bool {
        for c in self.controllers.lock().unwrap().values() {
            if device_id.starts_with(&format!("{}:", c.name()))
                && c.set_device_seat(device_id, seat)
            {
                return true;
            }
        }
        false
    }

    pub fn set_device_pilot(&self, device_id: &str, pilot_id: i64) -> bool {
        for c in self.controllers.lock().unwrap().values() {
            if device_id.starts_with(&format!("{}:", c.name()))
                && c.set_device_pilot(device_id, pilot_id)
            {
                return true;
            }
        }
        false
    }

    fn dispatch(&self, f: impl Fn(&Arc<dyn VRxController>, &Value)) {
        if !self.is_enabled() {
            return;
        }
        let controllers: Vec<Arc<dyn VRxController>> =
            self.controllers.lock().unwrap().values().cloned().collect();
        for c in controllers {
            f(&c, &json!({}));
        }
    }

    pub fn do_startup(&self, args: &Value) {
        self.dispatch(|c, a| c.on_startup(a));
        let _ = args;
    }
    pub fn do_heat_set(&self, args: &Value) {
        self.dispatch(|c, a| c.on_heat_set(a));
        let _ = args;
    }
    pub fn do_race_stage(&self, args: &Value) {
        self.dispatch(|c, a| c.on_race_stage(a));
        let _ = args;
    }
    pub fn do_race_start(&self, args: &Value) {
        self.dispatch(|c, a| c.on_race_start(a));
        let _ = args;
    }
    pub fn do_race_finish(&self, args: &Value) {
        self.dispatch(|c, a| c.on_race_finish(a));
        let _ = args;
    }
    pub fn do_race_stop(&self, args: &Value) {
        self.dispatch(|c, a| c.on_race_stop(a));
        let _ = args;
    }
    pub fn do_race_lap_recorded(&self, args: &Value) {
        self.dispatch(|c, a| c.on_race_lap_recorded(a));
        let _ = args;
    }
    pub fn do_race_pilot_done(&self, args: &Value) {
        self.dispatch(|c, a| c.on_race_pilot_done(a));
        let _ = args;
    }
    pub fn do_laps_clear(&self, args: &Value) {
        self.dispatch(|c, a| c.on_laps_clear(a));
        let _ = args;
    }
    pub fn do_lap_delete(&self, args: &Value) {
        self.dispatch(|c, a| c.on_lap_delete(a));
        let _ = args;
    }
    pub fn do_frequency_set(&self, args: &Value) {
        self.dispatch(|c, a| c.on_frequency_set(a));
        let _ = args;
    }
    pub fn do_send_priority_message(&self, args: &Value) {
        self.dispatch(|c, a| c.on_send_message(a));
        let _ = args;
    }
    pub fn do_option_set(&self, args: &Value) {
        self.dispatch(|c, a| c.on_option_set(a));
        let _ = args;
    }
    pub fn do_shutdown(&self, args: &Value) {
        self.dispatch(|c, a| c.on_shutdown(a));
        let _ = args;
    }
}
