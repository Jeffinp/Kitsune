//! System settings model and its `key=value` text form.
//!
//! [`Settings`] is plain data (`Copy`, no heap) that the kernel applies at boot
//! and the settings app edits. It is stored as text, one `key=value` per line:
//!
//! ```text
//! # Kitsune settings
//! version=1
//! wallpaper=2
//! wallpaper_path=fotos/praia.png
//! accent=3
//! clock=auto
//! language=pt-BR
//! tz=-180
//! keyboard=abnt2
//! toasts=1
//! appearance=auto
//! reduce_motion=0
//! toast_secs=4
//! dock_zoom=100
//! tz_city=17
//! terminal_font=17
//! editor_font=13
//! ```
//!
//! `language` is a tag of [`Lang`] (`pt-BR`, `en`; `pt` and `en-US` read the same). `clock` is `24`,
//! `12` or `auto` (the default: follow the language, 24 hours in Portuguese and 12 in English).
//!
//! `auto_lock` (minutes without input before the screen locks; 0 or absent = never; only for an account with a password).
//! `terminal_font` and `editor_font` (text size in pixels, [`FONT_MIN`] to [`FONT_MAX`]) are written
//! only when they differ from the default ([`FONT_DEFAULT`]).
//!
//! Parsing is total: [`Settings::parse`] never fails. Blank lines, `#`
//! comments, unknown keys and malformed or out-of-range values are skipped and
//! the field keeps its default, so a damaged or newer file still yields a usable
//! configuration. A file with no `version` is read as version 1.

use crate::hw::rtc::{TZ_MAX, TZ_MIN};
use crate::i18n::Lang;
use crate::system::keymap::Layout;
use crate::system::klog::FixedBuf;
use crate::tk;
use crate::ui::style::AppearanceSetting;
use crate::ui::wallpaper::PRESETS;
use alloc::vec::Vec;
use core::fmt::Write as _;

mod choices;
mod format;
mod text;
mod timezones;
pub use choices::*;
pub use text::*;
pub use timezones::*;

/// Version written by [`Settings::to_text`].
pub const VERSION: u32 = 1;
/// Longest wallpaper file path (bytes).
pub const PATH_CAP: usize = 40;
/// Name of the settings file in the FS (the FS v3 front maps it to
/// `/etc/kitsune.conf`).
pub const FILE_NAME: &[u8] = b"kitsune.conf";
/// Which wallpaper is active.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum WallpaperChoice {
    /// An entry of [`PRESETS`].
    Preset(u8),
    /// The user's image at `Settings::image_path`.
    Image,
}
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Settings {
    pub wallpaper: WallpaperChoice,
    image_path: [u8; PATH_CAP],
    image_len: u8,
    /// Index into [`ACCENTS`].
    pub accent: u8,
    /// 24-hour clock (otherwise 12-hour with AM/PM). While [`Settings::clock_auto`] is set this
    /// follows the language; change it with [`Settings::set_clock24`] so the choice sticks.
    pub clock24: bool,
    /// The clock format follows the language (until the user picks 24 or 12 hours).
    pub clock_auto: bool,
    /// The language of the interface.
    pub lang: Lang,
    /// Minutes east of UTC.
    pub tz_minutes: i16,
    pub layout: Layout,
    /// Show notification toasts.
    pub toasts: bool,
    /// Light, dark, or by the time of day.
    pub appearance: AppearanceSetting,
    /// Skip animations: every transition jumps to its end.
    pub reduce_motion: bool,
    /// Seconds a notification stays up.
    pub toast_secs: u8,
    /// Magnification of the app bar's icons, 0..=100 percent.
    pub dock_zoom: u8,
    /// The city picked for `tz_minutes` (index into [`TIMEZONES`]) or [`CITY_NONE`].
    pub tz_city: u8,
    /// Size of the terminal text in pixels ([`FONT_MIN`]..=[`FONT_MAX`]).
    pub terminal_font: u8,
    /// Size of the editor text in pixels.
    pub editor_font: u8,
    /// Minutes without input before the screen locks (0 = never).
    pub auto_lock_min: u8,
}
impl Default for Settings {
    /// The defaults: dynamic wallpaper, indigo accent, 24-hour clock, Brasilia time
    /// (UTC-3), US keyboard, toasts on, appearance by the time of day, motion on.
    fn default() -> Self {
        Self::new()
    }
}
impl Settings {
    pub const fn new() -> Self {
        Self {
            wallpaper: WallpaperChoice::Preset(0),
            image_path: [0; PATH_CAP],
            image_len: 0,
            accent: 0,
            clock24: true,
            clock_auto: true,
            lang: Lang::DEFAULT,
            tz_minutes: -180,
            layout: Layout::Us,
            toasts: true,
            appearance: AppearanceSetting::Auto,
            reduce_motion: false,
            toast_secs: TOAST_SECS_DEFAULT,
            dock_zoom: DOCK_ZOOM_MAX,
            tz_city: DEFAULT_CITY,
            terminal_font: FONT_DEFAULT,
            editor_font: FONT_DEFAULT,
            auto_lock_min: 0,
        }
    }

    /// Switch the interface language. While the clock format follows the language it changes
    /// with it (24 hours in Portuguese, 12 in English).
    pub fn set_language(&mut self, lang: Lang) {
        self.lang = lang;
        if self.clock_auto {
            self.clock24 = crate::i18n::locale::default_clock24(lang);
        }
    }

    /// Pick 24 or 12 hours for good (the format stops following the language).
    pub fn set_clock24(&mut self, on: bool) {
        self.clock24 = on;
        self.clock_auto = false;
    }

    /// Let the clock format follow the language again.
    pub fn follow_language_clock(&mut self) {
        self.clock_auto = true;
        self.clock24 = crate::i18n::locale::default_clock24(self.lang);
    }

    /// Set the time zone to listed city `i` (ignored for an index outside the list).
    pub fn set_city(&mut self, i: u8) {
        if let Some(&(_, m)) = TIMEZONES.get(i as usize) {
            self.tz_city = i;
            self.tz_minutes = m;
        }
    }

    pub fn image_path(&self) -> &[u8] {
        &self.image_path[..self.image_len as usize]
    }

    /// Set the wallpaper image path. Refused (`false`, nothing changed) when it
    /// is empty, too long, or has characters other than `A-Z a-z 0-9 . _ - /`.
    pub fn set_image_path(&mut self, path: &[u8]) -> bool {
        if !valid_path(path) {
            return false;
        }
        self.image_path = [0; PATH_CAP];
        self.image_path[..path.len()].copy_from_slice(path);
        self.image_len = path.len() as u8;
        true
    }

    /// The accent as `0xRRGGBB`.
    pub fn accent_rgb(&self) -> u32 {
        ACCENTS[(self.accent as usize).min(ACCENTS.len() - 1)]
    }
}
#[cfg(test)]
mod tests;
