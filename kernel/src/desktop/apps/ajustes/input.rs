//! Keys, wheel, clicks, drag, hover and per-frame state of Ajustes.

use super::state::A_CLOCKFMT;
use super::state::A_DOWN;
use super::state::A_LANG;
use super::state::A_LAYOUT;
use super::state::A_SEC;
use super::state::A_SWATCH;
use super::state::A_THEME;
use super::state::A_TZ;
use super::state::A_TZLIST;
use super::state::A_UP;
use super::state::A_WALL;
use super::state::DOWN;
use super::state::Focus;
use super::state::SECTIONS;
use super::state::TZ_ROW_H;
use super::state::TZ_ROWS;
use super::state::pane_of;
use super::state::read_local;
use super::state::*;
use crate::desktop::*;
use kitsune_core::activity::Glide;
use kitsune_core::hw::rtc::{Field, local_to_utc};
use kitsune_core::i18n::Lang;
use kitsune_core::keymap::Layout;
use kitsune_core::settings::{
    AUTO_LOCK_CHOICES, DOCK_ZOOM_MAX, PATH_CAP, Settings, TIMEZONES, TOAST_SECS_MAX,
    TOAST_SECS_MIN, WallpaperChoice, search_timezones,
};
use kitsune_core::tk;
use kitsune_core::wallpaper::PRESETS;
use kitsune_core::widgets as wg;

impl Desktop {
    pub(crate) fn settings_key(&mut self, id: WindowId, key: Key) {
        let Some(rect) = self.wm.get(id).map(|w| w.rect) else {
            return;
        };
        let view_h = pane_of(rect).h;
        let Some(st) = self.settings_mut(id) else {
            return;
        };
        match st.focus {
            Focus::Path => {
                match key {
                    Key::Enter => {
                        self.settings_use_path(id);
                        return;
                    }
                    Key::Esc => st.focus = Focus::None,
                    Key::Backspace => st.path_len = st.path_len.saturating_sub(1),
                    Key::Char(b) if (0x21..0x7F).contains(&b) && st.path_len < PATH_CAP => {
                        st.path[st.path_len] = b;
                        st.path_len += 1;
                    }
                    _ => {}
                }
                return;
            }
            Focus::TzSearch => {
                match key {
                    Key::Esc => {
                        st.tz_query.clear();
                        st.focus = Focus::None;
                    }
                    Key::Backspace => {
                        st.tz_query.pop();
                    }
                    Key::Enter => {
                        let first = search_timezones(&st.tz_query).first().copied();
                        if let Some(i) = first {
                            let mut s = crate::settings::get();
                            s.set_city(i);
                            let _ = self.settings_apply(s);
                            if let Some(st) = self.settings_mut(id) {
                                st.edit = read_local();
                            }
                        }
                        return;
                    }
                    Key::Char(b) if (0x20..0x7F).contains(&b) && st.tz_query.len() < 24 => {
                        st.tz_query.push(b as char);
                    }
                    _ => return,
                }
                st.tz_scroll = Glide::at(0);
                return;
            }
            Focus::Kbd => {
                match key {
                    Key::Esc => st.focus = Focus::None,
                    Key::Backspace => {
                        st.kbd_test.pop();
                    }
                    Key::Char(b) if st.kbd_test.len() < 40 => st.kbd_test.push(b),
                    _ => {}
                }
                return;
            }
            Focus::User(k) => {
                st.users_key(k, key);
                return;
            }
            Focus::None => {}
        }
        let n = SECTIONS.len() as u8;
        let max = (st.content_h.get() - view_h).max(0);
        match key {
            Key::Esc => {
                self.request_close(id);
                return;
            }
            Key::Tab | Key::Down => st.select_section((st.section + 1) % n),
            Key::Up => st.select_section((st.section + n - 1) % n),
            Key::PageDown => st
                .scroll
                .set((st.scroll.target() + view_h - 40).clamp(0, max)),
            Key::PageUp => st
                .scroll
                .set((st.scroll.target() - view_h + 40).clamp(0, max)),
            Key::Home => st.scroll.set(0),
            Key::End => st.scroll.set(max),
            _ => return,
        }
        st.sb.touch(crate::desktop::shell::toasts::now_ms());
    }

    pub(crate) fn settings_wheel(&mut self, id: WindowId, notches: i32) {
        let Some(rect) = self.wm.get(id).map(|w| w.rect) else {
            return;
        };
        let (cx, cy) = (self.cursor_x, self.cursor_y);
        let view_h = pane_of(rect).h;
        let over_list = {
            let Some(App::Settings(st)) = self.wm.get(id).map(|w| &w.app.app) else {
                return;
            };
            self.settings_probe(rect, st, cx, cy, None)
                .is_some_and(|(h, _)| h == A_TZLIST || h >= A_TZ)
        };
        let Some(st) = self.settings_mut(id) else {
            return;
        };
        if over_list {
            let n = search_timezones(&st.tz_query).len() as i32;
            let max = (n * TZ_ROW_H - TZ_ROWS * TZ_ROW_H).max(0);
            st.tz_scroll
                .set((st.tz_scroll.target() + notches * TZ_ROW_H * 2).clamp(0, max));
        } else {
            let max = (st.content_h.get() - view_h).max(0);
            st.scroll
                .set((st.scroll.target() + notches * 56).clamp(0, max));
        }
        st.sb.touch(crate::desktop::shell::toasts::now_ms());
    }

