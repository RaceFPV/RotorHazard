//! Serial node interface — port of serial_node.py + the RHInterface
//! read/write protocol.  Speaks the RH node wire protocol (command byte,
//! payload, checksum) over a serial port.

use std::io::{Read, Write};
use std::sync::Mutex;
use tracing::{debug, info};

// -- RH node command codes (RHInterface.py) --

pub const READ_FREQUENCY: u8 = 0x03;
pub const READ_LAP_STATS: u8 = 0x05;
pub const READ_RHFEAT_FLAGS: u8 = 0x11;
pub const READ_REVISION_CODE: u8 = 0x22;
pub const READ_ENTER_AT_LEVEL: u8 = 0x31;
pub const READ_EXIT_AT_LEVEL: u8 = 0x32;
pub const READ_TIME_MILLIS: u8 = 0x33;
pub const READ_MULTINODE_COUNT: u8 = 0x39;
pub const READ_CURNODE_INDEX: u8 = 0x3A;
pub const READ_NODE_SLOTIDX: u8 = 0x3C;
pub const READ_FW_VERSION: u8 = 0x3D;
pub const READ_FW_BUILDDATE: u8 = 0x3E;
pub const READ_FW_BUILDTIME: u8 = 0x3F;
pub const READ_FW_PROCTYPE: u8 = 0x40;

pub const WRITE_FREQUENCY: u8 = 0x51;
pub const WRITE_ENTER_AT_LEVEL: u8 = 0x71;
pub const WRITE_EXIT_AT_LEVEL: u8 = 0x72;
pub const WRITE_CURNODE_INDEX: u8 = 0x7A;
pub const SEND_STATUS_MESSAGE: u8 = 0x75;
pub const FORCE_END_CROSSING: u8 = 0x78;
pub const JUMP_TO_BOOTLOADER: u8 = 0x7E;

pub const MAX_RETRY_COUNT: u32 = 4;

pub const SERIAL_BAUD_RATES: [u32; 2] = [921600, 115200];

// -- checksum helpers (RHInterface.py) --

pub fn calculate_checksum(data: &[u8]) -> u8 {
    data.iter().sum::<u8>()
}

pub fn validate_checksum(data: &[u8]) -> bool {
    if data.is_empty() {
        return false;
    }
    calculate_checksum(&data[..data.len() - 1]) == data[data.len() - 1]
}

pub fn unpack_16(data: &[u8]) -> u16 {
    ((data[0] as u16) << 8) | data[1] as u16
}

pub fn pack_16(data: u16) -> Vec<u8> {
    vec![(data >> 8) as u8, (data & 0xFF) as u8]
}

pub fn unpack_32(data: &[u8]) -> u32 {
    ((data[0] as u32) << 24)
        | ((data[1] as u32) << 16)
        | ((data[2] as u32) << 8)
        | data[3] as u32
}

// -- lap stats decoding --

#[derive(Debug, Clone, Default)]
pub struct LapStats {
    pub crossing_flag: bool,
    pub lap_time_ms: u32,
    pub rssi_val: i64,
    pub node_peak_rssi: i64,
    pub node_nadir_rssi: i64,
    pub pass_peak_rssi: i64,
    pub pass_nadir_rssi: i64,
    pub pn_peak_rssi: i64,
    pub pn_nadir_rssi: i64,
    pub pn_peak_first_ms: u16,
    pub pn_peak_last_ms: u16,
    pub pn_nadir_first_ms: u16,
    pub pn_nadir_last_ms: u16,
}

pub fn decode_lap_stats(data: &[u8]) -> Option<LapStats> {
    if data.len() < 20 {
        return None;
    }
    let u16at = |i: usize| -> u16 { ((data[i] as u16) << 8) | data[i + 1] as u16 };
    Some(LapStats {
        crossing_flag: data[0] & 0x01 != 0,
        lap_time_ms: unpack_32(&data[1..5]),
        rssi_val: data[4] as i64,
        node_peak_rssi: data[5] as i64,
        node_nadir_rssi: data[6] as i64,
        pass_peak_rssi: data[7] as i64,
        pass_nadir_rssi: data[8] as i64,
        pn_peak_rssi: data[9] as i64,
        pn_nadir_rssi: data[10] as i64,
        pn_peak_first_ms: u16at(11),
        pn_peak_last_ms: u16at(13),
        pn_nadir_first_ms: u16at(15),
        pn_nadir_last_ms: u16at(17),
    })
}

// -- serial node --

