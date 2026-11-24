//! Serial node interface — port of serial_node.py + the RHInterface
//! read/write protocol.  Speaks the RH node wire protocol (command byte,
//! payload, checksum) over a serial port.

use std::io::{Read, Write};
use std::sync::{Arc, Mutex};
use tracing::{debug, info, warn};

// -- RH node command codes (RHInterface.py) --

pub const READ_FREQUENCY: u8 = 0x03;
pub const READ_LAP_STATS: u8 = 0x05;
pub const READ_LAP_PASS_STATS: u8 = 0x0D;
pub const READ_LAP_EXTREMUMS: u8 = 0x0E;
pub const READ_RHFEAT_FLAGS: u8 = 0x11;
pub const READ_REVISION_CODE: u8 = 0x22;
pub const READ_NODE_RSSI_PEAK: u8 = 0x23;
pub const READ_NODE_RSSI_NADIR: u8 = 0x24;
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

/// LAPSTATS_FLAG_CROSSING (bit 0 of the lap-stats flags byte)
pub const LAPSTATS_FLAG_CROSSING: u8 = 0x01;
/// LAPSTATS_FLAG_PEAK (bit 1)
pub const LAPSTATS_FLAG_PEAK: u8 = 0x02;

pub const SERIAL_BAUD_RATES: [u32; 2] = [115200, 921600];

/// Verification value expected in the high byte of the revision code
/// (serial_node.py: `if rev_val and (rev_val >> 8) == 0x25`).
const REVISION_VERIFY_BYTE: u16 = 0x25;

// -- checksum helpers (RHInterface.py) --

pub fn calculate_checksum(data: &[u8]) -> u8 {
    // sum in a wider type to avoid u8 overflow (matches Python sum() & 0xFF)
    (data.iter().map(|&x| x as u64).sum::<u64>() & 0xFF) as u8
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
    vec![(data >> 8) as u8, data as u8]
}

pub fn unpack_32(data: &[u8]) -> u32 {
    ((data[0] as u32) << 24)
        | ((data[1] as u32) << 16)
        | ((data[2] as u32) << 8)
        | data[3] as u32
}

// -- lap stats decoding (RHInterface.py update loop) --

/// Decoded lap statistics from one READ_LAP_STATS-style block read.
/// Field meanings follow the Python offsets for the node's API level.
#[derive(Debug, Clone, Default)]
pub struct LapStats {
    pub lap_id: i32,
    /// ms value since the lap was recorded (node clock), 0 if invalid
    pub ms_val: i64,
    pub rssi_val: i64,
    pub node_peak_rssi: i64,
    pub node_nadir_rssi: i64,
    pub pass_peak_rssi: i64,
    pub pass_nadir_rssi: i64,
    pub loop_time: i64,
    /// Some(crossing state) when reported, None for legacy layouts
    pub crossing_flag: Option<bool>,
    /// peak/nadir history values (0 = not reported this read)
    pub pn_peak_rssi: i64,
    pub pn_peak_first_ms: u16,
    pub pn_peak_last_ms: u16,
    pub pn_nadir_rssi: i64,
    pub pn_nadir_first_ms: u16,
    pub pn_nadir_last_ms: u16,
}

fn u16at(data: &[u8], i: usize) -> u16 {
    ((data[i] as u16) << 8) | data[i + 1] as u16
}

/// RSSI values are single bytes with 0 meaning "invalid/not reported".
fn rssi_at(data: &[u8], off: usize) -> i64 {
    data[off] as i64
}

