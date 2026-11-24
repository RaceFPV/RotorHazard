//! LED event manager + strip effects — port of led_event_manager.py and
//! the rh_led_handler_strip bundled plugin (the classic color effects).
//!
//! The strip backends: ANSI terminal emulation is always available; the
//! ws281x GPIO backend is not yet wired (needs the rpi-ws281x crate on
//! the Pi) and falls back to the ANSI/no-op backend with a warning.

use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use tracing::debug;

// -- colors (led_event_manager.py) --

pub const fn color(red: u32, green: u32, blue: u32) -> u32 {
    (red << 16) | (green << 8) | blue
}

pub struct ColorVal;
#[allow(dead_code)]
impl ColorVal {
    pub const NONE: u32 = color(0, 0, 0);
    pub const BLUE: u32 = color(0, 31, 255);
    pub const CYAN: u32 = color(0, 255, 255);
    pub const DARK_ORANGE: u32 = color(255, 63, 0);
    pub const DARK_YELLOW: u32 = color(250, 210, 0);
    pub const GREEN: u32 = color(0, 255, 0);
    pub const LIGHT_GREEN: u32 = color(127, 255, 0);
    pub const ORANGE: u32 = color(255, 128, 0);
    pub const MINT: u32 = color(63, 255, 63);
    pub const PINK: u32 = color(255, 0, 127);
    pub const PURPLE: u32 = color(127, 0, 255);
    pub const RED: u32 = color(255, 0, 0);
    pub const SKY: u32 = color(0, 191, 255);
    pub const WHITE: u32 = color(255, 255, 255);
    pub const YELLOW: u32 = color(255, 255, 0);
}

/// Color patterns are [on, off] arrays; SOLID is null.
pub fn pattern(v: &[i64]) -> Value {
    Value::Array(v.iter().map(|&x| Value::from(x)).collect())
}
#[allow(dead_code)]
pub const PATTERN_SOLID: Value = Value::Null;

// -- strip --

/// LED strip output backends.
pub enum StripBackend {
    /// Terminal emulation (ANSI_leds.py)
    Ansi {
        pixels: Vec<u32>,
        width: usize,
        height: usize,
    },
    /// LedCtrlr over a serial port (ledctrlr_leds.py)
    LedCtrlr {
        pixels: Vec<u32>,
        changed_count: usize,
        last_changed_idx: usize,
        same_run_end: isize, // -1 when not a solid run
    },
    /// WS2812 via SPI (/dev/spidev*, as rpi5-ws2812 does)
    Spi {
        pixels: Vec<u32>,
        #[allow(dead_code)] // kept for diagnostics
        device: String,
    },
    /// No strip attached; effects run silently
    #[allow(dead_code)]
    None,
}

/// WS281x color-order remap (led_strip codes in ws281x_leds.py).
fn reorder(color: u32, order: &str) -> [u8; 3] {
    let r = ((color >> 16) & 0xFF) as u8;
    let g = ((color >> 8) & 0xFF) as u8;
    let b = (color & 0xFF) as u8;
    match order {
        "RGB" | "RGBW" => [r, g, b],
        "RBG" | "RBGW" => [r, b, g],
        "GRB" | "GRBW" => [g, r, b],
        "GBR" | "GBRW" => [g, b, r],
        "BRG" | "BRGW" => [b, r, g],
        "BGR" | "BGRW" => [b, g, r],
        _ => [r, g, b],
    }
}

/// Encodes RGB pixels as WS2812 SPI bytes (3 SPI bits per bit at 2.4MHz:
/// '1' -> 0b110, '0' -> 0b100) plus a reset-latch tail.
fn ws2812_spi_bytes(pixels: &[u32], brightness: u8, order: &str) -> Vec<u8> {
    let mut bits: Vec<bool> = Vec::with_capacity(pixels.len() * 24);
    let scale = |v: u8| (v as u16 * brightness as u16 / 255) as u8;
    for &px in pixels {
        // reorder() returns channels already in WS2812 wire order
        let wire = reorder(px, order);
        for channel in wire {
            for bit in (0..8).rev() {
                let on = scale(channel) >> bit & 1 == 1;
                bits.push(on); // 1
                bits.push(on); // 2
                bits.push(!on); // 3
            }
        }
    }
    let mut out = Vec::with_capacity(bits.len() / 8 + 32);
    for chunk in bits.chunks(8) {
        let mut byte = 0u8;
        for (i, &b) in chunk.iter().enumerate() {
            if b {
                byte |= 0x80 >> i;
            }
        }
        out.push(byte);
    }
    out.extend(std::iter::repeat(0u8).take(32)); // reset latch
    out
}

/// The serial LED controller (LedCtrlr) command/response protocol.
mod ledctrlr {
    use std::io::{Read, Write};
    use std::sync::Mutex;

    pub struct Ctrlr {
        port: Mutex<Box<dyn serialport::SerialPort>>,
    }

    impl Ctrlr {
        pub fn open(path: &str, baud: u32) -> Result<Ctrlr, String> {
            let sp = serialport::new(path, baud)
                .timeout(std::time::Duration::from_secs(1))
                .open()
                .map_err(|e| format!("LedCtrlr open failed: {}", e))?;
            Ok(Ctrlr { port: Mutex::new(sp) })
        }

