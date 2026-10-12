//! `calc()`, `min()`, `max()` and `clamp()`, and the viewport units, as lengths.
//!
//! A value is a percentage part plus a pixel part (both in thousandths) or a plain number; sums,
//! products by numbers and quotients by numbers keep that shape. A `min`/`max` of one percentage and
//! one length (`min(100%, 600px)`) cannot be folded before the container is known: it becomes a
//! [`Len::Calc`] with a mode and is resolved at layout time. Everything is integer maths.

use super::*;
use crate::browsing::web::css::{viewport_h, viewport_w};

#[derive(Clone, Copy)]
struct V {
    /// Percent of the container, in thousandths of a percent.
    pct: i64,
    /// Pixels, in thousandths (or the number itself when `scalar`).
    px: i64,
    scalar: bool,
    /// 0 plain, 1 `min(pct, px)`, 2 `max(pct, px)`.
    mode: u8,
}

const NOMINAL_W: i64 = 800;

impl V {
    fn num(n: i64) -> V {
        V {
            pct: 0,
            px: n,
            scalar: true,
            mode: 0,
        }
    }

    fn px(n: i64) -> V {
        V {
            pct: 0,
            px: n,
            scalar: false,
            mode: 0,
        }
    }

    /// The value in thousandths of a pixel against a nominal container (for comparisons that cannot
    /// be settled statically).
    fn nominal(self) -> i64 {
        self.pct * NOMINAL_W / 100 + self.px
    }

    fn plain(self) -> bool {
        self.mode == 0
    }
}

struct P<'a> {
    s: &'a [u8],
    i: usize,
    em: i32,
    depth: u8,
}

pub(super) fn is_math(v: &str) -> bool {
    let l = v.trim_start();
    ["calc(", "-webkit-calc(", "min(", "max(", "clamp("]
        .iter()
        .any(|p| l.len() >= p.len() && l.as_bytes()[..p.len()].eq_ignore_ascii_case(p.as_bytes()))
}

/// Evaluate a math function or an expression made of one.
pub(super) fn parse_math(v: &str, em: i32) -> Option<Len> {
    let mut p = P {
        s: v.trim().as_bytes(),
        i: 0,
        em,
        depth: 0,
    };
    let r = p.expr()?;
    p.ws();
    if p.i != p.s.len() {
        return None;
    }
    to_len(r)
}

fn to_len(v: V) -> Option<Len> {
    if v.scalar {
        return None;
    }
    let px_round = |m: i64| -> i32 {
        let r = if m >= 0 {
            (m + 500) / 1000
        } else {
            -((-m + 500) / 1000)
        };
        r.clamp(-i64::from(MAX_PX), i64::from(MAX_PX)) as i32
    };
    if v.mode == 0 && v.pct == 0 {
        return Some(Len::Px(px_round(v.px)));
    }
    if v.mode == 0 && v.px == 0 && v.pct % 1000 == 0 {
        return Some(Len::Pct((v.pct / 1000).clamp(-10_000, 10_000) as i32));
    }
    Some(Len::Calc {
        pct: (v.pct).clamp(-10_000_000, 10_000_000) as i32,
        px: px_round(v.px),
        mode: v.mode,
    })
}

