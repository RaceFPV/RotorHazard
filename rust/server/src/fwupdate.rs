//! Node firmware update — port of util/stm32loader.py (the STM32 native
//! UART bootloader, ST AN3155) plus the server's node-update flow.
//!
//! Flow (do_bpillfw_update in server.py): stop background threads, jump
//! the node to its bootloader (JUMP_TO_BOOTLOADER command), reconnect at
//! 57600 8E1, synchronize, read chip id, erase flash, write the image,
//! verify by read-back, GO, then flag a restart.

use crate::serial::{calculate_checksum, SerialNode};
use std::io::{Read, Write};
use std::sync::{Arc, Mutex};
use tracing::warn;

pub const BOOT_BAUD: u32 = 57600;
pub const FLASH_BASE: u32 = 0x0800_0000;
/// Sanity cap for parsed images (largest common target is 1 MiB; the
/// flasher refuses images beyond this rather than allocating megabytes
/// for stray records like option bytes).
const MAX_IMAGE_LEN: usize = 2 * 1024 * 1024;

const ACK: u8 = 0x79;
const NACK: u8 = 0x1F;
const SYNCHRONIZE: u8 = 0x7F;
const CMD_GET: u8 = 0x00;
const CMD_GET_ID: u8 = 0x02;
const CMD_READ_MEMORY: u8 = 0x11;
const CMD_GO: u8 = 0x21;
const CMD_WRITE_MEMORY: u8 = 0x31;
const CMD_ERASE: u8 = 0x43;
const CMD_EXT_ERASE: u8 = 0x44;

/// Progress callback (drives the upd_messages_* socket events).
#[allow(dead_code)]
pub type ProgressFn = Box<dyn Fn(&str) + Send>;

pub struct Flasher {
    port: Mutex<Box<dyn serialport::SerialPort>>,
}

fn flash_error(msg: &str) -> String {
    format!("stm32loader: {}", msg)
}

impl Flasher {
    /// Opens the port in bootloader mode (57600, 8E1).
    pub fn open(port_path: &str) -> Result<Flasher, String> {
        let sp = serialport::new(port_path, BOOT_BAUD)
            .data_bits(serialport::DataBits::Eight)
            .parity(serialport::Parity::Even)
            .stop_bits(serialport::StopBits::One)
            .timeout(std::time::Duration::from_secs(3))
            .open()
            .map_err(|e| flash_error(&format!("Error opening serial port: {}", e)))?;
        Ok(Flasher { port: Mutex::new(sp) })
    }

    fn read_byte(&self) -> Result<u8, String> {
        let mut buf = [0u8; 1];
        let mut ser = self.port.lock().unwrap();
        ser.read_exact(&mut buf)
            .map_err(|_| flash_error("Timeout waiting for bootloader response"))?;
        Ok(buf[0])
    }

    fn write_all(&self, data: &[u8]) -> Result<(), String> {
        let mut ser = self.port.lock().unwrap();
        ser.write_all(data)
            .map_err(|e| flash_error(&format!("Serial write error: {}", e)))?;
        ser.flush().ok();
        Ok(())
    }

    /// Sends a command byte with its XOR complement, expects ACK.
    fn send_command(&self, cmd: u8) -> Result<(), String> {
        self.write_all(&[cmd, !cmd])?;
        let resp = self.read_byte()?;
        if resp == ACK {
            Ok(())
        } else {
            Err(flash_error(&format!("Command {:#04x} not ACKed (got {:#04x})", cmd, resp)))
        }
    }

    /// Sends a 4-byte big-endian address with XOR-of-bytes checksum.
    fn send_address(&self, addr: u32) -> Result<(), String> {
        let b = addr.to_be_bytes();
        let checksum = b[0] ^ b[1] ^ b[2] ^ b[3];
        self.write_all(&[b[0], b[1], b[2], b[3], checksum])?;
        let resp = self.read_byte()?;
        if resp == ACK {
            Ok(())
        } else {
            Err(flash_error("Address not ACKed"))
        }
    }

    /// Synchronizes with the bootloader (0x7F handshake).
    pub fn synchronize(&self) -> Result<(), String> {
        self.write_all(&[SYNCHRONIZE])?;
        // the bootloader may need a byte to settle; read the reply
        match self.read_byte() {
            Ok(ACK) => Ok(()),
            Ok(NACK) => Ok(()), // NACK after 0x7F still means alive (some bootloaders)
            Ok(other) => Err(flash_error(&format!(
                "Bad synchronize reply {:#04x}", other))),
            Err(e) => Err(e),
        }
    }

