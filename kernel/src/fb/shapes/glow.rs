//! The soft glow (radial falloff from a lookup table).

use super::*;

impl Canvas<'_> {
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
