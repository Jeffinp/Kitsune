//! Painting a laid-out page: boxes, pictures, text, form controls and the selection.

use super::cache::page_key;
use super::metrics::KernelMetrics;
use super::metrics::darker;
use super::metrics::draw_run;
use super::metrics::face_of;
use super::metrics::rgb;
use crate::desktop::*;
use crate::text::{self, Weight};
use kitsune_core::web::{
    Cmd as WebCmd, DECO_STRIKE, DECO_UNDERLINE, Font, Page,
    form::{FieldKind, caret_row, wrap_rows},
    textops::Span,
};

/// Copy `img` into the box `(x, y, w, h)` (screen coordinates), clipped to `clip`. A picture whose
/// size is not the box's (the layout moved on, an image just arrived) is sampled nearest-neighbour.
fn paint_picture(
    c: &mut Canvas,
    img: &kitsune_core::image::Image,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    clip: Rect,
) {
    let (iw, ih) = (img.width() as i64, img.height() as i64);
    if iw == 0 || ih == 0 {
        return;
    }
    let (w, h) = (i64::from(w.max(1)), i64::from(h.max(1)));
    let px = img.pixels();
    let y0 = y.max(clip.y);
    let y1 = (y + h as i32).min(clip.bottom());
    let x0 = x.max(clip.x);
    let x1 = (x + w as i32).min(clip.right());
    for sy in y0..y1 {
        let iy = ((i64::from(sy - y) * ih / h).min(ih - 1)) as usize;
        let row = &px[iy * iw as usize..(iy + 1) * iw as usize];
        for sx in x0..x1 {
            let ix = ((i64::from(sx - x) * iw / w).min(iw - 1)) as usize;
            let p = row[ix];
            c.put(
                sx as usize,
                sy as usize,
                Color::rgb((p >> 16) as u8, (p >> 8) as u8, p as u8),
            );
        }
    }
}

/// Stroke a border of per-side widths inside `r`.
fn paint_border(c: &mut Canvas, r: Rect, widths: [i32; 4], radius: i32, color: Color) {
    let [t, rt, b, l] = widths;
    let radius = radius.clamp(0, 48).min(r.w / 2).min(r.h / 2);
    if radius > 0 && t == rt && rt == b && b == l {
        for k in 0..t.max(1) {
            c.stroke_rrect(
                r.inflated(-k),
                (radius - k).max(0),
                Corner::Circle,
                color,
                256,
            );
        }
        return;
    }
    let fill = |c: &mut Canvas, rr: Rect| {
        if rr.w > 0 && rr.h > 0 {
            c.fill_rect(
                rr.x.max(0) as usize,
                rr.y.max(0) as usize,
                rr.w as usize,
                rr.h as usize,
                color,
            );
        }
    };
    fill(c, Rect::new(r.x, r.y, r.w, t));
    fill(c, Rect::new(r.x, r.bottom() - b, r.w, b));
    fill(c, Rect::new(r.x, r.y + t, l, r.h - t - b));
    fill(c, Rect::new(r.right() - rt, r.y + t, rt, r.h - t - b));
}

