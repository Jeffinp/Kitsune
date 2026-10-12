//! Colours, gradients and the presentation properties that make up a shape's style.

use super::*;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Paint {
    None,
    /// 0xRRGGBB.
    Color(u32),
}

pub(super) fn parse_color(s: &str) -> Option<u32> {
    let s = s.trim();
    if let Some(h) = s.strip_prefix('#') {
        if !h.bytes().all(|b| b.is_ascii_hexdigit()) {
            return None;
        }
        let v = u32::from_str_radix(h, 16).ok()?;
        return match h.len() {
            3 => Some(((v >> 8 & 15) * 17) << 16 | ((v >> 4 & 15) * 17) << 8 | (v & 15) * 17),
            6 => Some(v),
            8 => Some(v >> 8),
            _ => None,
        };
    }
    let l = s.to_ascii_lowercase();
    if let Some(inner) = l
        .strip_prefix("rgb")
        .and_then(|r| r.trim_start_matches('a').strip_prefix('('))
    {
        let inner = inner.trim_end_matches(')');
        let mut c = [0u32; 3];
        let mut rest = inner;
        for o in &mut c {
            let (n, r) = num(skip_sep(rest))?;
            let (n, r) = match r.strip_prefix('%') {
                Some(r2) => (n * 255 / 100, r2),
                None => (n, r),
            };
            *o = ((n >> 16).clamp(0, 255)) as u32;
            rest = r;
        }
        return Some(c[0] << 16 | c[1] << 8 | c[2]);
    }
    const NAMED: &[(&str, u32)] = &[
        ("black", 0x000000),
        ("white", 0xffffff),
        ("red", 0xff0000),
        ("green", 0x008000),
        ("lime", 0x00ff00),
        ("blue", 0x0000ff),
        ("yellow", 0xffff00),
        ("orange", 0xffa500),
        ("gray", 0x808080),
        ("grey", 0x808080),
        ("silver", 0xc0c0c0),
        ("purple", 0x800080),
        ("navy", 0x000080),
        ("teal", 0x008080),
        ("maroon", 0x800000),
        ("aqua", 0x00ffff),
        ("cyan", 0x00ffff),
        ("fuchsia", 0xff00ff),
        ("magenta", 0xff00ff),
        ("pink", 0xffc0cb),
        ("brown", 0xa52a2a),
        ("gold", 0xffd700),
    ];
    NAMED.iter().find(|(n, _)| *n == l).map(|&(_, c)| c)
}

#[derive(Clone)]
pub(super) struct Style {
    pub(super) fill: Paint,
    pub(super) stroke: Paint,
    /// 16.16 user units.
    pub(super) stroke_w: i64,
    /// Opacities 0..=255.
    pub(super) opacity: u32,
    pub(super) fill_op: u32,
    pub(super) stroke_op: u32,
    pub(super) even_odd: bool,
    /// What `currentColor` means.
    pub(super) current: u32,
}

impl Style {
    pub(super) fn root() -> Style {
        Style {
            fill: Paint::Color(0),
            stroke: Paint::None,
            stroke_w: 65536,
            opacity: 255,
            fill_op: 255,
            stroke_op: 255,
            even_odd: false,
            current: 0,
        }
    }
}

pub(super) struct Gradients<'a> {
    /// `(id, first stop colour)`.
    pub(super) list: Vec<(&'a str, u32)>,
}

pub(super) fn paint_of(v: &str, st: &Style, g: &Gradients) -> Option<Paint> {
    let v = v.trim();
    if v.eq_ignore_ascii_case("none") || v.eq_ignore_ascii_case("transparent") {
        return Some(Paint::None);
    }
    if v.eq_ignore_ascii_case("currentcolor") {
        return Some(Paint::Color(st.current));
    }
    if let Some(r) = v.strip_prefix("url(") {
        let id = r
            .split(')')
            .next()?
            .trim()
            .trim_matches(['"', '\''])
            .trim_start_matches('#');
        return Some(match g.list.iter().find(|(n, _)| *n == id) {
            Some(&(_, c)) => Paint::Color(c),
            // A fallback colour after the reference, else nothing.
            None => r
                .split(')')
                .nth(1)
                .and_then(parse_color)
                .map_or(Paint::None, Paint::Color),
        });
    }
    parse_color(v).map(Paint::Color)
}

pub(super) fn opacity(v: &str) -> Option<u32> {
    let (n, rest) = num(v.trim())?;
    let n = if rest.trim() == "%" { n / 100 } else { n };
    Some(((n * 255) >> 16).clamp(0, 255) as u32)
}

/// Apply one presentation property to `st`.
pub(super) fn apply_prop(st: &mut Style, k: &str, v: &str, g: &Gradients) {
    match k {
        "fill" => {
            if let Some(p) = paint_of(v, st, g) {
                st.fill = p;
            }
        }
        "stroke" => {
            if let Some(p) = paint_of(v, st, g) {
                st.stroke = p;
            }
        }
        "stroke-width" => {
            if let Some(w) = length(v, 65536) {
                st.stroke_w = w.max(0);
            }
        }
        "opacity" => {
            if let Some(o) = opacity(v) {
                st.opacity = st.opacity * o / 255;
            }
        }
        "fill-opacity" => {
            if let Some(o) = opacity(v) {
                st.fill_op = o;
            }
        }
        "stroke-opacity" => {
            if let Some(o) = opacity(v) {
                st.stroke_op = o;
            }
        }
        "fill-rule" => st.even_odd = v.trim() == "evenodd",
        "color" => {
            if let Some(c) = parse_color(v) {
                st.current = c;
            }
        }
        _ => {}
    }
}

pub(super) fn style_of(parent: &Style, t: &Tag, g: &Gradients) -> Style {
    let mut st = parent.clone();
    // `opacity` is not inherited.
    st.opacity = 255;
    for (k, v) in &t.attrs {
        apply_prop(&mut st, k, v, g);
    }
    if let Some(css) = t.get("style") {
        for decl in css.split(';') {
            if let Some((k, v)) = decl.split_once(':') {
                apply_prop(&mut st, k.trim(), v.trim(), g);
            }
        }
    }
    st
}
