//! The values the settings pages offer: ranges (notifications, dock zoom, auto-lock, text size) and the accent palette.

use super::*;

/// How long a notification stays, in seconds (the slider's range and the default).
pub const TOAST_SECS_MIN: u8 = 2;

pub const TOAST_SECS_MAX: u8 = 15;

pub const TOAST_SECS_DEFAULT: u8 = 4;

/// Strength of the app bar's magnification in percent (0 turns it off).
pub const DOCK_ZOOM_MAX: u8 = 100;

/// The choices for locking the screen after inactivity, in minutes (0 = never).
pub const AUTO_LOCK_CHOICES: [u8; 4] = [0, 5, 15, 30];

/// Accent palette (`0xRRGGBB`). Entry 0 is the default indigo and must stay equal
/// to `theme::ACCENT_DEFAULT`.
pub const ACCENTS: [u32; 8] = [
    0x5B5CF6, // indigo (default)
    0x14B8C4, // turquoise
    0xA855F7, // violet
    0xEC4899, // rose
    0xFB6F4B, // coral
    0xF59E0B, // amber
    0x22C55E, // green
    0x8B90A0, // graphite
];

/// Catalog keys of the names of the [`ACCENTS`] (look them up with [`accent_name`]).
pub const ACCENT_NAMES: [&str; 8] = [
    tk!("settings.accent.indigo"),
    tk!("settings.accent.turquoise"),
    tk!("settings.accent.violet"),
    tk!("settings.accent.rose"),
    tk!("settings.accent.coral"),
    tk!("settings.accent.amber"),
    tk!("settings.accent.green"),
    tk!("settings.accent.graphite"),
];

/// The name of accent `i` in the language in effect (empty out of range).
pub fn accent_name(i: usize) -> &'static str {
    ACCENT_NAMES.get(i).map_or("", |&key| crate::i18n::tr(key))
}

/// Smallest, largest and default size of the terminal and editor text, in pixels.
pub const FONT_MIN: u8 = 11;

pub const FONT_MAX: u8 = 24;

pub const FONT_DEFAULT: u8 = 15;

/// The size one zoom step away from `px` (`dir` > 0 larger, < 0 smaller), kept in range;
/// `dir == 0` is the default size.
pub fn font_step(px: u8, dir: i32) -> u8 {
    let px = px.clamp(FONT_MIN, FONT_MAX);
    match dir {
        0 => FONT_DEFAULT,
        d if d > 0 => (px + 1).min(FONT_MAX),
        _ => (px - 1).max(FONT_MIN),
    }
}
