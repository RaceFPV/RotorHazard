//! Frequency tables and small helpers ported from frequencies.py / RHUtils.

use serde_json::json;
use serde_json::Value;

/// The band/channel frequency table, in table order (order decides which
/// band/channel a shared frequency resolves to, matching frequencies.py).
pub const BAND_CHANNEL_FREQUENCIES: &[(&str, u8, u16)] = &[
    ("R", 1, 5658), ("R", 2, 5695), ("R", 3, 5732), ("R", 4, 5769),
    ("R", 5, 5806), ("R", 6, 5843), ("R", 7, 5880), ("R", 8, 5917),
    ("F", 1, 5740), ("F", 2, 5760), ("F", 3, 5780), ("F", 4, 5800),
    ("F", 5, 5820), ("F", 6, 5840), ("F", 7, 5860), ("F", 8, 5880),
    ("E", 1, 5705), ("E", 2, 5685), ("E", 3, 5665), ("E", 4, 5645),
    ("E", 5, 5885), ("E", 6, 5905), ("E", 7, 5925), ("E", 8, 5945),
    ("B", 1, 5733), ("B", 2, 5752), ("B", 3, 5771), ("B", 4, 5790),
    ("B", 5, 5809), ("B", 6, 5828), ("B", 7, 5847), ("B", 8, 5866),
    ("A", 1, 5865), ("A", 2, 5845), ("A", 3, 5825), ("A", 4, 5805),
    ("A", 5, 5785), ("A", 6, 5765), ("A", 7, 5745), ("A", 8, 5725),
    ("L", 1, 5362), ("L", 2, 5399), ("L", 3, 5436), ("L", 4, 5473),
    ("L", 5, 5510), ("L", 6, 5547), ("L", 7, 5584), ("L", 8, 5621),
    ("U", 0, 5300), ("U", 1, 5325), ("U", 2, 5348), ("U", 3, 5366),
    ("U", 4, 5384), ("U", 5, 5402), ("U", 6, 5420), ("U", 7, 5438),
    ("U", 8, 5456), ("U", 9, 5985),
    ("D", 1, 5660), ("D", 2, 5695), ("D", 3, 5735), ("D", 4, 5770),
    ("D", 5, 5805), ("D", 6, 5880), ("D", 7, 5914), ("D", 8, 5839),
    ("J", 1, 5695), ("J", 2, 5770), ("J", 3, 5880),
    ("S", 1, 5660), ("S", 2, 5695), ("S", 3, 5735), ("S", 4, 5770),
    ("S", 5, 5805), ("S", 6, 5839), ("S", 7, 5878), ("S", 8, 5914),
    ("O", 1, 5669), ("O", 2, 5705), ("O", 3, 5768), ("O", 4, 5804),
    ("O", 5, 5839), ("O", 6, 5876), ("O", 7, 5912),
    ("Q", 1, 5677), ("Q", 2, 5794), ("Q", 3, 5902),
];

/// Returns (band, channel) for a frequency; the first table entry wins for
/// shared frequencies; None for unknown/zero.
pub fn band_channel_for(frequency: u16) -> Option<(&'static str, u8)> {
    if frequency == 0 {
        return None;
    }
    BAND_CHANNEL_FREQUENCIES
        .iter()
        .find(|(_, _, f)| *f == frequency)
        .map(|(b, c, _)| (*b, *c))
}

pub fn frequency_for(band: &str, channel: u8) -> Option<u16> {
    BAND_CHANNEL_FREQUENCIES
        .iter()
        .find(|(b, c, _)| *b == band && *c == channel)
        .map(|(_, _, f)| *f)
}

/// Frequency presets, matching set_frequency_preset supported values.
pub fn preset_frequencies(preset: &str) -> Vec<(&'static str, u8, u16)> {
    match preset {
        "RB" => BAND_CHANNEL_FREQUENCIES[..8].to_vec(), // R1-R8, plus B handled below
        _ => BAND_CHANNEL_FREQUENCIES[..8].to_vec(),
    }
}

/// Random full-saturation color, matching RHUtils.hslToHex(random_hue, 100, 50).
pub fn random_hue_hex() -> String {
    let hue = (std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos() as u32)
        .unwrap_or(0)
        % 360) as f32;
    hsl_to_hex(hue, 100.0, 50.0)
}

/// HSL to hex, matching RHUtils.hslToHex (including the fixed grayscale path).
pub fn hsl_to_hex(h: f32, s: f32, l: f32) -> String {
    let h = h / 360.0;
    let s = s / 100.0;
    let l = l / 100.0;

    if s == 0.0 {
        let v = (l * 255.0).round() as u32;
        return format!("#{:02x}{:02x}{:02x}", v, v, v);
    }

    let hue2rgb = |p: f32, q: f32, mut t: f32| -> f32 {
        if t < 0.0 {
            t += 1.0;
        }
        if t > 1.0 {
            t -= 1.0;
        }
        if t < 1.0 / 6.0 {
            p + (q - p) * 6.0 * t
        } else if t < 1.0 / 2.0 {
            q
        } else if t < 2.0 / 3.0 {
            p + (q - p) * (2.0 / 3.0 - t) * 6.0
        } else {
            p
        }
    };

    let (r, g, b) = if l < 0.5 {
        let q = l * (1.0 + s);
        let p = 2.0 * l - q;
        (
            hue2rgb(p, q, h + 1.0 / 3.0),
            hue2rgb(p, q, h),
            hue2rgb(p, q, h - 1.0 / 3.0),
        )
    } else {
        let q = l + s - l * s;
        let p = 2.0 * l - q;
        (
            hue2rgb(p, q, h + 1.0 / 3.0),
            hue2rgb(p, q, h),
            hue2rgb(p, q, h - 1.0 / 3.0),
        )
    };

    let to8 = |v: f32| (v * 255.0).round() as u32;
    format!("#{:02x}{:02x}{:02x}", to8(r), to8(g), to8(b))
}

/// Preset frequency sets keyed the way the UI sends them.
pub fn frequency_preset(preset: &str) -> Vec<(String, u8, u16)> {
    let mk = |band: &str| -> Vec<(String, u8, u16)> {
        (1..=8)
            .map(|c| {
                let ch = c as u8;
                (
                    band.to_string(),
                    ch,
                    frequency_for(band, ch).unwrap_or(0),
                )
            })
            .collect()
    };
    match preset {
        "RB" => {
            let mut v = mk("R");
            v.extend(mk("B"));
            v
        }
        "R" => mk("R"),
        "B" => mk("B"),
        "F" => mk("F"),
        "E" => mk("E"),
        "A" => mk("A"),
        "L" => mk("L"),
        _ => mk("R"),
    }
}

pub fn _unused(_: Value) {
    let _ = json!(null);
}
