//! Anti-aliased shapes, shadows, gradients and surface blits for [`Canvas`].
//!
//! Corner shapes come from the shared coverage masks of `kitsune_core::raster`, so
//! what is drawn live matches what is baked into cached surfaces. Everything is
//! integer arithmetic and honours the canvas clip.
//!
//! This is toolkit API: parts of it are consumed by the chrome and the widgets, the
//! rest is there for the apps (wave 2), so unused items are expected.
#![allow(dead_code)]

use super::Canvas;
use crate::fb::Color;
use crate::sync::RacyCell;
use bootloader_api::info::PixelFormat;
use kitsune_core::Rect;
pub use kitsune_core::raster::Corner;
use kitsune_core::raster::{self, CornerMasks, Surface};

/// Largest radius with a live mask (icon tiles are baked into surfaces, so the live
/// shapes never need more).
pub const LIVE_MAX_RADIUS: usize = 48;

static MASKS: RacyCell<Option<CornerMasks>> = RacyCell::new(None);

/// The shared corner masks, built on first use (the compositor thread).
pub fn masks() -> &'static CornerMasks {
    // SAFETY: only the compositor (and boot) thread draws shapes, so the lazy
    // initialisation cannot race; the shared reference handed out is read-only
    // afterwards (the cell is never written again once `Some`).
    // NOTE: not guaranteed by the type: relies on that single-thread convention.
    unsafe {
        let slot = &mut *MASKS.get();
        if slot.is_none() {
            *slot = Some(CornerMasks::with_max_radius(LIVE_MAX_RADIUS));
        }
        slot.as_ref().expect("masks just built")
    }
}

/// Opacity of a shadow layer and its geometry.
#[derive(Clone, Copy, Debug)]
pub struct Shadow {
    /// Blur radius: the edge fades over `2 * blur` pixels.
    pub blur: i32,
    /// Vertical offset of the shadow.
    pub dy: i32,
    /// Opacity, 0..=256.
    pub alpha: u32,
}

