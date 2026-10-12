//! A small SVG renderer for pictures on web pages (logos, icons, simple illustrations).
//!
//! Integer arithmetic only (the kernel is soft-float): coordinates are 16.16 fixed point while
//! parsing, 24.8 in device space. Shapes are filled by a scanline rasteriser with four sub-scanlines
//! per pixel and exact horizontal coverage; strokes are drawn as unions of thick segments and
//! round-ish joints.
//!
//! Supported: `svg` (`viewBox`, `width`, `height`), `g`, `path` (all commands; an elliptical arc is
//! flattened to its chord), `rect` (with `rx`), `circle`, `ellipse`, `line`, `polyline`, `polygon`,
//! `fill`, `stroke`, `stroke-width`, the three opacities, `fill-rule`, `transform` (`matrix`,
//! `translate`, `scale`, `rotate`, `skewX/Y`), `style="..."`, solid colours (`#rgb`, `#rrggbb`,
//! `rgb()`, names) and gradients (the first stop stands for the whole gradient). Not drawn: text,
//! `use`, filters, masks, clip paths, dashes, `<style>` classes.
//!
//! Hostile input is bounded: element count, path segments, nesting and output pixels.

// Fixed-point maths reads best as `a * b >> 16`.
#![allow(clippy::precedence)]

use super::image::{Image, over, rgba};
use alloc::vec::Vec;

/// Largest picture drawn, in pixels.
pub const MAX_PIXELS: usize = 4 * 1024 * 1024;
const MAX_ELEMENTS: usize = 6000;
const MAX_SEGMENTS: usize = 60_000;
const MAX_DEPTH: usize = 24;
/// Sub-scanlines per pixel.
const SS: i64 = 4;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SvgError {
    /// Not an SVG document.
    NotSvg,
    /// A size of zero, or a picture over [`MAX_PIXELS`].
    BadSize,
    OutOfMemory,
}

/// Does `bytes` look like an SVG document (an `<svg` element near the start)?
pub fn looks_like(bytes: &[u8]) -> bool {
    let head = &bytes[..bytes.len().min(2048)];
    let lower: Vec<u8> = head.iter().map(u8::to_ascii_lowercase).collect();
    let t = lower
        .iter()
        .position(|b| !b.is_ascii_whitespace() && *b != 0xef && *b != 0xbb && *b != 0xbf)
        .unwrap_or(0);
    let start = &lower[t..];
    (start.starts_with(b"<svg")
        || start.starts_with(b"<?xml")
        || start.starts_with(b"<!doctype svg")
        || start.starts_with(b"<!--"))
        && find(&lower, b"<svg").is_some()
}

fn find(h: &[u8], n: &[u8]) -> Option<usize> {
    h.windows(n.len()).position(|w| w == n)
}

// ---- numbers ----

/// A decimal number (sign, digits, fraction, exponent) as 16.16 fixed point, and the rest of `s`.
fn num(s: &str) -> Option<(i64, &str)> {
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
        int = (int * 10 + i64::from(b[i] - b'0')).min(1 << 30);
        i += 1;
    }
    let mut frac: i64 = 0;
    let mut digits = i - start;
    if i < b.len() && b[i] == b'.' {
        i += 1;
        let mut scale: i64 = 65536;
        while i < b.len() && b[i].is_ascii_digit() {
            scale /= 10;
            frac += i64::from(b[i] - b'0') * scale;
            i += 1;
            digits += 1;
        }
    }
    if digits == 0 {
        return None;
    }
    let mut v = int * 65536 + frac;
    if i < b.len() && (b[i] == b'e' || b[i] == b'E') {
        let mut j = i + 1;
        let eneg = b.get(j) == Some(&b'-');
        if matches!(b.get(j), Some(b'-' | b'+')) {
            j += 1;
        }
        let es = j;
        let mut e = 0i32;
        while j < b.len() && b[j].is_ascii_digit() {
            e = (e * 10 + i32::from(b[j] - b'0')).min(20);
            j += 1;
        }
        if j > es {
            for _ in 0..e {
                v = if eneg {
                    v / 10
                } else {
                    v.saturating_mul(10).min(1 << 46)
                };
            }
            i = j;
        }
    }
    Some((if neg { -v } else { v }, &s[i..]))
}

fn skip_sep(s: &str) -> &str {
    s.trim_start_matches(|c: char| c.is_ascii_whitespace() || c == ',')
}

/// A length attribute (`12`, `12px`, `50%`) as 16.16; percentages need `base`.
fn length(s: &str, base: i64) -> Option<i64> {
    let (n, rest) = num(s.trim())?;
    Some(match rest.trim() {
        "" | "px" => n,
        "pt" => n * 4 / 3,
        "pc" => n * 16,
        "in" => n * 96,
        "mm" => n * 96 * 10 / 254,
        "cm" => n * 96 * 100 / 254,
        "em" => n * 16,
        "%" => n * base / 100,
        _ => return None,
    })
}

// ---- XML ----

