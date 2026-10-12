//! Surface and pixel blits: whole surfaces, regions read back and written, rounded blits and corner restore.

use super::*;

impl Canvas<'_> {
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
}
