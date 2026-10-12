//! `@media` conditions and the viewport they are checked against.
//!
//! A condition is a comma separated list of alternatives; an alternative is media types and
//! `(feature: value)` tests joined by `and`, optionally negated with `not`. Features the engine can
//! answer: the viewport's width and height (`min-`, `max-`, plain, and the range syntax
//! `(width >= 600px)`), the colour scheme (always light), reduced motion (no preference), hover and
//! pointer (a mouse), orientation, and pixel density (1). Anything else makes the alternative false,
//! so a rule is never applied on a guess.

use alloc::vec::Vec;
use core::sync::atomic::{AtomicI32, Ordering};

/// The viewport assumed when none was set (a laptop window).
pub const DEFAULT_W: i32 = 960;
pub const DEFAULT_H: i32 = 700;

static VIEWPORT_W: AtomicI32 = AtomicI32::new(DEFAULT_W);
static VIEWPORT_H: AtomicI32 = AtomicI32::new(DEFAULT_H);

/// The viewport the page is laid out for, in CSS pixels (zoom removed). `vw`, `vh` and `@media`
/// read it; the layout sets it before it styles anything.
pub fn set_viewport(w: i32, h: i32) {
    VIEWPORT_W.store(w.clamp(1, 20_000), Ordering::Relaxed);
    VIEWPORT_H.store(h.clamp(1, 20_000), Ordering::Relaxed);
}

pub fn viewport_w() -> i32 {
    VIEWPORT_W.load(Ordering::Relaxed)
}