impl Desktop {
    /// Rasterize a `web` engine display list into the content box, offset by the
    /// current scroll and clipped to the visible area. The finished area is kept: the next
    /// paint of the same page at the same size copies it back, and a scroll moves it and
    /// paints only the rows that came into view.
    pub(crate) fn paint_web_page(
        &self,
        c: &mut Canvas,
        page: &Page,
        content: Rect,
        scroll: i32,
        bs: &BrowserState,
    ) {
        // Only a video's new picture to paint: the video paints all of its box itself.
        if super::super::media::video_covers(page, content, scroll, c.clip_rect()) {
            return;
        }
        let key = page_key(bs, content);
        let info = c.fb_info();
        let whole = content.x >= 0
            && content.y >= 0
            && content.right() <= info.width as i32
            && content.bottom() <= info.height as i32
            && c.clip_rect().intersection(&content) == Some(content);
        let bpp = info.bytes_per_pixel;
        let (w, h) = (content.w.max(0) as usize, content.h.max(0) as usize);
        if !whole || w == 0 || h == 0 || bpp == 0 {
            self.paint_page_range(c, page, content, scroll, bs, 0, content.h);
            return;
        }
        let mut cache = bs.cache.borrow_mut();
        let usable =
            cache.valid && cache.key == key && cache.w == w && cache.h == h && cache.bpp == bpp;
        let stride = info.stride;
        if usable && cache.scroll == scroll {
            cache.restore(c.buffer_mut(), stride, content);
            return;
        }
        if usable && (scroll - cache.scroll).unsigned_abs() as usize * 2 < h {
            // Scrolled a little: keep the rows that are still in view, paint the new ones.
            let dy = scroll - cache.scroll; // > 0: the page moved up
            let keep = h as i32 - dy.abs();
            let (strip_y, keep_y, keep_from) = if dy > 0 { (keep, 0, dy) } else { (0, -dy, 0) };
            cache.restore_rows(c.buffer_mut(), stride, content, keep_y, keep_from, keep);
            cache.shift(dy);
            drop(cache);
            let (y0, y1) = if dy > 0 {
                (strip_y, h as i32)
            } else {
                (0, -dy)
            };
            self.paint_page_range(c, page, content, scroll, bs, y0, y1);
            let mut cache = bs.cache.borrow_mut();
            cache.store_rows(c.buffer_mut(), stride, content, y0, y1);
            cache.scroll = scroll;
            return;
        }
        drop(cache);
        self.paint_page_range(c, page, content, scroll, bs, 0, content.h);
        let mut cache = bs.cache.borrow_mut();
        cache.begin(key, w, h, bpp, scroll);
        cache.store_rows(c.buffer_mut(), stride, content, 0, content.h);
    }