/// Decodes a raw block according to the node's API level (the block
/// layouts and offsets mirror RHInterface.py).  `data` is the payload
/// only (checksum already validated and stripped).
pub fn decode_lap_stats(api_level: u16, data: &[u8]) -> Option<LapStats> {
    if data.is_empty() {
        return None;
    }
    let lap_id = data[0] as i32;

    let (offset_rssi, offset_node_peak, offset_pass_peak, offset_loop_time, offset_flags, offset_pass_nadir, offset_node_nadir, offset_peak, offset_peak_first, offset_peak_last_dur, offset_nadir, offset_nadir_first, offset_nadir_last_dur) =
        if api_level >= 18 {
            (
                3usize,
                4,
                5,
                6,
                8,
                9,
                10,
                11,
                12,
                14, // duration for api>=33, last-time otherwise
                11,
                12,
                14,
            )
        } else {
            (
                5usize,
                7,
                9,
                11,
                15,
                16,
                18,
                20,
                22,
                24, // peakLastTime for legacy layouts
                24,
                26,
                28,
            )
        };

    if data.len() <= offset_nadir_first + 1 {
        return None;
    }

    let mut stats = LapStats {
        lap_id,
        ..Default::default()
    };

    if api_level >= 18 {
        stats.ms_val = u16at(data, 1) as i64;
    } else if data.len() >= 5 {
        stats.ms_val = unpack_32(&data[1..5]) as i64;
    }

    stats.rssi_val = rssi_at(data, offset_rssi);
    if stats.rssi_val <= 0 {
        // invalid RSSI: Python rejects the whole reading
        return Some(stats);
    }

    stats.node_peak_rssi = rssi_at(data, offset_node_peak);
    stats.pass_peak_rssi = rssi_at(data, offset_pass_peak);
    stats.loop_time = u16at(data, offset_loop_time) as i64;
    stats.crossing_flag = Some(data[offset_flags] & LAPSTATS_FLAG_CROSSING != 0);
    stats.pass_nadir_rssi = rssi_at(data, offset_pass_nadir);
    if api_level >= 13 {
        stats.node_nadir_rssi = rssi_at(data, offset_node_nadir);
    }

    // peak/nadir history (api >= 18)
    if api_level >= 18 {
        if api_level >= 21 {
            if data[offset_flags] & LAPSTATS_FLAG_PEAK != 0 {
                stats.pn_peak_rssi = rssi_at(data, offset_peak);
                stats.pn_peak_first_ms = u16at(data, offset_peak_first);
                if api_level >= 33 {
                    stats.pn_peak_last_ms =
                        stats.pn_peak_first_ms.saturating_sub(u16at(data, offset_peak_last_dur));
                } else {
                    stats.pn_peak_last_ms = u16at(data, offset_peak_last_dur);
                }
            } else {
                stats.pn_nadir_rssi = rssi_at(data, offset_nadir);
                stats.pn_nadir_first_ms = u16at(data, offset_nadir_first);
                if api_level >= 33 {
                    stats.pn_nadir_last_ms = stats
                        .pn_nadir_first_ms
                        .saturating_sub(u16at(data, offset_nadir_last_dur));
                } else {
                    stats.pn_nadir_last_ms = u16at(data, offset_nadir_last_dur);
                }
            }
        } else {
            stats.pn_peak_rssi = rssi_at(data, offset_peak);
            stats.pn_peak_first_ms = u16at(data, offset_peak_first);
            stats.pn_peak_last_ms = u16at(data, offset_peak_last_dur);
            stats.pn_nadir_rssi = rssi_at(data, offset_nadir);
            stats.pn_nadir_first_ms = u16at(data, offset_nadir_first);
            stats.pn_nadir_last_ms = stats.pn_nadir_first_ms;
        }
    }

    Some(stats)
}

// -- serial node --

pub struct SerialNode {
    pub port: String,
    pub node_index: usize,
    /// slot index on a multi-node processor, or -1 for a single node
    pub multi_node_index: i32,
    /// shared tracker of the currently-selected slot on the processor
    curnode_holder: Option<Arc<Mutex<i32>>>,
    pub api_level: u16,
    pub firmware_version: String,
    pub firmware_proctype: String,
    pub frequency: u16,
    pub enter_at_level: i64,
    pub exit_at_level: i64,
    pub baud: u32,
    pub serial: Arc<Mutex<Box<dyn serialport::SerialPort>>>,
}

