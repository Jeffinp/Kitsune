//! Methods of the browser window state: tab lookup, the tab strip, the security badge and busy flags.

use crate::desktop::*;
use crate::text;
use kitsune_core::t;

/// Wheel step: three lines of body text.
pub(super) const WHEEL_PX: i32 = 3 * 24;

impl BrowserState {
    /// The tab with id `id`.
    pub(crate) fn tab_by_id(&mut self, id: u32) -> Option<(usize, &mut TabData)> {
        let i = self.tabs.iter().position(|t| t.id == id)?;
        self.tabs.get_mut(i).map(|t| (i, t))
    }

    /// Width of the security indicator for the page on screen (0: none).
    pub(crate) fn shield_width(&self) -> i32 {
        match self.security_badge() {
            Some((label, _, _)) => {
                8 + 16 + 6 + text::measure(label, text::FOOTNOTE, text::Weight::Medium) + 10
            }
            None => 0,
        }
    }

    /// The indicator of the page on screen: label, glyph, kind. `None` for the start page,
    /// the browser's own pages and a load that has not finished.
    pub(crate) fn security_badge(
        &self,
    ) -> Option<(&'static str, kitsune_core::iconart::Glyph, SecurityTone)> {
        use kitsune_core::browser::Security;
        use kitsune_core::iconart::Glyph;
        let b = &self.tabs.active().browser;
        if b.is_home() || b.is_internal() {
            return None;
        }
        match b.security() {
            Security::HttpsVerified => {
                Some((t!("web.sec.secure"), Glyph::Lock, SecurityTone::Good))
            }
            Security::Http => Some((t!("web.sec.insecure"), Glyph::Warning, SecurityTone::Warn)),
            Security::HttpsInvalid => {
                Some((t!("web.sec.invalid"), Glyph::Warning, SecurityTone::Bad))
            }
            Security::None => None,
        }
    }

    /// Chrome geometry for window `r` right now (the strip height is animated).
    pub(crate) fn chrome(&self, r: Rect) -> BrowserChrome {
        BrowserChrome::with_strip(
            r,
            self.shield_width(),
            (self.strip_h.value().max(0.0) + 0.5) as i32,
        )
        .with_blocker(self.blocker_width())
    }

    /// The tab strip's entries as `(entry index, tab index)` for the real tabs.
    pub(crate) fn strip_tab_slots(&self) -> Vec<(usize, usize)> {
        let mut out = Vec::new();
        let mut ti = 0;
        for (i, e) in self.strip.iter().enumerate() {
            if e.id.is_some() {
                out.push((i, ti));
                ti += 1;
            }
        }
        out
    }

    /// Growth of each strip entry, 0..=256.
    pub(crate) fn strip_weights(&self) -> Vec<i32> {
        self.strip
            .iter()
            .map(|e| (e.weight.value().clamp(0.0, 1.0) * 256.0) as i32)
            .collect()
    }

    /// Does anything in the window still move (so it needs a frame every tick)?
    pub(crate) fn busy(&self) -> bool {
        let now = crate::desktop::shell::toasts::now_ms();
        self.tabs.iter().any(|t| {
            !t.scroll_spring.at_rest()
                || t.load.alpha() > 0
                || t.load.is_loading()
                || !t.star.finished()
                || t.scroll_bar.active(now)
        }) || !self.strip_h.finished()
            || self
                .strip
                .iter()
                .any(|e| !e.weight.finished() || e.id.is_none())
            || self.notice_flash.active()
            || self.zoom_flash.active()
    }

    /// Something changed that the painted page area depends on.
    pub(crate) fn touch(&mut self) {
        self.rev = self.rev.wrapping_add(1);
    }

    /// Show a line over the bottom of the page for a moment.
    pub(crate) fn say(&mut self, msg: &str) {
        self.notice = Some(String::from(msg));
        self.notice_flash.show(1.8);
    }
}

/// How alarming the security indicator is.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum SecurityTone {
    Good,
    Warn,
    Bad,
}
