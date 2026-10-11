//! Drawing the editor window: text, status bar and find bar. State is `EditorState`, the geometry
//! is `kitsune_core::editor2::ui` (the same rectangles the mouse handler uses).
//!
//! Layout: the text on the content colour with a quiet gutter of line numbers on the left, the
//! current line washed with the accent, optional indent guides, an overlay scrollbar, a slim
//! find / replace bar under the title (not a modal) and the status bar.

use crate::desktop::apps::editor::geometry::find_lay;
use crate::desktop::apps::editor::geometry::font_px;
use crate::desktop::apps::editor::geometry::geom;
use crate::desktop::apps::editor::state::EdHit;
use crate::desktop::apps::editor::state::EdModal;
use crate::desktop::kit::appui::{self, level};
use crate::desktop::kit::ui::ButtonKind;
use crate::desktop::*;
use crate::text::{self, BODY, FOOTNOTE, Weight};
use kitsune_core::appart::Tool;
use kitsune_core::editor2::ui::{self as eui, FindHit, FindLay, Lay};
use kitsune_core::editor2::{Notice, PromptKind, status_bar};
use kitsune_core::{t, tp};

pub(super) fn tertiary() -> Color {
    theme::solid(theme::pal().text_tertiary)
}

pub(super) fn secondary() -> Color {
    theme::solid(theme::pal().text_secondary)
}

/// Where an editor's content is clipped: the window minus its title bar.
fn body_of(r: Rect) -> Rect {
    Rect::new(r.x, r.y + TITLE_H, r.w, (r.h - TITLE_H).max(0))
}

impl Desktop {
    pub(crate) fn draw_editor(&self, c: &mut Canvas, r: Rect, e: &EditorState, focused: bool) {
        let lay = geom(r, &e.ed);
        let saved = c.set_clip(
            body_of(r)
                .intersection(&c.clip_rect())
                .unwrap_or(Rect::new(0, 0, 0, 0)),
        );
        self.draw_editor_text(c, &lay, e, focused);
        if let Some(fl) = find_lay(&lay, &e.ed) {
            self.draw_editor_bar(c, &lay, &fl, e);
        }
        self.draw_editor_status(c, &lay, e);
        match &e.modal {
            Some(EdModal::Close(ask)) => self.draw_close_sheet(c, r, e, ask),
            Some(EdModal::Open(p)) => self.draw_picker_sheet(c, r, e, p, t!("edit.open_title")),
            Some(EdModal::SaveAs { picker, .. }) => {
                self.draw_picker_sheet(c, r, e, picker, t!("edit.save_as_title"));
            }
            None => {}
        }
        c.restore_clip(saved);
    }

    fn hover_level(&self, e: &EditorState, hit: EdHit) -> u32 {
        if e.hover == Some(hit) {
            level(&e.hover_t)
        } else {
            0
        }
    }

    // ---- the text ----