struct Tag<'a> {
    name: &'a str,
    /// `key="value"` pairs (the value decoded for the five predefined entities only).
    attrs: Vec<(&'a str, &'a str)>,
    open: bool,
    close: bool,
}

impl<'a> Tag<'a> {
    fn get(&self, k: &str) -> Option<&'a str> {
        self.attrs.iter().find(|(n, _)| *n == k).map(|(_, v)| *v)
    }
}

/// The next tag at or after `*pos` (comments, declarations and text are skipped).
fn next_tag<'a>(s: &'a str, pos: &mut usize) -> Option<Tag<'a>> {
    loop {
        let i = s[*pos..].find('<')? + *pos;
        let rest = &s[i..];
        if rest.starts_with("<!--") {
            *pos = rest.find("-->").map_or(s.len(), |e| i + e + 3);
            continue;
        }
        if rest.starts_with("<![CDATA[") {
            *pos = rest.find("]]>").map_or(s.len(), |e| i + e + 3);
            continue;
        }
        if rest.starts_with("<?") || rest.starts_with("<!") {
            *pos = rest.find('>').map_or(s.len(), |e| i + e + 1);
            continue;
        }
        // Find the closing `>` outside quotes.
        let b = rest.as_bytes();
        let mut j = 1;
        let mut q = 0u8;
        while j < b.len() {
            match (q, b[j]) {
                (0, b'"' | b'\'') => q = b[j],
                (c, d) if c != 0 && c == d => q = 0,
                (0, b'>') => break,
                _ => {}
            }
            j += 1;
        }
        if j >= b.len() {
            *pos = s.len();
            return None;
        }
        let inner = &rest[1..j];
        *pos = i + j + 1;
        let (close, inner) = match inner.strip_prefix('/') {
            Some(r) => (true, r),
            None => (false, inner),
        };
        let (self_close, inner) = match inner.strip_suffix('/') {
            Some(r) => (true, r),
            None => (false, inner),
        };
        let name_end = inner
            .find(|c: char| c.is_ascii_whitespace())
            .unwrap_or(inner.len());
        let name = &inner[..name_end];
        // The namespace prefix (`svg:rect`) does not matter.
        let name = name.rsplit(':').next().unwrap_or(name);
        let mut attrs = Vec::new();
        let mut a = inner[name_end..].trim_start();
        while let Some(eq) = a.find('=') {
            let key = a[..eq].trim();
            let v = a[eq + 1..].trim_start();
            let (val, rest) = match v.chars().next() {
                Some(c @ ('"' | '\'')) => match v[1..].find(c) {
                    Some(e) => (&v[1..1 + e], &v[e + 2..]),
                    None => break,
                },
                _ => {
                    let e = v.find(|c: char| c.is_ascii_whitespace()).unwrap_or(v.len());
                    (&v[..e], &v[e..])
                }
            };
            if attrs.len() < 40 {
                attrs.push((key, val));
            }
            a = rest.trim_start();
        }
        return Some(Tag {
            name,
            attrs,
            open: !close,
            close: close || self_close,
        });
    }
}

// ---- document size ----

/// `(width, height)` of the picture in pixels from the root element, without rendering.
pub fn dims(bytes: &[u8]) -> Option<(usize, usize)> {
    let s = core::str::from_utf8(bytes).ok()?;
    let mut pos = 0;
    while let Some(t) = next_tag(s, &mut pos) {
        if t.name.eq_ignore_ascii_case("svg") && t.open {
            return root_size(&t).map(|(w, h, ..)| (w, h));
        }
    }
    None
}

/// `(width, height, view box)` of the root: the declared size, else the view box, else 300 x 150.
fn root_size(t: &Tag) -> Option<(usize, usize, [i64; 4])> {
    let vb = t.get("viewBox").or_else(|| t.get("viewbox")).and_then(|v| {
        let mut s = v.trim();
        let mut out = [0i64; 4];
        for o in &mut out {
            let (n, r) = num(skip_sep(s))?;
            *o = n;
            s = r;
        }
        (out[2] > 0 && out[3] > 0).then_some(out)
    });
    let w = t
        .get("width")
        .and_then(|w| length(w, vb.map_or(300 << 16, |v| v[2])));
    let h = t
        .get("height")
        .and_then(|h| length(h, vb.map_or(150 << 16, |v| v[3])));
    let (w, h) = match (w, h, vb) {
        (Some(w), Some(h), _) => (w, h),
        (Some(w), None, Some(v)) => (w, w * v[3] / v[2].max(1)),
        (None, Some(h), Some(v)) => (h * v[2] / v[3].max(1), h),
        (None, None, Some(v)) => (v[2], v[3]),
        _ => (300 << 16, 150 << 16),
    };
    let (w, h) = (((w + 32768) >> 16) as usize, ((h + 32768) >> 16) as usize);
    if w == 0 || h == 0 || w > 16384 || h > 16384 {
        return None;
    }
    let view = vb.unwrap_or([0, 0, (w as i64) << 16, (h as i64) << 16]);
    Some((w, h, view))
}

// ---- transform ----

