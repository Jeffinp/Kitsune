//! Geometry in device space: path data, curves, basic shapes and strokes as polygons.

use super::*;

pub(super) type Pt = (i64, i64);

/// Flatten path data into polylines in device space (24.8). Each is `(points, closed)`.
pub(super) fn flatten_path(d: &str, m: &Mat, out: &mut Vec<(Vec<Pt>, bool)>, budget: &mut usize) {
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

pub(super) fn cubic(out: &mut Vec<Pt>, m: &Mat, p0: Pt, p1: Pt, p2: Pt, p3: Pt) {
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
pub(super) fn ellipse_poly(m: &Mat, cx: i64, cy: i64, rx: i64, ry: i64) -> Vec<Pt> {
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

pub(super) fn rect_poly(m: &Mat, x: i64, y: i64, w: i64, h: i64, rx: i64, ry: i64) -> Vec<Pt> {
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
pub(super) fn stroke_polys(pts: &[Pt], closed: bool, half: i64, out: &mut Vec<Vec<Pt>>) {
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
