//! values (split out of `style.rs`).

use super::*;
use crate::browsing::web::css::{viewport_h, viewport_w};

/// Parse a CSS number with up to three decimals into thousandths: `1.5` is 1500.
/// Returns the value and the rest of the string. Page-controlled, so every step
/// saturates.
pub(super) fn parse_milli(s: &str) -> Option<(i64, &str)> {
    let b = s.as_bytes();
    let mut i = 0;
    let neg = match b.first() {
        Some(b'-') => {
            i = 1;
            true
        }
        Some(b'+') => {
            i = 1;
            false
        }
        _ => false,
    };
    let start = i;
    let mut int: i64 = 0;
    while i < b.len() && b[i].is_ascii_digit() {
        int = (int * 10 + i64::from(b[i] - b'0')).min(1_000_000_000);
        i += 1;
    }
    let mut frac: i64 = 0;
    let mut digits = i - start;
    if i < b.len() && b[i] == b'.' {
        i += 1;
        let mut scale = 100;
        while i < b.len() && b[i].is_ascii_digit() {
            frac += i64::from(b[i] - b'0') * scale;
            scale /= 10;
            i += 1;
            digits += 1;
        }
    }
    if digits == 0 {
        return None;
    }
    let v = int * 1000 + frac;
    Some((if neg { -v } else { v }, &s[i..]))
}

/// Parse one CSS length. `em` is the font size `em` refers to. `None` for anything
/// not understood (`calc()`, `vw`, `var()`).
pub(crate) fn parse_len(v: &str, em: i32) -> Option<Len> {
    let v = v.trim();
    if v.eq_ignore_ascii_case("auto") {
        return Some(Len::Auto);
    }
    if super::calc::is_math(v) {
        return super::calc::parse_math(v, em);
    }
    let (n, unit) = parse_milli(v)?;
    let unit = unit.trim().to_ascii_lowercase();
    // Rounded to the nearest pixel (0.67em of 16 px is 11, not 10).
    let px = |milli: i64| -> Option<Len> {
        let r = if milli >= 0 {
            (milli + 500) / 1000
        } else {
            -((-milli + 500) / 1000)
        };
        Some(Len::Px(
            r.clamp(-i64::from(MAX_PX), i64::from(MAX_PX)) as i32
        ))
    };
    match unit.as_str() {
        "" => {
            if n == 0 {
                px(0)
            } else {
                // Unitless non-zero lengths are invalid CSS but common in attributes and
                // quirks mode; read them as pixels.
                px(n)
            }
        }
        "px" => px(n),
        "pt" => px(n * 4 / 3),
        "pc" => px(n * 16),
        "in" => px(n * 96),
        "cm" => px(n * 96 * 100 / 254),
        "mm" => px(n * 96 * 10 / 254),
        "em" => px(n * i64::from(em)),
        "rem" => px(n * 16),
        "ex" => px(n * i64::from(em) / 2),
        "%" => Some(Len::Pct((n / 1000).clamp(-10_000, 10_000) as i32)),
        "ch" => px(n * i64::from(em) / 2),
        "vw" | "svw" | "lvw" | "dvw" => px(n * i64::from(viewport_w()) / 100),
        "vh" | "svh" | "lvh" | "dvh" => px(n * i64::from(viewport_h()) / 100),
        "vmin" => px(n * i64::from(viewport_w().min(viewport_h())) / 100),
        "vmax" => px(n * i64::from(viewport_w().max(viewport_h())) / 100),
        _ => None,
    }
}

/// A length that must be absolute (no percent, no auto).
pub(super) fn parse_px(v: &str, em: i32) -> Option<i32> {
    match parse_len(v, em)? {
        Len::Px(p) => Some(p),
        _ => None,
    }
}

/// Split a value into whitespace separated tokens, keeping `(...)` groups whole.
pub(super) fn tokens(v: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let b = v.as_bytes();
    let (mut start, mut depth) = (0usize, 0i32);
    let mut in_tok = false;
    for (i, &c) in b.iter().enumerate() {
        match c {
            b'(' => {
                depth += 1;
                in_tok = true;
            }
            b')' => depth = (depth - 1).max(0),
            c if c.is_ascii_whitespace() && depth == 0 => {
                if in_tok {
                    out.push(&v[start..i]);
                    in_tok = false;
                }
                continue;
            }
            _ => {}
        }
        if !in_tok {
            start = i;
            in_tok = true;
        }
    }
    if in_tok {
        out.push(&v[start..]);
    }
    out.truncate(16);
    out
}