        pub fn flush_initial(&self) {
            let mut ser = self.port.lock().unwrap();
            let _ = ser.write_all(b"\r");
            std::thread::sleep(std::time::Duration::from_millis(100));
            let mut sink = [0u8; 99];
            for _ in 0..5 {
                if ser.read(&mut sink).map(|n| n > 0).unwrap_or(false) {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
            let _ = ser.clear(serialport::ClearBuffer::Input);
        }

        /// Sends 'cmd\r', consumes the '.'-terminated ack line, then reads
        /// the '>'-terminated response body (doSendCommandToCtrlr).
        pub fn command(&self, cmd: &str, want_resp: bool) -> String {
            let mut ser = self.port.lock().unwrap();
            if ser.write_all(format!("{}\r", cmd).as_bytes()).is_err() {
                return String::new();
            }
            let mut ack = [0u8; 1];
            loop {
                match ser.read(&mut ack) {
                    Ok(1) => {
                        let ch = ack[0] as char;
                        if ch == '.' {
                            break;
                        }
                        if ch == '\0' {
                            tracing::warn!("No cmd response char from LedCtrlr for: {}", cmd);
                            break;
                        }
                        // '>' from a previous command is ignored; 'E' logged
                        if ch != '>' {
                            tracing::warn!("Unexpected LedCtrlr response char '{}' for: {}", ch, cmd);
                        }
                    }
                    _ => {
                        tracing::warn!("No cmd response char from LedCtrlr for: {}", cmd);
                        break;
                    }
                }
            }
            let mut body = String::new();
            if want_resp {
                loop {
                    match ser.read(&mut ack) {
                        Ok(1) => {
                            let ch = ack[0] as char;
                            if ch == '>' {
                                break;
                            }
                            if ch == '\0' {
                                tracing::warn!("Unexpected end of LedCtrlr response for: {}", cmd);
                                break;
                            }
                            body.push(ch);
                            if body.len() >= 99 {
                                tracing::warn!("Too-long LedCtrlr response for: {}", cmd);
                                break;
                            }
                        }
                        _ => break,
                    }
                }
            }
            body.trim().to_string()
        }

        pub fn send_bytes(&self, data: &[u8]) {
            let mut ser = self.port.lock().unwrap();
            if ser.write_all(data).is_err() {
                tracing::warn!("LedCtrlr pixel-data write failed");
            }
        }

        /// Writes one raw byte (the pixel-array terminator read).
        pub fn read_one(&self) -> Option<u8> {
            let mut buf = [0u8; 1];
            self.port.lock().unwrap().read(&mut buf).ok().map(|_| buf[0])
        }
    }
}

pub struct Strip {
    pub backend: StripBackend,
    /// LedCtrlr serial handle (LedCtrlr backend)
    ctrlr: Option<ledctrlr::Ctrlr>,
    /// SPI device handle (Spi backend)
    spi: Option<std::fs::File>,
    brightness: u8,
    order: String,
    #[allow(dead_code)] // kept for diagnostics
    spi_device: String,
}

impl Strip {
    pub fn new(count: usize, rows: usize) -> Strip {
        let (width, height) = if rows < 1 {
            (count, 1)
        } else {
            (count / rows.max(1), rows)
        };
        Strip {
            backend: StripBackend::Ansi {
                pixels: vec![0; count],
                width,
                height,
            },
            ctrlr: None,
            spi: None,
            brightness: 255,
            order: "GRB".to_string(),
            spi_device: String::new(),
        }
    }

    /// Backend selection per get_pixel_interface in Python: a configured
    /// serial LED controller wins, then WS2812-over-SPI, then ANSI
    /// terminal emulation.
    pub fn new_from_config(config: &Value, brightness: u8) -> Strip {
        let led = config.get("LED").cloned().unwrap_or(json!({}));
        let count = led.get("LED_COUNT").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
        let order = led
            .get("LED_STRIP")
            .and_then(|v| v.as_str())
            .unwrap_or("GRB")
            .to_string();

        // serial LED controller (ledctrlr_leds.get_pixel_interface)
        let ctrlr_port = led
            .get("SERIAL_CTRLR_PORT")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty());
        if let (Some(port), true) = (ctrlr_port, count > 0) {
            let baud = led
                .get("SERIAL_CTRLR_BAUD")
                .and_then(|v| v.as_u64())
                .unwrap_or(115200) as u32;
            match ledctrlr::Ctrlr::open(port, baud) {
                Ok(ctrlr) => {
                    ctrlr.flush_initial();
                    let version = ctrlr.command("V", true);
                    if version.contains("LED Controller") {
                        tracing::info!("Established LedCtrlr connection to: {}", version);
                    } else {
                        tracing::warn!("Unexpected first response from LedCtrlr: {}", version);
                    }
                    let max_resp = ctrlr.command("M", true);
                    if let Ok(max_leds) = max_resp.parse::<usize>() {
                        if count > max_leds {
                            tracing::warn!(
                                "Configured LED_COUNT ({}) is greater than LedCtrlr maximum ({})",
                                count, max_leds
                            );
                        }
                    } else {
                        tracing::warn!("Unable to parse LedCtrlr 'M' response as integer: {}", max_resp);
                    }
                    ctrlr.command(&format!("C {} {}", count, order), false);
                    let mut strip = Strip {
                        backend: StripBackend::LedCtrlr {
                            pixels: vec![0; count],
                            changed_count: 0,
                            last_changed_idx: 0,
                            same_run_end: -1,
                        },
                        ctrlr: Some(ctrlr),
                        spi: None,
                        brightness,
                        order,
                        spi_device: String::new(),
                    };
                    tracing::info!(
                        "LED: LedCtrlr via serial port ({}, {} baud, count={})",
                        port, baud, count
                    );
                    strip.set_brightness(brightness);
                    return strip;
                }
                Err(e) => tracing::warn!("LedCtrlr open failed; falling back: {}", e),
            }
        }

        // WS2812 over SPI when a spidev device exists (the rpi5-ws2812
        // path; only GPIO10/MOSI wiring is supported, as on Python's Pi 5)
        if count > 0 {
            let spi_dev = "/dev/spidev0.0";
            if std::path::Path::new(spi_dev).exists() {
                match std::fs::OpenOptions::new().write(true).open(spi_dev) {
                    Ok(f) => {
                        tracing::info!("LED: WS2812 via SPI ({}) count={}", spi_dev, count);
                        return Strip {
                            backend: StripBackend::Spi {
                                pixels: vec![0; count],
                                device: spi_dev.into(),
                            },
                            ctrlr: None,
                            spi: Some(f),
                            brightness,
                            order,
                            spi_device: spi_dev.into(),
                        };
                    }
                    Err(e) => tracing::warn!("SPI LED open failed; falling back to ANSI: {}", e),
                }
            }
        }

        if count > 0 {
            tracing::info!("LED: locally emulated via ANSI terminal ({} pixels)", count);
        }
        Strip::new(count, 1)
    }

    pub fn num_pixels(&self) -> usize {
        match &self.backend {
            StripBackend::Ansi { pixels, .. }
            | StripBackend::LedCtrlr { pixels, .. }
            | StripBackend::Spi { pixels, .. } => pixels.len(),
            StripBackend::None => 0,
        }
    }