impl Canvas<'_> {
    #[inline]
    pub(super) fn order(&self) -> Option<(usize, bool)> {
        match self.info.pixel_format {
            PixelFormat::Rgb => Some((self.info.bytes_per_pixel, false)),
            PixelFormat::Bgr => Some((self.info.bytes_per_pixel, true)),
            _ => None,
        }
    }

    /// Blend one pixel (already clip-checked) with opacity `a` (0..=256).
    #[inline]
    fn blend_at(&mut self, x: usize, y: usize, c: Color, a: u32, bgr: bool, bpp: usize) {
        let o = (y * self.info.stride + x) * bpp;
        let (c0, c1, c2) = if bgr {
            (c.b as u32, c.g as u32, c.r as u32)
        } else {
            (c.r as u32, c.g as u32, c.b as u32)
        };
        let px = &mut self.buf[o..o + 3];
        px[0] = ((px[0] as u32 * (256 - a) + c0 * a) >> 8) as u8;
        px[1] = ((px[1] as u32 * (256 - a) + c1 * a) >> 8) as u8;
        px[2] = ((px[2] as u32 * (256 - a) + c2 * a) >> 8) as u8;
    }

    /// Blend a horizontal span `[xs, xe)` of row `y` (canvas coordinates, clipped).
    fn span(&mut self, y: i32, xs: i32, xe: i32, c: Color, a: u32) {
        if a == 0 || y < self.cy0 as i32 || y >= self.cy1 as i32 {
            return;
        }
        let xs = xs.max(self.cx0 as i32);
        let xe = xe.min(self.cx1 as i32);
        if xs >= xe {
            return;
        }
        if a >= 256 {
            self.fill_rect_inner(xs as usize, y as usize, (xe - xs) as usize, 1, c);
            return;
        }
        let Some((bpp, bgr)) = self.order() else {
            return;
        };
        let (c0, c1, c2) = if bgr {
            (c.b as u32, c.g as u32, c.r as u32)
        } else {
            (c.r as u32, c.g as u32, c.b as u32)
        };
        let (s0, s1, s2) = (c0 * a, c1 * a, c2 * a);
        let ia = 256 - a;
        let o = (y as usize * self.info.stride + xs as usize) * bpp;
        let n = (xe - xs) as usize;
        let row = &mut self.buf[o..o + n * bpp];
        if bpp == 4 {
            for px in row.as_chunks_mut::<4>().0.iter_mut() {
                px[0] = ((px[0] as u32 * ia + s0) >> 8) as u8;
                px[1] = ((px[1] as u32 * ia + s1) >> 8) as u8;
                px[2] = ((px[2] as u32 * ia + s2) >> 8) as u8;
            }
        } else {
            for px in row.as_chunks_mut::<3>().0.iter_mut() {
                px[0] = ((px[0] as u32 * ia + s0) >> 8) as u8;
                px[1] = ((px[1] as u32 * ia + s1) >> 8) as u8;
                px[2] = ((px[2] as u32 * ia + s2) >> 8) as u8;
            }
        }
    }

    /// Alpha-blend a plain rectangle (`alpha` 0..=256).
    pub fn blend_rect(&mut self, r: Rect, c: Color, alpha: u16) {
        let a = alpha.min(256) as u32;
        for y in r.y..r.bottom() {
            self.span(y, r.x, r.right(), c, a);
        }
    }

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

    fn fill_rrect_inner(
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

    /// Soft drop shadow of the rounded rectangle `body` (analytic, separable
    /// profile). Pixels inside `hole` (normally the opaque body minus its corners)
    /// are left alone: the body will overwrite them.
    pub fn draw_shadow(&mut self, body: Rect, sh: Shadow, hole: Rect) {
        if sh.alpha == 0 || body.w <= 0 || body.h <= 0 {
            return;
        }
        let Some((bpp, _)) = self.order() else {
            return;
        };
        let t0 = crate::trace::t();
        let b = sh.blur.max(1);
        let area = Rect::new(
            body.x - b,
            body.y + sh.dy - b,
            body.w + 2 * b,
            body.h + 2 * b,
        );
        let clip = self.clip_rect();
        let Some(vis) = area.intersection(&clip) else {
            return;
        };
        let px_profile = raster::shadow_profile(body.w as usize, b as usize);
        let py_profile = raster::shadow_profile(body.h as usize, b as usize);
        for y in vis.y..vis.bottom() {
            let pyv = py_profile[(y - area.y) as usize] as u32;
            if pyv == 0 {
                continue;
            }
            let pyo = pyv * sh.alpha.min(256);
            // The visible part of the row minus the hole: at most two runs.
            let in_hole = hole.w > 0 && y >= hole.y && y < hole.bottom();
            let runs: [(i32, i32); 2] = if in_hole {
                [
                    (vis.x, hole.x.min(vis.right())),
                    (hole.right().max(vis.x), vis.right()),
                ]
            } else {
                [(vis.x, vis.right()), (0, 0)]
            };
            for (x0, x1) in runs {
                if x1 <= x0 {
                    continue;
                }
                let o = (y as usize * self.info.stride + x0 as usize) * bpp;
                let n = (x1 - x0) as usize;
                let row = &mut self.buf[o..o + n * bpp];
                let base = (x0 - area.x) as usize;
                if bpp == 4 {
                    for (i, p) in row.as_chunks_mut::<4>().0.iter_mut().enumerate() {
                        let a = (px_profile[base + i] as u32 * pyo) >> 16;
                        if a != 0 {
                            let ia = 256 - (a + (a >> 7));
                            p[0] = ((p[0] as u32 * ia) >> 8) as u8;
                            p[1] = ((p[1] as u32 * ia) >> 8) as u8;
                            p[2] = ((p[2] as u32 * ia) >> 8) as u8;
                        }
                    }
                } else {
                    for (i, p) in row.as_chunks_mut::<3>().0.iter_mut().enumerate() {
                        let a = (px_profile[base + i] as u32 * pyo) >> 16;
                        if a != 0 {
                            let ia = 256 - (a + (a >> 7));
                            p[0] = ((p[0] as u32 * ia) >> 8) as u8;
                            p[1] = ((p[1] as u32 * ia) >> 8) as u8;
                            p[2] = ((p[2] as u32 * ia) >> 8) as u8;
                        }
                    }
                }
            }
        }
        crate::trace::prim(crate::trace::Prim::Alpha, t0);
    }

    /// Source-over a premultiplied surface with its top-left at `(x, y)`, scaled
    /// by `opacity` (0..=256). Clipped.
    pub fn blit_surface(&mut self, s: &Surface, x: i32, y: i32, opacity: u32) {
        let Some((bpp, bgr)) = self.order() else {
            return;
        };
        let op = opacity.min(256);
        if op == 0 {
            return;
        }
        let clip = self.clip_rect();
        let Some(vis) = Rect::new(x, y, s.w as i32, s.h as i32).intersection(&clip) else {
            return;
        };
        for py in vis.y..vis.bottom() {
            let sy = (py - y) as usize;
            let src = &s.px[sy * s.w + (vis.x - x) as usize..][..vis.w as usize];
            let o = (py as usize * self.info.stride + vis.x as usize) * bpp;
            let row = &mut self.buf[o..o + vis.w as usize * bpp];
            for (i, &p) in src.iter().enumerate() {
                let mut p = p;
                if p >> 24 == 0 {
                    continue;
                }
                if op != 256 {
                    p = raster::scale_premul(p, op);
                }
                let sa = p >> 24;
                let d = &mut row[i * bpp..i * bpp + 3];
                let (sr, sg, sb) = ((p >> 16) & 0xFF, (p >> 8) & 0xFF, p & 0xFF);
                let (s0, s1, s2) = if bgr { (sb, sg, sr) } else { (sr, sg, sb) };
                if sa == 255 {
                    d[0] = s0 as u8;
                    d[1] = s1 as u8;
                    d[2] = s2 as u8;
                } else {
                    let ia = 256 - (sa + (sa >> 7));
                    d[0] = (s0 + ((d[0] as u32 * ia + 128) >> 8)).min(255) as u8;
                    d[1] = (s1 + ((d[1] as u32 * ia + 128) >> 8)).min(255) as u8;
                    d[2] = (s2 + ((d[2] as u32 * ia + 128) >> 8)).min(255) as u8;
                }
            }
        }
    }

    /// Copy the pixels of `r` (clipped to the canvas, not to the clip) into `out`
    /// as opaque `0xFFRRGGBB`, row-major `r.w * r.h` (pixels outside the canvas are
    /// black).
    pub fn read_region(&self, r: Rect, out: &mut alloc::vec::Vec<u32>) {
        let Some((bpp, bgr)) = self.order() else {
            return;
        };
        out.clear();
        out.resize((r.w.max(0) * r.h.max(0)) as usize, 0xFF00_0000);
        for ly in 0..r.h.max(0) {
            let py = r.y + ly;
            if py < 0 || py as usize >= self.info.height {
                continue;
            }
            for lx in 0..r.w.max(0) {
                let px = r.x + lx;
                if px < 0 || px as usize >= self.info.width {
                    continue;
                }
                let o = (py as usize * self.info.stride + px as usize) * bpp;
                let (a, b, c) = (
                    self.buf[o] as u32,
                    self.buf[o + 1] as u32,
                    self.buf[o + 2] as u32,
                );
                let (rr, gg, bb) = if bgr { (c, b, a) } else { (a, b, c) };
                out[(ly * r.w + lx) as usize] = 0xFF00_0000 | (rr << 16) | (gg << 8) | bb;
            }
        }
    }

    /// Write opaque `0xFFRRGGBB` pixels (as produced by [`read_region`]) at the
    /// top-left of `r`; clipped.
    pub fn write_region(&mut self, r: Rect, px: &[u32]) {
        let Some((bpp, bgr)) = self.order() else {
            return;
        };
        if px.len() < (r.w.max(0) * r.h.max(0)) as usize {
            return;
        }
        let clip = self.clip_rect();
        let Some(vis) = r.intersection(&clip) else {
            return;
        };
        for py in vis.y..vis.bottom() {
            let src = &px[((py - r.y) * r.w + (vis.x - r.x)) as usize..][..vis.w as usize];
            let o = (py as usize * self.info.stride + vis.x as usize) * bpp;
            let row = &mut self.buf[o..o + vis.w as usize * bpp];
            for (i, &p) in src.iter().enumerate() {
                let d = &mut row[i * bpp..i * bpp + 3];
                let (rr, gg, bb) = ((p >> 16) as u8, (p >> 8) as u8, p as u8);
                if bgr {
                    d[0] = bb;
                    d[1] = gg;
                    d[2] = rr;
                } else {
                    d[0] = rr;
                    d[1] = gg;
                    d[2] = bb;
                }
            }
        }
    }

    /// Blend opaque `0xFFRRGGBB` pixels `px` (covering `region`, row-major) into the
    /// part of `dest` that `region` covers, with `alpha` (0..=256) and the rounded
    /// corners of `dest` (radius `radius`, 0 = square). A cached blurred backdrop
    /// goes through here, inside a panel's shape.
    pub fn blit_pixels(&mut self, px: &[u32], region: Rect, dest: Rect, radius: i32, alpha: u32) {
        let Some((bpp, bgr)) = self.order() else {
            return;
        };
        let a0 = alpha.min(256);
        if a0 == 0 || px.len() < (region.w.max(0) * region.h.max(0)) as usize {
            return;
        }
        let clip = self.clip_rect();
        let Some(vis) = dest
            .intersection(&region)
            .and_then(|r| r.intersection(&clip))
        else {
            return;
        };
        let rad = (radius.max(0) as usize)
            .min(dest.w as usize / 2)
            .min(dest.h as usize / 2)
            .min(LIVE_MAX_RADIUS);
        let mask = masks().get(Corner::Circle, rad);
        let (dw, dh) = (dest.w as usize, dest.h as usize);
        for y in vis.y..vis.bottom() {
            let ly = (y - dest.y) as usize;
            let band = rad > 0 && (ly < rad || ly + rad >= dh);
            let src =
                &px[((y - region.y) * region.w + (vis.x - region.x)) as usize..][..vis.w as usize];
            let o = (y as usize * self.info.stride + vis.x as usize) * bpp;
            let row = &mut self.buf[o..o + vis.w as usize * bpp];
            for (i, &p) in src.iter().enumerate() {
                let lx = (vis.x - dest.x) as usize + i;
                let mut a = a0;
                if band || lx < rad || lx + rad >= dw {
                    let cov = raster::rrect_cov(mask, rad, dw, dh, lx, ly) as u32;
                    if cov == 0 {
                        continue;
                    }
                    a = (a * (cov + (cov >> 7))) >> 8;
                    if a == 0 {
                        continue;
                    }
                }
                let d = &mut row[i * bpp..i * bpp + 3];
                let (sr, sg, sb) = ((p >> 16) & 0xFF, (p >> 8) & 0xFF, p & 0xFF);
                let (s0, s1, s2) = if bgr { (sb, sg, sr) } else { (sr, sg, sb) };
                if a >= 256 {
                    d[0] = s0 as u8;
                    d[1] = s1 as u8;
                    d[2] = s2 as u8;
                } else {
                    d[0] = ((d[0] as u32 * (256 - a) + s0 * a) >> 8) as u8;
                    d[1] = ((d[1] as u32 * (256 - a) + s1 * a) >> 8) as u8;
                    d[2] = ((d[2] as u32 * (256 - a) + s2 * a) >> 8) as u8;
                }
            }
        }
    }

    /// Re-round a window corner after its content was painted: `saved` holds what the
    /// canvas showed under the `radius x radius` square `rect` before the window was
    /// drawn; pixels outside the quarter circle go back to it (anti-aliased).
    /// `mirror_x` is set for the right-hand corner.
    pub fn restore_corner(&mut self, rect: Rect, saved: &[u32], radius: i32, mirror_x: bool) {
        let Some((bpp, bgr)) = self.order() else {
            return;
        };
        let rad = (radius.max(0) as usize).min(LIVE_MAX_RADIUS);
        if rad == 0 || rect.w as usize != rad || rect.h as usize != rad || saved.len() < rad * rad {
            return;
        }
        let mask = masks().get(Corner::Circle, rad);
        let clip = self.clip_rect();
        for ly in 0..rad {
            for lx in 0..rad {
                let (px, py) = (rect.x + lx as i32, rect.y + ly as i32);
                if px < clip.x || px >= clip.right() || py < clip.y || py >= clip.bottom() {
                    continue;
                }
                // Bottom corners: the mask's rows run from the outer edge inwards.
                let mx = if mirror_x { rad - 1 - lx } else { lx };
                let cov = mask[(rad - 1 - ly) * rad + mx] as u32;
                if cov >= 255 {
                    continue;
                }
                let s = saved[ly * rad + lx];
                let (sr, sg, sb) = ((s >> 16) & 0xFF, (s >> 8) & 0xFF, s & 0xFF);
                let (s0, s1, s2) = if bgr { (sb, sg, sr) } else { (sr, sg, sb) };
                let o = (py as usize * self.info.stride + px as usize) * bpp;
                let d = &mut self.buf[o..o + 3];
                let a = cov + (cov >> 7); // window weight, 0..=256
                d[0] = ((s0 * (256 - a) + d[0] as u32 * a) >> 8) as u8;
                d[1] = ((s1 * (256 - a) + d[1] as u32 * a) >> 8) as u8;
                d[2] = ((s2 * (256 - a) + d[2] as u32 * a) >> 8) as u8;
            }
        }
    }

    /// Soft radial glow centred at `(cx, cy)` with radius `rad` and peak opacity
    /// `peak` (0..=255): the wallpaper's blobs.
    pub fn glow(&mut self, cx: i32, cy: i32, rad: i32, c: Color, peak: u32, lut: &[u8; 256]) {
        let Some((bpp, bgr)) = self.order() else {
            return;
        };
        if rad <= 0 {
            return;
        }
        let area = Rect::new(cx - rad, cy - rad, 2 * rad, 2 * rad);
        let Some(vis) = area.intersection(&self.clip_rect()) else {
            return;
        };
        let r2 = (rad as i64) * (rad as i64);
        // idx = d2 * 255 / r2 via a Q16 reciprocal.
        let recip = (255i64 << 16) / r2.max(1);
        // The opacity for each table entry, with the peak and the 0..=256 widening already applied.
        let mut tab = [0u32; 256];
        for (t, &l) in tab.iter_mut().zip(lut.iter()) {
            let a = (l as u32 * peak) / 255;
            *t = a + (a >> 7);
        }
        let (c0, c1, c2) = if bgr {
            (c.b as u32, c.g as u32, c.r as u32)
        } else {
            (c.r as u32, c.g as u32, c.b as u32)
        };
        for y in vis.y..vis.bottom() {
            let dy = (y - cy) as i64;
            let dy2 = dy * dy;
            if dy2 >= r2 {
                continue;
            }
            // Only the chord of the circle on this row: `d2 < r2` <=> `|dx| <= isqrt(r2 - dy2 - 1)`.
            let hx = isqrt((r2 - dy2 - 1) as u64) as i32;
            let x0 = vis.x.max(cx - hx);
            let x1 = vis.right().min(cx + hx + 1);
            if x0 >= x1 {
                continue;
            }
            let o = (y as usize * self.info.stride + x0 as usize) * bpp;
            let row = &mut self.buf[o..o + (x1 - x0) as usize * bpp];
            let mut d2 = dy2 + ((x0 - cx) as i64) * ((x0 - cx) as i64);
            for (dx, px) in ((x0 - cx) as i64..).zip(row.chunks_exact_mut(bpp)) {
                let idx = ((d2 * recip) >> 16).min(255) as usize;
                let a = tab[idx];
                if a != 0 {
                    let ia = 256 - a;
                    px[0] = ((px[0] as u32 * ia + c0 * a) >> 8) as u8;
                    px[1] = ((px[1] as u32 * ia + c1 * a) >> 8) as u8;
                    px[2] = ((px[2] as u32 * ia + c2 * a) >> 8) as u8;
                }
                d2 += 2 * dx + 1;
            }
        }
    }
}

/// Integer square root: the largest `r` with `r * r <= n`.
fn isqrt(n: u64) -> u64 {
    if n < 2 {
        return n;
    }
    let mut x = 1u64 << (32 - (n.leading_zeros() / 2));
    loop {
        let y = (x + n / x) / 2;
        if y >= x {
            return x;
        }
        x = y;
    }
}
