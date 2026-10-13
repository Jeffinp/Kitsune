//! The toolbar: back / forward / reload, the omnibox and the buttons at its right.

use super::helpers::AMBER;
use super::helpers::argb;
use super::helpers::icon_button;
use crate::desktop::BrowserHover as H;
use crate::desktop::apps::browser::SecurityTone;
use crate::desktop::*;
use crate::text::{self, BODY, FOOTNOTE, Weight};
use kitsune_core::browser::tabs as tabs_model;
use kitsune_core::iconart::Glyph;
use kitsune_core::layout as geo;
use kitsune_core::t;

impl Desktop {
    // ---- toolbar ----

    pub(super) fn draw_toolbar(
        &self,
        c: &mut Canvas,
        ch: &geo::BrowserChrome,
        bs: &BrowserState,
        focused: bool,
    ) {
        let p = theme::pal();
        let t = bs.tabs.active();
        let hov = bs.hover;
        icon_button(
            c,
            ch.back,
            Glyph::ChevronLeft,
            t.browser.can_back(),
            hov == H::Back,
        );
        icon_button(
            c,
            ch.forward,
            Glyph::ChevronRight,
            t.browser.can_forward(),
            hov == H::Forward,
        );
        let loading = t.browser.is_loading();
        icon_button(
            c,
            ch.reload,
            if loading { Glyph::Close } else { Glyph::Reload },
            loading || !t.browser.is_home(),
            hov == H::Reload,
        );
        icon_button(
            c,
            ch.newtab,
            Glyph::Plus,
            bs.tabs.can_open(),
            hov == H::NewTab,
        );

        // The omnibox.
        let bar = ch.bar;
        let editing = focused && t.browser.bar_focus();
        let rad = bar.h / 2;
        ui::fill_token(c, bar, rad, p.field_bg);
        if editing {
            let acc = theme::accent();
            c.stroke_rrect(bar.inflated(2), rad + 2, Corner::Circle, acc, 90);
            c.stroke_rrect(bar, rad, Corner::Circle, acc, 256);
        } else {
            ui::stroke_token(c, bar, rad, p.control_border);
        }

        // The security indicator, or a plain glyph on the start page and the browser's own pages.
        if let Some((label, glyph, tone)) = bs.security_badge() {
            let (col, wash) = match tone {
                SecurityTone::Good => (theme::ok(), 40u16),
                SecurityTone::Warn => (
                    if theme::dark() {
                        Color::rgb(0xFB, 0xBF, 0x24)
                    } else {
                        Color::rgb(0xB4, 0x5F, 0x06)
                    },
                    44,
                ),
                SecurityTone::Bad => (theme::danger(), 44),
            };
            let s = ch.shield;
            let hot = hov == H::Shield || bs.popover;
            c.fill_rrect(
                s,
                s.h / 2,
                Corner::Circle,
                col,
                if hot { wash * 2 } else { wash },
            );
            ui::draw_glyph(c, glyph, s.x + 8, s.y + (s.h - 16) / 2, 16, argb(col, 255));
            text::draw(
                c,
                s.x + 8 + 16 + 6,
                text::center_y(s.y, s.h, FOOTNOTE, Weight::Medium),
                label,
                FOOTNOTE,
                Weight::Medium,
                col,
            );
        } else {
            ui::draw_glyph(
                c,
                if t.browser.is_home() {
                    Glyph::Search
                } else {
                    Glyph::Globe
                },
                bar.x + 12,
                bar.y + (bar.h - 16) / 2,
                16,
                argb(theme::solid(p.text_tertiary), 255),
            );
        }

        // The star: an outline, with a filled one growing over it for a favourite.
        let star = ch.star;
        let can_star = !t.browser.is_home();
        if can_star {
            let ink = if hov == H::Star {
                theme::solid(p.text)
            } else {
                theme::solid(p.text_tertiary)
            };
            let v = t.star.value().clamp(0.0, 1.0);
            ui::draw_glyph(
                c,
                Glyph::Star,
                star.x + (star.w - 16) / 2,
                star.y + (star.h - 16) / 2,
                16,
                argb(if v > 0.5 { AMBER } else { ink }, 255),
            );
            if v > 0.02 {
                // Overshoots a little and settles: 6 px growing to 18 px, then 16.
                let grow = if v < 0.7 {
                    v / 0.7 * 1.12
                } else {
                    1.12 - (v - 0.7) / 0.3 * 0.12
                };
                let size = ((16.0 * grow) as i32).max(4);
                ui::draw_glyph(
                    c,
                    Glyph::StarFill,
                    star.x + (star.w - size) / 2,
                    star.y + (star.h - size) / 2,
                    size,
                    argb(AMBER, (v * 255.0) as u32),
                );
            }
        }

        self.draw_blocker_button(c, ch, bs);

        // The address.
        let tx = ch.text_x();
        let right = ch.text_right(can_star);
        let room = (right - tx).max(8);
        let ty = text::center_y(bar.y, bar.h, BODY, Weight::Regular);
        let url = text::from_bytes(t.browser.url()).into_owned();
        let saved = c.set_clip(
            Rect::new(tx, bar.y, room, bar.h)
                .intersection(&c.clip_rect())
                .unwrap_or(Rect::new(0, 0, 0, 0)),
        );
        if url.is_empty() {
            text::draw(
                c,
                tx,
                ty,
                t!("web.omnibox"),
                BODY,
                Weight::Regular,
                theme::solid(p.text_tertiary),
            );
        } else if editing {
            // The tail of an address wider than the field, selected or with a caret.
            let mut start = 0usize;
            while start < url.len()
                && text::measure(&url[start..], BODY, Weight::Regular) > room - 2
            {
                start += url[start..].chars().next().map_or(1, char::len_utf8);
            }
            let shown = &url[start..];
            if t.browser.bar_selected() {
                let w = text::measure(shown, BODY, Weight::Regular);
                c.fill_rrect(
                    Rect::new(tx - 3, bar.y + 6, w + 6, bar.h - 12),
                    4,
                    Corner::Circle,
                    theme::accent(),
                    96,
                );
            }
            text::draw(
                c,
                tx,
                ty,
                shown,
                BODY,
                Weight::Regular,
                theme::solid(p.text),
            );
            let caret = t
                .browser
                .caret()
                .min(url.len())
                .saturating_sub(start)
                .min(shown.len());
            let cx = tx + text::measure(&shown[..caret.min(shown.len())], BODY, Weight::Regular);
            c.fill_rect(
                cx.max(0) as usize,
                (bar.y + 7).max(0) as usize,
                1,
                (bar.h - 14).max(0) as usize,
                theme::accent(),
            );
        } else {
            // At rest: the host stands out, the rest of the address is dim.
            let (hs, he) = tabs_model::host_range(&url);
            let shown = &url[hs..];
            let host = &url[hs..he];
            let hw = text::measure(host, BODY, Weight::Regular);
            text::draw(c, tx, ty, host, BODY, Weight::Regular, theme::solid(p.text));
            text::draw(
                c,
                tx + hw,
                ty,
                &shown[host.len()..],
                BODY,
                Weight::Regular,
                theme::solid(p.text_tertiary),
            );
        }
        c.restore_clip(saved);

        // The progress bar under the field.
        let a = t.load.alpha();
        if a > 0 {
            let pr = ch.progress;
            let w = (pr.w as i64 * i64::from(t.load.permille()) / 1000) as i32;
            if w > 0 {
                c.fill_rrect(
                    Rect::new(pr.x, pr.y, w.max(2), pr.h),
                    1,
                    Corner::Circle,
                    theme::accent(),
                    a as u16,
                );
            }
        }
    }
}