    pub fn set_pixel_color(&mut self, i: usize, color: u32) {
        match &mut self.backend {
            StripBackend::Ansi { pixels, .. }
            | StripBackend::LedCtrlr { pixels, .. }
            | StripBackend::Spi { pixels, .. } => {
                if let Some(px) = pixels.get_mut(i) {
                    *px = color;
                }
            }
            StripBackend::None => {}
        }
        if let StripBackend::LedCtrlr {
            changed_count,
            last_changed_idx,
            same_run_end,
            pixels,
        } = &mut self.backend
        {
            *changed_count += 1;
            *last_changed_idx = i;
            if i == 0 {
                *same_run_end = 0;
            } else if i as isize == *same_run_end + 1
                && pixels.get(i).copied().unwrap_or(0)
                    == pixels.get(*same_run_end as usize).copied().unwrap_or(1)
            {
                *same_run_end = i as isize;
            } else {
                *same_run_end = -1;
            }
        }
    }

    pub fn get_pixel_color(&self, i: usize) -> u32 {
        match &self.backend {
            StripBackend::Ansi { pixels, .. }
            | StripBackend::LedCtrlr { pixels, .. }
            | StripBackend::Spi { pixels, .. } => pixels.get(i).copied().unwrap_or(0),
            StripBackend::None => 0,
        }
    }

    pub fn set_brightness(&mut self, brightness: u8) {
        self.brightness = brightness;
        if let Some(ctrlr) = &self.ctrlr {
            ctrlr.command(&format!("B {}", brightness), false);
        }
    }

