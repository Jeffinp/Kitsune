//! The ad blocker in the address bar: the shield with its count, and the popover with the
//! per-site switch. Geometry and state are in `browser/shield.rs`.

use super::helpers::argb;
use crate::desktop::BrowserHover as H;
use crate::desktop::apps::browser::shield::{BlockerMode, popover_geom};
use crate::desktop::kit::glass::panel;
use crate::desktop::*;
use crate::text::{self, BODY, CAPTION, FOOTNOTE, TITLE3, Weight};
use kitsune_core::iconart::Glyph;
use kitsune_core::layout as geo;
use kitsune_core::style::R_POPOVER;
use kitsune_core::t;

impl Desktop {
    /// The shield inside the omnibox, left of the star: green with the number of things left out
    /// while it filters, grey and crossed out while it is off.
    pub(super) fn draw_blocker_button(
        &self,
        c: &mut Canvas,
        ch: &geo::BrowserChrome,
        bs: &BrowserState,
    ) {
        let r = ch.blocker;
        if r.w == 0 {
            return;
        }
        let p = theme::pal();
        let mode = bs.blocker_mode();
        let mut buf = [0u8; 3];
        let count = bs.blocker_count(&mut buf);
        let hot = bs.hover == H::Blocker || bs.shield_pop;
        let (glyph, col) = match mode {
            BlockerMode::On if !count.is_empty() => (Glyph::Shield, theme::ok()),
            BlockerMode::On => (Glyph::Shield, theme::solid(p.text_tertiary)),
            _ => (Glyph::ShieldOff, theme::solid(p.text_tertiary)),
        };
        if !count.is_empty() {
            c.fill_rrect(r, r.h / 2, Corner::Circle, col, if hot { 80 } else { 40 });
        } else if hot {
            ui::fill_token(c, r, r.h / 2, p.hover);
        }
        let gx = if count.is_empty() {
            r.x + (r.w - 16) / 2
        } else {
            r.x + 8
        };
        ui::draw_glyph(c, glyph, gx, r.y + (r.h - 16) / 2, 16, argb(col, 255));
        if !count.is_empty() {
            text::draw(
                c,
                gx + 16 + 4,
                text::center_y(r.y, r.h, CAPTION, Weight::Semibold),
                count,
                CAPTION,
                Weight::Semibold,
                col,
            );
        }
    }

    /// The popover under the shield: the per-site switch, what was left out of this page and a
    /// sentence on what the blocker does.
    pub(super) fn draw_blocker_popover(
        &self,
        c: &mut Canvas,
        win: Rect,
        ch: &geo::BrowserChrome,
        bs: &BrowserState,
    ) {
        if ch.blocker.w == 0 {
            return;
        }
        let p = theme::pal();
        let t = bs.tabs.active();
        let mode = bs.blocker_mode();
        let g = popover_geom(bs, ch, win);
        let pr = g.pop;
        panel(
            c,
            pr,
            R_POPOVER,
            &bs.glass[1],
            10,
            p.menu_tint,
            p.separator,
            Shadow {
                blur: 14,
                dy: 8,
                alpha: 80,
            },
            256,
        );
        let col = if mode == BlockerMode::On {
            theme::ok()
        } else {
            theme::solid(p.text_tertiary)
        };
        let icon = Rect::new(pr.x + 16, pr.y + 16, 36, 36);
        c.fill_rrect(icon, 18, Corner::Circle, col, 44);
        ui::draw_glyph(
            c,
            if mode == BlockerMode::On {
                Glyph::Shield
            } else {
                Glyph::ShieldOff
            },
            icon.x + 10,
            icon.y + 10,
            16,
            argb(col, 255),
        );
        text::draw(
            c,
            icon.right() + 12,
            pr.y + 18,
            t!("web.shield.title"),
            TITLE3,
            Weight::Semibold,
            theme::solid(p.text),
        );
        text::draw_ellipsis(
            c,
            icon.right() + 12,
            pr.y + 38,
            pr.w - 16 - 36 - 12 - 16,
            t.shield.host(),
            FOOTNOTE,
            Weight::Regular,
            theme::solid(p.text_secondary),
        );
        let (sc, sa) = theme::tint(p.separator);
        c.blend_rect(
            Rect::new(pr.x + 16, icon.bottom() + 6, pr.w - 32, 1),
            sc,
            sa,
        );

        // The per-site switch (off and dim while the global switch in the settings is off).
        let usable = mode != BlockerMode::GlobalOff;
        if usable && bs.hover == H::BlockerSwitch {
            ui::fill_token(c, g.row, 8, p.hover);
        }
        let ink = if usable { p.text } else { p.text_tertiary };
        text::draw(
            c,
            g.row.x + 8,
            text::center_y(g.row.y, g.row.h, BODY, Weight::Regular),
            t!("web.shield.site_switch"),
            BODY,
            Weight::Regular,
            theme::solid(ink),
        );
        ui::switch(
            c,
            g.switch,
            if mode == BlockerMode::On { 256 } else { 0 },
            usable,
        );

        if g.show_counts {
            let tally = t.shield.tally();
            let rows = [
                (t!("web.shield.ads"), tally.ads),
                (t!("web.shield.trackers"), tally.trackers),
                (t!("web.shield.hidden"), tally.hidden),
            ];
            for (i, (label, n)) in rows.iter().enumerate() {
                let y = g.counts_y + i as i32 * 24;
                text::draw(
                    c,
                    pr.x + 16,
                    text::center_y(y, 24, FOOTNOTE, Weight::Regular),
                    label,
                    FOOTNOTE,
                    Weight::Regular,
                    theme::solid(p.text_secondary),
                );
                let mut num = alloc::string::String::new();
                {
                    use core::fmt::Write as _;
                    let _ = write!(num, "{n}");
                }
                text::draw_right(
                    c,
                    Rect::new(pr.x + 16, y, pr.w - 32, 24),
                    &num,
                    FOOTNOTE,
                    Weight::Medium,
                    theme::solid(p.text),
                );
            }
        }
        for (i, rg) in g.lines.iter().enumerate() {
            text::draw(
                c,
                pr.x + 16,
                g.note_y + i as i32 * 18,
                &g.note[rg.0..rg.1],
                FOOTNOTE,
                Weight::Regular,
                theme::solid(p.text_secondary),
            );
        }
    }
}
