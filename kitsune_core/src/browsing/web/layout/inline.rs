//! inline (split out of `layout.rs`).

use super::*;

impl Line {
    pub(super) fn new(strut: (i32, i32)) -> Line {
        Line {
            pieces: Vec::new(),
            w: 0,
            top: strut.0,
            bottom: strut.1,
        }
    }
}

/// `(above, below)` the baseline of the block's own strut.
pub(super) fn strut_of(p: &Painter, c: &Computed) -> (i32, i32) {
    let f = p.font(c);
    let lh = p.lh(c, f);
    let asc = p.asc(f);
    let nat = p.nat(f);
    let top = asc + (lh - nat) / 2;
    (top, lh - top)
}

/// Extents of a text run: (above baseline, below baseline).
pub(super) fn text_extents(p: &Painter, st: &Style) -> (i32, i32) {
    let asc = p.asc(st.font);
    let nat = p.nat(st.font);
    let top = asc + (st.lh - nat) / 2;
    ((top + st.shift).max(0), (st.lh - top - st.shift).max(0))
}

/// Where the next line goes: `area` narrowed by the floats beside it, and its width in q8. A line
/// that would have almost no room beside a float starts below it.
fn lane(p: &Painter, area: Area, mut y: i32, strut: (i32, i32)) -> (i32, Area, i32) {
    let mut cur = area;
    if !p.floats.is_empty() {
        let h = (strut.0 + strut.1).max(1);
        for _ in 0..64 {
            cur = band(p, area, y, h);
            if cur.w >= area.w.min(p.z(80)).max(1) {
                break;
            }
            // Too narrow: below the lowest float that is in the way.
            let next = p
                .floats
                .iter()
                .filter(|f| f.top < y + h && f.bottom > y)
                .map(|f| f.bottom)
                .min();
            match next {
                Some(n) if n > y => y = n,
                _ => break,
            }
        }
    }
    (y, cur, cur.w.max(1).saturating_mul(Q))
}

/// Emit the buffered inline words as wrapped, aligned line boxes starting at `y`;
/// returns the y below the last line. `align_override` replaces the container's
/// `text-align` (block-level images centred by auto margins).
pub(super) fn flush_inline(
    p: &mut Painter,
    container: &Computed,
    area: Area,
    mut y: i32,
    align_override: Option<Align>,
) -> i32 {
    if p.items.is_empty() {
        return y;
    }
    // The margin waiting above the first line.
    y = adv(y, core::mem::take(&mut p.pending));
    p.space = None;
    let spare = core::mem::take(&mut p.spare_items);
    let mut items = core::mem::replace(&mut p.items, spare);
    let align = align_override.unwrap_or(container.align);
    let strut = strut_of(p, container);
    let (mut y, mut cur, mut avail) = lane(p, area, y, strut);
    let mut line = Line::new(strut);

    for item in items.drain(..) {
        match item {
            Item::Br(st) => {
                if line.pieces.is_empty() {
                    // An empty line still has the height of its strut and of the break.
                    let (t, b) = text_extents(p, &st);
                    y = adv(y, (t.max(strut.0) + b.max(strut.1)).max(1));
                    (y, cur, avail) = lane(p, area, y, strut);
                } else {
                    y = emit_line(p, &mut line, cur, align, y, strut);
                    (y, cur, avail) = lane(p, area, y, strut);
                }
            }
            Item::Obj { obj, st, sp } => {
                let (w, h) = obj_size(p, &obj, area.w);
                let below = obj_below(p, &obj, h);
                let space = match sp {
                    Some(f) if !line.pieces.is_empty() => p.space_q8(f),
                    _ => 0,
                };
                let wq_ = w.saturating_mul(Q);
                if !line.pieces.is_empty()
                    && line.w.saturating_add(space).saturating_add(wq_) > avail
                {
                    y = emit_line(p, &mut line, cur, align, y, strut);
                    (y, cur, avail) = lane(p, area, y, strut);
                }
                let space = if line.pieces.is_empty() { 0 } else { space };
                let x = line.w.saturating_add(space);
                line.w = x.saturating_add(wq_);
                line.top = line.top.max(h - below);
                line.bottom = line.bottom.max(below);
                line.pieces.push(Piece {
                    kind: PieceKind::Obj(obj, w, h, below),
                    st,
                    x,
                    w: wq_,
                });
            }
            Item::Gap(w, st, sp) => {
                let wq_ = w.max(0).saturating_mul(Q);
                let space = match sp {
                    Some(f) if !line.pieces.is_empty() => p.space_q8(f),
                    _ => 0,
                };
                let x = line.w.saturating_add(space);
                line.w = x.saturating_add(wq_);
                line.pieces.push(Piece {
                    kind: PieceKind::Gap,
                    st,
                    x,
                    w: wq_,
                });
            }
            Item::Word {
                mut text,
                st,
                sp,
                any,
                nb,
            } => {
                let mut sp = sp;
                loop {
                    let ww = wq(p, &text, st.font);
                    let space = match sp {
                        Some(f) if !line.pieces.is_empty() => p.space_q8(f),
                        _ => 0,
                    };
                    if line.w.saturating_add(space).saturating_add(ww) <= avail
                        || (nb && !line.pieces.is_empty())
                    {
                        place_text(p, &mut line, text, st, space, ww);
                        break;
                    }
                    if !line.pieces.is_empty() {
                        // Fill the rest of this line with the head of a word that may break anywhere.
                        if any {
                            let room = avail.saturating_sub(line.w).saturating_sub(space);
                            if room > 0 {
                                let k = fit_prefix(p, &text, st.font, room);
                                if k < text.len() && wq(p, &text[..k], st.font) <= room {
                                    let tail = text.split_off(k);
                                    let hw = wq(p, &text, st.font);
                                    place_text(p, &mut line, text, st, space, hw);
                                    y = emit_line(p, &mut line, cur, align, y, strut);
                                    (y, cur, avail) = lane(p, area, y, strut);
                                    text = tail;
                                    sp = None;
                                    continue;
                                }
                            }
                        }
                        y = emit_line(p, &mut line, cur, align, y, strut);
                        (y, cur, avail) = lane(p, area, y, strut);
                        sp = None;
                        continue;
                    }
                    // Alone on an empty line and still too wide: cut it.
                    let k = fit_prefix(p, &text, st.font, avail);
                    if k >= text.len() {
                        place_text(p, &mut line, text, st, 0, ww);
                        break;
                    }
                    let tail = text.split_off(k);
                    let hw = wq(p, &text, st.font);
                    place_text(p, &mut line, text, st, 0, hw);
                    y = emit_line(p, &mut line, cur, align, y, strut);
                    (y, cur, avail) = lane(p, area, y, strut);
                    text = tail;
                    sp = None;
                }
            }
        }
    }
    if !line.pieces.is_empty() {
        y = emit_line(p, &mut line, cur, align, y, strut);
    }
    // Keep the list's room for the next flush (nothing was queued while this one ran).
    if p.items.is_empty() {
        p.spare_items = core::mem::replace(&mut p.items, items);
    } else {
        p.spare_items = items;
    }
    y
}