    pub fn show(&mut self) {
        match &mut self.backend {
            StripBackend::Ansi { pixels, width, height } => {
                if pixels.is_empty() {
                    return; // nothing attached: don't paint the terminal
                }
                // ANSI row rendering (ANSI_leds.py show()); one row per line
                let mut out = String::new();
                for row in 0..*height {
                    let start = row * *width;
                    let end = (start + *width).min(pixels.len());
                    for &px in &pixels[start..end] {
                        let (r, g, b) = ((px >> 16) & 0xff, (px >> 8) & 0xff, px & 0xff);
                        // approximate the ANSI color classes with truecolor
                        out.push_str(&format!("\x1b[48;2;{};{};{}m \x1b[0m", r, g, b));
                    }
                    out.push_str("\x1b[K\n");
                }
                print!("\x1b[H{}", out);
            }
            StripBackend::LedCtrlr { pixels, changed_count, last_changed_idx, same_run_end } => {
                if let Some(ctrlr) = &self.ctrlr {
                    if *changed_count == 1 {
                        ctrlr.command(
                            &format!("P {} {:06X}", last_changed_idx, pixels[*last_changed_idx]),
                            false,
                        );
                    } else if *same_run_end == (pixels.len() as isize) - 1 {
                        ctrlr.command(&format!("F {:06X}", pixels[0]), false);
                    } else {
                        ctrlr.command("A", false);
                        let mut bytes = Vec::with_capacity(pixels.len() * 3);
                        for &px in pixels.iter() {
                            bytes.push(((px >> 16) & 0xFF) as u8);
                            bytes.push(((px >> 8) & 0xFF) as u8);
                            bytes.push((px & 0xFF) as u8);
                        }
                        ctrlr.send_bytes(&bytes);
                        if let Some(resp) = ctrlr.read_one() {
                            let ch = resp as char;
                            if ch == 'E' {
                                let err = ctrlr.command("L", true);
                                tracing::warn!("LedCtrlr error after pixel array: {}", err);
                            } else if ch != '>' && ch != '.' {
                                tracing::warn!("Unexpected LedCtrlr char after pixel array: '{}'", ch);
                            }
                        }
                    }
                    ctrlr.command("S", false); // show
                }
                *changed_count = 0;
                *same_run_end = -1;
            }
            StripBackend::Spi { pixels, .. } => {
                if let Some(f) = self.spi.as_ref() {
                    let bytes = ws2812_spi_bytes(pixels, self.brightness, &self.order);
                    use std::io::Write;
                    let mut f = f;
                    if f.write_all(&bytes).is_err() {
                        tracing::warn!("SPI LED write failed");
                    }
                }
            }
            StripBackend::None => {}
        }
    }
}

// -- effect plumbing --

/// Cancellation flag shared with the running effect task.
#[derive(Clone, Default)]
pub struct Terminator(Arc<AtomicBool>);

impl Terminator {
    pub fn terminate(&self) -> bool {
        self.0.load(Ordering::Relaxed)
    }
    pub fn set_terminate(&self) {
        self.0.store(true, Ordering::Relaxed);
    }
}

/// effect_delay(): no-op when terminated, otherwise sleeps; the caller
/// returns early after termination is flagged.  Blocking sleep: effects
/// run in a spawn_blocking task (Python ran them in greenlets).
pub fn effect_delay(ms: u64, term: &Terminator) -> Result<(), ()> {
    if term.terminate() {
        return Err(());
    }
    if ms > 0 {
        std::thread::sleep(std::time::Duration::from_millis(ms));
        if term.terminate() {
            return Err(());
        }
    }
    Ok(())
}

pub type EffectFn = fn(&mut Strip, &Value, &Terminator) -> ();

/// Runs an effect body off the calling thread: spawn_blocking inside a
/// Tokio runtime, a plain thread otherwise (tests, early startup).
fn spawn_effect_task(f: impl FnOnce() + Send + 'static) {
    match tokio::runtime::Handle::try_current() {
        Ok(handle) => {
            handle.spawn_blocking(f);
        }
        Err(_) => {
            std::thread::spawn(f);
        }
    }
}

struct EffectDef {
    #[allow(dead_code)]
    label: &'static str,
    handler: EffectFn,
    default_args: Value,
}

// -- effects (rh_led_handler_strip) --

pub fn led_on(strip: &mut Strip, color: u32, pattern: &Value, offset: usize) {
    let is_solid = pattern.is_null();
    let pat: Vec<usize> = pattern
        .as_array()
        .map(|a| a.iter().filter_map(|v| v.as_u64().map(|x| x as usize)).collect())
        .unwrap_or_default();
    let n = strip.num_pixels();
    for i in 0..n {
        if is_solid {
            strip.set_pixel_color(i, color);
        } else if pat.len() == 2 {
            let patternlength = pat[0] + pat[1];
            if (i + offset) % patternlength < pat[0] {
                strip.set_pixel_color(i, color);
            } else {
                strip.set_pixel_color(i, ColorVal::NONE);
            }
        }
    }
    strip.show();
}

pub fn led_off(strip: &mut Strip) {
    led_on(strip, ColorVal::NONE, &Value::Null, 0);
}

fn get_u64(args: &Value, key: &str, default: u64) -> u64 {
    args.get(key).and_then(|v| v.as_u64()).unwrap_or(default)
}
fn get_f64(args: &Value, key: &str, default: f64) -> f64 {
    args.get(key).and_then(|v| v.as_f64()).unwrap_or(default)
}
fn get_color(args: &Value, key: &str, default: u32) -> u32 {
    args.get(key).and_then(|v| v.as_u64()).map(|v| v as u32).unwrap_or(default)
}

fn show_color(strip: &mut Strip, args: &Value, _t: &Terminator) {
    let color = get_color(args, "color", ColorVal::WHITE);
    let pattern = args.get("pattern").cloned().unwrap_or(Value::Null);
    led_on(strip, color, &pattern, 0);
}

fn clear(strip: &mut Strip, _args: &Value, _t: &Terminator) {
    led_off(strip);
}

fn chase(strip: &mut Strip, args: &Value, t: &Terminator) {
    let color = get_color(args, "color", ColorVal::WHITE);
    let pattern = args
        .get("pattern")
        .cloned()
        .unwrap_or_else(|| pattern(&[1, 2]));
    let speed_delay = get_u64(args, "speedDelay", 50);
    let iterations = get_u64(args, "iterations", 5);
    let pat_len = pattern
        .as_array()
        .map(|a| a.iter().filter_map(|v| v.as_u64()).sum::<u64>() as usize)
        .unwrap_or(1);
    led_off(strip);
    for i in 0..iterations * pat_len as u64 {
        led_on(strip, color, &pattern, i as usize);
        if effect_delay(speed_delay, t).is_err() {
            return;
        }
    }
}

fn color_wheel(pos: u32) -> u32 {
    if pos < 85 {
        color(pos * 3, 255 - pos * 3, 0)
    } else if pos < 170 {
        let pos = pos - 85;
        color(255 - pos * 3, 0, pos * 3)
    } else {
        let pos = pos - 170;
        color(0, pos * 3, 255 - pos * 3)
    }
}

fn rainbow(strip: &mut Strip, _args: &Value, _t: &Terminator) {
    let n = strip.num_pixels() as u32;
    if n == 0 {
        return;
    }
    for i in 0..n {
        strip.set_pixel_color(i as usize, color_wheel((i * 256 / n) & 255));
    }
    strip.show();
}

fn rainbow_cycle(strip: &mut Strip, args: &Value, t: &Terminator) {
    let n = strip.num_pixels() as u32;
    if n == 0 {
        return;
    }
    let mut wait_ms = get_u64(args, "wait_ms", 2);
    if wait_ms == 0 {
        wait_ms = 2;
    }
    loop {
        for j in 0..256u32 {
            for i in 0..n {
                strip.set_pixel_color(i as usize, color_wheel((i * 256 / n + j) & 255));
            }
            strip.show();
            if effect_delay(wait_ms, t).is_err() {
                return;
            }
        }
    }
}

fn color_wipe(strip: &mut Strip, args: &Value, t: &Terminator) {
    let color = get_color(args, "color", ColorVal::WHITE);
    let n = strip.num_pixels();
    if n == 0 {
        return;
    }
    let speed_delay = get_f64(args, "speedDelay", 256.0) / n as f64;
    for i in 0..n {
        strip.set_pixel_color(i, color);
        strip.show();
        if effect_delay(speed_delay as u64, t).is_err() {
            return;
        }
    }
}

pub fn dim(color: u32, decay: f64) -> u32 {
    let r = (color & 0x00ff_0000) >> 16;
    let g = (color & 0x0000_ff00) >> 8;
    let b = color & 0x0000_00ff;
    let r = if r <= 1 { 0 } else { (r as f64 * decay) as u32 };
    let g = if g <= 1 { 0 } else { (g as f64 * decay) as u32 };
    let b = if b <= 1 { 0 } else { (b as f64 * decay) as u32 };
    crate::led::color(r.min(255), g.min(255), b.min(255))
}

fn fade(strip: &mut Strip, args: &Value, t: &Terminator) {
    let color = get_color(args, "color", ColorVal::WHITE);
    let pattern = args.get("pattern").cloned().unwrap_or(Value::Null);
    let steps = get_u64(args, "steps", 25);
    let speed_delay = get_u64(args, "speedDelay", 10);
    let on_time = get_u64(args, "onTime", 250);
    let mut off_time = get_u64(args, "offTime", 250);
    let iterations = get_u64(args, "iterations", 1);
    let out_steps = get_u64(args, "outSteps", steps);

    led_off(strip);
    // never exceed 3Hz (prevent seizures)
    off_time = ((333u64).saturating_sub(steps * speed_delay + out_steps * speed_delay + on_time))
        .min(off_time);

    for _ in 0..iterations {
        // fade in
        if steps > 0 {
            led_off(strip);
            for j in 0..steps {
                let c = dim(color, j as f64 / steps as f64);
                led_on(strip, c, &pattern, 0);
                strip.show();
                if effect_delay(speed_delay, t).is_err() {
                    return;
                }
            }
            led_on(strip, color, &pattern, 0);
            if effect_delay(on_time, t).is_err() {
                return;
            }
        }
        // fade out
        if out_steps > 0 {
            led_on(strip, color, &pattern, 0);
            for j in (1..=out_steps).rev() {
                let c = dim(color, j as f64 / out_steps as f64);
                led_on(strip, c, &pattern, 0);
                strip.show();
                if effect_delay(speed_delay, t).is_err() {
                    return;
                }
            }
            led_off(strip);
        }
        if effect_delay(off_time, t).is_err() {
            return;
        }
    }
}

fn sparkle(strip: &mut Strip, args: &Value, t: &Terminator) {
    let color = get_color(args, "color", ColorVal::WHITE);
    let chance = get_f64(args, "chance", 1.0);
    let decay = get_f64(args, "decay", 0.95);
    let speed_delay = get_u64(args, "speedDelay", 100);
    let iterations = get_u64(args, "iterations", 100);
    let n = strip.num_pixels();
    if n == 0 {
        return;
    }
    let decay_steps = if decay != 0.0 {
        (0.00390625f64.ln() / decay.ln()).ceil() as u64
    } else {
        0
    };
    led_off(strip);
    // deterministic pseudo-random per tick (sparkle is decorative)
    let mut seed = (std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos() as u64)
        .unwrap_or(1))
    | 1;
    for i in 0..iterations + decay_steps {
        for j in 0..n {
            let c = strip.get_pixel_color(j);
            strip.set_pixel_color(j, dim(c, decay));
        }
        if i < iterations {
            for px in 0..n {
                seed = seed
                    .wrapping_mul(6364136223846793005)
                    .wrapping_add(1442695040888963407);
                let r = ((seed >> 33) as f64 / (u64::MAX >> 33) as f64) as f64;
                if r < chance / n as f64 {
                    strip.set_pixel_color(px, color);
                }
            }
        }
        strip.show();
        if effect_delay(speed_delay, t).is_err() {
            return;
        }
    }
}

