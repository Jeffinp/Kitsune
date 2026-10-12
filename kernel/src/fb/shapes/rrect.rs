//! Rounded rectangles: fills (flat and vertical gradient) and strokes, anti-aliased with the shared corner masks.

use super::*;

impl Canvas<'_> {
    /// Anti-aliased rounded rectangle blended with `alpha` (0..=256).
    pub fn fill_rrect(&mut self, r: Rect, radius: i32, style: Corner, c: Color, alpha: u16) {
        self.fill_rrect_vgrad(r, radius, style, c, c, alpha);
    }

    /// Rounded rectangle with a vertical gradient from `top` to `bottom`.
    pub fn fill_rrect_vgrad(
        &mut self,
        r: Rect,
        radius: i32,
        style: Corner,
        top: Color,
        bottom: Color,
        alpha: u16,
    ) {
        let t0 = crate::trace::t();
        self.fill_rrect_inner(r, radius, style, top, bottom, alpha.min(256) as u32);
        crate::trace::prim(crate::trace::Prim::RoundRect, t0);
    }

    pub(super) fn fill_rrect_inner(
        &mut self,
        r: Rect,
        radius: i32,
        style: Corner,
        top: Color,
        bottom: Color,
        a: u32,
    ) {
        if a == 0 || r.w <= 0 || r.h <= 0 {
            return;
        }
        let Some((bpp, bgr)) = self.order() else {
            return;
        };
        let rad = (radius.max(0) as usize)
            .min(r.w as usize / 2)
            .min(r.h as usize / 2)
            .min(LIVE_MAX_RADIUS);
        let mask = masks().get(style, rad);
        let h = r.h as usize;
        let solid = top == bottom;
        let lerp_y = |ly: usize| -> Color {
            if solid {
                top
            } else {
                top.lerp(bottom, ((ly * 255 + h / 2) / h.max(1)) as u16)
            }
        };
        for ly in 0..h {
            let py = r.y + ly as i32;
            if py < self.cy0 as i32 || py >= self.cy1 as i32 {
                continue;
            }
            let col = lerp_y(ly);
            let in_band = rad > 0 && (ly < rad || ly + rad >= h);
            if !in_band {
                self.span(py, r.x, r.right(), col, a);
                continue;
            }
            let cy = if ly < rad { ly } else { h - 1 - ly };
            // Interior of the corner band.
            self.span(py, r.x + rad as i32, r.right() - rad as i32, col, a);
            for cx in 0..rad {
                let cov = mask[cy * rad + cx] as u32;
                if cov == 0 {
                    continue;
                }
                let eff = ((cov + (cov >> 7)) * a) >> 8;
                if eff == 0 {
                    continue;
                }
                for px in [r.x + cx as i32, r.right() - 1 - cx as i32] {
                    if px >= self.cx0 as i32 && px < self.cx1 as i32 {
                        self.blend_at(px as usize, py as usize, col, eff, bgr, bpp);
                    }
                }
            }
        }
    }

    /// A 1 px border drawn just inside the rounded rectangle, blended with `alpha`.
    pub fn stroke_rrect(&mut self, r: Rect, radius: i32, style: Corner, c: Color, alpha: u16) {
        if r.w < 3 || r.h < 3 || alpha == 0 {
            return;
        }
        let Some((bpp, bgr)) = self.order() else {
            return;
        };
        let rad = (radius.max(0) as usize)
            .min(r.w as usize / 2)
            .min(r.h as usize / 2)
            .min(LIVE_MAX_RADIUS);
        let (w, h) = (r.w as usize, r.h as usize);
        let outer = masks().get(style, rad);
        let ri = rad.saturating_sub(1);
        let (wi, hi) = (w - 2, h - 2);
        let inner = masks().get(style, ri);
        let a = alpha.min(256) as u32;
        let cov_at = |lx: usize, ly: usize| -> u32 {
            let co = raster::rrect_cov(outer, rad, w, h, lx, ly) as i32;
            let ci = if lx >= 1 && ly >= 1 && lx - 1 < wi && ly - 1 < hi {
                raster::rrect_cov(inner, ri, wi, hi, lx - 1, ly - 1) as i32
            } else {
                0
            };
            (co - ci).max(0) as u32
        };
        let put = |cv: &mut Self, lx: usize, ly: usize| {
            let (px, py) = (r.x + lx as i32, r.y + ly as i32);
            if px < cv.cx0 as i32
                || px >= cv.cx1 as i32
                || py < cv.cy0 as i32
                || py >= cv.cy1 as i32
            {
                return;
            }
            let cov = cov_at(lx, ly);
            if cov != 0 {
                let eff = ((cov + (cov >> 7)) * a) >> 8;
                if eff != 0 {
                    cv.blend_at(px as usize, py as usize, c, eff, bgr, bpp);
                }
            }
        };
        for ly in 0..h {
            let edge_row = ly == 0 || ly + 1 == h;
            let band = ly < rad || ly + rad >= h;
            if edge_row || band {
                for lx in 0..w {
                    if edge_row || lx < rad || lx + rad >= w || lx == 0 || lx + 1 == w {
                        put(self, lx, ly);
                    }
                }
            } else {
                put(self, 0, ly);
                put(self, w - 1, ly);
            }
        }
    }
}
