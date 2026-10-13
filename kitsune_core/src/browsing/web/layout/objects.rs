//! objects (split out of `layout.rs`).

use super::*;

/// Parse a pixel attribute (`width="120"`, `"120px"`); percentages and
/// anything else are ignored (0).
pub(super) fn attr_px(el: &Element, name: &str) -> i32 {
    let Some(v) = el.attrs.get(name) else {
        return 0;
    };
    let v = v.trim().trim_end_matches("px").trim();
    if v.is_empty() || !v.bytes().all(|b| b.is_ascii_digit()) {
        return 0;
    }
    v.parse::<i32>().unwrap_or(0).clamp(0, 4096)
}

/// Width and height of an image box, given what the page declared, what is
/// known of the picture and the room on the line. Integer math throughout,
/// never zero, never wider than `avail`.
pub(crate) fn img_box(dw: i32, dh: i32, state: ImgState, avail: i32, zoom: i32) -> (i32, i32) {
    let nat = match state {
        ImgState::Ready { w, h } if w > 0 && h > 0 => Some((
            (i64::from(w) * i64::from(zoom) / 100).max(1),
            (i64::from(h) * i64::from(zoom) / 100).max(1),
        )),
        _ => None,
    };
    let (dw, dh) = (i64::from(dw), i64::from(dh));
    let (mut w, mut h) = match (dw > 0, dh > 0, nat) {
        (true, true, _) => (dw, dh),
        (true, false, Some((nw, nh))) => (dw, (dw * nh / nw).max(1)),
        (false, true, Some((nw, nh))) => ((dh * nw / nh).max(1), dh),
        (true, false, None) => (dw, (dw * 3 / 4).max(1)),
        (false, true, None) => ((dh * 4 / 3).max(1), dh),
        (false, false, Some((nw, nh))) => (nw, nh),
        (false, false, None) => match state {
            // A picture that will not come: a compact message box.
            ImgState::Pending => (160 * i64::from(zoom) / 100, 120 * i64::from(zoom) / 100),
            _ => (240 * i64::from(zoom) / 100, 48 * i64::from(zoom) / 100),
        },
    };
    let avail = i64::from(avail.max(8));
    if w > avail {
        h = (h * avail / w).max(1);
        w = avail;
    }
    (w.clamp(1, 20_000) as i32, h.clamp(1, 20_000) as i32)
}

pub(super) fn control_font(p: &Painter) -> Font {
    Font::new(p.z(CONTROL_PX).clamp(6, 200) as u16)
}

/// The (width, height) of an object on a line with `avail` pixels.
pub(super) fn obj_size(p: &Painter, obj: &Obj, avail: i32) -> (i32, i32) {
    match obj {
        Obj::Img { dw, dh, state, .. } => img_box(*dw, *dh, *state, avail, p.zoom),
        Obj::Field {
            kind,
            chars,
            rows,
            label,
            ..
        } => {
            let f = control_font(p);
            let h = if *kind == FieldKind::TextArea {
                *rows as i32 * p.nat(f) + 2 * p.z(8)
            } else {
                p.z(30)
            };
            let w = match kind {
                FieldKind::Submit | FieldKind::PushButton => {
                    let tw = (wq(p, label, f) + Q - 1) / Q;
                    (tw + 2 * p.z(16)).max(p.z(64))
                }
                FieldKind::Checkbox | FieldKind::Radio => {
                    // The box and a gap before the label that follows.
                    let s = p.z(16).max(8);
                    return (s + p.z(6), s);
                }
                FieldKind::Select => {
                    // The longest label, the arrow and the padding.
                    let cw = (wq(p, "0", f) + Q - 1) / Q;
                    cw * (*chars as i32).clamp(1, 80) + 2 * p.z(10) + p.z(22)
                }
                _ => {
                    let cw = (wq(p, "0", f) + Q - 1) / Q;
                    cw * (*chars as i32).clamp(1, 80) + 2 * p.z(10)
                }
            };
            (w.min(avail.max(20)).max(8), h)
        }
    }
}