fn meteor(strip: &mut Strip, args: &Value, t: &Terminator) {
    let color = get_color(args, "color", ColorVal::WHITE);
    let meteor_size = get_u64(args, "meteorSize", 10) as usize;
    let decay = get_f64(args, "decay", 0.75);
    let random_decay = args.get("randomDecay").map(|v| v.as_bool().unwrap_or(true)).unwrap_or(true);
    let speed_delay = get_u64(args, "speedDelay", 1);
    let n = strip.num_pixels();
    if n == 0 {
        return;
    }
    led_off(strip);
    let mut seed = (std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos() as u64)
        .unwrap_or(1))
    | 1;
    for i in 0..(n * 2) {
        for j in 0..n {
            let roll = {
                seed = seed
                    .wrapping_mul(6364136223846793005)
                    .wrapping_add(1442695040888963407);
                ((seed >> 33) as i64).rem_euclid(100) >= 50
            };
            if !random_decay || roll {
                let c = strip.get_pixel_color(j);
                strip.set_pixel_color(j, dim(c, decay));
            }
        }
        for j in 0..meteor_size {
            if i.checked_sub(j).map(|v| v < n).unwrap_or(false) {
                strip.set_pixel_color(i - j, color);
            }
        }
        strip.show();
        if effect_delay(speed_delay, t).is_err() {
            return;
        }
    }
}

