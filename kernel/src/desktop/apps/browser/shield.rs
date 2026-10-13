//! The ad blocker's side of the browser window: the list of sites where it is off (a file), the
//! per-page blocker, the toggle in the address bar and the geometry of its popover.
//!
//! The rules and the counting are `kitsune_core::shields`; this file only connects them to the
//! settings (the global switch), to the window state and to the serial log (`shields: ...` lines,
//! which the QEMU scenario `w53-shields.sh` reads).

use crate::desktop::*;
use crate::text;
use kitsune_core::layout as geo;
use kitsune_core::shields::{PageShield, SiteList};
use kitsune_core::t;
use kitsune_core::widgets as wg;

/// Where the sites with the blocker off are kept: a hidden file in the signed-in person's home
/// folder (`/home/<name>/.shields`; plain `/home` is not writable by a signed-in account).
fn path() -> Vec<u8> {
    let mut p = crate::desktop::services::accounts::home();
    p.extend_from_slice(b"/.shields");
    p
}

/// Read the saved list (a missing or damaged file is an empty list: the blocker is on everywhere).
pub(super) fn load_sites() -> SiteList {
    let text = crate::desktop::services::vfs::read_file(&path()).unwrap_or_default();
    SiteList::parse(&text)
}

fn save_sites(list: &SiteList) {
    if crate::desktop::services::vfs::write_file(&path(), &list.to_text()).is_err() {
        crate::notify::notify_key(
            crate::klog::Level::Warn,
            kitsune_core::tk!("notify.shields_unsaved"),
            &[],
        );
    }
}

/// The blocker for the page just loaded from `nav_url`: on when the global switch is on and the
/// person did not turn it off for this site.
pub(super) fn page_shield(sites: &SiteList, nav_url: &[u8]) -> PageShield {
    let global = crate::settings::get().ad_block;
    PageShield::decide(nav_url, |host| global && !sites.is_off(host.as_bytes()))
}

/// Log the addresses `shield` blocked since it had `from` entries (`what` names the kind of
/// resource that asked).
pub(super) fn log_new(shield: &PageShield, from: usize, what: &str) {
    for (url, kind) in shield.blocked().iter().skip(from) {
        crate::serial_println!("shields: blocked {} {} {}", kind.name(), what, url);
    }
}

/// Log the page's totals (after the pictures are registered, when the count is complete).
pub(super) fn log_tally(shield: &PageShield) {
    let t = shield.tally();
    crate::serial_println!(
        "shields: page {} on={} ads={} trackers={} hidden={} total={}",
        shield.host(),
        shield.is_on(),
        t.ads,
        t.trackers,
        t.hidden,
        t.total()
    );
}

/// What the button in the address bar says about the page on screen.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum BlockerMode {
    /// Filtering this page.
    On,
    /// Off for this site (the person's choice).
    SiteOff,
    /// Off for everything, in the settings.
    GlobalOff,
}

impl BrowserState {
    /// Does the page on screen get a button? Not the start page, the browser's own pages, an error
    /// page or a load that has not produced a document yet.
    pub(crate) fn blocker_visible(&self) -> bool {
        let t = self.tabs.active();
        t.doc.is_some() && t.shield.has_host() && !t.browser.is_home() && !t.browser.is_internal()
    }

    pub(crate) fn blocker_mode(&self) -> BlockerMode {
        if !crate::settings::get().ad_block {
            BlockerMode::GlobalOff
        } else if self.tabs.active().shield.is_on() {
            BlockerMode::On
        } else {
            BlockerMode::SiteOff
        }
    }