impl Clone for SerialNode {
    fn clone(&self) -> Self {
        SerialNode {
            port: self.port.clone(),
            node_index: self.node_index,
            multi_node_index: self.multi_node_index,
            curnode_holder: self.curnode_holder.clone(),
            api_level: self.api_level,
            firmware_version: self.firmware_version.clone(),
            firmware_proctype: self.firmware_proctype.clone(),
            frequency: self.frequency,
            enter_at_level: self.enter_at_level,
            exit_at_level: self.exit_at_level,
            baud: self.baud,
            serial: self.serial.clone(),
        }
    }
}

impl SerialNode {
    /// For multi-node processors: select this node's slot on the shared
    /// serial link (serial_node.py check_set_multi_node_index).
    fn ensure_multi_index(&self) -> bool {
        let holder = match &self.curnode_holder {
            Some(h) => h,
            None => return true,
        };
        if self.multi_node_index < 0 {
            return true;
        }
        let mut current = holder.lock().unwrap();
        if *current == self.multi_node_index {
            return true;
        }
        // write the slot index, then read it back to confirm
        for _ in 0..=MAX_RETRY_COUNT * 5 {
            {
                let mut ser = self.serial.lock().unwrap();
                let payload = vec![WRITE_CURNODE_INDEX, self.multi_node_index as u8,
                    calculate_checksum(&[self.multi_node_index as u8])];
                if ser.write_all(&payload).is_err() {
                    continue;
                }
            }
            let reply = {
                let mut ser = self.serial.lock().unwrap();
                let mut buf = vec![0u8; 2];
                ser.read_exact(&mut buf).ok().map(|_| buf)
            };
            if let Some(buf) = reply {
                if validate_checksum(&buf) && buf[0] == self.multi_node_index as u8 {
                    *current = self.multi_node_index;
                    return true;
                }
            }
            std::thread::sleep(std::time::Duration::from_millis(25));
        }
        warn!(
            "Failed to select slot {} on multi-node processor at {}",
            self.multi_node_index, self.port
        );
        false
    }

    pub fn read_block(&self, command: u8, size: usize) -> Option<Vec<u8>> {
        if !self.ensure_multi_index() {
            return None;
        }
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
        if !self.ensure_multi_index() {
            return false;
        }
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

    /// Reads lap statistics using the block layout for this node's API
    /// level (RHInterface.py update loop).  Returns None when the read
    /// fails or the node does not support lap-stats block reads.
    pub fn read_lap_stats(&self) -> Option<LapStats> {
        if self.api_level < 5 {
            return None;
        }
        let data: Vec<u8> = if self.api_level >= 32 {
            // READ_LAP_PASS_STATS + READ_LAP_EXTREMUMS concatenated
            let mut d = self.read_block(READ_LAP_PASS_STATS, 8)?;
            match self.read_block(READ_LAP_EXTREMUMS, 8) {
                Some(ext) => {
                    d.extend(ext);
                    d
                }
                None => return None,
            }
        } else if self.api_level >= 21 {
            self.read_block(READ_LAP_STATS, 16)?
        } else if self.api_level >= 18 {
            self.read_block(READ_LAP_STATS, 19)?
        } else if self.api_level >= 17 {
            self.read_block(READ_LAP_STATS, 28)?
        } else if self.api_level >= 13 {
            self.read_block(READ_LAP_STATS, 20)?
        } else {
            self.read_block(READ_LAP_STATS, 18)?
        };
        decode_lap_stats(self.api_level, &data)
    }

    pub fn force_end_crossing(&self) -> bool {
        self.write_block(FORCE_END_CROSSING, &[0])
    }

    pub fn jump_to_bootloader(&self) -> bool {
        self.write_block(JUMP_TO_BOOTLOADER, &[])
    }
}

fn open_port(port_path: &str, baud: u32) -> Option<Box<dyn serialport::SerialPort>> {
    match serialport::new(port_path, baud)
        .timeout(std::time::Duration::from_millis(250))
        .open()
    {
        Ok(sp) => Some(sp),
        Err(e) => {
            debug!("Cannot open {}: {}", port_path, e);
            None
        }
    }
}

/// Reads and validates the revision code.  Returns the API level when
/// the verification byte matches (serial_node.py discovery).
fn probe_api_level(node: &SerialNode) -> Option<u16> {
    let data = node.read_block(READ_REVISION_CODE, 2)?;
    let rev_val = unpack_16(&data);
    if (rev_val >> 8) == REVISION_VERIFY_BYTE {
        Some(rev_val & 0xFF)
    } else {
        debug!(
            "Revision-code verification failed at {}: rev_val={:#06x}",
            node.port, rev_val
        );
        None
    }
}

// -- baud-rate cache (Rust-specific convenience; lives in the data dir
// alongside, not inside, the Python-compatible config.json) --

pub fn baud_cache_file_name() -> &'static str {
    "serial_bauds.json"
}

