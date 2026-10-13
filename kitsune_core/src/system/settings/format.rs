//! The text form of [`Settings`]: total parsing of `key=value` lines and writing them back.

use super::*;

impl Settings {
    /// Build settings from text. Total (see the module docs).
    pub fn parse(text: &[u8]) -> Settings {
        let mut s = Settings::new();
        for line in text.split(|&b| b == b'\n') {
            let line = trim(line);
            if line.is_empty() || line[0] == b'#' {
                continue;
            }
            let Some(eq) = line.iter().position(|&b| b == b'=') else {
                continue;
            };
            let (key, val) = (trim(&line[..eq]), trim(&line[eq + 1..]));
            s.apply(key, val);
        }
        // `wallpaper=image` with no usable path means the default wallpaper.
        if s.wallpaper == WallpaperChoice::Image && s.image_len == 0 {
            s.wallpaper = WallpaperChoice::Preset(0);
        }
        // `clock=auto` means the language decides, whichever line came first.
        if s.clock_auto {
            s.clock24 = crate::i18n::locale::default_clock24(s.lang);
        }
        // The city must agree with the offset; otherwise the offset wins.
        let agrees = TIMEZONES
            .get(s.tz_city as usize)
            .is_some_and(|&(_, m)| m == s.tz_minutes);
        if !agrees {
            s.tz_city = city_for_offset(s.tz_minutes as i32);
        }
        s
    }

    /// Apply one `key=value`; anything invalid is ignored.
    pub(super) fn apply(&mut self, key: &[u8], val: &[u8]) {
        match key {
            b"wallpaper" => {
                if val.eq_ignore_ascii_case(b"image") {
                    // Only meaningful with a path; `wallpaper_path` may come later.
                    self.wallpaper = WallpaperChoice::Image;
                } else if let Some(n) = parse_u32(val).filter(|&n| (n as usize) < PRESETS.len()) {
                    self.wallpaper = WallpaperChoice::Preset(n as u8);
                }
            }
            b"wallpaper_path" => {
                self.set_image_path(val);
            }
            b"accent" => {
                if let Some(n) = parse_u32(val).filter(|&n| (n as usize) < ACCENTS.len()) {
                    self.accent = n as u8;
                }
            }
            b"clock" => match val {
                b"24" => self.set_clock24(true),
                b"12" => self.set_clock24(false),
                b"auto" => self.clock_auto = true,
                _ => {}
            },
            b"language" => {
                if let Some(l) = Lang::from_code(val) {
                    self.lang = l;
                }
            }
            b"tz" => {
                if let Some(m) = parse_i32(val).filter(|m| (TZ_MIN..=TZ_MAX).contains(m)) {
                    self.tz_minutes = m as i16;
                }
            }
            b"keyboard" => {
                if let Some(l) = Layout::from_name(val) {
                    self.layout = l;
                }
            }
            b"toasts" => match val {
                b"1" | b"on" => self.toasts = true,
                b"0" | b"off" => self.toasts = false,
                _ => {}
            },
            b"appearance" => {
                if let Some(a) = AppearanceSetting::from_name(val) {
                    self.appearance = a;
                }
            }
            b"reduce_motion" => match val {
                b"1" | b"on" => self.reduce_motion = true,
                b"0" | b"off" => self.reduce_motion = false,
                _ => {}
            },
            b"toast_secs" => {
                if let Some(n) = parse_u32(val)
                    .filter(|n| (TOAST_SECS_MIN as u32..=TOAST_SECS_MAX as u32).contains(n))
                {
                    self.toast_secs = n as u8;
                }
            }
            b"dock_zoom" => {
                if let Some(n) = parse_u32(val).filter(|&n| n <= DOCK_ZOOM_MAX as u32) {
                    self.dock_zoom = n as u8;
                }
            }
            b"tz_city" => {
                if let Some(n) = parse_u32(val).filter(|&n| (n as usize) < TIMEZONES.len()) {
                    self.tz_city = n as u8;
                }
            }
            b"terminal_font" => {
                if let Some(n) =
                    parse_u32(val).filter(|n| (FONT_MIN as u32..=FONT_MAX as u32).contains(n))
                {
                    self.terminal_font = n as u8;
                }
            }
            b"editor_font" => {
                if let Some(n) =
                    parse_u32(val).filter(|n| (FONT_MIN as u32..=FONT_MAX as u32).contains(n))
                {
                    self.editor_font = n as u8;
                }
            }
            b"auto_lock" => {
                if let Some(n) = parse_u32(val).filter(|&n| n <= 240) {
                    self.auto_lock_min = n as u8;
                }
            }
            b"adblock" => match val {
                b"1" | b"on" => self.ad_block = true,
                b"0" | b"off" => self.ad_block = false,
                _ => {}
            },
            b"volume" => {
                if let Some(n) = parse_u32(val).filter(|&n| n <= 100) {
                    self.volume = n as u8;
                }
            }
            b"mute" => match val {
                b"1" | b"on" => self.muted = true,
                b"0" | b"off" => self.muted = false,
                _ => {}
            },
            // `version` and anything unknown: nothing to do (forward compatible).
            _ => {}
        }
    }

    /// The text form (see the module docs).
    pub fn to_text(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(200);
        out.extend_from_slice(b"# Kitsune settings\n");
        push_kv(&mut out, b"version", VERSION);
        match self.wallpaper {
            WallpaperChoice::Preset(n) => push_kv(&mut out, b"wallpaper", n as u32),
            WallpaperChoice::Image => out.extend_from_slice(b"wallpaper=image\n"),
        }
        if self.image_len > 0 {
            out.extend_from_slice(b"wallpaper_path=");
            out.extend_from_slice(self.image_path());
            out.push(b'\n');
        }
        push_kv(&mut out, b"accent", self.accent as u32);
        if self.clock_auto {
            out.extend_from_slice(b"clock=auto\n");
        } else {
            push_kv(&mut out, b"clock", if self.clock24 { 24 } else { 12 });
        }
        out.extend_from_slice(b"language=");
        out.extend_from_slice(self.lang.code().as_bytes());
        out.push(b'\n');
        out.extend_from_slice(b"tz=");
        push_i32(&mut out, self.tz_minutes as i32);
        out.push(b'\n');
        out.extend_from_slice(b"keyboard=");
        out.extend_from_slice(self.layout.name().as_bytes());
        out.push(b'\n');
        push_kv(&mut out, b"toasts", self.toasts as u32);
        out.extend_from_slice(b"appearance=");
        out.extend_from_slice(self.appearance.name().as_bytes());
        out.push(b'\n');
        push_kv(&mut out, b"reduce_motion", self.reduce_motion as u32);
        push_kv(&mut out, b"toast_secs", self.toast_secs as u32);
        push_kv(&mut out, b"dock_zoom", self.dock_zoom as u32);
        push_kv(&mut out, b"adblock", self.ad_block as u32);
        if (self.tz_city as usize) < TIMEZONES.len() {
            push_kv(&mut out, b"tz_city", self.tz_city as u32);
        }
        if self.terminal_font != FONT_DEFAULT {
            push_kv(&mut out, b"terminal_font", self.terminal_font as u32);
        }
        if self.editor_font != FONT_DEFAULT {
            push_kv(&mut out, b"editor_font", self.editor_font as u32);
        }
        if self.auto_lock_min != 0 {
            push_kv(&mut out, b"auto_lock", self.auto_lock_min as u32);
        }
        if self.volume != VOLUME_DEFAULT {
            push_kv(&mut out, b"volume", self.volume as u32);
        }
        if self.muted {
            push_kv(&mut out, b"mute", 1);
        }

        out
    }
}
