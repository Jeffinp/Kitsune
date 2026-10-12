//! Pure 2-D drawing toolkit on premultiplied `0xAARRGGBB` surfaces: anti-aliased
//! rounded rectangles (circular and "continuous" corners), exact-area vector
//! paths, gradients, separable box blur, area/bilinear resampling, drop shadows
//! and the 1-D shadow profile the compositor uses for window shadows.
//!
//! Everything is integer / fixed point: the kernel has no hardware floating point.
//! The kernel's `Canvas` blits these surfaces (icons, cached backdrops) and uses
//! the same corner masks and profiles for its direct drawing, so the shapes drawn
//! into a cache and the ones drawn live agree pixel for pixel.

use crate::ui::gfx::isqrt;
use crate::ui::glyph::Path;
use alloc::vec;
use alloc::vec::Vec;

mod color;
mod corners;
mod resample;
mod shadow;
pub use color::*;
pub use corners::*;
pub use resample::*;
pub use shadow::*;

// ---- the surface

/// A premultiplied-ARGB image.
#[derive(Clone)]
pub struct Surface {
    pub w: usize,
    pub h: usize,
    pub px: Vec<u32>,
}
impl Surface {
    /// A transparent surface.
    pub fn new(w: usize, h: usize) -> Surface {
        Surface {
            w,
            h,
            px: vec![0; w * h],
        }
    }

    pub fn fill(&mut self, straight: u32) {
        let p = premul(straight);
        self.px.fill(p);
    }

    #[inline]
    pub fn get(&self, x: usize, y: usize) -> u32 {
        self.px.get(y * self.w + x).copied().unwrap_or(0)
    }

    #[inline]
    fn put_over(&mut self, x: usize, y: usize, premultiplied: u32) {
        if x < self.w && y < self.h {
            let d = &mut self.px[y * self.w + x];
            *d = over(*d, premultiplied);
        }
    }

    /// Composite `paint` through coverage `a` (0..=255) at a pixel.
    #[inline]
    fn shade(&mut self, x: usize, y: usize, c: u32, a: u32) {
        let ca = (c >> 24) * a;
        let a8 = (ca + 127) / 255;
        if a8 == 0 {
            return;
        }
        self.put_over(x, y, premul((a8 << 24) | (c & 0x00FF_FFFF)));
    }

    /// Anti-aliased rounded rectangle. `r` is clamped to half the shorter side
    /// (and to [`MAX_RADIUS`]).
    #[allow(clippy::too_many_arguments)]
    pub fn fill_rrect(
        &mut self,
        x: i32,
        y: i32,
        w: usize,
        h: usize,
        r: usize,
        style: Corner,
        paint: Paint,
        masks: &CornerMasks,
    ) {
        let r = r.min(w / 2).min(h / 2).min(MAX_RADIUS);
        let mask = masks.get(style, r);
        for ly in 0..h {
            let py = y + ly as i32;
            if py < 0 || py as usize >= self.h {
                continue;
            }
            for lx in 0..w {
                let px = x + lx as i32;
                if px < 0 || px as usize >= self.w {
                    continue;
                }
                let cov = rrect_cov(mask, r, w, h, lx, ly) as u32;
                if cov != 0 {
                    self.shade(px as usize, py as usize, paint.at(lx, ly, w, h), cov);
                }
            }
        }
    }

    /// A `t`-pixel border drawn inside the rounded rectangle.
    #[allow(clippy::too_many_arguments)]
    pub fn stroke_rrect(
        &mut self,
        x: i32,
        y: i32,
        w: usize,
        h: usize,
        r: usize,
        t: usize,
        style: Corner,
        paint: Paint,
        masks: &CornerMasks,
    ) {
        let r = r.min(w / 2).min(h / 2).min(MAX_RADIUS);
        let ri = r.saturating_sub(t);
        let (wi, hi) = (w.saturating_sub(2 * t), h.saturating_sub(2 * t));
        let outer = masks.get(style, r);
        let inner = masks.get(style, ri.min(wi / 2).min(hi / 2));
        let ri = ri.min(wi / 2).min(hi / 2);
        for ly in 0..h {
            let py = y + ly as i32;
            if py < 0 || py as usize >= self.h {
                continue;
            }
            for lx in 0..w {
                let px = x + lx as i32;
                if px < 0 || px as usize >= self.w {
                    continue;
                }
                let co = rrect_cov(outer, r, w, h, lx, ly) as i32;
                let ci = if lx >= t && ly >= t && lx - t < wi && ly - t < hi {
                    rrect_cov(inner, ri, wi, hi, lx - t, ly - t) as i32
                } else {
                    0
                };
                let cov = (co - ci).max(0) as u32;
                if cov != 0 {
                    self.shade(px as usize, py as usize, paint.at(lx, ly, w, h), cov);
                }
            }
        }
    }