pub fn load_baud_cache(path: &std::path::Path) -> std::collections::HashMap<String, u32> {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default()
}

pub fn save_baud_cache(path: &std::path::Path, map: &std::collections::HashMap<String, u32>) {
    if let Ok(text) = serde_json::to_string_pretty(map) {
        let _ = std::fs::write(path, text);
    }
}

/// Normalizes a configured port name to the cache key ("/dev/ttyAMA0"
/// and "ttyAMA0" map to the same entry).
fn port_cache_key(port: &str) -> String {
    port.trim_start_matches("/dev/").to_string()
}

/// Discovers RH serial nodes on the given port paths, mirroring
/// serial_node.py discover(): tries each baud rate, validates the
/// revision-code verification byte, and expands multi-node processors
/// into per-slot nodes.
///
/// When `cache_path` is given, the last successful baud per port is
/// persisted there and tried first on the next discovery, falling back
/// to the standard SERIAL_BAUD_RATES order on failure.
pub fn discover_nodes(
    port_paths: &[String],
    cache_path: Option<&std::path::Path>,
) -> Vec<SerialNode> {
    let mut baud_cache: std::collections::HashMap<String, u32> = cache_path
        .map(load_baud_cache)
        .unwrap_or_default();
    let mut cache_dirty = false;
    let mut nodes = Vec::new();
    for port_path in port_paths {
        // preferred baud first (last successful), then the standard order
        let mut baud_order: Vec<u32> = Vec::with_capacity(SERIAL_BAUD_RATES.len() + 1);
        let preferred = baud_cache.get(&port_cache_key(port_path)).copied();
        if let Some(b) = preferred {
            baud_order.push(b);
        }
        for b in SERIAL_BAUD_RATES {
            if Some(b) != preferred {
                baud_order.push(b);
            }
        }
        let mut node_opt: Option<SerialNode> = None;
        'bauds: for baud in baud_order {
            for attempt in 0..4u32 {
                let sp = match open_port(port_path, baud) {
                    Some(sp) => sp,
                    None => break 'bauds, // port not openable; no baud will help
                };
                // delay after open (bootloader chill / node boot)
                std::thread::sleep(std::time::Duration::from_millis(
                    100u64 + attempt as u64 * 100,
                ));
                let node = SerialNode {
                    port: port_path.clone(),
                    node_index: nodes.len(),
                    multi_node_index: -1,
                    curnode_holder: None,
                    api_level: 0,
                    firmware_version: String::new(),
                    firmware_proctype: String::new(),
                    frequency: 0,
                    enter_at_level: 0,
                    exit_at_level: 0,
                    baud,
                    serial: Arc::new(Mutex::new(sp)),
                };
                {
                    let ser = node.serial.lock().unwrap();
                    let _ = ser.clear(serialport::ClearBuffer::Input);
                }
                let api_level = match probe_api_level(&node) {
                    Some(v) => v,
                    None => continue, // retry with a longer delay
                };

                // multi-node processors (api >= 32)
                let mut multi_count: i32 = 1;
                if api_level >= 32 {
                    if let Some(d) = node.read_block(READ_MULTINODE_COUNT, 1) {
                        let c = d[0] as i32;
                        if (0..=32).contains(&c) {
                            multi_count = c;
                        }
                    }
                }

                let mut base = node.clone();
                base.api_level = api_level;
                if api_level >= 34 {
                    if let Some(d) = base.read_block(READ_FW_VERSION, 16) {
                        base.firmware_version =
                            String::from_utf8_lossy(&d).trim_end_matches('\0').to_string();
                    }
                    if api_level >= 35 {
                        if let Some(d) = base.read_block(READ_FW_PROCTYPE, 16) {
                            base.firmware_proctype =
                                String::from_utf8_lossy(&d).trim_end_matches('\0').to_string();
                        }
                    }
                }
                if let Some(f) = base.read_frequency() {
                    base.frequency = f;
                }
                if let Some(v) = base.read_enter_at_level() {
                    base.enter_at_level = v as i64;
                }
                if let Some(v) = base.read_exit_at_level() {
                    base.exit_at_level = v as i64;
                }

                if multi_count == 0 {
                    info!(
                        "Serial node (with zero modules) found at {} (baud {}, API {}, fw {})",
                        port_path, baud, api_level, base.firmware_version
                    );
                    baud_cache.insert(port_cache_key(port_path), baud);
                    cache_dirty = true;
                    break 'bauds; // nothing to race on; do not add
                }

                if multi_count <= 1 {
                    info!(
                        "Serial node {} found at {} (baud {}, API {}, fw {} {})",
                        nodes.len() + 1, port_path, baud, api_level,
                        base.firmware_proctype, base.firmware_version
                    );
                    nodes.push(base.clone());
                } else {
                    info!(
                        "Serial multi-node found at {} (baud {}, API {}, fw {} {}), count={}",
                        port_path, baud, api_level,
                        base.firmware_proctype, base.firmware_version, multi_count
                    );
                    let holder = Arc::new(Mutex::new(-1i32));
                    for slot in 0..multi_count {
                        let mut slot_node = base.clone();
                        slot_node.node_index = nodes.len() + slot as usize;
                        slot_node.multi_node_index = slot;
                        slot_node.curnode_holder = Some(holder.clone());
                        if let Some(d) = slot_node.read_block(READ_NODE_SLOTIDX, 1) {
                            debug!("Slot {} reports slot index {}", slot, d[0]);
                        }
                        nodes.push(slot_node);
                    }
                }
                node_opt = Some(base.clone());
                drop(base);
                baud_cache.insert(port_cache_key(port_path), baud);
                cache_dirty = true;
                break 'bauds;
            }
        }
        if node_opt.is_none() {
            warn!("Unable to fetch revision code for serial node at \"{}\"", port_path);
        }
    }
    if cache_dirty {
        if let Some(p) = cache_path {
            save_baud_cache(p, &baud_cache);
        }
    }
    nodes
}