    /// Paint rows `y0..y1` (page-area coordinates) of the page.
    #[allow(clippy::too_many_arguments)]
    fn paint_page_range(
        &self,
        c: &mut Canvas,
        page: &Page,
        content: Rect,
        scroll: i32,
        bs: &BrowserState,
        y0: i32,
        y1: i32,
    ) {
        let band = Rect::new(content.x, content.y + y0, content.w, (y1 - y0).max(0));
        let Some(clip) = band.intersection(&c.clip_rect()) else {
            return;
        };
        let saved = c.set_clip(clip);
        let m = KernelMetrics;
        let (top, bottom) = (scroll + y0, scroll + y1);
        let (ox, oy) = (content.x, content.y - scroll);
        // The page's own background, whatever the system appearance.
        c.fill_rect(
            band.x.max(0) as usize,
            band.y.max(0) as usize,
            band.w.max(0) as usize,
            band.h.max(0) as usize,
            rgb(page.background),
        );
        let visible = |y: i32, h: i32| y + h >= top - 2 && y <= bottom + 2;

        // Boxes and pictures first, in document order.
        for cmd in &page.cmds {
            match cmd {
                WebCmd::Rect {
                    x,
                    y,
                    w,
                    h,
                    color,
                    radius,
                } if visible(*y, *h) => {
                    let r = Rect::new(ox + x, oy + y, *w, *h);
                    if *radius > 0 {
                        let rad = (*radius).clamp(0, 48).min(r.w / 2).min(r.h / 2);
                        c.fill_rrect(r, rad, Corner::Circle, rgb(*color), 256);
                    } else {
                        c.fill_rect(
                            r.x.max(0) as usize,
                            r.y.max(0) as usize,
                            r.w.max(0) as usize,
                            r.h.max(0) as usize,
                            rgb(*color),
                        );
                    }
                }
                WebCmd::Image { x, y, w, h, idx } if visible(*y, *h) => {
                    let key = bs.img_keys.get(*idx).and_then(|k| k.as_deref());
                    if let Some(img) = key.and_then(|k| bs.images.image(k)) {
                        paint_picture(c, img, ox + *x, oy + *y, *w, *h, clip);
                    }
                }
                _ => {}
            }
        }

        // Under the text: find matches, then the mouse selection.
        let accent = theme::accent();
        let mark = |c: &mut Canvas, s: &Span, col: Color, alpha: u16| {
            if !visible(s.y, s.h) {
                return;
            }
            c.fill_rrect(
                Rect::new(ox + s.x - 1, oy + s.y, s.w + 2, s.h),
                3,
                Corner::Circle,
                col,
                alpha,
            );
        };
        for s in bs.find.other_spans() {
            mark(c, s, Color::rgb(0xFF, 0xD9, 0x5E), 200);
        }
        for s in bs.find.current_spans() {
            mark(c, s, Color::rgb(0xFF, 0x9F, 0x2E), 256);
        }
        if let Some(sel) = &bs.sel {
            for s in &page.selection_spans(sel, &m) {
                mark(c, s, accent, 92);
            }
        }

        // Text.
        for cmd in &page.cmds {
            let WebCmd::Text {
                x,
                y,
                w,
                h,
                text: t,
                color,
                font,
                deco,
                link,
            } = cmd
            else {
                continue;
            };
            if !visible(*y, *h) || *x > content.w {
                continue;
            }
            let hovered = link.is_some() && bs.hover_link == link.map(|l| l as usize);
            let col = if hovered {
                darker(rgb(*color), 70)
            } else {
                rgb(*color)
            };
            let (sx, sy) = (ox + *x, oy + *y);
            draw_run(c, sx, sy, t, *font, col);
            let underline = deco & DECO_UNDERLINE != 0 || hovered;
            if underline || deco & DECO_STRIKE != 0 {
                let (px, wt) = face_of(*font);
                let v = text::vmetrics(px, wt);
                let thick = (px as i32 / 14).max(1);
                if underline {
                    c.fill_rect(
                        sx.max(0) as usize,
                        (sy + v.ascent + 1 + px as i32 / 12).max(0) as usize,
                        (*w).max(0) as usize,
                        thick as usize,
                        col,
                    );
                }
                if deco & DECO_STRIKE != 0 {
                    c.fill_rect(
                        sx.max(0) as usize,
                        (sy + v.ascent - v.x_height / 2).max(0) as usize,
                        (*w).max(0) as usize,
                        thick as usize,
                        col,
                    );
                }
            }
        }

        // Borders over the content of their boxes.
        for cmd in &page.cmds {
            if let WebCmd::Border {
                x,
                y,
                w,
                h,
                widths,
                radius,
                color,
            } = cmd
                && visible(*y, *h)
            {
                paint_border(
                    c,
                    Rect::new(ox + x, oy + y, *w, *h),
                    *widths,
                    *radius,
                    rgb(*color),
                );
            }
        }

        self.paint_page_controls(c, page, content, scroll, bs);
        c.restore_clip(saved);
    }