    /// Fill an arbitrary path (24.8 coordinates in surface pixels) with `paint`.
    pub fn fill_path(&mut self, path: &Path, paint: Paint) {
        let Some((x0, y0, x1, y1)) = path.bounds() else {
            return;
        };
        let bx = x0.div_euclid(256).max(0);
        let by = y0.div_euclid(256).max(0);
        let ex = ((x1 + 255).div_euclid(256)).min(self.w as i32);
        let ey = ((y1 + 255).div_euclid(256)).min(self.h as i32);
        if ex <= bx || ey <= by {
            return;
        }
        let (w, h) = ((ex - bx) as usize, (ey - by) as usize);
        let cov = path.coverage(w, h, bx * 256, by * 256);
        for ly in 0..h {
            for lx in 0..w {
                let c = cov[ly * w + lx] as u32;
                if c != 0 {
                    self.shade(
                        bx as usize + lx,
                        by as usize + ly,
                        paint.at(lx, ly, w, h),
                        c,
                    );
                }
            }
        }
    }

    /// Source-over `src` at `(dx, dy)` with opacity `0..=256`.
    pub fn blit(&mut self, src: &Surface, dx: i32, dy: i32, opacity: u32) {
        let op = opacity.min(256);
        for sy in 0..src.h {
            let y = dy + sy as i32;
            if y < 0 || y as usize >= self.h {
                continue;
            }
            for sx in 0..src.w {
                let x = dx + sx as i32;
                if x < 0 || x as usize >= self.w {
                    continue;
                }
                let mut p = src.px[sy * src.w + sx];
                if p >> 24 == 0 {
                    continue;
                }
                if op != 256 {
                    p = scale_premul(p, op);
                }
                self.put_over(x as usize, y as usize, p);
            }
        }
    }

    /// Multiply every pixel (all four channels) by `a256 / 256`.
    pub fn fade(&mut self, a256: u32) {
        for p in &mut self.px {
            *p = scale_premul(*p, a256);
        }
    }

    /// Keep only the part of the surface inside a rounded rectangle covering all of it.
    pub fn mask_rrect(&mut self, r: usize, style: Corner, masks: &CornerMasks) {
        let (w, h) = (self.w, self.h);
        let r = r.min(w / 2).min(h / 2).min(MAX_RADIUS);
        let mask = masks.get(style, r);
        for ly in 0..h {
            for lx in 0..w {
                let c = rrect_cov(mask, r, w, h, lx, ly) as u32;
                if c != 255 {
                    let p = &mut self.px[ly * w + lx];
                    *p = scale_premul(*p, c + (c >> 7));
                }
            }
        }
    }

    /// Separable box blur of all four channels, `passes` times (3 approximates a
    /// Gaussian). Edge pixels are repeated.
    pub fn blur(&mut self, radius: usize, passes: usize) {
        blur_u32(&mut self.px, self.w, self.h, radius, passes);
    }

    /// Resample to `nw x nh`: area average when shrinking, bilinear when growing.
    pub fn resized(&self, nw: usize, nh: usize) -> Surface {
        if nw == 0 || nh == 0 || self.w == 0 || self.h == 0 {
            return Surface::new(nw, nh);
        }
        if nw == self.w && nh == self.h {
            return self.clone();
        }
        // Horizontal pass into (nw x h), then vertical into (nw x nh).
        let taps_x = taps(self.w, nw);
        let taps_y = taps(self.h, nh);
        let mut mid = vec![0u32; nw * self.h];
        for y in 0..self.h {
            let row = &self.px[y * self.w..(y + 1) * self.w];
            for (x, t) in taps_x.iter().enumerate() {
                mid[y * nw + x] = weigh(t, |i| row[i]);
            }
        }
        let mut out = Surface::new(nw, nh);
        for (y, t) in taps_y.iter().enumerate() {
            for x in 0..nw {
                out.px[y * nw + x] = weigh(t, |i| mid[i * nw + x]);
            }
        }
        out
    }

    /// A copy with a soft drop shadow (`dy` down, blur radius `blur`, `alpha`
    /// 0..=255 of black) around the shape, on a surface `margin` pixels larger on
    /// each side.
    pub fn with_shadow(&self, margin: usize, dy: i32, blur: usize, alpha: u32) -> Surface {
        let (w, h) = (self.w + 2 * margin, self.h + 2 * margin);
        let mut sh = Surface::new(w, h);
        for y in 0..self.h {
            for x in 0..self.w {
                let a = self.px[y * self.w + x] >> 24;
                let ty = y as i32 + margin as i32 + dy;
                if a != 0 && ty >= 0 && (ty as usize) < h {
                    sh.px[ty as usize * w + x + margin] = (a * alpha / 255) << 24;
                }
            }
        }
        sh.blur(blur, 3);
        sh.blit(self, margin as i32, margin as i32, 256);
        sh
    }
}
#[cfg(test)]
mod tests;