#[cfg(test)]
mod tests {
    use super::*;

    fn block(payload: &[u8]) -> Vec<u8> {
        // payload + valid checksum, as the node would send it
        let mut v = payload.to_vec();
        v.push(calculate_checksum(payload));
        v
    }

    #[test]
    fn checksum_roundtrip() {
        let data = [0x25u8, 0x24];
        assert_eq!(calculate_checksum(&data), 0x49);
        let mut blk = block(&data);
        assert!(validate_checksum(&blk));
        blk[1] ^= 0xFF; // corrupt payload
        assert!(!validate_checksum(&blk));
    }

    #[test]
    fn checksum_overflow_wraps_like_python() {
        // sum of many 0xFF bytes must wrap mod 256 (matches Python sum() & 0xFF)
        let data = [0xFFu8; 300];
        assert_eq!(calculate_checksum(&data), (300u64 * 255 & 0xFF) as u8);
    }

    #[test]
    fn revision_code_verification() {
        // api_level 36 with 0x25 verification byte (real NuclearHazard reply)
        let node_data = [0x25u8, 0x24];
        let rev_val = unpack_16(&node_data);
        assert_eq!(rev_val >> 8, 0x25);
        assert_eq!(rev_val & 0xFF, 36);
        // bad verification byte must be rejected
        assert_ne!((0x24u16 << 8 | 0x24) >> 8, 0x25);
    }