    pub(crate) fn settings_click(&mut self, id: WindowId, rect: Rect, px: i32, py: i32) {
        let hit = {
            let Some(App::Settings(st)) = self.wm.get(id).map(|w| &w.app.app) else {
                return;
            };
            self.settings_probe(rect, st, px, py, None)
        };
        let Some(st) = self.settings_mut(id) else {
            return;
        };
        st.msg = None;
        st.focus = Focus::None;
        let Some((hid, hrect)) = hit else {
            return;
        };
        if (A_UFIELD..A_UFIELD + 0x100).contains(&hid) {
            st.users_click(hid);
            return;
        }
        let mut s = crate::settings::get();
        match hid {
            h if (A_SEC..A_SEC + 16).contains(&h) => {
                st.select_section((h - A_SEC) as u8);
                return;
            }
            h if (A_THEME..A_THEME + 3).contains(&h) => {
                use kitsune_core::style::AppearanceSetting as A;
                s.appearance = [A::Auto, A::Light, A::Dark][(h - A_THEME) as usize];
            }
            h if (A_SWATCH..A_SWATCH + 8).contains(&h) => s.accent = (h - A_SWATCH) as u8,
            h if (A_ULOCK..A_ULOCK + AUTO_LOCK_CHOICES.len() as u32).contains(&h) => {
                s.auto_lock_min = AUTO_LOCK_CHOICES[(h - A_ULOCK) as usize];
            }
            A_MOTION => s.reduce_motion = !s.reduce_motion,
            A_TOASTS => s.toasts = !s.toasts,
            A_TOAST_SECS | A_ZOOM => {
                st.drag = hid;
                self.settings_slider(id, hid, hrect, px, &mut s);
                self.settings_apply_live(s);
                self.drag = Some(Drag {
                    win: id,
                    mode: DragMode::Ui,
                });
                return;
            }
            h if (A_WALL..A_WALL + 8).contains(&h) => {
                let i = (h - A_WALL) as usize;
                if i < PRESETS.len() {
                    s.wallpaper = WallpaperChoice::Preset(i as u8);
                } else if st.path_len == 0 {
                    st.say(tk!("settings.wp.pick_hint"), false);
                    return;
                } else {
                    self.settings_use_path(id);
                    return;
                }
            }
            A_PATH => {
                st.focus = Focus::Path;
                return;
            }
            A_APPLY => {
                self.settings_use_path(id);
                return;
            }
            A_PICK => {
                st.say(tk!("settings.wp.pick_files"), false);
                self.launch(Kind::Files);
                return;
            }
            h if (A_LAYOUT..A_LAYOUT + 2).contains(&h) => {
                s.layout = if h == A_LAYOUT {
                    Layout::Us
                } else {
                    Layout::Abnt2
                };
            }
            A_KBD => {
                st.focus = Focus::Kbd;
                return;
            }
            A_CLOCK24 => s.set_clock24(!s.clock24),
            h if (A_LANG..A_LANG + Lang::ALL.len() as u32).contains(&h) => {
                s.set_language(Lang::ALL[(h - A_LANG) as usize]);
            }
            h if (A_CLOCKFMT..A_CLOCKFMT + 3).contains(&h) => match h - A_CLOCKFMT {
                0 => s.follow_language_clock(),
                1 => s.set_clock24(true),
                _ => s.set_clock24(false),
            },
            A_KBDUSE => s.layout = Layout::Abnt2,
            A_TZSEARCH => {
                st.focus = Focus::TzSearch;
                return;
            }
            A_TZLIST => return,
            h if h >= A_TZ && h < A_TZ + TIMEZONES.len() as u32 => {
                s.set_city((h - A_TZ) as u8);
                let _ = self.settings_apply(s);
                if let Some(st) = self.settings_mut(id) {
                    st.edit = read_local();
                }
                return;
            }
            h if (A_UP..A_UP + 6).contains(&h) || (A_DOWN..A_DOWN + 6).contains(&h) => {
                let (i, d) = if h >= A_DOWN {
                    ((h - A_DOWN) as usize, -1)
                } else {
                    ((h - A_UP) as usize, 1)
                };
                let f = [
                    Field::Day,
                    Field::Month,
                    Field::Year,
                    Field::Hour,
                    Field::Minute,
                    Field::Second,
                ][i];
                st.edit = st.edit.step(f, d);
                return;
            }
            A_READTIME => {
                st.edit = read_local();
                st.say(tk!("settings.time.read_ok"), false);
                return;
            }
            A_SETTIME => {
                let tzm = crate::rtc::tz_minutes();
                if st.edit.is_valid() {
                    crate::rtc::set_utc(&local_to_utc(st.edit, tzm));
                    st.say(tk!("settings.time.set_ok"), false);
                    crate::klog!(Info, "rtc: clock set by the user");
                } else {
                    st.say(tk!("settings.time.invalid"), true);
                }
                return;
            }
            A_REBOOT => {
                self.ask_power(false);
                return;
            }
            A_SHUTDOWN => {
                self.ask_power(true);
                return;
            }
            _ => return,
        }
        let _ = self.settings_apply(s);
    }