/// `x' = a x + c y + e`, `y' = b x + d y + f`, all 16.16.
#[derive(Clone, Copy)]
struct Mat {
    a: i64,
    b: i64,
    c: i64,
    d: i64,
    e: i64,
    f: i64,
}

impl Mat {
    const ID: Mat = Mat {
        a: 65536,
        b: 0,
        c: 0,
        d: 65536,
        e: 0,
        f: 0,
    };

    /// `self` applied after `o` (the point goes through `o` first).
    fn mul(self, o: Mat) -> Mat {
        let m = |x: i64, y: i64| (x * y) >> 16;
        Mat {
            a: m(self.a, o.a) + m(self.c, o.b),
            b: m(self.b, o.a) + m(self.d, o.b),
            c: m(self.a, o.c) + m(self.c, o.d),
            d: m(self.b, o.c) + m(self.d, o.d),
            e: m(self.a, o.e) + m(self.c, o.f) + self.e,
            f: m(self.b, o.e) + m(self.d, o.f) + self.f,
        }
    }

    /// A 16.16 point to device 24.8.
    fn apply(&self, x: i64, y: i64) -> (i64, i64) {
        let m = |p: i64, q: i64| (p * q) >> 16;
        let (nx, ny) = (
            m(self.a, x) + m(self.c, y) + self.e,
            m(self.b, x) + m(self.d, y) + self.f,
        );
        (nx >> 8, ny >> 8)
    }

    /// How much lengths grow (square root of the area scale), 16.16.
    fn scale(&self) -> i64 {
        let det = ((self.a * self.d - self.b * self.c) >> 16).abs();
        isqrt(det << 16)
    }
}

fn isqrt(n: i64) -> i64 {
    if n <= 0 {
        return 0;
    }
    let mut x = n;
    let mut y = (x + 1) / 2;
    while y < x {
        x = y;
        y = (x + n / x) / 2;
    }
    x
}

/// Sine and cosine of `deg` degrees (16.16), by a quarter-wave table of 16 steps and linear
/// interpolation: good to about 0.2 %, which is a fraction of a pixel on a logo.
fn sin_cos(deg: i64) -> (i64, i64) {
    const TABLE: [i64; 17] = [
        0, 6393, 12540, 18205, 23170, 27246, 30274, 32138, 32768, 32138, 30274, 27246, 23170,
        18205, 12540, 6393, 0,
    ];
    // sin(k * 5.625 deg) * 65536 for k in 0..=16 is TABLE-like only up to 90: build it properly.
    let _ = TABLE;
    const SIN: [i64; 17] = [
        0, 6424, 12785, 19024, 25080, 30893, 36410, 41576, 46341, 50660, 54491, 57797, 60547,
        62714, 64277, 65220, 65536,
    ];
    let sin_q = |d: i64| -> i64 {
        // d in 0..=90 degrees.
        let pos = d * 16 * 256 / 90;
        let i = (pos >> 8) as usize;
        let f = pos & 255;
        if i >= 16 {
            SIN[16]
        } else {
            SIN[i] + ((SIN[i + 1] - SIN[i]) * f >> 8)
        }
    };
    let d = deg.rem_euclid(360);
    let sin = match d {
        0..=90 => sin_q(d),
        91..=180 => sin_q(180 - d),
        181..=270 => -sin_q(d - 180),
        _ => -sin_q(360 - d),
    };
    let cd = (d + 90) % 360;
    let cos = match cd {
        0..=90 => sin_q(cd),
        91..=180 => sin_q(180 - cd),
        181..=270 => -sin_q(cd - 180),
        _ => -sin_q(360 - cd),
    };
    (sin, cos)
}

fn parse_transform(s: &str) -> Mat {
    let mut m = Mat::ID;
    let mut rest = s;
    while let Some(open) = rest.find('(') {
        let name = rest[..open].trim().trim_start_matches([',', ' ']);
        let Some(close) = rest[open..].find(')') else {
            break;
        };
        let args_s = &rest[open + 1..open + close];
        rest = &rest[open + close + 1..];
        let mut args = [0i64; 6];
        let mut n = 0;
        let mut a = args_s;
        while n < 6 {
            match num(skip_sep(a)) {
                Some((v, r)) => {
                    args[n] = v;
                    n += 1;
                    a = r;
                }
                None => break,
            }
        }
        let t = match (name, n) {
            ("matrix", 6) => Mat {
                a: args[0],
                b: args[1],
                c: args[2],
                d: args[3],
                e: args[4],
                f: args[5],
            },
            ("translate", 1) => Mat {
                e: args[0],
                ..Mat::ID
            },
            ("translate", 2) => Mat {
                e: args[0],
                f: args[1],
                ..Mat::ID
            },
            ("scale", 1) => Mat {
                a: args[0],
                d: args[0],
                ..Mat::ID
            },
            ("scale", 2) => Mat {
                a: args[0],
                d: args[1],
                ..Mat::ID
            },
            ("rotate", 1 | 3) => {
                let (s, c) = sin_cos(args[0] >> 16);
                let r = Mat {
                    a: c,
                    b: s,
                    c: -s,
                    d: c,
                    e: 0,
                    f: 0,
                };
                if n == 3 {
                    let (cx, cy) = (args[1], args[2]);
                    let to = Mat {
                        e: cx,
                        f: cy,
                        ..Mat::ID
                    };
                    let back = Mat {
                        e: -cx,
                        f: -cy,
                        ..Mat::ID
                    };
                    to.mul(r).mul(back)
                } else {
                    r
                }
            }
            ("skewX", 1) => {
                let (s, c) = sin_cos(args[0] >> 16);
                Mat {
                    c: if c == 0 { 0 } else { (s << 16) / c },
                    ..Mat::ID
                }
            }
            ("skewY", 1) => {
                let (s, c) = sin_cos(args[0] >> 16);
                Mat {
                    b: if c == 0 { 0 } else { (s << 16) / c },
                    ..Mat::ID
                }
            }
            _ => Mat::ID,
        };
        m = m.mul(t);
    }
    m
}