fn larson_scanner(strip: &mut Strip, args: &Value, t: &Terminator) {
    let num_pixels = strip.num_pixels();
    if num_pixels == 0 {
        return;
    }
    let color = get_color(args, "color", ColorVal::WHITE);
    let eye_size_default = if num_pixels > 20 { 4 } else { (num_pixels / 8).max(1) };
    let eye_size = get_u64(args, "eyeSize", eye_size_default as u64) as usize;
    let speed_delay_default = if num_pixels > 20 { 256 } else { 320 };
    let speed_delay = (get_f64(args, "speedDelay", speed_delay_default as f64) / num_pixels as f64) as u64;
    let return_delay = get_u64(args, "returnDelay", 50);
    let iterations = get_u64(args, "iterations", 3);

    led_off(strip);
    for _k in 0..iterations {
        for i in 0..num_pixels.saturating_sub(eye_size + 1) {
            if i > 0 {
                strip.set_pixel_color(i - 1, ColorVal::NONE);
            }
            strip.set_pixel_color(i, dim(color, 0.25));
            for j in 0..eye_size {
                strip.set_pixel_color(i + j + 1, color);
            }
            strip.set_pixel_color(i + eye_size + 1, dim(color, 0.25));
            strip.show();
            if effect_delay(speed_delay, t).is_err() {
                return;
            }
        }
        if effect_delay(return_delay, t).is_err() {
            return;
        }
        for i in (0..num_pixels.saturating_sub(eye_size + 1)).rev() {
            if i + eye_size + 2 < num_pixels {
                strip.set_pixel_color(i + eye_size + 2, ColorVal::NONE);
            }
            strip.set_pixel_color(i, dim(color, 0.25));
            for j in 0..eye_size {
                strip.set_pixel_color(i + j + 1, color);
            }
            strip.set_pixel_color(i + eye_size + 1, dim(color, 0.25));
            strip.show();
            if effect_delay(speed_delay, t).is_err() {
                return;
            }
        }
        if effect_delay(return_delay, t).is_err() {
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strip(n: usize) -> Strip {
        Strip::new(n, 1)
    }

    #[test]
    fn color_packing() {
        assert_eq!(color(255, 0, 0), 0x00ff_0000);
        assert_eq!(color(0, 255, 0), 0x0000_ff00);
        assert_eq!(color(0, 0, 255), 0x0000_00ff);
        assert_eq!(color(1, 2, 3), 0x010203);
        assert_eq!(ColorVal::WHITE, 0x00ff_ffff);
        assert_eq!(ColorVal::NONE, 0);
    }

    #[test]
    fn dim_fades_and_floor_zeroes() {
        assert_eq!(dim(ColorVal::WHITE, 1.0), ColorVal::WHITE);
        assert_eq!(dim(ColorVal::WHITE, 0.5), color(127, 127, 127)); // 127.5 truncates
        // components of 1 or less become zero (Python: 0 if r <= 1)
        assert_eq!(dim(color(1, 2, 1), 0.5), color(0, 1, 0));
        assert_eq!(dim(ColorVal::NONE, 0.5), 0);
    }

    #[test]
    fn color_wheel_boundaries_match_python() {
        // pos<85: red->green ramp; 85..170 green->blue; 170..255 blue->red
        assert_eq!(color_wheel(0), color(0, 255, 0));
        assert_eq!(color_wheel(84), color(252, 3, 0));
        assert_eq!(color_wheel(85), color(255, 0, 0));
        assert_eq!(color_wheel(169), color(3, 0, 252));
        assert_eq!(color_wheel(170), color(0, 0, 255));
        assert_eq!(color_wheel(255), color(0, 255, 0));
    }

    #[test]
    fn led_on_solid_and_patterns() {
        let mut s = strip(8);
        led_on(&mut s, ColorVal::RED, &Value::Null, 0);
        for i in 0..8 {
            assert_eq!(s.get_pixel_color(i), ColorVal::RED);
        }
        // pattern [1,2]: one on, two off, repeating; offset shifts the phase
        let mut s = strip(6);
        led_on(&mut s, ColorVal::GREEN, &pattern(&[1, 2]), 0);
        let expected = [ColorVal::GREEN, 0, 0, ColorVal::GREEN, 0, 0];
        for i in 0..6 {
            assert_eq!(s.get_pixel_color(i), expected[i]);
        }
        let mut s = strip(6);
        led_on(&mut s, ColorVal::GREEN, &pattern(&[1, 2]), 1);
        // (i+1)%3 < 1 -> i = 2, 5
        let expected = [0, 0, ColorVal::GREEN, 0, 0, ColorVal::GREEN];
        for i in 0..6 {
            assert_eq!(s.get_pixel_color(i), expected[i]);
        }
        led_off(&mut s);
        for i in 0..6 {
            assert_eq!(s.get_pixel_color(i), 0);
        }
    }

    #[test]
    fn show_color_and_clear_effects() {
        let mut s = strip(4);
        show_color(&mut s, &json!({"color": ColorVal::BLUE, "pattern": [2, 2]}), &Terminator::default());
        // (i+0)%4 < 2: pixels 0,1 lit; 2,3 off
        assert_eq!(s.get_pixel_color(0), ColorVal::BLUE);
        assert_eq!(s.get_pixel_color(1), ColorVal::BLUE);
        assert_eq!(s.get_pixel_color(2), 0);
        assert_eq!(s.get_pixel_color(3), 0);
        // no color arg defaults to white, solid
        show_color(&mut s, &json!({}), &Terminator::default());
        assert_eq!(s.get_pixel_color(3), ColorVal::WHITE);
        clear(&mut s, &json!({}), &Terminator::default());
        assert_eq!(s.get_pixel_color(3), 0);
    }

    #[test]
    fn color_wipe_fills_strip_then_stops() {
        let mut s = strip(5);
        // speedDelay scaled per pixel; terminator never trips
        color_wipe(&mut s, &json!({"color": ColorVal::ORANGE, "speedDelay": 0}), &Terminator::default());
        for i in 0..5 {
            assert_eq!(s.get_pixel_color(i), ColorVal::ORANGE);
        }
    }

    #[test]
    fn terminated_effect_stops_early() {
        let mut s = strip(100);
        let term = Terminator::default();
        term.set_terminate();
        // a rainbow cycle with a pre-terminated flag exits immediately
        rainbow_cycle(&mut s, &json!({"wait_ms": 0}), &term);
        // chase with termination also returns promptly without panicking
        chase(&mut s, &json!({"speedDelay": 0, "iterations": 1000}), &term);
    }

    #[test]
    fn chase_pattern_offsets_advance() {
        let mut s = strip(6);
        chase(
            &mut s,
            &json!({"color": ColorVal::RED, "pattern": [1, 2], "speedDelay": 0, "iterations": 1}),
            &Terminator::default(),
        );
        // after the final iteration (offset 2) the pattern phase is offset by 2:
        // (i+2)%3 < 1 -> pixels 1 and 4 are lit
        assert_eq!(s.get_pixel_color(1), ColorVal::RED);
        assert_eq!(s.get_pixel_color(4), ColorVal::RED);
        assert_eq!(s.get_pixel_color(0), 0);
    }

    #[test]
    fn empty_strip_effects_are_noops() {
        let mut s = strip(0);
        let t = Terminator::default();
        rainbow(&mut s, &json!({}), &t);
        rainbow_cycle(&mut s, &json!({}), &t);
        color_wipe(&mut s, &json!({}), &t);
        sparkle(&mut s, &json!({}), &t);
        meteor(&mut s, &json!({}), &t);
        larson_scanner(&mut s, &json!({}), &t);
    }

    #[test]
    fn manager_registers_defaults_and_resolves() {
        let mgr = LedManager::new(&json!({"LED": {"LED_COUNT": 8, "LED_ROWS": 1}}));
        // defaults are applied at server startup (init_LED_effects)
        mgr.apply_assignments(None);
        // Python default assignments (init_LED_effects)
        assert_eq!(mgr.get_event_effect("raceStage").as_deref(), Some("stripColor2_1"));
        assert_eq!(mgr.get_event_effect("raceStart").as_deref(), Some("stripColorSolid"));
        assert_eq!(mgr.get_event_effect("raceFinish").as_deref(), Some("stripColor4_4"));
        assert_eq!(mgr.get_event_effect("raceStop").as_deref(), Some("stripColorSolid"));
        assert_eq!(mgr.get_event_effect("lapsClear").as_deref(), Some("clear"));
        assert_eq!(mgr.get_event_effect("crossingEnter").as_deref(), Some("stripSparkle"));
        assert_eq!(mgr.get_event_effect("crossingExit").as_deref(), Some("none"));
        assert_eq!(mgr.get_event_effect("startup").as_deref(), Some("rainbowCycle"));
        assert_eq!(mgr.get_event_effect("manualColor").as_deref(), Some("stripColor"));
        // unknown event has no binding
        assert!(mgr.get_event_effect("noSuchEvent").is_none());
    }

    #[test]
    fn manager_saved_assignments_override_defaults() {
        let mgr = LedManager::new(&json!({"LED": {"LED_COUNT": 4}}));
        mgr.apply_assignments(Some(&json!({"raceStart": "stripBlink"})));
        assert_eq!(mgr.get_event_effect("raceStart").as_deref(), Some("stripBlink"));
        // defaults still hold for unassigned events
        assert_eq!(mgr.get_event_effect("raceStop").as_deref(), Some("stripColorSolid"));
        // unknown effect names are rejected
        assert!(!mgr.set_event_effect("raceStart", "notAnEffect"));
        assert!(mgr.set_event_effect("raceStart", "stripWipe"));
        assert_eq!(mgr.get_event_effect("raceStart").as_deref(), Some("stripWipe"));
    }

    #[test]
    fn manager_trigger_runs_effect_and_stops_previous() {
        let mgr = LedManager::new(&json!({"LED": {"LED_COUNT": 8}}));
        mgr.apply_assignments(None);
        // rainbowCycle runs until terminated; triggering another effect must
        // stop it rather than run two at once
        mgr.trigger("startup", &json!({}));
        std::thread::sleep(std::time::Duration::from_millis(50));
        mgr.trigger("raceStart", &json!({}));
        std::thread::sleep(std::time::Duration::from_millis(50));
        // no panic and the strip reflects the later effect (solid white)
        let s = mgr.strip.lock().unwrap();
        assert_eq!(s.get_pixel_color(0), ColorVal::WHITE);
    }

    #[test]
    fn strip_creation_sizes() {
        // rows < 1 -> single row of count pixels
        let s = Strip::new(8, 0);
        assert_eq!(s.num_pixels(), 8);
        // rows split the count
        let s = Strip::new(16, 2);
        assert_eq!(s.num_pixels(), 16);
    }

    #[test]
    fn color_order_remap() {
        let c = color(1, 2, 3);
        assert_eq!(reorder(c, "GRB"), [2, 1, 3]);
        assert_eq!(reorder(c, "RGB"), [1, 2, 3]);
        assert_eq!(reorder(c, "BGR"), [3, 2, 1]);
        assert_eq!(reorder(c, "GBR"), [2, 3, 1]);
        // unknown falls back to RGB
        assert_eq!(reorder(c, "junk"), [1, 2, 3]);
    }

    #[test]
    fn ws2812_spi_encoding_matches_bit_pattern() {
        // one GRB pixel: G=0b10101010 R=0b00001111 B=0b11110000, full brightness
        let bytes = ws2812_spi_bytes(&[color(0x0F, 0xAA, 0xF0)], 255, "GRB");
        // 24 data bits -> 9 SPI bytes (72 bits), then 32 zero latch bytes
        assert_eq!(bytes.len(), 9 + 32);
        assert!(bytes[9..].iter().all(|&b| b == 0));
        // reconstruct the bit stream: '1' -> 110, '0' -> 100
        let mut bits = Vec::new();
        for &b in &bytes[..9] {
            for i in (0..8).rev() {
                let pair = (b >> i) & 1;
                bits.push(pair == 1); // first bit of the 3-bit group
            }
        }
        // bits.len() is 72 but only 24 groups matter: reassemble as triplets
        let mut groups = Vec::new();
        for chunk in bits.chunks(3) {
            groups.push((chunk[0], chunk[2]));
        }
        // GRB order: G=0xAA -> 10101010, R=0x0F, B=0xF0
        let expected_bits = {
            let mut v = Vec::new();
            for channel in [0xAAu8, 0x0F, 0xF0] {
                for i in (0..8).rev() {
                    let on = (channel >> i) & 1 == 1;
                    v.push((on, !on));
                }
            }
            v
        };
        assert_eq!(groups, expected_bits);
        // brightness scaling: half brightness zeroes the low bit patterns' scale
        let dimmed = ws2812_spi_bytes(&[ColorVal::WHITE], 128, "GRB");
        assert_ne!(dimmed[..9], ws2812_spi_bytes(&[ColorVal::WHITE], 255, "GRB")[..9]);
    }

    #[test]
    fn backend_selection_prefers_ansi_without_hardware() {
        // no LED_COUNT and no serial controller -> plain ANSI strip
        let s = Strip::new_from_config(&json!({"LED": {"LED_COUNT": 0}}), 32);
        assert!(matches!(s.backend, StripBackend::Ansi { .. }));
        // LED_COUNT without any spidev in the test env -> ANSI emulation
        let s = Strip::new_from_config(&json!({"LED": {"LED_COUNT": 8}}), 32);
        assert!(matches!(s.backend, StripBackend::Ansi { .. }));
    }
}

// -- manager --

pub const IDLE_READY: &str = "ledIdleReady";
pub const IDLE_DONE: &str = "ledIdleDone";
pub const IDLE_RACING: &str = "ledIdleRacing";

pub struct LedManager {
    strip: Arc<Mutex<Strip>>,
    effects: HashMap<String, EffectDef>,
    events: Mutex<HashMap<String, String>>,
    running: Mutex<Option<Terminator>>,
}

impl LedManager {
    /// Builds the manager with the strip chosen from the LED config
    /// (get_pixel_interface chain: serial controller -> WS2812 SPI -> ANSI).
    pub fn new(config: &Value) -> LedManager {
        let brightness = config
            .get("LED")
            .and_then(|l| l.get("ledBrightness"))
            .and_then(|v| v.as_u64())
            .unwrap_or(32) as u8;
        let strip = Strip::new_from_config(config, brightness);
        let mut mgr = LedManager {
            strip: Arc::new(Mutex::new(strip)),
            effects: HashMap::new(),
            events: Mutex::new(HashMap::new()),
            running: Mutex::new(None),
        };
        mgr.register_builtin_effects();
        mgr
    }

    fn register(&mut self, name: &str, label: &'static str, handler: EffectFn, default_args: Value) {
        self.effects.insert(
            name.to_string(),
            EffectDef { label, handler, default_args },
        );
    }

    fn register_builtin_effects(&mut self) {
        // manager built-ins (LEDEventManager.__init__)
        self.register("hold", "Hold", |_, _, _| {}, json!({}));
        self.register("none", "No Effect", |_, _, _| {}, json!({}));
        // strip handler effects (rh_led_handler_strip.discover())
        self.register("stripColor", "Color/Pattern (Args)", show_color, json!({"time": 4}));
        self.register("stripColorSolid", "Solid", show_color, json!({"time": 4}));
        self.register("stripColor1_1", "Pattern 1-1", show_color,
            json!({"pattern": [1, 1], "time": 4}));
        self.register("stripColor1_2", "Pattern 1-2", show_color,
            json!({"pattern": [1, 2], "time": 4}));
        self.register("stripColor2_1", "Pattern 2-1", show_color,
            json!({"pattern": [2, 1], "time": 4}));
        self.register("stripColor4_4", "Pattern 4-4", show_color,
            json!({"pattern": [4, 4], "time": 4}));
        self.register("stripStaging", "Staging Pulse 2-1", fade,
            json!({"pattern": [2, 1], "ontime": 0, "steps": 0, "outSteps": 10, "time": 2}));
        self.register("stripChase1_2", "Chase Pattern 1-2", chase,
            json!({"pattern": [1, 2], "speedDelay": 50, "iterations": 5}));
        self.register("stripChase2_1", "Chase Pattern 2-1", chase,
            json!({"pattern": [2, 1], "speedDelay": 50, "iterations": 5}));
        self.register("stripChase4_4", "Chase Pattern 4-4", chase,
            json!({"pattern": [4, 4], "speedDelay": 50, "iterations": 5}));
        self.register("rainbow", "Rainbow", rainbow, json!({"time": 4}));
        self.register("rainbowCycle", "Rainbow Cycle", rainbow_cycle, json!({}));
        self.register("stripWipe", "Wipe", color_wipe, json!({"speedDelay": 3, "time": 2}));
        self.register("stripFadeIn", "Fade In", fade,
            json!({"steps": 50, "outSteps": 0, "speedDelay": 10, "onTime": 0, "offTime": 0, "iterations": 1, "time": 4}));
        self.register("stripPulse", "Pulse 3x", fade,
            json!({"steps": 10, "outSteps": 10, "speedDelay": 1, "onTime": 10, "offTime": 10, "iterations": 3, "time": 3}));
        self.register("stripFadeOut", "Fade Out", fade,
            json!({"steps": 10, "outSteps": 128, "speedDelay": 1, "onTime": 0, "offTime": 0, "iterations": 1, "time": 4}));
        self.register("stripBlink", "Blink 3x", fade,
            json!({"steps": 1, "speedDelay": 1, "onTime": 100, "offTime": 100, "iterations": 3, "time": 3}));
        self.register("stripSparkle", "Sparkle", sparkle,
            json!({"chance": 1.0, "decay": 0.95, "speedDelay": 10, "iterations": 50, "time": 0}));
        self.register("stripMeteor", "Meteor Fall", meteor,
            json!({"meteorSize": 10, "decay": 0.75, "randomDecay": true, "speedDelay": 1, "time": 0}));
        self.register("stripScanner", "Scanner", larson_scanner,
            json!({"returnDelay": 50, "iterations": 3, "time": 0}));
        self.register("clear", "Turn Off", clear, json!({"time": 8}));
    }

    #[allow(dead_code)]
    pub fn effect_names(&self) -> Vec<String> {
        self.effects.keys().cloned().collect()
    }

    /// Assigns an effect to an event; 'none' unbinds (setEventEffect).
    pub fn set_event_effect(&self, event: &str, name: &str) -> bool {
        if !self.effects.contains_key(name) {
            return false;
        }
        self.events
            .lock()
            .unwrap()
            .insert(event.to_string(), name.to_string());
        true
    }

    pub fn get_event_effect(&self, event: &str) -> Option<String> {
        self.events.lock().unwrap().get(event).cloned()
    }

    /// Runs the effect bound to the event, stopping any running effect
    /// first (activateEffect).  Args override the effect's defaults.
    pub fn trigger(&self, event: &str, args: &Value) {
        let name = match self.events.lock().unwrap().get(event).cloned() {
            Some(n) => n,
            None => return,
        };
        let def = match self.effects.get(&name) {
            Some(d) => d,
            None => return,
        };
        // stop the running effect
        if let Some(prev) = self.running.lock().unwrap().take() {
            prev.set_terminate();
        }
        let mut merged = def.default_args.clone();
        if let (Some(m), Some(a)) = (merged.as_object_mut(), args.as_object()) {
            for (k, v) in a {
                m.insert(k.clone(), v.clone());
            }
        }
        let term = Terminator::default();
        let strip = self.strip.clone();
        let handler = def.handler;
        self.running.lock().unwrap().replace(term.clone());
        spawn_effect_task(move || {
            let mut strip_guard = strip.lock().unwrap();
            handler(&mut strip_guard, &merged, &term);
        });
        debug!("LED effect '{}' triggered by {}", name, event);
    }

    /// Runs the idle effect for the current race status, if configured
    /// (get_idle_fn chaining in Python).
    pub fn trigger_idle(&self, race_status: i64) {
        let event = match race_status {
            2 => IDLE_DONE,
            0 => IDLE_READY,
            1 => IDLE_RACING,
            _ => return,
        };
        if let Some(name) = self.get_event_effect(event) {
            if name != "none" {
                self.trigger(event, &json!({}));
            }
        }
    }

    /// manager.clear(): turn the strip off.
    #[allow(dead_code)]
    pub fn clear(&self) {
        self.trigger("manual", &json!({"time": 0, "preventIdle": true}));
    }

    /// Applies saved effect assignments (config LED.ledEffects) and the
    /// Python defaults (init_LED_effects).
    pub fn apply_assignments(&self, saved: Option<&Value>) {
        let defaults = json!({
            "raceStage": "stripColor2_1",
            "raceStart": "stripColorSolid",
            "raceFinish": "stripColor4_4",
            "raceStop": "stripColorSolid",
            "lapsClear": "clear",
            "crossingEnter": "stripSparkle",
            "crossingExit": "none",
            "raceLapRecorded": "none",
            "raceWin": "none",
            "messageStandard": "none",
            "messageInterrupt": "none",
            "startup": "rainbowCycle",
            "shutdown": "clear",
            IDLE_DONE: "clear",
            IDLE_READY: "clear",
            IDLE_RACING: "clear",
        });
        let mut effects = defaults.as_object().unwrap().clone();
        if let Some(saved_map) = saved.and_then(|v| v.as_object()) {
            for (k, v) in saved_map {
                if let Some(name) = v.as_str() {
                    effects.insert(k.clone(), json!(name));
                }
            }
        }
        self.set_event_effect("manualColor", "stripColor");
        for (event, name) in &effects {
            if let Some(name) = name.as_str() {
                self.set_event_effect(event, name);
            }
        }
    }
}