pub(super) fn place_text(
    p: &Painter,
    line: &mut Line,
    text: String,
    st: Style,
    space: i32,
    w: i32,
) {
    let x = line.w.saturating_add(space);
    line.w = x.saturating_add(w);
    let (t, b) = text_extents(p, &st);
    line.top = line.top.max(t);
    line.bottom = line.bottom.max(b);
    line.pieces.push(Piece {
        kind: PieceKind::Text(text),
        st,
        x,
        w,
    });
}

/// Place the pieces of `line` and start a new one; returns the y below it.
pub(super) fn emit_line(
    p: &mut Painter,
    line: &mut Line,
    area: Area,
    align: Align,
    y: i32,
    strut: (i32, i32),
) -> i32 {
    let mut done = core::mem::replace(line, Line::new(strut));
    // The pieces are read out of this vector, which then goes back to the next line empty
    // but with the room it grew to.
    let mut pieces = core::mem::take(&mut done.pieces);
    let line_h = (done.top + done.bottom).max(1);
    let baseline = y.saturating_add(done.top);
    let slack = (area.w.saturating_mul(Q) - done.w).max(0);
    let shift = match align {
        Align::Left => 0,
        Align::Center => slack / 2,
        Align::Right => slack,
    };
    let x0 = area.x.saturating_mul(Q) + shift;

    // A marker belongs to the first line of its item.
    if let Some(mk) = p.marker.take() {
        let f = pieces
            .iter()
            .find_map(|pc| matches!(pc.kind, PieceKind::Text(_)).then_some(pc.st.font))
            .unwrap_or(p.style(&Computed::root()).font);
        emit_marker(p, &mk, baseline, f);
    }

    // Merge neighbouring text pieces that look the same into one run.
    let mut runs: Vec<(i32, i32, String, Style)> = Vec::with_capacity(pieces.len().min(8)); // x q8, w q8, text, style
    let mut objs: Vec<(i32, Obj, i32, i32, i32, Style)> = Vec::new();
    // Backgrounds of inline boxes: (left, right, top, height, colour), joined when they touch.
    let mut bgs: Vec<(i32, i32, i32, i32, Rgb)> = Vec::new();
    let add_bg =
        |bgs: &mut Vec<(i32, i32, i32, i32, Rgb)>, l: i32, r: i32, t: i32, h: i32, c: Rgb| {
            if let Some(last) = bgs.last_mut()
                && last.4 == c
                && last.2 == t
                && last.3 == h
                && l <= last.1 + 1
            {
                last.1 = last.1.max(r);
                return;
            }
            bgs.push((l, r, t, h, c));
        };
    for pc in pieces.drain(..) {
        match pc.kind {
            PieceKind::Text(t) => {
                if let Some(last) = runs.last_mut()
                    && last.3 == pc.st
                    && last.0 + last.1 <= pc.x
                    && pc.x - (last.0 + last.1) <= p.space_q8(pc.st.font) + Q
                {
                    // The gap between the pieces is the collapsed space.
                    if pc.x > last.0 + last.1 {
                        last.2.push(' ');
                    }
                    last.2.push_str(&t);
                    last.1 = pc.x + pc.w - last.0;
                } else {
                    runs.push((pc.x, pc.w, t, pc.st));
                }
            }
            PieceKind::Obj(o, w, h, below) => objs.push((pc.x, o, w, h, below, pc.st)),
            PieceKind::Gap => {
                if let Some(bg) = pc.st.bg {
                    let nat = p.nat(pc.st.font);
                    let ty = baseline - p.asc(pc.st.font) - pc.st.shift;
                    let l = (x0.saturating_add(pc.x).saturating_add(Q / 2)) / Q;
                    let r = (x0
                        .saturating_add(pc.x)
                        .saturating_add(pc.w)
                        .saturating_add(Q / 2))
                        / Q;
                    bgs.push((l, r, ty - 1, nat + 2, bg));
                }
            }
        }
    }
    // Background boxes of the runs (and of the gaps between them), in order.
    let mut run_bgs: Vec<(i32, i32, i32, i32, Rgb)> = Vec::new();
    for (x, w, _, st) in &runs {
        if let Some(bg) = st.bg {
            let nat = p.nat(st.font);
            let ty = baseline - p.asc(st.font) - st.shift;
            let l = (x0.saturating_add(*x).saturating_add(Q / 2)) / Q;
            let r = (x0
                .saturating_add(*x)
                .saturating_add(*w)
                .saturating_add(Q - 1))
                / Q;
            run_bgs.push((l, r, ty - 1, nat + 2, bg));
        }
    }
    bgs.extend(run_bgs);
    bgs.sort_by_key(|b| (b.2, b.0));
    let mut joined: Vec<(i32, i32, i32, i32, Rgb)> = Vec::new();
    for b in bgs {
        add_bg(&mut joined, b.0, b.1, b.2, b.3, b.4);
    }
    for (l, r, t, h, col) in joined {
        p.push_cmd(Cmd::Rect {
            x: l,
            y: t,
            w: (r - l).max(1),
            h,
            color: col,
            radius: 4,
        });
    }
    for (x, text, w, st) in runs.into_iter().map(|(x, w, t, s)| (x, t, w, s)) {
        let px = (x0.saturating_add(x).saturating_add(Q / 2)) / Q;
        let pw = (w + Q - 1) / Q;
        let nat = p.nat(st.font);
        let ty = baseline - p.asc(st.font) - st.shift;
        if let Some(l) = st.link {
            p.hits.push(LinkHit {
                x: px,
                y,
                w: pw,
                h: line_h,
                link: l,
            });
        }
        p.push_cmd(Cmd::Text {
            x: px,
            y: ty,
            w: pw,
            h: nat,
            text,
            color: st.color,
            font: st.font,
            deco: st.deco,
            link: st.link.map(|l| l as u32),
        });
    }
    for (x, obj, w, h, below, st) in objs {
        let px = (x0.saturating_add(x).saturating_add(Q / 2)) / Q;
        emit_obj(p, &obj, px, baseline - (h - below), w, h, st);
    }
    line.pieces = pieces;
    adv(y, line_h)
}