/// How far an object hangs below the baseline of its line: nothing for a picture; a control
/// sits with the baseline of its own text on the line's, a toggle roughly centred on the
/// x-height.
pub(super) fn obj_below(p: &Painter, obj: &Obj, h: i32) -> i32 {
    match obj {
        Obj::Img { .. } => 0,
        Obj::Field { kind, .. } => match kind {
            FieldKind::Checkbox | FieldKind::Radio => h / 4,
            FieldKind::TextArea => {
                // The first line's baseline is on the line's.
                let f = control_font(p);
                (h - (p.z(8) + p.asc(f))).clamp(0, h)
            }
            _ => {
                let f = control_font(p);
                let text_top = (h - p.nat(f)) / 2;
                (h - (text_top + p.asc(f))).clamp(0, h)
            }
        },
    }
}

/// Fit `text` in `max_w` pixels, cutting with an ellipsis.
pub(super) fn ellipsize(p: &Painter, text: &str, f: Font, max_w: i32) -> String {
    let max_q = max_w.max(0).saturating_mul(Q);
    if wq(p, text, f) <= max_q {
        return String::from(text);
    }
    let room = max_q - wq(p, "\u{2026}", f);
    if room <= 0 {
        return String::new();
    }
    let k = fit_prefix(p, text, f, room);
    let mut s = String::from(&text[..k]);
    if wq(p, &s, f) > room {
        s.clear();
    }
    s.push('\u{2026}');
    s
}

/// Draw the message box of an image that has no picture: border, alt text
/// and the reason.
pub(super) fn paint_missing(
    p: &mut Painter,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    alt: &str,
    state: ImgState,
) {
    p.push_cmd(Cmd::Rect {
        x,
        y,
        w,
        h,
        color: Rgb(0xEE, 0xEF, 0xF3),
        radius: p.z(6),
    });
    p.push_cmd(Cmd::Border {
        x,
        y,
        w,
        h,
        widths: [1; 4],
        radius: p.z(6),
        color: Rgb(0xC9, 0xCC, 0xD6),
    });
    let f = Font::new(p.z(12).clamp(6, 100) as u16);
    let lh = p.nat(f);
    let inner = w - 2 * p.z(8);
    let mut ty = y + p.z(8);
    let alt = alt.trim();
    if !alt.is_empty() && inner > 0 && ty + lh <= y + h {
        let t = ellipsize(p, alt, f, inner);
        let tw = (wq(p, &t, f) + Q - 1) / Q;
        p.push_cmd(Cmd::Text {
            x: x + p.z(8),
            y: ty,
            w: tw,
            h: lh,
            text: t,
            color: Rgb(0x33, 0x40, 0x55),
            font: f,
            deco: 0,
            link: None,
        });
        ty += lh + p.z(2);
    }
    if let Some(m) = state.message()
        && inner > 0
        && ty + lh <= y + h
    {
        let t = ellipsize(p, m, f, inner);
        let tw = (wq(p, &t, f) + Q - 1) / Q;
        p.push_cmd(Cmd::Text {
            x: x + p.z(8),
            y: ty,
            w: tw,
            h: lh,
            text: t,
            color: Rgb(0x9A, 0x2B, 0x2B),
            font: f,
            deco: 0,
            link: None,
        });
    }
}

/// Place a laid-out object at `(x, y)` (top-left of its box) and record hit boxes.
pub(super) fn emit_obj(p: &mut Painter, obj: &Obj, x: i32, y: i32, w: i32, h: i32, st: Style) {
    match obj {
        Obj::Img {
            idx, state, alt, ..
        } => {
            match state {
                ImgState::Ready { .. } => p.push_cmd(Cmd::Image {
                    x,
                    y,
                    w,
                    h,
                    idx: *idx,
                }),
                ImgState::Pending => p.push_cmd(Cmd::Rect {
                    x,
                    y,
                    w,
                    h,
                    color: Rgb(0xDD, 0xE1, 0xE8),
                    radius: p.z(6),
                }),
                other => paint_missing(p, x, y, w, h, alt, *other),
            }
            p.push_hit(x, y, w, h, st.link, st.node);
        }
        Obj::Field {
            form, field, kind, ..
        } => {
            let f = control_font(p);
            let w = if matches!(kind, FieldKind::Checkbox | FieldKind::Radio) {
                (w - p.z(6)).max(1)
            } else {
                w
            };
            p.fields.push(FieldBox {
                form: *form,
                field: *field,
                kind: *kind,
                x,
                y,
                w,
                h,
                size: f.size,
                pad_x: p.z(10),
                line_h: p.nat(f),
                pad_y: p.z(8),
            });
            p.push_hit(x, y, w, h, None, st.node);
        }
    }
}