    /// Reads the chip id via GET_ID; returns the 16-bit product id
    /// (0x410/0x411/... F1 family, 0x431/0x433 F4 family).
    pub fn get_chip_id(&self) -> Result<u16, String> {
        self.send_command(CMD_GET_ID)?;
        let count = self.read_byte()?; // N-1
        let mut data = vec![0u8; count as usize + 1];
        {
            let mut ser = self.port.lock().unwrap();
            ser.read_exact(&mut data)
                .map_err(|_| flash_error("Timeout reading chip id"))?;
        }
        // data: id (2 bytes, big-endian) + trailing checksum byte
        if data.len() >= 2 {
            Ok(((data[0] as u16) << 8) | data[1] as u16)
        } else {
            Err(flash_error("Short chip-id response"))
        }
    }

    /// Reads the bootloader version and supported-command list via GET.
    pub fn get_version_and_commands(&self) -> Result<(u8, Vec<u8>), String> {
        self.send_command(CMD_GET)?;
        let count = self.read_byte()?;
        let mut data = vec![0u8; count as usize + 1];
        {
            let mut ser = self.port.lock().unwrap();
            ser.read_exact(&mut data)
                .map_err(|_| flash_error("Timeout reading GET response"))?;
        }
        if data.is_empty() {
            return Err(flash_error("Empty GET response"));
        }
        let version = data[0];
        let commands = data[1..data.len() - 1].to_vec();
        Ok((version, commands))
    }

    fn supported(&self, commands: &[u8], cmd: u8) -> bool {
        commands.contains(&cmd)
    }

    /// Global erase (extended erase when supported, legacy otherwise).
    pub fn erase_all(&self) -> Result<(), String> {
        let (version, commands) = self.get_version_and_commands()?;
        let _ = version;
        if self.supported(&commands, CMD_EXT_ERASE) {
            // extended erase: global erase = 0xFFFF, sum = 0xFFFF ^ 0xFFFF... per
            // AN3155: number of pages (2 bytes) + complement; 0xFFFF marks global
            self.send_command(CMD_EXT_ERASE)?;
            // global mass erase: N=0xFFFF, complement 0x0000
            self.write_all(&[0xFF, 0xFF, 0x00, 0x00])?;
            let resp = self.read_byte()?;
            if resp != ACK {
                return Err(flash_error("Extended erase not ACKed"));
            }
            // extended erase may take a while; wait for the completion ACK
            let resp = self.read_byte()?;
            if resp != ACK {
                return Err(flash_error("Extended erase completion not ACKed"));
            }
        } else if self.supported(&commands, CMD_ERASE) {
            self.send_command(CMD_ERASE)?;
            // legacy erase-all: N=0xFF, complement 0x00
            self.write_all(&[0xFF, 0x00])?;
            let resp = self.read_byte()?;
            if resp != ACK {
                return Err(flash_error("Legacy erase not ACKed"));
            }
            let resp = self.read_byte()?;
            if resp != ACK {
                return Err(flash_error("Legacy erase completion not ACKed"));
            }
        } else {
            return Err(flash_error("Bootloader does not support erase commands"));
        }
        Ok(())
    }

    /// Writes one 256-byte-max block at the aligned address.
    #[allow(dead_code)] // single-block helper (write_image loops inline)
    fn write_block(&self, addr: u32, data: &[u8]) -> Result<(), String> {
        self.send_command(CMD_WRITE_MEMORY)?;
        self.send_address(addr)?;
        let n = (data.len() - 1) as u8; // N-1 per AN3155
        let mut frame = vec![n];
        frame.extend_from_slice(data);
        frame.push(calculate_checksum(&frame));
        self.write_all(&frame)?;
        let resp = self.read_byte()?;
        if resp == ACK {
            Ok(())
        } else {
            Err(flash_error(&format!("Write block at {:#x} not ACKed", addr)))
        }
    }