pub fn viewport_h() -> i32 {
    VIEWPORT_H.load(Ordering::Relaxed)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Test {
    /// Viewport width in `lo..=hi` (CSS px).
    Width(i32, i32),
    Height(i32, i32),
    /// A feature with a fixed answer.
    Const(bool),
    /// `orientation: landscape` (true) or `portrait` (false).
    Landscape(bool),
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Alt {
    negated: bool,
    tests: Vec<Test>,
}

/// A parsed `@media` condition.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Media {
    alts: Vec<Alt>,
}

impl Media {
    /// Parse the text between `@media` and `{`. `None` when nothing in it can be true on a screen
    /// (so the block is dropped at once).
    pub fn parse(text: &str) -> Option<Media> {
        let text = text.trim();
        let mut alts = Vec::new();
        for part in split_top(text, ',') {
            if alts.len() >= 8 {
                break;
            }
            if let Some(a) = parse_alt(part.trim()) {
                alts.push(a);
            }
        }
        // An empty condition (`@media {`) is the same as `all`.
        if text.is_empty() {
            alts.push(Alt {
                negated: false,
                tests: Vec::new(),
            });
        }
        (!alts.is_empty()).then_some(Media { alts })
    }

    /// Does the condition hold for a `w` x `h` viewport?
    pub fn matches(&self, w: i32, h: i32) -> bool {
        self.alts.iter().any(|a| {
            let all = a.tests.iter().all(|t| match *t {
                Test::Width(lo, hi) => (lo..=hi).contains(&w),
                Test::Height(lo, hi) => (lo..=hi).contains(&h),
                Test::Const(b) => b,
                Test::Landscape(l) => (w >= h) == l,
            });
            all != a.negated
        })
    }
}

/// Split `s` at `sep` outside parentheses.
fn split_top(s: &str, sep: char) -> Vec<&str> {
    let (mut depth, mut start) = (0i32, 0usize);
    let mut out = Vec::new();
    for (i, c) in s.char_indices() {
        match c {
            '(' => depth += 1,
            ')' => depth = (depth - 1).max(0),
            c if c == sep && depth == 0 => {
                out.push(&s[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }
    out.push(&s[start..]);
    out
}

fn parse_alt(s: &str) -> Option<Alt> {
    let lower = s.to_ascii_lowercase();
    let mut rest = lower.trim();
    let mut negated = false;
    if let Some(r) = rest.strip_prefix("not ") {
        negated = true;
        rest = r.trim();
    } else if let Some(r) = rest.strip_prefix("only ") {
        rest = r.trim();
    }
    let mut tests = Vec::new();
    // Tokens: a media type word or a parenthesised feature, joined by `and`.
    let mut i = 0;
    let b = rest.as_bytes();
    while i < b.len() {
        if b[i].is_ascii_whitespace() {
            i += 1;
            continue;
        }
        if b[i] == b'(' {
            let mut depth = 0;
            let start = i;
            while i < b.len() {
                match b[i] {
                    b'(' => depth += 1,
                    b')' => {
                        depth -= 1;
                        if depth == 0 {
                            break;
                        }
                    }
                    _ => {}
                }
                i += 1;
            }
            let inner = rest.get(start + 1..i)?;
            i += 1;
            tests.push(parse_feature(inner.trim())?);
        } else {
            let start = i;
            while i < b.len() && !b[i].is_ascii_whitespace() && b[i] != b'(' {
                i += 1;
            }
            match &rest[start..i] {
                "and" => {}
                "all" | "screen" => {}
                // Print, speech and the other types never apply to what is on the screen.
                _ => tests.push(Test::Const(false)),
            }
        }
    }
    Some(Alt { negated, tests })
}

/// A CSS length in `px` for media conditions (`em` is 16 px).
fn px(v: &str) -> Option<i32> {
    let v = v.trim();
    let split = v
        .find(|c: char| c.is_ascii_alphabetic() || c == '%')
        .unwrap_or(v.len());
    let (num, unit) = v.split_at(split);
    let n = milli(num)?;
    let m = match unit {
        "px" | "" => 1000,
        "em" | "rem" => 16_000,
        "pt" => 1333,
        _ => return None,
    };
    Some((n * m / 1_000_000).clamp(-100_000, 100_000) as i32)
}

/// A decimal number in thousandths (`1.5` is 1500); integer arithmetic only.
fn milli(s: &str) -> Option<i64> {
    let s = s.trim();
    let (neg, s) = match s.strip_prefix('-') {
        Some(r) => (true, r),
        None => (false, s),
    };
    let (int, frac) = s.split_once('.').unwrap_or((s, ""));
    let i: i64 = if int.is_empty() { 0 } else { int.parse().ok()? };
    let mut f = 0i64;
    let mut scale = 100;
    for c in frac.chars().take(3) {
        f += i64::from(c.to_digit(10)?) * scale;
        scale /= 10;
    }
    let v = i.checked_mul(1000)?.checked_add(f)?;
    Some(if neg { -v } else { v })
}

fn parse_feature(f: &str) -> Option<Test> {
    // Range syntax: `width >= 600px`, `400px <= width < 900px`.
    if f.contains(['<', '>']) {
        return parse_range(f);
    }
    let (name, value) = match f.split_once(':') {
        Some((n, v)) => (n.trim(), Some(v.trim())),
        None => (f.trim(), None),
    };
    let dim = |lo: bool, v: Option<&str>| -> Option<(i32, i32)> {
        let n = px(v?)?;
        Some(if lo { (n, i32::MAX) } else { (i32::MIN, n) })
    };
    Some(match name {
        "min-width" => {
            let (lo, hi) = dim(true, value)?;
            Test::Width(lo, hi)
        }
        "max-width" => {
            let (lo, hi) = dim(false, value)?;
            Test::Width(lo, hi)
        }
        "width" => {
            let n = px(value?)?;
            Test::Width(n, n)
        }
        "min-height" => {
            let (lo, hi) = dim(true, value)?;
            Test::Height(lo, hi)
        }
        "max-height" => {
            let (lo, hi) = dim(false, value)?;
            Test::Height(lo, hi)
        }
        "height" => {
            let n = px(value?)?;
            Test::Height(n, n)
        }
        "orientation" => Test::Landscape(value? == "landscape"),
        "prefers-color-scheme" => Test::Const(value? == "light"),
        "prefers-reduced-motion"
        | "prefers-contrast"
        | "prefers-reduced-data"
        | "forced-colors" => Test::Const(matches!(value, Some("no-preference" | "none"))),
        "hover" | "any-hover" => Test::Const(value == Some("hover") || value.is_none()),
        "pointer" | "any-pointer" => Test::Const(value == Some("fine") || value.is_none()),
        "min-resolution" | "-webkit-min-device-pixel-ratio" => {
            let v = value?;
            let n = milli(v.trim_end_matches(|c: char| c.is_ascii_alphabetic()))?;
            Test::Const(n <= 1000)
        }
        "color" | "monochrome" if value.is_none() => Test::Const(name == "color"),
        "display-mode" => Test::Const(value == Some("browser")),
        "scripting" => Test::Const(value == Some("none")),
        _ => Test::Const(false),
    })
}

fn parse_range(f: &str) -> Option<Test> {
    // Tokenise into words and operators.
    let mut words: Vec<&str> = Vec::new();
    let mut ops: Vec<&str> = Vec::new();
    let b = f.as_bytes();
    let (mut start, mut i) = (0usize, 0usize);
    while i < b.len() {
        if b[i] == b'<' || b[i] == b'>' {
            words.push(f[start..i].trim());
            let len = if b.get(i + 1) == Some(&b'=') { 2 } else { 1 };
            ops.push(&f[i..i + len]);
            i += len;
            start = i;
        } else {
            i += 1;
        }
    }
    words.push(f[start..].trim());
    let is_w = |w: &str| w == "width";
    let is_h = |w: &str| w == "height";
    // Bounds as inclusive limits, strictness folded in (+/- 1 px).
    let (mut lo, mut hi) = (i32::MIN, i32::MAX);
    let mut dim: Option<bool> = None; // true = width
    for (k, op) in ops.iter().enumerate() {
        let (l, r) = (words[k], words[k + 1]);
        let (name, val, flipped) = if is_w(l) || is_h(l) {
            (l, px(r)?, false)
        } else if is_w(r) || is_h(r) {
            (r, px(l)?, true)
        } else {
            return None;
        };
        let w = is_w(name);
        if *dim.get_or_insert(w) != w {
            return None;
        }
        // `name op val`; when `val op name` the operator reads the other way round.
        let greater = op.starts_with('>') != flipped;
        let strict = !op.ends_with('=');
        if greater {
            lo = lo.max(val + i32::from(strict));
        } else {
            hi = hi.min(val - i32::from(strict));
        }
    }
    Some(if dim? {
        Test::Width(lo, hi)
    } else {
        Test::Height(lo, hi)
    })
}