/// One to four lengths in `top right bottom left` order.
pub(super) fn four(v: &str, em: i32) -> Option<[Len; 4]> {
    let t = tokens(v);
    let get = |i: usize| parse_len(t[i], em);
    Some(match t.len() {
        1 => [get(0)?; 4],
        2 => {
            let (a, b) = (get(0)?, get(1)?);
            [a, b, a, b]
        }
        3 => {
            let (a, b, c) = (get(0)?, get(1)?, get(2)?);
            [a, b, c, b]
        }
        4 => [get(0)?, get(1)?, get(2)?, get(3)?],
        _ => return None,
    })
}

pub(super) fn non_neg(l: Len) -> Len {
    match l {
        Len::Px(p) => Len::Px(p.max(0)),
        Len::Pct(p) => Len::Pct(p.max(0)),
        l @ Len::Calc { .. } => l,
        Len::Auto => Len::Px(0),
    }
}

/// The first colour in `v`, looking inside gradients (their first stop stands in
/// for the whole gradient).
pub(super) fn first_color(v: &str, back: Rgb) -> Option<Rgb> {
    for t in tokens(v) {
        let lower = t.to_ascii_lowercase();
        if lower.starts_with("url(") {
            continue;
        }
        if let Some(c) = parse_color_over(t, back) {
            return Some(c);
        }
        if lower.contains("gradient(")
            && let Some(open) = t.find('(')
        {
            let inner = t[open + 1..].trim_end_matches(')');
            // Arguments separated by commas outside parentheses.
            let mut depth = 0i32;
            let mut start = 0;
            let bytes = inner.as_bytes();
            for (i, &c) in bytes.iter().enumerate().chain([(bytes.len(), &b',')]) {
                match c {
                    b'(' => depth += 1,
                    b')' => depth -= 1,
                    b',' if depth == 0 => {
                        for part in tokens(&inner[start..i]) {
                            if let Some(c) = parse_color_over(part, back) {
                                return Some(c);
                            }
                        }
                        start = i + 1;
                    }
                    _ => {}
                }
            }
        }
    }
    None
}

/// Does a `font-family` list select the monospace face?
pub(super) fn family_is_mono(v: &str) -> bool {
    let l = v.to_ascii_lowercase();
    l.split(',').any(|f| {
        let f = f.trim().trim_matches(|c| c == '"' || c == '\'');
        matches!(
            f,
            "monospace"
                | "ui-monospace"
                | "courier"
                | "courier new"
                | "consolas"
                | "menlo"
                | "monaco"
                | "lucida console"
                | "sf mono"
                | "jetbrains mono"
                | "fira code"
                | "source code pro"
                | "dejavu sans mono"
        )
    })
}

pub(super) fn font_size_value(v: &str, parent_px: i32) -> Option<i32> {
    let l = v.trim().to_ascii_lowercase();
    let px = match l.as_str() {
        "xx-small" => 9,
        "x-small" => 10,
        "small" => 13,
        "medium" => 16,
        "large" => 18,
        "x-large" => 24,
        "xx-large" => 32,
        "xxx-large" => 48,
        "smaller" => parent_px * 5 / 6,
        "larger" => parent_px * 6 / 5,
        _ => match parse_len(&l, parent_px)? {
            Len::Px(p) => p,
            Len::Pct(p) => (i64::from(parent_px) * i64::from(p) / 100) as i32,
            Len::Calc { .. } | Len::Auto => return None,
        },
    };
    Some(px.clamp(MIN_FONT, MAX_FONT))
}

