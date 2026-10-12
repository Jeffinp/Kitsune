//! The scanline rasteriser: polygons to coverage to pixels.

use super::*;

pub(super) struct Canvas {
    pub(super) w: usize,
    pub(super) h: usize,
    pub(super) px: Vec<u32>,
}

/// Fill the union of `polys` (24.8 device coordinates) with `color` at `alpha`.
pub(super) fn fill(cv: &mut Canvas, polys: &[Vec<Pt>], even_odd: bool, color: u32, alpha: u32) {
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
pub(super) fn span(cov: &mut [u32], w: usize, xa: i64, xb: i64, touched: &mut (usize, usize)) {
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

pub(super) fn flush_row(
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
