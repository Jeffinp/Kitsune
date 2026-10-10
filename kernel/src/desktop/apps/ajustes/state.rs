//! State of the Ajustes window: sections, focus, messages and the geometry of the window.

use crate::desktop::*;
use core::cell::Cell;
use kitsune_core::activity::Glide;
use kitsune_core::hw::rtc::{DateTime, utc_to_local};
use kitsune_core::i18n::{self, Arg};
use kitsune_core::iconart::Glyph;
use kitsune_core::secret::Secret;
use kitsune_core::settings::{PATH_CAP, search_timezones};
use kitsune_core::tk;
use kitsune_core::widgets as wg;

pub(super) const SIDE_W: i32 = 208;
pub(super) const ROW: i32 = 48;
/// Longest settings page text column.
pub(super) const COL_MAX: i32 = 600;

/// The sections, in order: name, glyph and the colour of its badge.
pub(super) const SECTIONS: [(&str, Glyph, u32); 11] = [
    (tk!("settings.sec.appearance"), Glyph::Sun, 0x5B5CF6),
    (tk!("settings.sec.wallpaper"), Glyph::Image, 0xEC4899),
    (tk!("settings.sec.dock"), Glyph::Dock, 0x14B8C4),
    (tk!("settings.sec.keyboard"), Glyph::Keyboard, 0x8E8E93),
    (tk!("settings.sec.time"), Glyph::Clock, 0xFB6F4B),
    (tk!("settings.sec.language"), Glyph::Wave, 0x30B0C7),
    (tk!("settings.sec.network"), Glyph::Network, 0x0A84FF),
    (tk!("settings.sec.disk"), Glyph::Disk, 0x8E8E93),
    (tk!("settings.sec.power"), Glyph::Power, 0xFF9F0A),
    (tk!("settings.sec.users"), Glyph::User, 0x5B5CF6),
    (tk!("settings.sec.about"), Glyph::Info, 0x8B90A0),
];
/// Section numbers other code opens.
pub(crate) const ABOUT: u8 = 10;
pub(super) const S_USERS: u8 = 9;
pub(super) const S_TIME: u8 = 4;
pub(super) const S_LANG: u8 = 5;

// Control ids (what a click or the pointer can hit).
pub(super) const A_SEC: u32 = 0x100;
pub(super) const A_THEME: u32 = 0x200;
pub(super) const A_SWATCH: u32 = 0x210;
pub(super) const A_MOTION: u32 = 0x220;
pub(super) const A_TOASTS: u32 = 0x221;
pub(super) const A_TOAST_SECS: u32 = 0x222;
pub(super) const A_WALL: u32 = 0x230;
pub(super) const A_PATH: u32 = 0x240;
pub(super) const A_APPLY: u32 = 0x241;
pub(super) const A_PICK: u32 = 0x242;
pub(super) const A_ZOOM: u32 = 0x250;
pub(super) const A_LAYOUT: u32 = 0x260;
pub(super) const A_KBD: u32 = 0x264;
pub(super) const A_CLOCK24: u32 = 0x270;
pub(super) const A_TZSEARCH: u32 = 0x271;
pub(super) const A_TZLIST: u32 = 0x272;
pub(super) const A_UP: u32 = 0x280;
pub(super) const A_DOWN: u32 = 0x290;
pub(super) const A_SETTIME: u32 = 0x2A0;
pub(super) const A_READTIME: u32 = 0x2A1;
pub(super) const A_REBOOT: u32 = 0x2B0;
pub(super) const A_SHUTDOWN: u32 = 0x2B1;
pub(super) const A_LANG: u32 = 0x2C0;
pub(super) const A_CLOCKFMT: u32 = 0x2D0;
pub(super) const A_KBDUSE: u32 = 0x2E0;
pub(super) const A_TZ: u32 = 0x300;
// The Users page.
pub(super) const A_UFIELD: u32 = 0x500;
pub(super) const A_UADMIN: u32 = 0x510;
pub(super) const A_UCREATE: u32 = 0x511;
pub(super) const A_UMINE: u32 = 0x512;
pub(super) const A_UMINE_SAVE: u32 = 0x513;
pub(super) const A_UMINE_CANCEL: u32 = 0x514;
pub(super) const A_USET: u32 = 0x520;
pub(super) const A_UDEL: u32 = 0x540;
pub(super) const A_UROLE: u32 = 0x560;
pub(super) const A_USET_SAVE: u32 = 0x580;
pub(super) const A_USET_CANCEL: u32 = 0x581;
pub(super) const A_UDEL_YES: u32 = 0x582;
pub(super) const A_UDEL_NO: u32 = 0x583;
/// The auto-lock choices (outside the 0x500 range the Users logic owns).
pub(super) const A_ULOCK: u32 = 0x600;
pub(super) const DOWN: u32 = 1 << 31;

/// Rows of the time-zone list shown at once.
pub(super) const TZ_ROWS: i32 = 7;
pub(super) const TZ_ROW_H: i32 = 28;