    fn draw_editor_text(&self, c: &mut Canvas, lay: &Lay, e: &EditorState, focused: bool) {
        let ed = &e.ed;
        let p = theme::pal();
        let m = lay.m;
        let px = font_px();
        let (_, mono_lh) = text::mono_cell_px(px);
        let dy = (m.lh - mono_lh) / 2;
        let win = lay.window;
        ui::fill(c, lay.area, theme::surface());
        // The gutter: a quiet band with a hairline on its right.
        if lay.gutter_cols > 0 {
            let band = Rect::new(win.x, lay.area.y, lay.gutter.w, lay.area.h);
            ui::fill(c, band, theme::sidebar());
            ui::fill_token(
                c,
                Rect::new(band.right() - 1, band.y, 1, band.h),
                0,
                p.separator,
            );
        }
        let text_rect = Rect::new(
            lay.gutter.right(),
            lay.area.y,
            (win.right() - lay.gutter.right()).max(0),
            lay.area.h,
        );
        let cursor_row = ed.cursor_screen().map(|(r, _)| r);
        let cursor_line = ed.cursor().0;
        let acc = theme::accent();
        let text_cols = lay.cols - lay.gutter_cols;
        let tab = ed.config().tab_width;
        let left = ed.left_col();
        let sel_level = level(&e.sel_t);
        // Selection colour: the accent in the focused window, a grey one otherwise.
        let (sel_col, sel_a) = if focused {
            theme::tint(0x59_00_00_00 | appui::rgb_of(acc))
        } else {
            theme::tint(if theme::dark() {
                0x40_80_80_88
            } else {
                0x40_70_70_78
            })
        };
        let sel_a = (sel_a as u32 * sel_level / 256) as u16;
        let guide = p.separator;
        let clip = c.clip_rect();
        for (i, row) in ed.visible_rows().enumerate().take(lay.rows) {
            let y = lay.top + i as i32 * m.lh;
            // A frame that repaints only a part of the window (the caret's blink) skips the rows
            // that lie outside it: their text would be clipped away anyway.
            if y + m.lh <= clip.y || y >= clip.bottom() {
                continue;
            }
            // The line the caret is on.
            if cursor_row == Some(i) && !ed.has_selection() {
                let (col, a) = theme::tint(0x14_00_00_00 | appui::rgb_of(acc));
                c.blend_rect(Rect::new(text_rect.x, y, text_rect.w, m.lh), col, a);
            }
            let cells: Vec<_> = row.cells().take(text_cols).collect();
            // The selection, drawn under the glyphs.
            for (a, n) in eui::selection_runs(cells.iter().map(|c| c.selected)) {
                let rx = lay.text_x + a as i32 * m.cw;
                c.blend_rect(Rect::new(rx, y, n as i32 * m.cw, m.lh), sel_col, sel_a);
            }
            let line: String = cells.iter().map(|c| c.ch).collect();
            // Indent guides: one hairline at each indent level the line is past.
            if !row.continuation && line.chars().any(|ch| ch != ' ') {
                let lead = eui::leading_indent(&line, 1);
                for g in eui::indent_guides(left + lead, tab) {
                    if g >= left {
                        let gx = lay.text_x + (g - left) as i32 * m.cw;
                        ui::fill_token(c, Rect::new(gx, y, 1, m.lh), 0, guide);
                    }
                }
            }
            text::draw_mono(c, lay.text_x, y + dy, &line, px, theme::text());
            if let Some(n) = row.line_number {
                let s = alloc::format!("{n}");
                let w = s.chars().count() as i32 * m.cw;
                let col = if row.line == cursor_line {
                    secondary()
                } else {
                    tertiary()
                };
                text::draw_mono(c, lay.number_right() - w, y + dy, &s, px, col);
            }
        }
        // The caret: glides along its row, blinks eased.
        if focused
            && e.modal.is_none()
            && let Some((row, col)) = ed.cursor_screen()
            && row < lay.rows
            && col < lay.cols
        {
            let x = lay.text_x + appui::round(e.caret_x.value().max(0.0));
            let y = lay.top + row as i32 * m.lh;
            let alpha = appui::caret_alpha(e.last_input);
            if alpha > 0 {
                c.fill_rrect(
                    Rect::new(x, y + 2, 2, m.lh - 4),
                    1,
                    Corner::Circle,
                    acc,
                    alpha as u16,
                );
            }
        }
        // The overlay scrollbar.
        let track = Rect::new(win.right() - 12, lay.top, 12, lay.rows as i32 * m.lh);
        ui::overlay_scrollbar(
            c,
            track,
            ed.top_line(),
            ed.line_count(),
            lay.rows,
            e.scroll_fade.alpha(appui::now_ms()),
        );
    }

    // ---- the status bar ----

    fn draw_editor_status(&self, c: &mut Canvas, lay: &Lay, e: &EditorState) {
        let st = lay.status;
        let p = theme::pal();
        ui::fill(c, st, theme::toolbar());
        appui::hairline(c, st.x, st.y, st.w);
        let bar = status_bar(&e.ed.status());
        let ty = text::center_y(st.y, st.h, FOOTNOTE, Weight::Regular);
        let pad = eui::PAD;
        let pos_w = text::draw(
            c,
            st.x + pad,
            ty,
            &bar.position,
            FOOTNOTE,
            Weight::Regular,
            secondary(),
        );
        let mut right = st.right() - pad;
        let left_limit = st.x + pad + pos_w + 16;
        if let Some((m, err)) = &e.msg {
            // A result replaces the facts until the next key.
            let col = if *err { theme::danger() } else { theme::ok() };
            let room = (right - left_limit).max(0);
            let t = text::ellipsize(m, FOOTNOTE, Weight::Medium, room);
            let w = text::measure(&t, FOOTNOTE, Weight::Medium);
            text::draw(c, right - w, ty, &t, FOOTNOTE, Weight::Medium, col);
            return;
        }
        for (k, f) in bar.facts.iter().enumerate().rev() {
            let w = text::measure(f, FOOTNOTE, Weight::Regular);
            if right - w < left_limit {
                break;
            }
            text::draw(c, right - w, ty, f, FOOTNOTE, Weight::Regular, secondary());
            right -= w;
            if k > 0 {
                // A dot between two facts.
                let cx = right - 9;
                c.fill_rrect(
                    Rect::new(cx - 1, st.y + st.h / 2 - 1, 3, 3),
                    1,
                    Corner::Circle,
                    theme::solid(p.text_tertiary),
                    256,
                );
                right -= 18;
            }
        }
    }

    // ---- the find bar ----