    /// Form controls: boxes in the page's light colours, the text they hold, the caret and
    /// the focus ring (the accent).
    fn paint_page_controls(
        &self,
        c: &mut Canvas,
        page: &Page,
        content: Rect,
        scroll: i32,
        bs: &BrowserState,
    ) {
        let focus = bs.forms.focus();
        let accent = theme::accent();
        let ink = Color::rgb(0x1D, 0x1D, 0x1F);
        let hint = Color::rgb(0x8E, 0x8E, 0x93);
        let edge = Color::rgb(0xC7, 0xC7, 0xCC);
        let white = Color::rgb(0xFF, 0xFF, 0xFF);
        for f in &page.fields {
            if f.y + f.h < scroll - 4 || f.y > scroll + content.h + 4 {
                continue;
            }
            let r = Rect::new(content.x + f.x, content.y - scroll + f.y, f.w, f.h);
            let focused = focus == Some((f.form, f.field));
            let info = page.forms.get(f.form).and_then(|x| x.fields.get(f.field));
            let font = Font::new(f.size);
            let (px, wt) = face_of(font);
            let radius = (f.h / 5).clamp(4, 12);
            match f.kind {
                FieldKind::Text | FieldKind::Password => {
                    c.fill_rrect(r, radius, Corner::Circle, white, 256);
                    if focused {
                        c.stroke_rrect(r.inflated(2), radius + 2, Corner::Circle, accent, 90);
                        c.stroke_rrect(r, radius, Corner::Circle, accent, 256);
                    } else {
                        c.stroke_rrect(r, radius, Corner::Circle, edge, 256);
                    }
                    let inner = Rect::new(r.x + f.pad_x, r.y, r.w - 2 * f.pad_x, r.h);
                    let value = bs.forms.value(f.form, f.field);
                    let ty = text::center_y(r.y, r.h, px, wt);
                    let saved = c.set_clip(
                        inner
                            .intersection(&c.clip_rect())
                            .unwrap_or(Rect::new(0, 0, 0, 0)),
                    );
                    let shown: alloc::borrow::Cow<'_, str> = if f.kind == FieldKind::Password {
                        alloc::borrow::Cow::Owned("\u{2022}".repeat(value.chars().count()))
                    } else {
                        alloc::borrow::Cow::Borrowed(value)
                    };
                    if shown.is_empty() {
                        if let Some(ph) = info.map(|i| i.placeholder.as_str()) {
                            text::draw(c, inner.x, ty, ph, px, wt, hint);
                        }
                    } else {
                        // The tail of a value wider than the box, so the caret stays visible.
                        let caret_byte = if focused {
                            bs.forms.caret().min(value.len())
                        } else {
                            value.len()
                        };
                        let caret_chars = value[..caret_byte].chars().count();
                        let caret_text: alloc::borrow::Cow<'_, str> =
                            if f.kind == FieldKind::Password {
                                alloc::borrow::Cow::Owned("\u{2022}".repeat(caret_chars))
                            } else {
                                alloc::borrow::Cow::Borrowed(&value[..caret_byte])
                            };
                        let caret_x = text::measure(&caret_text, px, wt);
                        let shift = (caret_x - inner.w + 2).max(0);
                        text::draw(c, inner.x - shift, ty, &shown, px, wt, ink);
                    }
                    if focused {
                        let value_caret = bs.forms.caret().min(value.len());
                        let before: alloc::borrow::Cow<'_, str> = if f.kind == FieldKind::Password {
                            alloc::borrow::Cow::Owned(
                                "\u{2022}".repeat(value[..value_caret].chars().count()),
                            )
                        } else {
                            alloc::borrow::Cow::Borrowed(&value[..value_caret])
                        };
                        let cx = text::measure(&before, px, wt);
                        let shift = (cx - inner.w + 2).max(0);
                        c.fill_rect(
                            (inner.x + cx - shift).max(0) as usize,
                            (r.y + f.h / 5).max(0) as usize,
                            1,
                            (f.h - 2 * (f.h / 5)).max(1) as usize,
                            accent,
                        );
                    }
                    c.restore_clip(saved);
                }
                FieldKind::Select => {
                    c.fill_rrect(r, radius, Corner::Circle, white, 256);
                    if focused {
                        c.stroke_rrect(r.inflated(2), radius + 2, Corner::Circle, accent, 90);
                        c.stroke_rrect(r, radius, Corner::Circle, accent, 256);
                    } else {
                        c.stroke_rrect(r, radius, Corner::Circle, edge, 256);
                    }
                    let arrow = (f.h / 2).clamp(10, 18);
                    let inner = Rect::new(r.x + f.pad_x, r.y, r.w - f.pad_x - arrow - 8, r.h);
                    let label = bs.forms.select_label(&page.forms, f.form, f.field);
                    let ty = text::center_y(r.y, r.h, px, wt);
                    text::draw_ellipsis(c, inner.x, ty, inner.w, label, px, wt, ink);
                    ui::draw_glyph(
                        c,
                        kitsune_core::iconart::Glyph::ChevronDown,
                        r.right() - arrow - 8,
                        r.y + (r.h - arrow) / 2,
                        arrow,
                        0xFF6E_6E73,
                    );
                }
                FieldKind::TextArea => {
                    c.fill_rrect(r, radius.min(8), Corner::Circle, white, 256);
                    if focused {
                        c.stroke_rrect(
                            r.inflated(2),
                            radius.min(8) + 2,
                            Corner::Circle,
                            accent,
                            90,
                        );
                        c.stroke_rrect(r, radius.min(8), Corner::Circle, accent, 256);
                    } else {
                        c.stroke_rrect(r, radius.min(8), Corner::Circle, edge, 256);
                    }
                    let inner = Rect::new(
                        r.x + f.pad_x,
                        r.y + f.pad_y,
                        r.w - 2 * f.pad_x,
                        r.h - 2 * f.pad_y,
                    );
                    let value = bs.forms.value(f.form, f.field);
                    let saved = c.set_clip(
                        inner
                            .intersection(&c.clip_rect())
                            .unwrap_or(Rect::new(0, 0, 0, 0)),
                    );
                    let line_h = f.line_h.max(1);
                    if value.is_empty()
                        && let Some(ph) = info.map(|i| i.placeholder.as_str())
                    {
                        let ty = text::center_y(inner.y, line_h, px, wt);
                        text::draw(c, inner.x, ty, ph, px, wt, hint);
                    }
                    let rows = wrap_rows(value, inner.w, |ch| {
                        text::measure(ch.encode_utf8(&mut [0; 4]), px, wt)
                    });
                    let visible = (inner.h / line_h).max(1) as usize;
                    let caret = if focused {
                        bs.forms.caret().min(value.len())
                    } else {
                        0
                    };
                    let (caret_r, caret_off) = caret_row(&rows, caret);
                    // The rows around the caret, so it stays visible; from the top when idle.
                    let top = if focused {
                        (caret_r + 1).saturating_sub(visible)
                    } else {
                        0
                    };
                    for (k, &(a, b)) in rows.iter().enumerate().skip(top).take(visible) {
                        let ly = inner.y + (k - top) as i32 * line_h;
                        let ty = text::center_y(ly, line_h, px, wt);
                        text::draw(c, inner.x, ty, &value[a..b], px, wt, ink);
                    }
                    if focused && caret_r >= top && caret_r < top + visible {
                        let (a, _) = rows.get(caret_r).copied().unwrap_or((0, 0));
                        let before = &value[a..(a + caret_off).min(value.len())];
                        let cx = text::measure(before, px, wt);
                        let ly = inner.y + (caret_r - top) as i32 * line_h;
                        c.fill_rect(
                            (inner.x + cx).max(0) as usize,
                            ly.max(0) as usize,
                            1,
                            line_h.max(1) as usize,
                            accent,
                        );
                    }
                    c.restore_clip(saved);
                }
                FieldKind::Submit | FieldKind::PushButton => {
                    let primary = f.kind == FieldKind::Submit;
                    let fill = if primary {
                        accent
                    } else {
                        Color::rgb(0xF2, 0xF2, 0xF7)
                    };
                    c.fill_rrect(r, radius, Corner::Circle, fill, 256);
                    if !primary {
                        c.stroke_rrect(r, radius, Corner::Circle, edge, 256);
                    }
                    if focused {
                        c.stroke_rrect(r.inflated(2), radius + 2, Corner::Circle, accent, 110);
                    }
                    let label = info.map_or("", |i| i.label.as_str());
                    let w = if primary { Weight::Medium } else { wt };
                    text::draw_centered(c, r, label, px, w, if primary { white } else { ink });
                }
                FieldKind::Checkbox | FieldKind::Radio => {
                    let on = bs.forms.is_checked(f.form, f.field);
                    let round = f.kind == FieldKind::Radio;
                    let rad = if round { f.h / 2 } else { (f.h / 4).max(3) };
                    if on {
                        c.fill_rrect(r, rad, Corner::Circle, accent, 256);
                        if round {
                            c.fill_rrect(r.inflated(-(f.h / 3)), f.h, Corner::Circle, white, 256);
                        } else {
                            ui::draw_glyph(
                                c,
                                kitsune_core::iconart::Glyph::Check,
                                r.x,
                                r.y,
                                f.h,
                                0xFFFF_FFFF,
                            );
                        }
                    } else {
                        c.fill_rrect(r, rad, Corner::Circle, white, 256);
                        c.stroke_rrect(r, rad, Corner::Circle, edge, 256);
                    }
                    if focused {
                        c.stroke_rrect(r.inflated(2), rad + 2, Corner::Circle, accent, 110);
                    }
                }
                FieldKind::Hidden => {}
            }
        }
    }
}