pub(super) fn emit_marker(p: &mut Painter, mk: &Marker, baseline: i32, f: Font) {
    match mk.kind {
        MarkerKind::Number => {
            let w = (wq(p, &mk.text, f) + Q - 1) / Q;
            let nat = p.nat(f);
            p.push_cmd(Cmd::Text {
                x: (mk.right - w).max(0),
                y: baseline - p.asc(f),
                w,
                h: nat,
                text: mk.text.clone(),
                color: mk.color,
                font: f,
                deco: 0,
                link: None,
            });
        }
        MarkerKind::Bullet(style) => {
            let d = (i32::from(f.size) * 3 / 10).max(3);
            let right = mk.right - p.z(2);
            // A list with no left padding would put its marker off the page.
            let x = (right - d).max(0);
            let y = (baseline - i32::from(f.size) * 3 / 8 - d / 2).max(0);
            match style {
                ListStyle::Circle => p.push_cmd(Cmd::Border {
                    x,
                    y,
                    w: d,
                    h: d,
                    widths: [1; 4],
                    radius: d / 2,
                    color: mk.color,
                }),
                ListStyle::Square => p.push_cmd(Cmd::Rect {
                    x,
                    y,
                    w: d,
                    h: d,
                    color: mk.color,
                    radius: 0,
                }),
                _ => p.push_cmd(Cmd::Rect {
                    x,
                    y,
                    w: d,
                    h: d,
                    color: mk.color,
                    radius: d / 2,
                }),
            }
        }
    }
}