pub(super) fn line_height_value(v: &str, font_px: i32) -> Option<LineH> {
    let l = v.trim().to_ascii_lowercase();
    if l == "normal" {
        return Some(LineH::Normal);
    }
    // A bare number multiplies the element's own font size.
    if let Some((n, rest)) = parse_milli(&l)
        && rest.trim().is_empty()
    {
        return Some(LineH::Mult((n / 10).clamp(50, 1000) as i32));
    }
    match parse_len(&l, font_px)? {
        Len::Px(p) => Some(LineH::Px(p.clamp(1, 1000))),
        Len::Pct(p) => Some(LineH::Mult(p.clamp(50, 1000))),
        Len::Calc { .. } | Len::Auto => None,
    }
}

pub(super) fn weight_is_bold(v: &str) -> Option<bool> {
    let l = v.trim().to_ascii_lowercase();
    match l.as_str() {
        "bold" | "bolder" => Some(true),
        "normal" | "lighter" => Some(false),
        n => n.parse::<u32>().ok().map(|w| w >= 600),
    }
}

/// `font: [style] [weight] size[/line-height] family`
pub(super) fn apply_font_shorthand(c: &mut Computed, v: &str, parent_px: i32) {
    let l = v.to_ascii_lowercase();
    if matches!(
        l.trim(),
        "inherit"
            | "initial"
            | "unset"
            | "caption"
            | "icon"
            | "menu"
            | "message-box"
            | "status-bar"
    ) {
        return;
    }
    c.bold = false;
    c.italic = false;
    for t in tokens(&l) {
        if t == "italic" || t == "oblique" {
            c.italic = true;
        } else if t == "normal" || t == "small-caps" {
            // the initial value: nothing to set
        } else if matches!(t, "bold" | "bolder") || t.parse::<u32>().is_ok_and(|w| w >= 600) {
            c.bold = true;
        } else if t == "lighter" || t.parse::<u32>().is_ok() {
            c.bold = false;
        } else {
            let (size, lh) = t.split_once('/').map_or((t, None), |(a, b)| (a, Some(b)));
            if let Some(px) = font_size_value(size, parent_px) {
                c.font_px = px;
                if let Some(lh) = lh
                    && let Some(h) = line_height_value(lh, px)
                {
                    c.line_h = h;
                }
                // Everything after the size is the family list.
                let end = (t.as_ptr() as usize - l.as_ptr() as usize) + t.len();
                c.mono = l.get(end..).is_some_and(family_is_mono);
                return;
            }
        }
    }
}

pub(super) fn apply_border_side(c: &mut Computed, sides: &[usize], v: &str) {
    let mut w = 3; // `medium`
    let mut style_none = false;
    let mut have_style = false;
    for t in tokens(v) {
        let l = t.to_ascii_lowercase();
        match l.as_str() {
            "none" | "hidden" => {
                style_none = true;
                have_style = true;
            }
            "solid" | "dashed" | "dotted" | "double" | "groove" | "ridge" | "inset" | "outset" => {
                have_style = true;
            }
            "thin" => w = 1,
            "medium" => w = 3,
            "thick" => w = 5,
            _ => {
                if let Some(Len::Px(p)) = parse_len(&l, c.font_px) {
                    w = p.clamp(0, 64);
                } else if let Some(col) = parse_color_over(t, c.backdrop) {
                    c.border_color = col;
                }
            }
        }
    }
    // Without a style the border does not exist (CSS), but `border: 1px #ccc` is common.
    let _ = have_style;
    let w = if style_none { 0 } else { w };
    for &s in sides {
        c.border_w[s] = w;
    }
}

pub(super) fn parse_justify(v: &str) -> Justify {
    match v.trim().to_ascii_lowercase().as_str() {
        "center" => Justify::Center,
        "flex-end" | "end" | "right" => Justify::End,
        "space-between" => Justify::Between,
        "space-around" => Justify::Around,
        "space-evenly" => Justify::Evenly,
        _ => Justify::Start,
    }
}

/// `align-items` / `align-self` values; `None` for `auto` and `normal`.
pub(super) fn parse_cross(v: &str) -> Option<CrossAlign> {
    match v.trim().to_ascii_lowercase().as_str() {
        "stretch" => Some(CrossAlign::Stretch),
        "flex-start" | "start" | "self-start" | "baseline" | "first baseline" => {
            Some(CrossAlign::Start)
        }
        "center" => Some(CrossAlign::Center),
        "flex-end" | "end" | "self-end" => Some(CrossAlign::End),
        _ => None,
    }
}

