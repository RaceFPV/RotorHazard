//! Linux sensors — port of interface/linux_sensor.py: core temperature
//! from thermal_zone0 and battery readings from power_supply, surfaced
//! through the environmental_data socket event.

use serde_json::{json, Value};

/// One discovered sensor's readings snapshot.
pub(crate) struct SensorReading {
    name: &'static str,
    values: Vec<(&'static str, f64, &'static str)>,
}

/// Discovery cache: sensor presence doesn't change at runtime, so
/// discover once and reuse (also keeps the log quiet).
fn cached_discover() -> &'static Vec<SensorReading> {
    static CACHE: std::sync::OnceLock<Vec<SensorReading>> = std::sync::OnceLock::new();
    CACHE.get_or_init(discover)
}

/// Discovers available sensors (linux_sensor.discover): core temperature
/// when thermal_zone0 exists, battery when power_supply/battery is present.
#[allow(dead_code)] // discover() is used by tests and future sensor UI
pub(crate) fn discover() -> Vec<SensorReading> {
    let mut sensors = Vec::new();

    if let Some(temp) = read_core_temp() {
        sensors.push(SensorReading {
            name: "Core",
            values: vec![("temperature", temp, "°C")],
        });
        tracing::info!("Core temperature available");
    }

    let battery_base = "/sys/class/power_supply/battery";
    if let Ok(present) = std::fs::read_to_string(format!("{}/present", battery_base)) {
        if present.trim().parse::<i32>().unwrap_or(0) == 1 {
            let mut values = Vec::new();
            if let Some(t) = read_f64(&format!("{}/temp", battery_base), 10.0) {
                values.push(("temperature", t, "°C"));
            }
            if let Some(v) = read_f64(&format!("{}/current_now", battery_base), 1000.0) {
                values.push(("current", v, "A"));
            }
            if let Some(v) = read_f64(&format!("{}/voltage_now", battery_base), 1_000_000.0) {
                values.push(("voltage", v, "V"));
            }
            if let Some(v) = read_f64(&format!("{}/capacity", battery_base), 1.0) {
                values.push(("capacity", v, "Ah"));
            }
            sensors.push(SensorReading { name: "Battery", values });
            tracing::info!("Battery status available");
        }
    }
    sensors
}

fn read_core_temp() -> Option<f64> {
    read_f64("/sys/class/thermal/thermal_zone0/temp", 1000.0)
}

fn read_f64(path: &str, divisor: f64) -> Option<f64> {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|s| s.trim().parse::<f64>().ok())
        .map(|v| v / divisor)
}

/// Builds the environmental_data payload (emit_environmental_data):
/// [{name: {reading: {value, units}}}, ...]
pub fn environmental_data() -> Value {
    let sensors = cached_discover();
    let payload: Vec<Value> = sensors
        .iter()
        .map(|s| {
            let mut readings = serde_json::Map::new();
            for (reading, value, units) in &s.values {
                readings.insert(
                    reading.to_string(),
                    json!({"value": value, "units": units}),
                );
            }
            json!({ s.name: readings })
        })
        .collect();
    json!(payload)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn environmental_data_shape() {
        // the payload is an array of {name: {reading: {value, units}}}
        let payload = environmental_data();
        let arr = payload.as_array().unwrap();
        for entry in arr {
            let obj = entry.as_object().unwrap();
            for (name, readings) in obj {
                let readings = readings.as_object().unwrap();
                for (reading, val) in readings {
                    assert!(val.get("value").is_some(), "{}.{}", name, reading);
                    assert!(val.get("units").is_some());
                }
            }
        }
    }
}
