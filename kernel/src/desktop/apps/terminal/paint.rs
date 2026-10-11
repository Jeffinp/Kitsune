//! Drawing the terminal window.

use super::state::font_px;
use crate::desktop::apps::terminal::term_grid;
use crate::desktop::kit::appui;
use crate::desktop::*;
use crate::text::{self, BODY, FOOTNOTE, Weight};
use kitsune_core::appart::Tool;
use kitsune_core::t;
use kitsune_core::termui::{self, Grid};

impl Desktop {
    // ---- drawing ----

    /// The terminal: a strip with the session's tab under the title bar, as many columns and
    /// rows as the window holds, the newest output at the bottom, the prompt in the accent
    /// colour, a selection, a block or bar cursor and an overlay scrollbar.
    pub(crate) fn draw_terminal(&self, c: &mut Canvas, r: Rect, t: &TermState, focused: bool) {
        let g = term_grid(r);
        let p = theme::pal();
        let body = Rect::new(r.x, r.y + TITLE_H, r.w, (r.h - TITLE_H).max(0));
        let saved = c.set_clip(
            body.intersection(&c.clip_rect())
                .unwrap_or(Rect::new(0, 0, 0, 0)),
        );
        ui::fill(c, body, theme::surface());
        self.draw_term_strip(c, &g, t);
        let v = t.term.view_in(g.cols, g.rows);
        let px = font_px();
        let (_, mono_lh) = text::mono_cell_px(px);
        let dy = (g.m.lh - mono_lh) / 2;
        let (path, sym) = termui::split_prompt(t.term.prompt());
        let path_n = path.chars().count();
        let sym_n = sym.chars().count();
        let acc = theme::accent();
        let (sel_col, sel_a) = if focused {
            theme::tint(0x59_00_00_00 | appui::rgb_of(acc))
        } else {
            theme::tint(if theme::dark() {
                0x40_80_80_88
            } else {
                0x40_70_70_78
            })
        };
        let clip = c.clip_rect();
        for (i, row) in v.rows.iter().enumerate() {
            let y = g.y + i as i32 * g.m.lh;
            // A frame that repaints only a part of the window (the caret's blink) skips the rows
            // that lie outside it: their text would be clipped away anyway.
            if y + g.m.lh <= clip.y || y >= clip.bottom() {
                continue;
            }
            let len = row.chars().count();
            if let Some((first, n)) = t.sel.and_then(|s| s.span(i, len)) {
                let rx = g.x + first as i32 * g.m.cw;
                c.blend_rect(Rect::new(rx, y, n as i32 * g.m.cw, g.m.lh), sel_col, sel_a);
            }
            // The prompt part of the live line: the path in the accent, the symbol quieter.
            let (a, b) = match v.live_first {
                Some(lf) if i >= lf => {
                    let start = (i - lf) * g.cols;
                    (
                        path_n.saturating_sub(start).min(len),
                        (path_n + sym_n).saturating_sub(start).min(len),
                    )
                }
                _ => (0, 0),
            };
            let cut = |n: usize| row.char_indices().nth(n).map_or(row.len(), |(b, _)| b);
            let (ia, ib) = (cut(a), cut(b));
            let mut x = g.x;
            for (seg, col) in [
                (&row[..ia], acc),
                (&row[ia..ib], theme::text_muted()),
                (&row[ib..], theme::text()),
            ] {
                if !seg.is_empty() {
                    text::draw_mono(c, x, y + dy, seg, px, col);
                }
                x += seg.chars().count() as i32 * g.m.cw;
            }
        }
        if let Some((row, col)) = v.cursor {
            let cell = g.cell_rect(row, col);
            let on_text = v.rows.get(row).is_some_and(|s| s.chars().count() > col);
            if !focused {
                c.stroke_rrect(cell, 2, Corner::Circle, acc, 220);
            } else {
                let alpha = appui::caret_alpha(t.last_input);
                if on_text {
                    // Between characters: a bar.
                    c.fill_rrect(
                        Rect::new(cell.x, cell.y + 2, 2, cell.h - 4),
                        1,
                        Corner::Circle,
                        acc,
                        alpha as u16,
                    );
                } else if alpha > 0 {
                    // At the end of the line: a block.
                    c.fill_rrect(
                        Rect::new(cell.x, cell.y + 2, cell.w, cell.h - 4),
                        2,
                        Corner::Circle,
                        acc,
                        (alpha * 200 / 256) as u16,
                    );
                }
            }
        }
        // The overlay scrollbar.
        let total = v.above + v.rows.len() + v.below;
        ui::overlay_scrollbar(
            c,
            g.track(),
            v.above,
            total,
            v.rows.len().max(1),
            t.scroll_fade.alpha(appui::now_ms()),
        );
        if t.term.is_running() {
            let msg = t!("term.running");
            let w = text::measure(msg, FOOTNOTE, Weight::Medium) + 24;
            let pill = Rect::new(r.right() - 14 - w, r.bottom() - 14 - 24, w, 24);
            ui::fill_token(c, pill, 12, p.control_bg);
            ui::stroke_token(c, pill, 12, p.control_border);
            text::draw_centered(c, pill, msg, FOOTNOTE, Weight::Medium, theme::text_muted());
        }
        c.restore_clip(saved);
    }

    /// The strip under the title bar: the session's tab with its folder.
    fn draw_term_strip(&self, c: &mut Canvas, g: &Grid, t: &TermState) {
        let p = theme::pal();
        let strip = g.strip;
        ui::fill(c, strip, theme::toolbar());
        appui::hairline(c, strip.x, strip.bottom() - 1, strip.w);
        let label = termui::tab_label(t.term.prompt());
        let room = (strip.w - 2 * termui::PAD - 40).clamp(40, 260);
        let label = text::ellipsize_middle(&label, BODY, Weight::Medium, room);
        let w = text::measure(&label, BODY, Weight::Medium) + 40;
        let tab = Rect::new(strip.x + termui::PAD - 4, strip.y + 5, w, strip.h - 11);
        ui::fill_token(c, tab, 7, p.control_bg);
        ui::stroke_token(c, tab, 7, p.control_border);
        appui::blit_tool_dim(
            c,
            Tool::Folder,
            tab.x + 10,
            tab.y + (tab.h - 14) / 2,
            14,
            appui::rgb_of(theme::accent()),
            256,
        );
        let ty = text::center_y(tab.y, tab.h, BODY, Weight::Medium);
        text::draw(
            c,
            tab.x + 30,
            ty,
            &label,
            BODY,
            Weight::Medium,
            theme::text(),
        );
    }
}

impl Desktop {
    /// The rectangle the caret of the terminal in window box `r` occupies, a little larger than the
    /// cell (the bar and the block are drawn inside it, anti-aliased).
    pub(crate) fn terminal_caret_rect(&self, r: Rect, t: &TermState) -> Option<Rect> {
        let g = term_grid(r);
        let (row, col) = t.term.view_in(g.cols, g.rows).cursor?;
        Some(g.cell_rect(row, col).inflated(3))
    }
}