    /// The number on the button: the tally, as text (empty for none or while off).
    pub(crate) fn blocker_count<'a>(&self, buf: &'a mut [u8; 3]) -> &'a str {
        let t = self.tabs.active();
        let n = if t.shield.is_on() {
            t.shield.tally().total()
        } else {
            0
        };
        match n {
            0 => "",
            1..=99 => {
                let mut len = 0;
                if n >= 10 {
                    buf[len] = b'0' + (n / 10) as u8;
                    len += 1;
                }
                buf[len] = b'0' + (n % 10) as u8;
                len += 1;
                core::str::from_utf8(&buf[..len]).unwrap_or("")
            }
            _ => {
                *buf = *b"99+";
                "99+"
            }
        }
    }

    /// Width of the button in the address bar (0: none).
    pub(crate) fn blocker_width(&self) -> i32 {
        if !self.blocker_visible() {
            return 0;
        }
        let mut buf = [0u8; 3];
        let n = self.blocker_count(&mut buf);
        if n.is_empty() {
            28
        } else {
            8 + 16 + 4 + text::measure(n, text::CAPTION, text::Weight::Semibold) + 8
        }
    }
}

/// Where the parts of the blocker's popover are (drawing and clicks use the same numbers).
pub(super) struct PopGeom {
    pub pop: Rect,
    /// The whole row of the per-site switch (a click anywhere on it toggles).
    pub row: Rect,
    pub switch: Rect,
    /// Top of the three count rows (only shown while the blocker is on).
    pub counts_y: i32,
    pub show_counts: bool,
    pub note: &'static str,
    pub note_y: i32,
    pub lines: Vec<(usize, usize)>,
}

pub(super) fn popover_geom(b: &BrowserState, ch: &geo::BrowserChrome, win: Rect) -> PopGeom {
    let mode = b.blocker_mode();
    let w = geo::browser_blocker_popover(ch.blocker, ch.bar, win, 0).w;
    let note = match mode {
        BlockerMode::On => t!("web.shield.note_on"),
        BlockerMode::SiteOff => t!("web.shield.note_off_site"),
        BlockerMode::GlobalOff => t!("web.shield.note_off_global"),
    };
    let lines = text::wrap(note, text::FOOTNOTE, text::Weight::Regular, w - 32, 6);
    let show_counts = mode == BlockerMode::On;
    let head = 16 + 36;
    let row_h = 40;
    let counts_h = if show_counts { 3 * 24 + 8 } else { 0 };
    let h = head + 12 + row_h + counts_h + 8 + lines.len() as i32 * 18 + 14;
    let pop = geo::browser_blocker_popover(ch.blocker, ch.bar, win, h);
    let row = Rect::new(pop.x + 8, pop.y + head + 12, pop.w - 16, row_h);
    let switch = wg::switch_rect(
        row.right() - 8 - wg::SWITCH_W,
        row.y + (row_h - wg::SWITCH_H) / 2,
    );
    let counts_y = row.bottom() + 4;
    let note_y = row.bottom() + counts_h + 8;
    PopGeom {
        pop,
        row,
        switch,
        counts_y,
        show_counts,
        note,
        note_y,
        lines,
    }
}

impl Desktop {
    /// The switch in the popover: turn the blocker off (or back on) for the site of the page on
    /// screen and load the page again, with or without the filter. Does nothing while the global
    /// switch in the settings is off.
    pub(crate) fn browser_toggle_site_shield(&mut self, id: WindowId) {
        if !crate::settings::get().ad_block {
            return;
        }
        let Some(b) = self.browser_state_mut(id) else {
            return;
        };
        let host = String::from(b.tabs.active().shield.host());
        if host.is_empty() {
            return;
        }
        let off = !b.sites.is_off(host.as_bytes());
        if b.sites.set_off(host.as_bytes(), off) {
            save_sites(&b.sites);
        } else {
            return;
        }
        crate::serial_println!(
            "shields: {} for {}",
            if off { "off" } else { "on" },
            host.as_str()
        );
        b.shield_pop = false;
        b.glass[1].clear();
        b.say(if off {
            t!("web.notice.shield_off")
        } else {
            t!("web.notice.shield_on")
        });
        self.browser_reload(id);
    }
}
