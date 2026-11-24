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
    /// No strip attached; effects run silently
    None,
}

pub struct Strip {
    pub backend: StripBackend,
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
        }
    }

    pub fn num_pixels(&self) -> usize {
        match &self.backend {
            StripBackend::Ansi { pixels, .. } => pixels.len(),
            StripBackend::None => 0,
        }
    }

    pub fn set_pixel_color(&mut self, i: usize, color: u32) {
        if let StripBackend::Ansi { pixels, .. } = &mut self.backend {
            if let Some(px) = pixels.get_mut(i) {
                *px = color;
            }
        }
    }

    pub fn get_pixel_color(&self, i: usize) -> u32 {
        match &self.backend {
            StripBackend::Ansi { pixels, .. } => pixels.get(i).copied().unwrap_or(0),
            StripBackend::None => 0,
        }
    }

    pub fn show(&self) {
        if let StripBackend::Ansi { pixels, width, height } = &self.backend {
            // ANSI row rendering (ANSI_leds.py show()); one row per line
            let mut out = String::new();
            for row in 0..*height {
                let start = row * width;
                let end = (start + width).min(pixels.len());
                for &px in &pixels[start..end] {
                    let (r, g, b) = ((px >> 16) & 0xff, (px >> 8) & 0xff, px & 0xff);
                    // approximate the ANSI color classes with truecolor
                    out.push_str(&format!("\x1b[48;2;{};{};{}m \x1b[0m", r, g, b));
                }
                out.push_str("\x1b[K\n");
            }
            print!("\x1b[H{}", out);
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
    /// (get_pixel_interface): LED_COUNT/LED_ROWS.
    pub fn new(config: &Value) -> LedManager {
        let count = config
            .get("LED")
            .and_then(|l| l.get("LED_COUNT"))
            .and_then(|v| v.as_u64())
            .unwrap_or(0) as usize;
        let rows = config
            .get("LED")
            .and_then(|l| l.get("LED_ROWS"))
            .and_then(|v| v.as_u64())
            .unwrap_or(1) as usize;
        let strip = if count > 0 {
            tracing::info!("LED: locally emulated via ANSI terminal ({} pixels)", count);
            Strip::new(count, rows)
        } else {
            Strip { backend: StripBackend::None }
        };
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
        tokio::task::spawn_blocking(move || {
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