    /// Writes the image to flash, padding to 256-byte blocks.
    pub fn write_image(&self, image: &[u8], progress: &dyn Fn(&str)) -> Result<(), String> {
        let block_size = 256usize;
        let padded_len = ((image.len() + block_size - 1) / block_size) * block_size;
        let mut padded = image.to_vec();
        padded.resize(padded_len, 0xFF);
        for (idx, block) in padded.chunks(block_size).enumerate() {
            // per-block: WRITE MEMORY command + address, then data
            self.send_command(CMD_WRITE_MEMORY)?;
            let addr = FLASH_BASE + (idx * block_size) as u32;
            self.send_address(addr)?;
            let n = (block.len() - 1) as u8;
            let mut frame = vec![n];
            frame.extend_from_slice(block);
            frame.push(calculate_checksum(&frame));
            self.write_all(&frame)?;
            let resp = self.read_byte()?;
            if resp != ACK {
                return Err(flash_error(&format!("Write block {} at {:#x} not ACKed", idx, addr)));
            }
            if idx % 16 == 0 {
                progress(&format!("Wrote {}/{} bytes", (idx + 1) * block_size, padded_len));
            }
        }
        Ok(())
    }

    /// Reads back `len` bytes from `addr` for verification.
    pub fn read_memory(&self, addr: u32, len: usize) -> Result<Vec<u8>, String> {
        self.send_command(CMD_READ_MEMORY)?;
        self.send_address(addr)?;
        let n = (len - 1) as u8;
        self.write_all(&[n, !n])?;
        let mut data = vec![0u8; len];
        {
            let mut ser = self.port.lock().unwrap();
            ser.read_exact(&mut data)
                .map_err(|_| flash_error("Timeout reading memory"))?;
        }
        Ok(data)
    }

    /// Jumps to the application (GO command).
    pub fn go(&self, addr: u32) -> Result<(), String> {
        self.send_command(CMD_GO)?;
        self.send_address(addr)?;
        Ok(())
    }
}

/// Loads a firmware image from a .bin or (Intel) .hex file
/// (load_source_file in stm32loader.py).
pub fn load_source_file(path: &str) -> Result<Vec<u8>, String> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| flash_error(&format!("Error loading file from '{}': {}", path, e)))?;
    let trimmed = text.trim_start();
    if trimmed.starts_with(':') {
        parse_intel_hex(&text)
    } else {
        // raw binary embedded in text response; re-read as bytes
        std::fs::read(path).map_err(|e| flash_error(&format!("Error loading file from '{}': {}", path, e)))
    }
}

/// Minimal Intel HEX parser (records :LLAAAATT[dd..]CC; types 00 data,
/// 01 EOF, 04 extended linear address).
fn parse_intel_hex(text: &str) -> Result<Vec<u8>, String> {
    let mut out: Vec<u8> = Vec::new();
    let mut upper = 0u32;
    for line in text.lines() {
        let line = line.trim();
        if !line.starts_with(':') {
            continue;
        }
        let hex = &line[1..];
        if hex.len() < 10 {
            return Err(flash_error("Short HEX record"));
        }
        let byte = |s: &str| u8::from_str_radix(s, 16).map_err(|_| flash_error("Bad HEX digit"));
        let count = byte(&hex[0..2])? as usize;
        let offset = (byte(&hex[2..4])? as u32) << 8 | byte(&hex[4..6])? as u32;
        let rectype = byte(&hex[6..8])?;
        let data_start = 8;
        let data_end = 8 + count * 2;
        if hex.len() < data_end + 2 {
            return Err(flash_error("Short HEX record data"));
        }
        match rectype {
            0x00 => {
                let base = (upper << 16) | offset;
                if base < FLASH_BASE || (base - FLASH_BASE) as usize > MAX_IMAGE_LEN {
                    // not flash-resident data (e.g. option bytes); skip it
                    // rather than allocating hundreds of MB for a stray record
                    continue;
                }
                let target_start = (base - FLASH_BASE) as usize;
                if out.len() < target_start + count {
                    out.resize(target_start + count, 0xFF);
                }
                for i in 0..count {
                    out[target_start + i] = byte(&hex[data_start + i * 2..data_start + i * 2 + 2])?;
                }
            }
            0x01 => break, // EOF
            0x04 => {
                upper = (byte(&hex[8..10])? as u32) << 8 | byte(&hex[10..12])? as u32;
            }
            _ => {} // 02/03/05: not needed for these images
        }
    }
    Ok(out)
}

