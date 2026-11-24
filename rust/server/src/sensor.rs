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
/// when thermal_zone0 exists; battery via sysfs power_supply, or an
/// INA219 on the I²C bus when that driver is enabled.
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

    // I²C INA219 battery monitor (stub — see ina219_stub below).
    // When enabled it takes precedence over the sysfs battery path.
    if let Some(values) = ina219_stub::discover_readings() {
        sensors.push(SensorReading { name: "Battery", values });
        tracing::info!("Battery status available (INA219)");
        return sensors;
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

/// INA219 battery-monitor stub (port of ina219_sensor.py, pending
/// hardware validation).
///
/// The INA219 sits on the I²C bus (default addr 0x41 per the Python
/// plugin's supported addresses) and reports shunt voltage, bus voltage,
/// current and power through its registers (0x01 shunt, 0x02 bus,
/// 0x03 power, 0x05 calibration).  A real driver needs:
///   1. an I²C transport (/dev/i2c-N handle),
///   2. the calibration-constant write (r_shunt + max current),
///   3. read/decode of the four registers,
///   4. live hardware to verify the readings against.
/// Until then this stub stays dormant: it never reports readings, so
/// discovery falls through to the sysfs battery path (or nothing).
/// The reading tuple shape already matches SensorReading, so activating
/// a real driver later only means returning Some(vec![...]) here.
mod ina219_stub {
    // (no imports needed while dormant; the driver will use super::SensorReading)

    /// Supported INA219 addresses (ina219_sensor.py).
    #[allow(dead_code)]
    pub const SUPPORTED_ADDRS: [u8; 4] = [0x40, 0x41, 0x44, 0x45];

    /// I²C bus index used by the Python server's HARDWARE.I2C_BUS config.
    #[allow(dead_code)]
    pub const DEFAULT_I2C_BUS: u8 = 1;

    /// Register addresses (INA219 datasheet), documented for the driver.
    #[allow(dead_code)]
    pub const REG_CONFIG: u8 = 0x00;
    #[allow(dead_code)]
    pub const REG_SHUNT_VOLTAGE: u8 = 0x01;
    #[allow(dead_code)]
    pub const REG_BUS_VOLTAGE: u8 = 0x02;
    #[allow(dead_code)]
    pub const REG_POWER: u8 = 0x03;
    #[allow(dead_code)]
    pub const REG_CURRENT: u8 = 0x04;
    #[allow(dead_code)]
    pub const REG_CALIBRATION: u8 = 0x05;

    /// Stub discovery: always dormant (returns None) until a real I²C
    /// driver replaces this body.  The tuple shape matches SensorReading
    /// values, so activating a driver later only means returning Some.
    pub fn discover_readings() -> Option<Vec<(&'static str, f64, &'static str)>> {
        // TODO(ina219): probe SUPPORTED_ADDRS on /dev/i2c-{DEFAULT_I2C_BUS},
        // write calibration, read REG_BUS_VOLTAGE/REG_CURRENT, decode to
        // (voltage V, current A, capacity Ah) readings and return Some.
        None
    }
}

#[allow(unused_imports)]
pub(crate) use ina219_stub::SUPPORTED_ADDRS as INA219_ADDRS;

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