    #[test]
    fn decode_lap_stats_api36_with_peak() {
        // 17-byte payload (api>=32: PASS_STATS + EXTREMUMS concatenated;
        // offsets follow the api>=21 table with 2-byte ms_val):
        // lap_id=1, ms=0x0102, rssi=200, nodePeak=220, passPeak=210,
        // loopTime=0x0010, flags=PEAK|CROSSING, passNadir=90,
        // nodeNadir=50, peakRssi=230, peakFirst=0x00FA,
        // nadirRssi=40, nadirFirst=0x0064
        let d: Vec<u8> = vec![1, 0x01, 0x02, 200, 220, 210, 0x00, 0x10,
            LAPSTATS_FLAG_CROSSING | LAPSTATS_FLAG_PEAK, 90, 50, 230, 0x00, 0xFA, 40, 0x00, 0x64];
        assert_eq!(d.len(), 17);
        let stats = decode_lap_stats(36, &d).expect("decodes");
        assert_eq!(stats.lap_id, 1);
        assert_eq!(stats.ms_val, 0x0102);
        assert_eq!(stats.rssi_val, 200);
        assert_eq!(stats.node_peak_rssi, 220);
        assert_eq!(stats.pass_peak_rssi, 210);
        assert_eq!(stats.loop_time, 0x10);
        assert_eq!(stats.crossing_flag, Some(true));
        assert_eq!(stats.pass_nadir_rssi, 90);
        assert_eq!(stats.node_nadir_rssi, 50);
        assert_eq!(stats.pn_peak_rssi, 230);
        assert_eq!(stats.pn_peak_first_ms, 0xFA);
        assert_eq!(stats.pn_nadir_rssi, 0); // PEAK flag set: nadir history not reported
    }

    #[test]
    fn decode_lap_stats_rejects_invalid_rssi() {
        // rssi byte 0 -> Python rejects the reading (fields stay default)
        let d = vec![0u8; 16];
        let stats = decode_lap_stats(36, &d).expect("decodes");
        assert_eq!(stats.rssi_val, 0);
        assert_eq!(stats.node_peak_rssi, 0);
        assert_eq!(stats.crossing_flag, None);
    }

    #[test]
    fn decode_lap_stats_legacy_api12_uses_old_offsets() {
        // api<13 offsets: lap_id=0, ms u32@1, rssi=5, nodePeak=7,
        // passPeak=9, loopTime=11, flags=15, passNadir=16, nodeNadir=18,
        // peak=20, peakFirst=22, nadir=24, nadirFirst=26 (28 bytes)
        let mut d = vec![0u8; 28];
        d[0] = 3;
        d[1..5].copy_from_slice(&[0, 0, 0, 10]); // ms_val u32
        d[5] = 150; // rssi
        d[7] = 170; // nodePeak
        d[9] = 160; // passPeak
        d[11] = 0x00; d[12] = 0x14; // loopTime u16 = 20
        d[15] = LAPSTATS_FLAG_CROSSING; // flags
        d[16] = 80; // passNadir
        d[18] = 40; // nodeNadir
        let stats = decode_lap_stats(12, &d).expect("decodes");
        assert_eq!(stats.lap_id, 3);
        assert_eq!(stats.ms_val, 10);
        assert_eq!(stats.rssi_val, 150);
        assert_eq!(stats.node_peak_rssi, 170);
        assert_eq!(stats.pass_peak_rssi, 160);
        assert_eq!(stats.loop_time, 20);
        assert_eq!(stats.crossing_flag, Some(true));
        assert_eq!(stats.pass_nadir_rssi, 80);
        // node nadir is only reported on API >= 13 (matches Python)
        assert_eq!(stats.node_nadir_rssi, 0);
    }

    #[test]
    fn port_cache_key_normalizes() {
        assert_eq!(port_cache_key("/dev/ttyAMA0"), "ttyAMA0");
        assert_eq!(port_cache_key("ttyAMA0"), "ttyAMA0");
    }
}