/// `flex: none | auto | <grow> [<shrink>] [<basis>]`.
pub(super) fn apply_flex_shorthand(c: &mut Computed, v: &str, em: i32) {
    let l = v.trim().to_ascii_lowercase();
    match l.as_str() {
        "none" => {
            c.flex.grow = 0;
            c.flex.shrink = 0;
            c.flex.basis = Len::Auto;
            return;
        }
        "auto" => {
            c.flex.grow = 1000;
            c.flex.shrink = 1000;
            c.flex.basis = Len::Auto;
            return;
        }
        "initial" => {
            c.flex.grow = 0;
            c.flex.shrink = 1000;
            c.flex.basis = Len::Auto;
            return;
        }
        _ => {}
    }
    let t = tokens(&l);
    let (mut grow, mut shrink, mut basis) = (None, None, None);
    for tok in t.iter().take(3) {
        // A bare number is a grow factor first, a shrink factor second.
        if let Some((n, rest)) = parse_milli(tok)
            && rest.is_empty()
        {
            if grow.is_none() {
                grow = Some(n);
            } else {
                shrink = Some(n);
            }
        } else if let Some(b) = parse_len(tok, em) {
            basis = Some(b);
        }
    }
    c.flex.grow = grow.unwrap_or(1000).clamp(0, 100_000) as i32;
    c.flex.shrink = shrink.unwrap_or(1000).clamp(0, 100_000) as i32;
    // `flex: 1` means a basis of 0%: the items share all the room.
    c.flex.basis = basis.unwrap_or(Len::Pct(0));
}

/// `grid-template-columns`: lengths, `fr`, `auto`, `minmax()` and `repeat()`.
pub(super) fn parse_tracks(v: &str, em: i32) -> GridSpec {
    let mut spec = GridSpec::default();
    tracks_into(&mut spec, v, em, 0);
    spec
}

fn track_of(t: &str, em: i32) -> Option<Track> {
    let l = t.trim().to_ascii_lowercase();
    if l.starts_with('[') {
        return None;
    }
    if let Some(inner) = l.strip_prefix("minmax(").and_then(|r| r.strip_suffix(')')) {
        let max = inner.rsplit(',').next()?.trim();
        return track_of(max, em);
    }
    if let Some(fr) = l.strip_suffix("fr")
        && let Some((n, rest)) = parse_milli(fr)
        && rest.is_empty()
    {
        return Some(Track::Fr(n.clamp(0, 100_000) as i32));
    }
    match parse_len(&l, em) {
        Some(Len::Px(p)) => Some(Track::Px(p)),
        Some(Len::Pct(p)) => Some(Track::Pct(p)),
        _ => Some(Track::Auto),
    }
}

fn tracks_into(spec: &mut GridSpec, v: &str, em: i32, depth: u8) {
    if depth > 2 {
        return;
    }
    for t in tokens(v) {
        let l = t.to_ascii_lowercase();
        if let Some(inner) = l.strip_prefix("repeat(").and_then(|r| r.strip_suffix(')')) {
            let (count, list) = inner.split_once(',').unwrap_or(("1", inner));
            let count = count.trim();
            if count == "auto-fill" || count == "auto-fit" {
                // `minmax(<min>, 1fr)` or a plain length: as many as fit, each at least that wide.
                let first = tokens(list).first().copied().unwrap_or("");
                let min = if let Some(m) = first.strip_prefix("minmax(") {
                    m.split(',').next().and_then(|a| parse_px(a, em))
                } else {
                    parse_px(first, em)
                };
                spec.auto_min = Some(min.unwrap_or(200).clamp(40, 2000));
            } else if let Ok(n) = count.parse::<usize>() {
                let mut one = GridSpec::default();
                tracks_into(&mut one, list, em, depth + 1);
                for _ in 0..n.min(12) {
                    if spec.cols.len() + one.cols.len() > 12 {
                        break;
                    }
                    spec.cols.extend_from_slice(&one.cols);
                }
            }
        } else if let Some(tr) = track_of(t, em)
            && spec.cols.len() < 12
        {
            spec.cols.push(tr);
        }
    }
}