/// Full update flow for one node: jump to bootloader, flash, verify, go.
/// Returns Ok(()) on success.  `node` is the already-discovered serial
/// node whose port we take over.
pub fn update_node_firmware(
    node: &Arc<SerialNode>,
    firmware_path: &str,
    progress: &dyn Fn(&str),
) -> Result<(), String> {
    progress(&format!("Jumping node {} to bootloader", node.node_index + 1));
    if !node.jump_to_bootloader() {
        warn!("JUMP_TO_BOOTLOADER not ACKed; attempting bootloader sync anyway");
    }
    // give the processor time to enter the bootloader
    std::thread::sleep(std::time::Duration::from_millis(600));

    progress(&format!("Opening {} at {} baud", node.port, BOOT_BAUD));
    let flasher = Flasher::open(&node.port)?;

    progress("Synchronizing with bootloader");
    let mut synced = false;
    for attempt in 0..3 {
        match flasher.synchronize() {
            Ok(()) => {
                synced = true;
                break;
            }
            Err(_) => {
                std::thread::sleep(std::time::Duration::from_millis(200));
                let _ = attempt;
            }
        }
    }
    if !synced {
        return Err(flash_error("Unable to synchronize with node bootloader"));
    }

    let chip_id = flasher.get_chip_id()?;
    progress(&format!("Device chip id: {:#06x}", chip_id));

    let image = load_source_file(firmware_path)?;
    if image.is_empty() {
        return Err(flash_error("Firmware image is empty"));
    }
    progress(&format!("Loaded {} bytes of firmware", image.len()));

    progress("Erasing flash");
    flasher.erase_all()?;

    flasher.write_image(&image, &|msg| progress(msg))?;

    progress("Verifying");
    let readback = flasher.read_memory(FLASH_BASE, image.len())?;
    if readback != image {
        return Err(flash_error("Verification FAILED: read-back mismatch"));
    }
    progress("Verification OK");

    flasher.go(FLASH_BASE)?;
    progress("Firmware flashed; GO executed");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn intel_hex_single_record() {
        // : LL AAAA TT data CC -> 04 bytes at flash offset 0
        // (real .hex files carry the extended-address record)
        let hex = ":020000040800F2\n:04000000DEADBEEF22\n:00000001FF";
        let image = parse_intel_hex(hex).unwrap();
        assert_eq!(image, vec![0xDE, 0xAD, 0xBE, 0xEF]);
    }

    #[test]
    fn intel_hex_skips_non_flash_records() {
        // data below the flash base (e.g. option bytes at 0x1FFFF800) is
        // skipped instead of wrapping into a huge allocation
        let hex = ":020000041FFBEC\n:020000000102E1\n:020000040800F2\n:0100000005F8\n:00000001FF";
        let image = parse_intel_hex(hex).unwrap();
        assert_eq!(image, vec![0x05]);
    }

    #[test]
    fn intel_hex_offset_and_padding() {
        // data at offset 0x0004 with a gap -> 0xFF fill
        let hex = ":020000040800F2\n:020004000102E1\n:00000001FF";
        // extended linear address 0x0800 -> base 0x08000000 = FLASH_BASE
        let image = parse_intel_hex(hex).unwrap();
        assert_eq!(image.len(), 6); // 4 bytes of gap + 2 data bytes
        assert_eq!(&image[0..4], &[0xFF, 0xFF, 0xFF, 0xFF]);
        assert_eq!(&image[4..6], &[0x01, 0x02]);
    }

    #[test]
    fn intel_hex_rejects_bad_digits() {
        // (the ELA record must come first; bare records are skipped)
        assert!(parse_intel_hex(":020000040800F2\n:04000000ZZZZBBBBCC").is_err());
        assert!(parse_intel_hex(":0400").is_err());
    }

    #[test]
    fn checksum_xor_complement_matches_an3155() {
        // command frames are cmd + !cmd
        assert_eq!(!CMD_GET, 0xFF ^ CMD_GET);
        assert_eq!(!CMD_WRITE_MEMORY, 0xFF ^ CMD_WRITE_MEMORY);
        // checksums over byte runs use the shared XOR helper
        let addr = 0x0800_0000u32.to_be_bytes();
        assert_eq!(calculate_checksum(&addr), addr[0] ^ addr[1] ^ addr[2] ^ addr[3]);
    }
}