impl P<'_> {
    fn ws(&mut self) {
        while self.i < self.s.len() && self.s[self.i].is_ascii_whitespace() {
            self.i += 1;
        }
    }

    fn eat(&mut self, c: u8) -> bool {
        self.ws();
        if self.s.get(self.i) == Some(&c) {
            self.i += 1;
            true
        } else {
            false
        }
    }

    fn expr(&mut self) -> Option<V> {
        let mut a = self.term()?;
        loop {
            self.ws();
            let op = match self.s.get(self.i) {
                Some(b'+') => 1,
                Some(b'-') => -1,
                _ => return Some(a),
            };
            self.i += 1;
            let b = self.term()?;
            if !a.plain() || !b.plain() || a.scalar != b.scalar {
                return None;
            }
            a.pct += op * b.pct;
            a.px += op * b.px;
        }
    }

    fn term(&mut self) -> Option<V> {
        let mut a = self.factor()?;
        loop {
            self.ws();
            let op = match self.s.get(self.i) {
                Some(b'*') => b'*',
                Some(b'/') => b'/',
                _ => return Some(a),
            };
            self.i += 1;
            let b = self.factor()?;
            if !a.plain() || !b.plain() {
                return None;
            }
            a = match op {
                b'*' => {
                    let (n, mut o) = if a.scalar {
                        (a.px, b)
                    } else if b.scalar {
                        (b.px, a)
                    } else {
                        return None;
                    };
                    o.pct = o.pct * n / 1000;
                    o.px = o.px * n / 1000;
                    o
                }
                _ => {
                    if !b.scalar || b.px == 0 {
                        return None;
                    }
                    V {
                        pct: a.pct * 1000 / b.px,
                        px: a.px * 1000 / b.px,
                        scalar: a.scalar,
                        mode: 0,
                    }
                }
            };
        }
    }

    fn factor(&mut self) -> Option<V> {
        self.ws();
        let c = *self.s.get(self.i)?;
        if c == b'(' {
            self.i += 1;
            self.depth += 1;
            if self.depth > 8 {
                return None;
            }
            let v = self.expr()?;
            self.depth -= 1;
            return self.eat(b')').then_some(v);
        }
        if c == b'-' || c == b'+' || c == b'.' || c.is_ascii_digit() {
            return self.number();
        }
        // A function name.
        let start = self.i;
        while self.i < self.s.len()
            && (self.s[self.i].is_ascii_alphabetic() || self.s[self.i] == b'-')
        {
            self.i += 1;
        }
        let name = core::str::from_utf8(&self.s[start..self.i])
            .ok()?
            .to_ascii_lowercase();
        if !self.eat(b'(') {
            return None;
        }
        self.depth += 1;
        if self.depth > 8 {
            return None;
        }
        let r = match name.as_str() {
            "calc" | "-webkit-calc" => {
                let v = self.expr()?;
                self.eat(b')').then_some(v)
            }
            "min" | "max" | "clamp" => {
                let mut args = Vec::new();
                loop {
                    args.push(self.expr()?);
                    if args.len() > 8 {
                        return None;
                    }
                    if self.eat(b',') {
                        continue;
                    }
                    break;
                }
                if !self.eat(b')') {
                    return None;
                }
                combine(&name, &args)
            }
            _ => None,
        };
        self.depth -= 1;
        r
    }

    fn number(&mut self) -> Option<V> {
        let rest = core::str::from_utf8(&self.s[self.i..]).ok()?;
        let (n, after) = parse_milli(rest)?;
        self.i = self.s.len() - after.len();
        let ustart = self.i;
        while self.i < self.s.len()
            && (self.s[self.i].is_ascii_alphabetic() || self.s[self.i] == b'%')
        {
            self.i += 1;
        }
        let unit = core::str::from_utf8(&self.s[ustart..self.i])
            .ok()?
            .to_ascii_lowercase();
        let (vw, vh) = (i64::from(viewport_w()), i64::from(viewport_h()));
        let em = i64::from(self.em);
        Some(match unit.as_str() {
            "" => V::num(n),
            "px" => V::px(n),
            "%" => V {
                pct: n,
                px: 0,
                scalar: false,
                mode: 0,
            },
            "pt" => V::px(n * 4 / 3),
            "pc" => V::px(n * 16),
            "in" => V::px(n * 96),
            "cm" => V::px(n * 96 * 100 / 254),
            "mm" => V::px(n * 96 * 10 / 254),
            "em" => V::px(n * em),
            "rem" => V::px(n * 16),
            "ex" | "ch" => V::px(n * em / 2),
            "vw" | "svw" | "lvw" | "dvw" => V::px(n * vw / 100),
            "vh" | "svh" | "lvh" | "dvh" => V::px(n * vh / 100),
            "vmin" => V::px(n * vw.min(vh) / 100),
            "vmax" => V::px(n * vw.max(vh) / 100),
            _ => return None,
        })
    }
}

/// `min`, `max` and `clamp` of already evaluated arguments.
fn combine(name: &str, a: &[V]) -> Option<V> {
    if a.iter().any(|v| v.scalar != a[0].scalar || !v.plain()) {
        return None;
    }
    let all_px = a.iter().all(|v| v.pct == 0);
    let all_pct = a.iter().all(|v| v.px == 0);
    let key = |v: &V| {
        if all_px {
            v.px
        } else if all_pct {
            v.pct
        } else {
            v.nominal()
        }
    };
    let pick = |is_max: bool, list: &[V]| -> V {
        let mut best = list[0];
        for v in &list[1..] {
            if (key(v) > key(&best)) == is_max && key(v) != key(&best) {
                best = *v;
            }
        }
        best
    };
    match name {
        "min" | "max" => {
            let is_max = name == "max";
            // One percentage and one length: only the container can tell which is smaller.
            if !all_px && !all_pct && a.len() == 2 {
                let (p, l) = if a[0].px == 0 && a[1].pct == 0 {
                    (a[0], a[1])
                } else if a[1].px == 0 && a[0].pct == 0 {
                    (a[1], a[0])
                } else {
                    return Some(pick(is_max, a));
                };
                return Some(V {
                    pct: p.pct,
                    px: l.px,
                    scalar: false,
                    mode: if is_max { 2 } else { 1 },
                });
            }
            Some(pick(is_max, a))
        }
        // clamp(lo, val, hi) = max(lo, min(val, hi))
        _ if a.len() == 3 => {
            let m = pick(false, &[a[1], a[2]]);
            Some(pick(true, &[a[0], m]))
        }
        _ => None,
    }
}