/// Which text field takes the keys.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Focus {
    None,
    Path,
    TzSearch,
    Kbd,
    /// A field of the Users page: 0 name, 1 full name, 2 password (new account); 3 current and
    /// 4 new password (own password); 5 an administrator's new password for somebody else.
    User(u8),
}

/// What the Users page is in the middle of.
#[derive(Default)]
pub(super) struct UsersForm {
    pub name: String,
    pub full: String,
    pub pw: Secret,
    pub admin: bool,
    /// Changing one's own password: the form is open, with the current and the new one.
    pub mine_open: bool,
    pub cur_pw: Secret,
    pub new_pw: Secret,
    /// An administrator setting somebody else's password.
    pub set_for: Option<String>,
    pub set_pw: Secret,
    /// Asking before removing an account.
    pub confirm_del: Option<String>,
}

/// Per-window state of the settings app.
pub(crate) struct SettingsState {
    pub(super) section: u8,
    pub(super) path: [u8; PATH_CAP],
    pub(super) path_len: usize,
    pub(super) focus: Focus,
    /// A message under a control: the catalog key, its `{why}` argument if any, and whether
    /// it is an error. Kept as a key so it follows the language.
    pub(super) msg: Option<(&'static str, String, bool)>,
    /// The date and time being edited (local time) on "Data e hora".
    pub(super) edit: DateTime,
    pub(super) kbd_test: Vec<u8>,
    pub(super) tz_query: String,
    pub(super) tz_scroll: Glide,
    /// Page scroll in pixels, and the overlay scrollbar.
    pub(super) scroll: Glide,
    pub(super) sb: wg::ScrollbarFade,
    pub(super) content_h: Cell<i32>,
    /// What the pointer is over (a control id, top bit = pressed).
    pub(super) hover: Cell<u32>,
    /// A slider being dragged (its control id).
    pub(super) drag: u32,
    /// Switch knobs: reduzir movimento, notificações, 24 horas.
    pub(super) knobs: [Glide; 3],
    pub(super) users: UsersForm,
}

impl SettingsState {
    /// Show section `i` (the system menu's "Sobre" opens the last one).
    pub(crate) fn select_section(&mut self, i: u8) {
        self.section = i.min(SECTIONS.len() as u8 - 1);
        self.focus = Focus::None;
        self.scroll = Glide::at(0);
        if self.section == S_TIME {
            self.edit = read_local();
            self.tz_scroll = Glide::at(tz_scroll_for(crate::settings::get().tz_city, ""));
        }
    }

    pub(crate) fn new() -> Self {
        let s = crate::settings::get();
        let mut st = Self {
            section: 0,
            path: [0; PATH_CAP],
            path_len: 0,
            focus: Focus::None,
            msg: None,
            edit: read_local(),
            kbd_test: Vec::new(),
            tz_query: String::new(),
            tz_scroll: Glide::at(0),
            scroll: Glide::at(0),
            sb: wg::ScrollbarFade::new(),
            content_h: Cell::new(0),
            hover: Cell::new(0),
            drag: 0,
            knobs: [
                Glide::at(if s.reduce_motion { 256 } else { 0 }),
                Glide::at(if s.toasts { 256 } else { 0 }),
                Glide::at(if s.clock24 { 256 } else { 0 }),
            ],
            users: UsersForm::default(),
        };
        let p = s.image_path();
        st.path[..p.len()].copy_from_slice(p);
        st.path_len = p.len();
        st
    }

    pub(super) fn say(&mut self, key: &'static str, error: bool) {
        self.msg = Some((key, String::new(), error));
    }

    /// The message in the language in effect.
    pub(super) fn message(&self) -> Option<(String, bool)> {
        self.msg
            .as_ref()
            .map(|(key, why, err)| (i18n::tr_fmt(key, &[("why", Arg::Str(why.as_str()))]), *err))
    }

    pub(crate) fn heap_bytes(&self) -> usize {
        self.kbd_test.capacity() + self.tz_query.capacity()
    }
}

/// Scroll position (pixels) that shows `city` in the middle of the zone list filtered by
/// `query`; 0 when it is not in the list.
fn tz_scroll_for(city: u8, query: &str) -> i32 {
    let found = search_timezones(query);
    let n = found.len() as i32;
    let Some(i) = found.iter().position(|&c| c == city) else {
        return 0;
    };
    let max = (n * TZ_ROW_H - TZ_ROWS * TZ_ROW_H).max(0);
    (i as i32 * TZ_ROW_H - (TZ_ROWS / 2) * TZ_ROW_H).clamp(0, max)
}

/// The RTC as local date and time.
pub(super) fn read_local() -> DateTime {
    utc_to_local(crate::rtc::read_utc(), crate::rtc::tz_minutes())
}

// ------------------------------------------------------------------ geometry of the window

/// The content pane and the sidebar rows.
pub(super) fn pane_of(r: Rect) -> Rect {
    let b = r.body();
    Rect::new(b.x + SIDE_W, b.y, b.w - SIDE_W, b.h)
}

pub(super) fn side_rect(r: Rect, i: usize) -> Rect {
    Rect::new(r.x + 10, r.body().y + 12 + i as i32 * 36, SIDE_W - 20, 32)
}