// ---- paint ----

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Paint {
    None,
    /// 0xRRGGBB.
    Color(u32),
}

fn parse_color(s: &str) -> Option<u32> {
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
struct Style {
    fill: Paint,
    stroke: Paint,
    /// 16.16 user units.
    stroke_w: i64,
    /// Opacities 0..=255.
    opacity: u32,
    fill_op: u32,
    stroke_op: u32,
    even_odd: bool,
    /// What `currentColor` means.
    current: u32,
}

impl Style {
    fn root() -> Style {
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

struct Gradients<'a> {
    /// `(id, first stop colour)`.
    list: Vec<(&'a str, u32)>,
}

fn paint_of(v: &str, st: &Style, g: &Gradients) -> Option<Paint> {
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

fn opacity(v: &str) -> Option<u32> {
    let (n, rest) = num(v.trim())?;
    let n = if rest.trim() == "%" { n / 100 } else { n };
    Some(((n * 255) >> 16).clamp(0, 255) as u32)
}

/// Apply one presentation property to `st`.
fn apply_prop(st: &mut Style, k: &str, v: &str, g: &Gradients) {
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

fn style_of(parent: &Style, t: &Tag, g: &Gradients) -> Style {
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

// ---- geometry ----

type Pt = (i64, i64);

/// Flatten path data into polylines in device space (24.8). Each is `(points, closed)`.
fn flatten_path(d: &str, m: &Mat, out: &mut Vec<(Vec<Pt>, bool)>, budget: &mut usize) {
    let mut s = d.trim();
    let (mut cx, mut cy) = (0i64, 0i64); // current point, user space 16.16
    let (mut sx, mut sy) = (0i64, 0i64); // start of the subpath
    let (mut lcx, mut lcy) = (0i64, 0i64); // last control point
    let mut last = b' ';
    let mut cur: Vec<Pt> = Vec::new();
    let mut cmd = b' ';
    macro_rules! flush {
        ($closed:expr) => {
            if cur.len() > 1 || $closed && !cur.is_empty() {
                out.push((core::mem::take(&mut cur), $closed));
            } else {
                cur.clear();
            }
        };
    }
    loop {
        s = skip_sep(s);
        let Some(c) = s.bytes().next() else { break };
        if c.is_ascii_alphabetic() {
            cmd = c;
            s = &s[1..];
            if cmd == b'z' || cmd == b'Z' {
                flush!(true);
                cx = sx;
                cy = sy;
                last = b'z';
                continue;
            }
        } else if cmd == b' ' {
            break;
        }
        if *budget == 0 {
            return;
        }
        let rel = cmd.is_ascii_lowercase();
        let up = cmd.to_ascii_uppercase();
        let take = |n: usize, s: &mut &str| -> Option<[i64; 7]> {
            let mut v = [0i64; 7];
            for o in v.iter_mut().take(n) {
                let (x, r) = num(skip_sep(s))?;
                *o = x;
                *s = r;
            }
            Some(v)
        };
        let (ox, oy) = if rel { (cx, cy) } else { (0, 0) };
        match up {
            b'M' => {
                let Some(v) = take(2, &mut s) else { break };
                flush!(false);
                cx = ox + v[0];
                cy = oy + v[1];
                sx = cx;
                sy = cy;
                cur.push(m.apply(cx, cy));
                // Further pairs are implicit linetos.
                cmd = if rel { b'l' } else { b'L' };
            }
            b'L' | b'T' if up == b'L' => {
                let Some(v) = take(2, &mut s) else { break };
                if cur.is_empty() {
                    cur.push(m.apply(cx, cy));
                }
                cx = ox + v[0];
                cy = oy + v[1];
                cur.push(m.apply(cx, cy));
            }
            b'H' => {
                let Some(v) = take(1, &mut s) else { break };
                if cur.is_empty() {
                    cur.push(m.apply(cx, cy));
                }
                cx = ox + v[0];
                cur.push(m.apply(cx, cy));
            }
            b'V' => {
                let Some(v) = take(1, &mut s) else { break };
                if cur.is_empty() {
                    cur.push(m.apply(cx, cy));
                }
                cy = oy + v[0];
                cur.push(m.apply(cx, cy));
            }
            b'C' | b'S' => {
                let (x1, y1, x2, y2, x, y);
                if up == b'C' {
                    let Some(v) = take(6, &mut s) else { break };
                    (x1, y1) = (ox + v[0], oy + v[1]);
                    (x2, y2) = (ox + v[2], oy + v[3]);
                    (x, y) = (ox + v[4], oy + v[5]);
                } else {
                    let Some(v) = take(4, &mut s) else { break };
                    // The first control point mirrors the last one of a previous curve.
                    (x1, y1) = if matches!(last, b'C' | b'S') {
                        (2 * cx - lcx, 2 * cy - lcy)
                    } else {
                        (cx, cy)
                    };
                    (x2, y2) = (ox + v[0], oy + v[1]);
                    (x, y) = (ox + v[2], oy + v[3]);
                }
                if cur.is_empty() {
                    cur.push(m.apply(cx, cy));
                }
                cubic(&mut cur, m, (cx, cy), (x1, y1), (x2, y2), (x, y));
                lcx = x2;
                lcy = y2;
                cx = x;
                cy = y;
            }
            b'Q' | b'T' => {
                let (x1, y1, x, y);
                if up == b'Q' {
                    let Some(v) = take(4, &mut s) else { break };
                    (x1, y1) = (ox + v[0], oy + v[1]);
                    (x, y) = (ox + v[2], oy + v[3]);
                } else {
                    let Some(v) = take(2, &mut s) else { break };
                    (x1, y1) = if matches!(last, b'Q' | b'T') {
                        (2 * cx - lcx, 2 * cy - lcy)
                    } else {
                        (cx, cy)
                    };
                    (x, y) = (ox + v[0], oy + v[1]);
                }
                if cur.is_empty() {
                    cur.push(m.apply(cx, cy));
                }
                // A quadratic as the cubic with the same shape.
                let c1 = (cx + 2 * (x1 - cx) / 3, cy + 2 * (y1 - cy) / 3);
                let c2 = (x + 2 * (x1 - x) / 3, y + 2 * (y1 - y) / 3);
                cubic(&mut cur, m, (cx, cy), c1, c2, (x, y));
                lcx = x1;
                lcy = y1;
                cx = x;
                cy = y;
            }
            b'A' => {
                // rx ry rotation large-arc sweep x y: the two flags may be written without
                // separators, so they are read one character at a time.
                let Some(v3) = take(3, &mut s) else { break };
                let _ = v3;
                let mut flags = [0u8; 2];
                for f in &mut flags {
                    s = skip_sep(s);
                    match s.bytes().next() {
                        Some(c @ (b'0' | b'1')) => {
                            *f = c - b'0';
                            s = &s[1..];
                        }
                        _ => return,
                    }
                }
                let Some(v2) = take(2, &mut s) else { break };
                if cur.is_empty() {
                    cur.push(m.apply(cx, cy));
                }
                // Flattened to its chord.
                cx = ox + v2[0];
                cy = oy + v2[1];
                cur.push(m.apply(cx, cy));
            }
            _ => break,
        }
        *budget = budget.saturating_sub(1);
        last = up;
    }
    flush!(false);
}

fn cubic(out: &mut Vec<Pt>, m: &Mat, p0: Pt, p1: Pt, p2: Pt, p3: Pt) {
    let d = [
        m.apply(p0.0, p0.1),
        m.apply(p1.0, p1.1),
        m.apply(p2.0, p2.1),
        m.apply(p3.0, p3.1),
    ];
    // Steps by the length of the control polygon in pixels (24.8 units).
    let mut len = 0i64;
    for w in d.windows(2) {
        len += isqrt((w[1].0 - w[0].0).pow(2) + (w[1].1 - w[0].1).pow(2));
    }
    let n = ((len >> 8) / 2).clamp(3, 32);
    for i in 1..=n {
        let t = (i << 16) / n; // 16.16
        let u = 65536 - t;
        let (a, b, c, e) = (
            (u * u >> 16) * u >> 16,
            3 * ((u * u >> 16) * t >> 16),
            3 * ((u * t >> 16) * t >> 16),
            (t * t >> 16) * t >> 16,
        );
        let x = (a * d[0].0 + b * d[1].0 + c * d[2].0 + e * d[3].0) >> 16;
        let y = (a * d[0].1 + b * d[1].1 + c * d[2].1 + e * d[3].1) >> 16;
        out.push((x, y));
    }
}

/// A circle or ellipse as a polygon in device space.
fn ellipse_poly(m: &Mat, cx: i64, cy: i64, rx: i64, ry: i64) -> Vec<Pt> {
    let mut v = Vec::new();
    // Largest device radius decides the number of points.
    let r = (rx.max(ry) * m.scale()) >> 16 >> 8;
    let n = (r.clamp(4, 120) * 2).clamp(16, 96);
    for i in 0..n {
        let (s, c) = sin_cos(i * 360 / n);
        v.push(m.apply(cx + (rx * c >> 16), cy + (ry * s >> 16)));
    }
    v
}

fn rect_poly(m: &Mat, x: i64, y: i64, w: i64, h: i64, rx: i64, ry: i64) -> Vec<Pt> {
    let (rx, ry) = (rx.min(w / 2), ry.min(h / 2));
    if rx <= 0 || ry <= 0 {
        return alloc::vec![
            m.apply(x, y),
            m.apply(x + w, y),
            m.apply(x + w, y + h),
            m.apply(x, y + h)
        ];
    }
    let mut v = Vec::new();
    let steps = 6;
    let corners = [
        (x + w - rx, y + ry, 270),
        (x + w - rx, y + h - ry, 0),
        (x + rx, y + h - ry, 90),
        (x + rx, y + ry, 180),
    ];
    for (cx, cy, start) in corners {
        for k in 0..=steps {
            let (s, c) = sin_cos(start + k * 90 / steps);
            v.push(m.apply(cx + (rx * c >> 16), cy + (ry * s >> 16)));
        }
    }
    v
}

/// Thick segments and joints for a polyline, as polygons (nonzero union when filled).
fn stroke_polys(pts: &[Pt], closed: bool, half: i64, out: &mut Vec<Vec<Pt>>) {
    let n = pts.len();
    if n == 0 {
        return;
    }
    let half = half.max(64); // at least a quarter pixel each side: hairlines stay visible
    let segs = if closed { n } else { n - 1 };
    for i in 0..segs {
        let (a, b) = (pts[i], pts[(i + 1) % n]);
        let (dx, dy) = (b.0 - a.0, b.1 - a.1);
        let len = isqrt(dx * dx + dy * dy);
        if len == 0 {
            continue;
        }
        let (nx, ny) = (-dy * half / len, dx * half / len);
        out.push(alloc::vec![
            (a.0 + nx, a.1 + ny),
            (b.0 + nx, b.1 + ny),
            (b.0 - nx, b.1 - ny),
            (a.0 - nx, a.1 - ny)
        ]);
    }
    // Joints and caps: a small round polygon at every vertex.
    for &(x, y) in pts {
        let mut c = Vec::new();
        for k in 0..8 {
            let (s, co) = sin_cos(k * 45);
            c.push((x + (half * co >> 16), y + (half * s >> 16)));
        }
        out.push(c);
    }
}

// ---- rasteriser ----

struct Canvas {
    w: usize,
    h: usize,
    px: Vec<u32>,
}

/// Fill the union of `polys` (24.8 device coordinates) with `color` at `alpha`.
fn fill(cv: &mut Canvas, polys: &[Vec<Pt>], even_odd: bool, color: u32, alpha: u32) {
    if alpha == 0 {
        return;
    }
    struct Edge {
        x0: i64,
        y0: i64,
        x1: i64,
        y1: i64,
        dir: i32,
    }
    let mut edges: Vec<Edge> = Vec::new();
    let (mut ymin, mut ymax) = (i64::MAX, i64::MIN);
    for p in polys {
        let n = p.len();
        if n < 3 {
            continue;
        }
        for i in 0..n {
            let (a, b) = (p[i], p[(i + 1) % n]);
            if a.1 == b.1 {
                continue;
            }
            let (e, dir) = if a.1 < b.1 { ((a, b), 1) } else { ((b, a), -1) };
            ymin = ymin.min(e.0.1);
            ymax = ymax.max(e.1.1);
            edges.push(Edge {
                x0: e.0.0,
                y0: e.0.1,
                x1: e.1.0,
                y1: e.1.1,
                dir,
            });
        }
    }
    if edges.is_empty() {
        return;
    }
    let first = (ymin * SS / 256).max(0);
    let last = ((ymax * SS / 256) + 1).min(cv.h as i64 * SS);
    let mut cov = alloc::vec![0u32; cv.w];
    let mut touched = (usize::MAX, 0usize);
    let mut xs: Vec<(i64, i32)> = Vec::new();
    let mut row_y = first / SS;
    for sy in first..last {
        // The sample is at the middle of the sub-scanline (24.8).
        let y = (sy * 256 + 128) / SS;
        xs.clear();
        for e in &edges {
            if y >= e.y0 && y < e.y1 {
                let x = e.x0 + (e.x1 - e.x0) * (y - e.y0) / (e.y1 - e.y0);
                xs.push((x, e.dir));
            }
        }
        xs.sort_unstable_by_key(|c| c.0);
        let mut wind = 0i32;
        for i in 0..xs.len() {
            wind += xs[i].1;
            let inside = if even_odd {
                (i + 1) % 2 == 1
            } else {
                wind != 0
            };
            if inside && i + 1 < xs.len() {
                span(&mut cov, cv.w, xs[i].0, xs[i + 1].0, &mut touched);
            }
        }
        if (sy + 1) % SS == 0 || sy + 1 == last {
            flush_row(cv, row_y as usize, &mut cov, &mut touched, color, alpha);
            row_y += 1;
        }
    }
}

/// Add the horizontal coverage of `[xa, xb)` (24.8) to one sub-scanline's pixels.
fn span(cov: &mut [u32], w: usize, xa: i64, xb: i64, touched: &mut (usize, usize)) {
    let xa = xa.max(0);
    let xb = xb.min(w as i64 * 256);
    if xb <= xa {
        return;
    }
    let (pa, pb) = ((xa >> 8) as usize, ((xb - 1) >> 8) as usize);
    touched.0 = touched.0.min(pa);
    touched.1 = touched.1.max(pb);
    if pa == pb {
        cov[pa] += (xb - xa) as u32;
        return;
    }
    cov[pa] += (256 - (xa & 255)) as u32;
    for c in &mut cov[pa + 1..pb] {
        *c += 256;
    }
    cov[pb] += (((xb - 1) & 255) + 1) as u32;
}

fn flush_row(
    cv: &mut Canvas,
    y: usize,
    cov: &mut [u32],
    touched: &mut (usize, usize),
    color: u32,
    alpha: u32,
) {
    if touched.0 <= touched.1 && y < cv.h {
        let end = touched.1.min(cv.w - 1);
        for (x, slot) in cov.iter_mut().enumerate().take(end + 1).skip(touched.0) {
            let c = core::mem::take(slot);
            if c == 0 {
                continue;
            }
            let a = (c as u64 * alpha as u64 / (256 * SS as u64)).min(255) as u32;
            let idx = y * cv.w + x;
            cv.px[idx] = over(
                rgba(
                    (color >> 16) as u8,
                    (color >> 8) as u8,
                    color as u8,
                    a as u8,
                ),
                cv.px[idx],
            );
        }
    }
    *touched = (usize::MAX, 0);
}

// ---- the document ----

/// Render `bytes` to an image at most `max_w` pixels wide (the aspect ratio kept; never enlarged
/// past the declared size).
pub fn render(bytes: &[u8], max_w: usize) -> Result<Image, SvgError> {
    let s = core::str::from_utf8(bytes).map_err(|_| SvgError::NotSvg)?;
    // Gradients first: they may be defined after their use.
    let mut grads = Gradients { list: Vec::new() };
    {
        let mut pos = 0;
        let mut cur_id: Option<&str> = None;
        let mut n = 0;
        while let Some(t) = next_tag(s, &mut pos) {
            n += 1;
            if n > MAX_ELEMENTS {
                break;
            }
            if t.name.ends_with("Gradient") && t.open {
                cur_id = t.get("id");
            } else if t.name == "stop"
                && let Some(id) = cur_id
                && !grads.list.iter().any(|(g, _)| *g == id)
            {
                let mut c = t.get("stop-color").and_then(parse_color);
                if c.is_none()
                    && let Some(st) = t.get("style")
                {
                    for d in st.split(';') {
                        if let Some(("stop-color", v)) =
                            d.split_once(':').map(|(k, v)| (k.trim(), v.trim()))
                        {
                            c = parse_color(v);
                        }
                    }
                }
                if let Some(c) = c {
                    grads.list.push((id, c));
                }
            } else if t.name.ends_with("Gradient") && t.close {
                cur_id = None;
            }
        }
    }
    let mut pos = 0;
    let (mut root, mut found) = (None, false);
    while let Some(t) = next_tag(s, &mut pos) {
        if t.name.eq_ignore_ascii_case("svg") && t.open {
            root = Some(root_size(&t).ok_or(SvgError::BadSize)?);
            found = true;
            break;
        }
    }
    if !found {
        return Err(SvgError::NotSvg);
    }
    let (nat_w, nat_h, view) = root.ok_or(SvgError::BadSize)?;
    let out_w = nat_w.min(max_w.max(1));
    let out_h = ((nat_h as u64 * out_w as u64 + nat_w as u64 / 2) / nat_w as u64).max(1) as usize;
    if out_w.checked_mul(out_h).is_none_or(|n| n > MAX_PIXELS) {
        return Err(SvgError::BadSize);
    }
    let mut cv = Canvas {
        w: out_w,
        h: out_h,
        px: Vec::new(),
    };
    cv.px
        .try_reserve_exact(out_w * out_h)
        .map_err(|_| SvgError::OutOfMemory)?;
    cv.px.resize(out_w * out_h, 0);
    // View box to device pixels (preserveAspectRatio xMidYMid meet).
    let sx = ((out_w as i64) << 16 << 16) / view[2].max(1);
    let sy = ((out_h as i64) << 16 << 16) / view[3].max(1);
    let k = sx.min(sy);
    let ox = (((out_w as i64) << 16) - (view[2] * k >> 16)) / 2;
    let oy = (((out_h as i64) << 16) - (view[3] * k >> 16)) / 2;
    let base = Mat {
        a: k,
        b: 0,
        c: 0,
        d: k,
        e: ox - (view[0] * k >> 16),
        f: oy - (view[1] * k >> 16),
    };
    walk(s, pos, &mut cv, base, &grads);
    Image::from_pixels(out_w, out_h, cv.px).map_err(|_| SvgError::BadSize)
}

fn walk(s: &str, mut pos: usize, cv: &mut Canvas, base: Mat, g: &Gradients) {
    struct Frame {
        m: Mat,
        st: Style,
        /// Inside `defs`, `symbol`, `clipPath`, `mask`, `pattern`, `text`...: not drawn.
        skip: bool,
    }
    let mut stack: Vec<Frame> = alloc::vec![Frame {
        m: base,
        st: Style::root(),
        skip: false
    }];
    let mut budget = MAX_SEGMENTS;
    let mut elements = 0usize;
    while let Some(t) = next_tag(s, &mut pos) {
        elements += 1;
        if elements > MAX_ELEMENTS {
            return;
        }
        if t.close && !t.open {
            if stack.len() > 1 {
                stack.pop();
            }
            continue;
        }
        let Some(top) = stack.last() else { return };
        let mut m = top.m;
        if let Some(tr) = t.get("transform") {
            m = m.mul(parse_transform(tr));
        }
        let st = style_of(&top.st, &t, g);
        let hidden = t.get("display").is_some_and(|d| d.trim() == "none")
            || t.get("visibility").is_some_and(|d| d.trim() == "hidden")
            || t.get("style").is_some_and(|c| c.contains("display:none"));
        let container_skip = matches!(
            t.name,
            "defs"
                | "symbol"
                | "clipPath"
                | "mask"
                | "pattern"
                | "marker"
                | "text"
                | "title"
                | "desc"
                | "style"
                | "metadata"
                | "linearGradient"
                | "radialGradient"
                | "filter"
                | "script"
                | "foreignObject"
        );
        let skip = top.skip || hidden || container_skip || t.get("clip-path").is_some() && false;
        let shape = if skip {
            None
        } else {
            shape_of(&t, &m, &mut budget)
        };
        if let Some(polys) = shape {
            draw_shape(cv, &polys, &t, &m, &st);
        }
        if t.open && !t.close {
            if stack.len() >= MAX_DEPTH {
                return;
            }
            stack.push(Frame { m, st, skip });
        }
    }
}

/// The outline of a shape element in device space, as `(polygon, closed)` pieces.
fn shape_of(t: &Tag, m: &Mat, budget: &mut usize) -> Option<Vec<(Vec<Pt>, bool)>> {
    let f = |k: &str| t.get(k).and_then(|v| length(v, 100 << 16)).unwrap_or(0);
    match t.name {
        "path" => {
            let mut out = Vec::new();
            flatten_path(t.get("d")?, m, &mut out, budget);
            Some(out)
        }
        "rect" => {
            let (w, h) = (f("width"), f("height"));
            if w <= 0 || h <= 0 {
                return None;
            }
            let (mut rx, mut ry) = (f("rx"), f("ry"));
            if rx == 0 {
                rx = ry;
            }
            if ry == 0 {
                ry = rx;
            }
            Some(alloc::vec![(
                rect_poly(m, f("x"), f("y"), w, h, rx, ry),
                true
            )])
        }
        "circle" => {
            let r = f("r");
            (r > 0).then(|| alloc::vec![(ellipse_poly(m, f("cx"), f("cy"), r, r), true)])
        }
        "ellipse" => {
            let (rx, ry) = (f("rx"), f("ry"));
            (rx > 0 && ry > 0)
                .then(|| alloc::vec![(ellipse_poly(m, f("cx"), f("cy"), rx, ry), true)])
        }
        "line" => Some(alloc::vec![(
            alloc::vec![m.apply(f("x1"), f("y1")), m.apply(f("x2"), f("y2"))],
            false
        )]),
        "polyline" | "polygon" => {
            let mut pts = Vec::new();
            let mut s = t.get("points")?;
            let mut nums = Vec::new();
            while let Some((v, r)) = num(skip_sep(s)) {
                nums.push(v);
                s = r;
                if nums.len() > 20_000 {
                    break;
                }
            }
            for c in nums.as_chunks::<2>().0 {
                pts.push(m.apply(c[0], c[1]));
            }
            Some(alloc::vec![(pts, t.name == "polygon")])
        }
        _ => None,
    }
}

fn draw_shape(cv: &mut Canvas, shape: &[(Vec<Pt>, bool)], t: &Tag, m: &Mat, st: &Style) {
    let _ = t;
    let op = st.opacity;
    if let Paint::Color(c) = st.fill {
        // A line has no area to fill.
        let polys: Vec<Vec<Pt>> = shape
            .iter()
            .filter(|(p, _)| p.len() > 2)
            .map(|(p, _)| p.clone())
            .collect();
        fill(cv, &polys, st.even_odd, c, st.fill_op * op / 255);
    }
    if let Paint::Color(c) = st.stroke {
        let half = (st.stroke_w * m.scale() >> 16) >> 9; // half the width, 24.8 device units
        let mut polys = Vec::new();
        for (p, closed) in shape {
            stroke_polys(p, *closed, half, &mut polys);
        }
        fill(cv, &polys, false, c, st.stroke_op * op / 255);
    }
}

#[cfg(test)]
mod tests;