pub struct SerialNode {
    pub port: String,
    pub node_index: usize,
    pub multi_node_index: i32,
    pub api_level: u16,
    pub firmware_version: String,
    pub firmware_proctype: String,
    pub frequency: u16,
    pub enter_at_level: i64,
    pub exit_at_level: i64,
    pub serial: Mutex<Box<dyn serialport::SerialPort>>,
}

impl SerialNode {
    pub fn read_block(&self, command: u8, size: usize) -> Option<Vec<u8>> {
        let mut ser = self.serial.lock().unwrap();
        for _ in 0..=MAX_RETRY_COUNT {
            let _ = ser.clear(serialport::ClearBuffer::Input);
            if ser.write_all(&[command]).is_err() {
                continue;
            }
            let mut buf = vec![0u8; size + 1];
            match ser.read_exact(&mut buf) {
                Ok(_) if validate_checksum(&buf) => return Some(buf[..size].to_vec()),
                _ => {}
            }
        }
        None
    }

    pub fn write_block(&self, command: u8, data: &[u8]) -> bool {
        let mut ser = self.serial.lock().unwrap();
        let mut payload = vec![command];
        payload.extend_from_slice(data);
        payload.push(calculate_checksum(data));
        for _ in 0..=MAX_RETRY_COUNT {
            if ser.write_all(&payload).is_ok() {
                return true;
            }
        }
        false
    }

    pub fn read_frequency(&self) -> Option<u16> {
        self.read_block(READ_FREQUENCY, 2).map(|d| unpack_16(&d))
    }

    pub fn write_frequency(&self, freq: u16) -> bool {
        self.write_block(WRITE_FREQUENCY, &pack_16(freq))
    }

    pub fn read_enter_at_level(&self) -> Option<u16> {
        self.read_block(READ_ENTER_AT_LEVEL, 1).map(|d| d[0] as u16)
    }

    pub fn read_exit_at_level(&self) -> Option<u16> {
        self.read_block(READ_EXIT_AT_LEVEL, 1).map(|d| d[0] as u16)
    }

    pub fn write_enter_at_level(&self, level: u16) -> bool {
        self.write_block(WRITE_ENTER_AT_LEVEL, &[level as u8])
    }

    pub fn write_exit_at_level(&self, level: u16) -> bool {
        self.write_block(WRITE_EXIT_AT_LEVEL, &[level as u8])
    }

    pub fn read_lap_stats(&self) -> Option<LapStats> {
        self.read_block(READ_LAP_STATS, 22)
            .and_then(|d| decode_lap_stats(&d))
    }

    pub fn force_end_crossing(&self) -> bool {
        self.write_block(FORCE_END_CROSSING, &[0])
    }
}

/// Discovers RH serial nodes on the given port paths.
pub fn discover_nodes(port_paths: &[String]) -> Vec<SerialNode> {
    let mut nodes = Vec::new();
    for port_path in port_paths {
        for baud in SERIAL_BAUD_RATES {
            match serialport::new(port_path, baud)
                .timeout(std::time::Duration::from_millis(500))
                .open()
            {
                Ok(sp) => {
                    let mut node = SerialNode {
                        port: port_path.clone(),
                        node_index: nodes.len(),
                        multi_node_index: -1,
                        api_level: 0,
                        firmware_version: String::new(),
                        firmware_proctype: String::new(),
                        frequency: 0,
                        enter_at_level: 0,
                        exit_at_level: 0,
                        serial: Mutex::new(sp),
                    };
                    match node.read_block(READ_REVISION_CODE, 2) {
                        Some(data) => {
                            node.api_level = unpack_16(&data);
                            info!(
                                "Serial node found at {} (baud {}, API {})",
                                port_path, baud, node.api_level
                            );
                            if let Some(d) = node.read_block(READ_FW_VERSION, 16) {
                                node.firmware_version =
                                    String::from_utf8_lossy(&d).trim_end_matches('\0').to_string();
                            }
                            if let Some(d) = node.read_block(READ_FW_PROCTYPE, 16) {
                                node.firmware_proctype =
                                    String::from_utf8_lossy(&d).trim_end_matches('\0').to_string();
                            }
                            if let Some(f) = node.read_frequency() {
                                node.frequency = f;
                            }
                            if let Some(v) = node.read_enter_at_level() {
                                node.enter_at_level = v as i64;
                            }
                            if let Some(v) = node.read_exit_at_level() {
                                node.exit_at_level = v as i64;
                            }
                            nodes.push(node);
                            break;
                        }
                        None => {
                            debug!("No valid response from {} at baud {}", port_path, baud);
                        }
                    }
                }
                Err(e) => {
                    debug!("Cannot open {}: {}", port_path, e);
                }
            }
        }
    }
    nodes
}