    /// Set the slider `hid` (whose rectangle is `rect`) from a pointer at `px`.
    fn settings_slider(&mut self, _id: WindowId, hid: u32, rect: Rect, px: i32, s: &mut Settings) {
        let track = rect.inflated(-6);
        match hid {
            A_TOAST_SECS => {
                s.toast_secs =
                    wg::slider_value(track, px, TOAST_SECS_MIN as i32, TOAST_SECS_MAX as i32) as u8;
            }
            A_ZOOM => {
                s.dock_zoom = wg::slider_value(track, px, 0, DOCK_ZOOM_MAX as i32) as u8;
            }
            _ => {}
        }
    }

    /// A slider drag in window `id` reached `(cx, cy)`.
    pub(crate) fn settings_drag(&mut self, id: WindowId, cx: i32, cy: i32) {
        let Some(rect) = self.wm.get(id).map(|w| w.rect) else {
            return;
        };
        let (drag, hit) = {
            let Some(App::Settings(st)) = self.wm.get(id).map(|w| &w.app.app) else {
                return;
            };
            if st.drag == 0 {
                return;
            }
            (
                st.drag,
                self.settings_probe(rect, st, cx, cy, Some(st.drag)),
            )
        };
        let Some((_, hrect)) = hit else {
            return;
        };
        let mut s = crate::settings::get();
        self.settings_slider(id, drag, hrect, cx, &mut s);
        if s != crate::settings::get() {
            self.settings_apply_live(s);
        }
    }

    /// Hover keys of every settings window for a pointer at `(cx, cy)`.
    pub(crate) fn settings_hover(&mut self, cx: i32, cy: i32, down: bool) -> bool {
        let top = if self.overlay_open() {
            None
        } else {
            self.topmost_at(cx, cy)
        };
        let mut changed = false;
        let mut dirty = Vec::new();
        for w in self.wm.windows() {
            let App::Settings(st) = &w.app.app else {
                continue;
            };
            if !w.shown() {
                continue;
            }
            let mut key = 0;
            if Some(w.id) == top && w.rect.body().contains(cx, cy) {
                key = self
                    .settings_probe(w.rect, st, cx, cy, None)
                    .map_or(0, |(h, _)| h);
                if key != 0 && down {
                    key |= DOWN;
                }
            }
            if key != st.hover.get() {
                st.hover.set(key);
                changed = true;
                dirty.push(self.window_box(w));
            }
        }
        for r in dirty {
            self.mark_dirty(r);
        }
        changed
    }

    pub(crate) fn settings_step(&mut self, dt_ms: u32) {
        if !self
            .wm
            .windows()
            .iter()
            .any(|w| matches!(w.app.app, App::Settings(_)))
        {
            return;
        }
        let ids: Vec<WindowId> = self
            .wm
            .windows()
            .iter()
            .filter(|w| matches!(w.app.app, App::Settings(_)))
            .map(|w| w.id)
            .collect();
        let s = crate::settings::get();
        for id in ids {
            if let Some(st) = self.settings_mut(id) {
                st.knobs[0].set(if s.reduce_motion { 256 } else { 0 });
                st.knobs[1].set(if s.toasts { 256 } else { 0 });
                st.knobs[2].set(if s.clock24 { 256 } else { 0 });
                for k in st.knobs.iter_mut() {
                    k.step(dt_ms, 60);
                }
                st.scroll.step(dt_ms, 80);
                st.tz_scroll.step(dt_ms, 80);
            }
        }
    }

    pub(crate) fn settings_busy_one(&self, w: &Win) -> bool {
        let App::Settings(st) = &w.app.app else {
            return false;
        };
        w.shown()
            && (st.knobs.iter().any(|k| k.moving())
                || st.scroll.moving()
                || st.tz_scroll.moving()
                || st.sb.active(crate::desktop::shell::toasts::now_ms())
                || {
                    let s = crate::settings::get();
                    [s.reduce_motion, s.toasts, s.clock24]
                        .iter()
                        .zip(&st.knobs)
                        .any(|(on, k)| k.target() != if *on { 256 } else { 0 })
                })
    }
}
