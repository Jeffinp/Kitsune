//! Measuring content: the narrowest and widest a box can be (tables, flex items and floats size themselves with this).

use super::*;

/// The unbreakable and the unwrapped width of some inline content.
#[derive(Default)]
pub(crate) struct Intr {
    /// Width of the current run (since the last forced break), q8.
    pub(crate) cur: i32,
    pub(crate) max: i32,
    pub(crate) min: i32,
}

impl Intr {
    pub(crate) fn end_run(&mut self) {
        self.max = self.max.max(self.cur);
        self.cur = 0;
    }
}

pub(crate) fn sat(a: i32, b: i32) -> i32 {
    a.saturating_add(b).clamp(0, 1 << 26)
}

/// Measure `nodes` (the content of a cell or block): into `acc`, q8 pixels.
pub(crate) fn intrinsic<'a>(
    p: &mut Painter<'a>,
    nodes: &'a [Node],
    parent: &Computed,
    acc: &mut Intr,
    depth: u32,
) {
    if depth > MAX_MEASURE_DEPTH {
        return;
    }
    for node in nodes {
        match node {
            Node::Text(t) => {
                let f = p.font(parent);
                let pre = parent.ws == Ws::Pre;
                let nowrap = parent.ws == Ws::NoWrap;
                let space = p.space_q8(f);
                if pre {
                    for (i, line) in t.split('\n').enumerate() {
                        if i > 0 {
                            acc.end_run();
                        }
                        let w = wq(p, &expand_tabs(line), f);
                        acc.min = acc.min.max(w);
                        acc.cur = sat(acc.cur, w);
                    }
                    continue;
                }
                let mut first = true;
                for word in t.split(|c: char| c != '\u{a0}' && c.is_whitespace()) {
                    if word.is_empty() {
                        continue;
                    }
                    let w = wq(p, word, f);
                    if breaks_anywhere(word.chars().next().unwrap_or(' ')) {
                        // No spaces in the script: one character is the smallest piece.
                        let ch = word.chars().next().map_or(0, |c| {
                            let mut b = [0u8; 4];
                            wq(p, c.encode_utf8(&mut b), f)
                        });
                        acc.min = acc.min.max(ch);
                    } else if !nowrap {
                        acc.min = acc.min.max(w);
                    }
                    if !first || acc.cur > 0 {
                        acc.cur = sat(acc.cur, space);
                    }
                    acc.cur = sat(acc.cur, w);
                    first = false;
                }
                if nowrap {
                    acc.min = acc.min.max(acc.cur);
                }
            }
            Node::Element(el) => {
                let c = compute(el, p.sheet, parent, &p.anc, &mut p.budget);
                match c.display {
                    Disp::None => {}
                    Disp::Inline => match el.tag.as_str() {
                        "br" => acc.end_run(),
                        "img" => {
                            let (w, _) = img_extent(p, el, &c);
                            let w = w.saturating_mul(Q);
                            acc.min = acc.min.max(w);
                            acc.cur = sat(acc.cur, w);
                        }
                        "input" | "button" => {
                            let w = control_width(p, el).saturating_mul(Q);
                            acc.min = acc.min.max(w);
                            acc.cur = sat(acc.cur, w);
                        }
                        _ => {
                            p.anc.push(el);
                            intrinsic(p, &el.children, &c, acc, depth + 1);
                            p.anc.pop();
                        }
                    },
                    _ => {
                        acc.end_run();
                        let (mn, mx) = outer_intr(p, el, &c, depth + 1);
                        acc.min = acc.min.max(mn);
                        acc.max = acc.max.max(mx);
                    }
                }
            }
        }
    }
}