    fn draw_editor_bar(&self, c: &mut Canvas, lay: &Lay, fl: &FindLay, e: &EditorState) {
        let Some(bar) = lay.bar else { return };
        let Some(p) = e.ed.prompt() else { return };
        ui::fill(c, bar, theme::toolbar());
        appui::hairline(c, bar.x, bar.bottom() - 1, bar.w);
        let caret = appui::caret_alpha(e.last_input);
        let goto = p.kind == PromptKind::Goto;
        let find_ph = match p.kind {
            PromptKind::Goto => t!("edit.find.goto"),
            _ => t!("edit.find.search"),
        };
        appui::field(
            c,
            fl.find,
            &appui::FieldText {
                text: p.text,
                caret: p.text.len(),
                selection: None,
            },
            find_ph,
            p.active == 0,
            caret,
            Some(if goto { Tool::ArrowDown } else { Tool::Search }),
            false,
        );
        if let Some(rep) = fl.replace {
            let t2 = p.text2.unwrap_or("");
            appui::field(
                c,
                rep,
                &appui::FieldText {
                    text: t2,
                    caret: t2.len(),
                    selection: None,
                },
                t!("edit.find.replace_with"),
                p.active == 1,
                caret,
                Some(Tool::Replace),
                false,
            );
        }
        let has_query = !p.text.is_empty();
        if !goto {
            let bar_btn = |c: &mut Canvas, r: Rect, tool: Tool, hit: FindHit, on: bool| {
                appui::tool_button(
                    c,
                    r,
                    tool,
                    on,
                    self.hover_level(e, EdHit::Bar(hit)),
                    false,
                    false,
                );
            };
            bar_btn(c, fl.prev, Tool::ChevronUp, FindHit::Prev, has_query);
            bar_btn(c, fl.next, Tool::ChevronDown, FindHit::Next, has_query);
            self.draw_case_toggle(c, fl.case, e, p.case_sensitive);
        }
        appui::tool_button(
            c,
            fl.close,
            Tool::Cancel,
            true,
            self.hover_level(e, EdHit::Bar(FindHit::Close)),
            false,
            false,
        );
        if let (Some(one), Some(all)) = (fl.replace_one, fl.replace_all) {
            for (r, label, hit) in [
                (one, t!("edit.replace"), FindHit::ReplaceOne),
                (all, t!("edit.all"), FindHit::ReplaceAll),
            ] {
                appui::sheet_button(
                    c,
                    r,
                    label,
                    ButtonKind::Secondary,
                    e.hover == Some(EdHit::Bar(hit)),
                    false,
                );
            }
        }
        // What the last search found.
        let (note, bad) = match p.notice {
            Notice::None => (String::new(), false),
            Notice::NotFound => (String::from(t!("edit.notice.not_found")), true),
            Notice::Wrapped => (String::from(t!("edit.notice.wrapped")), false),
            Notice::Replaced(n) => (tp!("edit.notice.replaced", n), false),
            Notice::InvalidLine => (String::from(t!("edit.notice.invalid_line")), true),
        };
        if !note.is_empty() && fl.notice.w > 8 {
            let col = if bad { theme::danger() } else { secondary() };
            let t = text::ellipsize(&note, FOOTNOTE, Weight::Regular, fl.notice.w);
            let ty = text::center_y(fl.notice.y, fl.notice.h, FOOTNOTE, Weight::Regular);
            text::draw(c, fl.notice.x, ty, &t, FOOTNOTE, Weight::Regular, col);
        }
    }

    /// The "Aa" toggle of the find bar: accent wash while case matters.
    fn draw_case_toggle(&self, c: &mut Canvas, r: Rect, e: &EditorState, on: bool) {
        let p = theme::pal();
        let hov = self.hover_level(e, EdHit::Bar(FindHit::Case));
        if on {
            let (col, a) = theme::tint(0x30_00_00_00 | appui::rgb_of(theme::accent()));
            c.fill_rrect(r, 6, Corner::Circle, col, a);
        } else if hov > 0 {
            let (col, a) = theme::tint(p.hover);
            c.fill_rrect(r, 6, Corner::Circle, col, (a as u32 * hov / 256) as u16);
        }
        text::draw_centered(
            c,
            r,
            "Aa",
            BODY,
            Weight::Semibold,
            if on { theme::accent() } else { secondary() },
        );
    }
}

impl Desktop {
    /// The rectangle the caret of the editor in window box `r` occupies (the bar, a little larger),
    /// where it is now; `None` when there is no caret to show.
    pub(crate) fn editor_caret_rect(&self, r: Rect, e: &EditorState) -> Option<Rect> {
        let lay = geom(r, &e.ed);
        let (row, col) = e.ed.cursor_screen()?;
        if row >= lay.rows || col >= lay.cols {
            return None;
        }
        let x = lay.text_x + appui::round(e.caret_x.value().max(0.0));
        let y = lay.top + row as i32 * lay.m.lh;
        Some(Rect::new(x, y, 2, lay.m.lh).inflated(3))
    }
}
