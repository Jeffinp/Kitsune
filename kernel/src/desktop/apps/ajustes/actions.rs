//! What Ajustes does: apply and persist settings, load them at boot, set the wallpaper.

use super::builder::Probe;
use super::builder::Ui;
use super::pages::page;
use super::state::A_SEC;
use super::state::SECTIONS;
use super::state::pane_of;
use super::state::side_rect;
use super::state::*;
use crate::desktop::*;
use kitsune_core::i18n::{self, Arg};
use kitsune_core::settings::{Settings, WallpaperChoice};
use kitsune_core::sysif::SettingsStore;
use kitsune_core::tk;
use kitsune_core::wallpaper::{self};

impl Desktop {
    pub(super) fn settings_mut(&mut self, id: WindowId) -> Option<&mut SettingsState> {
        match self.app_mut(id) {
            Some(App::Settings(s)) => Some(s),
            _ => None,
        }
    }

    /// Make `new` the settings in effect: accent, keyboard layout, time zone, clock format,
    /// bar zoom and toasts apply at once, a changed wallpaper or accent asks the compositor
    /// to repaint the background, and the text form is stored through the `SettingsStore`
    /// (`/etc/kitsune.conf`).
    pub(crate) fn settings_apply(
        &mut self,
        new: Settings,
    ) -> Result<(), kitsune_core::sysif::SinkError> {
        self.settings_apply_live(new);
        VfsStore.save(&new.to_text())
    }

    /// Make `new` take effect without storing it (a slider being dragged: the file is
    /// written once, when the button is released).
    pub(super) fn settings_apply_live(&mut self, new: Settings) {
        let old = crate::settings::get();
        crate::settings::set(new);
        self.keymap.set_layout(new.layout);
        if new.wallpaper != old.wallpaper
            || new.image_path() != old.image_path()
            || new.accent != old.accent
        {
            self.bg_dirty = true;
        }
        if new.lang != old.lang {
            self.language_changed(old.lang);
        }
        if new.volume != old.volume {
            crate::audio::set_master(new.volume);
        }
        if new.muted != old.muted {
            crate::audio::set_muted(new.muted);
        }
        if new.appearance != old.appearance {
            // Re-resolve the look now (Auto follows the clock, the others are fixed).
            self.poll_appearance(crate::rtc::now().h);
        }
        // Everything on screen may change (clock text, accent colours).
        self.force_full = true;
    }

    /// Store the settings in effect (after a drag).
    pub(crate) fn settings_persist(&mut self) {
        let _ = VfsStore.save(&crate::settings::get().to_text());
    }

    /// Load the stored settings at boot (before the first wallpaper paint).
    pub fn load_settings(&mut self) {
        match VfsStore.load() {
            Some(text) => {
                let s = Settings::parse(&text);
                crate::settings::set(s);
                // The stored system volume holds from boot on.
                crate::audio::set_master(s.volume);
                crate::audio::set_muted(s.muted);
                self.keymap.set_layout(s.layout);
                crate::klog!(
                    Info,
                    "settings: loaded {} bytes from kitsune.conf",
                    text.len()
                );
            }
            None => crate::settings::set(Settings::default()),
        }
        // The first wallpaper is painted in the look the clock asks for.
        let hour = crate::rtc::now().h;
        crate::theme::set_appearance(crate::settings::get().appearance.resolve(hour));
        self.shell.last_hour = hour;
    }

    /// Use the image at `path` as the wallpaper (file manager, viewer). `Some(message)`
    /// is the reason it was refused.
    pub(crate) fn set_wallpaper_path(&mut self, path: &[u8]) -> Option<String> {
        self.try_wallpaper_path(path)
            .map(|(key, why)| i18n::tr_fmt(key, &[("why", Arg::Str(why.as_str()))]))
    }

    /// [`Self::set_wallpaper_path`] with the refusal as a catalog key and its `{why}`.
    fn try_wallpaper_path(&mut self, path: &[u8]) -> Option<(&'static str, String)> {
        let (w, h) = (self.sw as usize, self.sh as usize);
        let mut s = crate::settings::get();
        if !s.set_image_path(path) {
            return Some((tk!("settings.wp.err_path"), String::new()));
        }
        match read_path(path) {
            None => return Some((tk!("settings.wp.err_missing"), String::new())),
            Some(bytes) => {
                if let Err(e) = wallpaper::load(&bytes, w, h) {
                    return Some((
                        tk!("settings.wp.err_refused"),
                        String::from(i18n::tr(e.why_key())),
                    ));
                }
            }
        }
        s.wallpaper = WallpaperChoice::Image;
        match self.settings_apply(s) {
            Ok(()) => None,
            Err(_) => Some((tk!("settings.wp.err_unsaved"), String::new())),
        }
    }

    /// Did the wallpaper or accent change since the compositor last painted it?
    /// Consumed by the main loop, which repaints the cached background.
    pub fn take_bg_repaint(&mut self) -> bool {
        core::mem::take(&mut self.bg_dirty)
    }

    /// Apply the path typed on the wallpaper page.
    pub(super) fn settings_use_path(&mut self, id: WindowId) {
        let Some(st) = self.settings_mut(id) else {
            return;
        };
        let path = st.path[..st.path_len].to_vec();
        if path.is_empty() {
            return;
        }
        match self.try_wallpaper_path(&path) {
            Some((key, why)) => {
                if let Some(st) = self.settings_mut(id) {
                    st.msg = Some((key, why, true));
                }
            }
            None => {
                if let Some(st) = self.settings_mut(id) {
                    st.say(tk!("settings.wp.applied"), false);
                }
            }
        }
    }

    /// Run the page in probe mode at `(px, py)`: the control there and its rectangle.
    pub(super) fn settings_probe(
        &self,
        r: Rect,
        st: &SettingsState,
        px: i32,
        py: i32,
        only: Option<u32>,
    ) -> Option<(u32, Rect)> {
        // The sidebar.
        if only.is_none() {
            for i in 0..SECTIONS.len() {
                if side_rect(r, i).contains(px, py) {
                    return Some((A_SEC + i as u32, side_rect(r, i)));
                }
            }
        }
        let pane = pane_of(r);
        let mut ui = Ui::new(None, pane, st, Some(Probe { px, py, only }));
        page(&mut ui, self);
        ui.hit
    }
}