/// The narrowest and widest the content of `el` (style `c`) can be, q8 pixels, padding and border
/// not included. A flex row is the sum of its boxes, a grid the widest of its rows, a table its
/// columns; any other block the widest of its lines. `p.anc` must end with the parents of `el`.
pub(crate) fn content_intr<'a>(
    p: &mut Painter<'a>,
    el: &'a Element,
    c: &Computed,
    depth: u32,
) -> (i32, i32) {
    if depth > MAX_MEASURE_DEPTH {
        return (0, 0);
    }
    match el.tag.as_str() {
        "img" => {
            let (w, _) = img_extent(p, el, c);
            let q = w.saturating_mul(Q);
            return (q, q);
        }
        "input" | "button" => {
            let q = control_width(p, el).saturating_mul(Q);
            return (q, q);
        }
        _ => {}
    }
    if c.display == Disp::Table {
        let t = collect(p, el, c);
        let (lo, hi) = t.totals();
        return (lo.saturating_mul(Q), hi.saturating_mul(Q));
    }
    p.anc.push(el);
    let out = if matches!(c.display, Disp::Flex | Disp::Grid)
        && (c.display == Disp::Grid || c.flex.row)
    {
        // The boxes of the container, each with its margins.
        let mut kids: Vec<(i32, i32)> = Vec::new();
        for node in &el.children {
            match node {
                Node::Text(t) if t.chars().all(|ch| ch.is_whitespace()) => {}
                Node::Text(_) => {
                    let mut a = Intr::default();
                    intrinsic(p, core::slice::from_ref(node), c, &mut a, depth + 1);
                    a.end_run();
                    kids.push((a.min, a.max));
                }
                Node::Element(e) => {
                    let ec = compute(e, p.sheet, c, &p.anc, &mut p.budget);
                    if ec.display != Disp::None && kids.len() < 400 {
                        kids.push(outer_intr(p, e, &ec, depth + 1));
                    }
                }
            }
        }
        let gap = p.z(c.flex.gap_col).saturating_mul(Q);
        if c.display == Disp::Grid {
            let n = c.flex.tracks.cols.len().max(1);
            let (mut max, mut min) = (0, 0);
            for row in kids.chunks(n) {
                let sum = row.iter().fold(0i32, |s, k| sat(s, k.1));
                max = max.max(sat(sum, gap.saturating_mul(row.len() as i32 - 1)));
                min = row.iter().fold(min, |m, k| m.max(k.0));
            }
            (min, max)
        } else {
            let n = kids.len() as i32;
            let gaps = gap.saturating_mul((n - 1).max(0));
            let max = kids.iter().fold(0i32, |s, k| sat(s, k.1));
            let min = if c.flex.wrap {
                kids.iter().fold(0, |m, k| m.max(k.0))
            } else {
                kids.iter().fold(0i32, |s, k| sat(s, k.0))
            };
            (sat(min, gaps), sat(max, gaps))
        }
    } else {
        let mut sub = Intr::default();
        intrinsic(p, &el.children, c, &mut sub, depth + 1);
        sub.end_run();
        (sub.min, sub.max)
    };
    p.anc.pop();
    out
}

/// [`content_intr`] plus the box around it: padding, border and margins, and a `width` in pixels
/// when given (q8 pixels).
pub(crate) fn outer_intr<'a>(
    p: &mut Painter<'a>,
    el: &'a Element,
    c: &Computed,
    depth: u32,
) -> (i32, i32) {
    let (mut mn, mut mx) = content_intr(p, el, c, depth);
    let zoom = p.zoom;
    let ex = (side(c.padding[1], 0, zoom)
        + side(c.padding[3], 0, zoom)
        + side(c.margin[1], 0, zoom).max(0)
        + side(c.margin[3], 0, zoom).max(0)
        + c.border_w[1]
        + c.border_w[3])
        .max(0)
        .saturating_mul(Q);
    if let Len::Px(w) = c.width {
        let w = p.z(w).saturating_mul(Q);
        mx = w;
        mn = mn.min(w);
    }
    (sat(mn, ex), sat(mx, ex))
}

/// Width of an `<img>` (box and all) for measuring: the declared or natural width.
pub(crate) fn img_extent(p: &Painter, el: &Element, c: &Computed) -> (i32, i32) {
    let src = decode_attr(el.attrs.get("src").map(String::as_str).unwrap_or(""));
    let state = if src.trim().is_empty() {
        ImgState::Failed
    } else {
        p.lookup.lookup(&src)
    };
    let (dw, dh) = (p.z(attr_px(el, "width")), p.z(attr_px(el, "height")));
    let dw = match c.width {
        Len::Px(w) => p.z(w),
        _ => dw,
    };
    img_box(dw, dh, state, 1 << 16, p.zoom)
}

/// Width of a form control for measuring (kind from its attributes).
pub(crate) fn control_width(p: &Painter, el: &Element) -> i32 {
    let ty = el
        .attrs
        .get("type")
        .map_or(String::new(), |t| t.trim().to_ascii_lowercase());
    let f = control_font(p);
    match (el.tag.as_str(), ty.as_str()) {
        (_, "hidden") => 0,
        ("button", _) => {
            let t = fold_display(text_content(&el.children).trim());
            (wq(p, &t, f) / Q + 2 * p.z(16)).max(p.z(64))
        }
        (_, "submit" | "button" | "reset") => {
            let v = decode_attr(el.attrs.get("value").map_or("", String::as_str));
            (wq(p, &v, f) / Q + 2 * p.z(16)).max(p.z(64))
        }
        (_, "checkbox" | "radio") => p.z(16).max(8),
        _ => {
            let n = el
                .attrs
                .get("size")
                .and_then(|s| s.trim().parse::<i32>().ok())
                .filter(|&n| n > 0)
                .unwrap_or(20)
                .min(80);
            wq(p, "0", f) / Q * n + 2 * p.z(10)
        }
    }
}
