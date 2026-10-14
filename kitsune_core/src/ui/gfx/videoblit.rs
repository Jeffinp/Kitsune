//! A decoded video picture into the screen's pixels: scaled by nearest neighbour, in the
//! framebuffer's own layout (3 or 4 bytes a pixel, R, G, B or B, G, R), clipped.
//!
//! This runs for every picture of a playing video, so it goes a row at a time: a picture of the
//! box's size in the framebuffer's own layout is one copy per row, anything else a table of source
//! columns (no division per pixel) with each source pixel read once however many screen pixels show
//! it, and a screen row showing the same source row as the one above is a copy of it. A pixel is one
//! 4-byte load and one 4-byte store (under emulation every memory access counts): with 3 bytes a
//! pixel the store's last byte is the next pixel's first, written over by it, so a row's last pixel
//! goes byte by byte. The copies go through the caller's `copy` (the kernel's avoids `rep movs`,
//! which QEMU's TCG runs one element at a time).

use alloc::vec::Vec;

/// The screen's pixels.
pub struct Surface<'a> {
    pub buf: &'a mut [u8],
    /// Pixels from one row to the next.
    pub stride: usize,
    /// 3 or 4.
    pub bpp: usize,
    /// The bytes of a pixel are B, G, R (else R, G, B).
    pub bgr: bool,
}

/// A rectangle in screen pixels.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Area {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

impl Area {
    fn intersection(&self, o: &Area) -> Option<Area> {
        let x0 = self.x.max(o.x);
        let y0 = self.y.max(o.y);
        let x1 = (self.x + self.w).min(o.x + o.w);
        let y1 = (self.y + self.h).min(o.y + o.h);
        (x1 > x0 && y1 > y0).then_some(Area {
            x: x0,
            y: y0,
            w: x1 - x0,
            h: y1 - y0,
        })
    }
}

/// Copy `dst` from `src` (the same length, not overlapping).
pub type Copier<'a> = &'a dyn Fn(&mut [u8], &[u8]);

/// The plain copier.
pub fn plain_copy(dst: &mut [u8], src: &[u8]) {
    dst.copy_from_slice(src);
}

/// Copy the picture `src` (`sw` x `sh`, 4 bytes a pixel: B, G, R, A with `src_bgr`, else R, G, B,
/// A) into `dst` (screen coordinates) of `s`, scaled, only inside `clip`. Returns whether anything
/// was drawn (`false` for a surface of another pixel size or a picture shorter than it says).
pub fn blit_scaled(
    s: &mut Surface,
    src: &[u8],
    (sw, sh): (usize, usize),
    src_bgr: bool,
    dst: Area,
    clip: Area,
    copy: Copier,
) -> bool {
    let bpp = s.bpp;
    if !(bpp == 3 || bpp == 4) || sw == 0 || sh == 0 || src.len() < sw * sh * 4 {
        return false;
    }
    let Some(r) = dst.intersection(&clip) else {
        return true;
    };
    if r.x < 0 || r.y < 0 {
        return false;
    }
    let (dw, dh) = (dst.w as usize, dst.h as usize);
    // Swap R and B when the picture's order is not the screen's.
    let swap = s.bgr != src_bgr;
    let stride = s.stride;
    // The source column of every screen column, once.
    let cols: Vec<usize> = (r.x..r.x + r.w)
        .map(|x| ((x - dst.x) as usize * sw / dw).min(sw - 1))
        .collect();
    let same_cols = sw == dw && r.x == dst.x && cols.len() == sw;
    let len = cols.len() * bpp;
    let buf = &mut *s.buf;
    let mut above: Option<(usize, usize)> = None;
    for y in r.y..r.y + r.h {
        let sy = ((y - dst.y) as usize * sh / dh).min(sh - 1);
        let start = (y as usize * stride + r.x as usize) * bpp;
        if buf.len() < start + len {
            continue;
        }
        if let Some((prev_sy, prev_start)) = above
            && prev_sy == sy
        {
            // The same picture row as the screen row above: a copy of it.
            let (a, b) = buf.split_at_mut(start);
            copy(&mut b[..len], &a[prev_start..prev_start + len]);
            continue;
        }
        above = Some((sy, start));
        let row = &src[sy * sw * 4..(sy + 1) * sw * 4];
        let out = &mut buf[start..start + len];
        if bpp == 4 && !swap && same_cols {
            // The picture's size and order: the row as it is.
            copy(out, row);
            continue;
        }
        let n = cols.len();
        let whole = if bpp == 4 { n } else { n.saturating_sub(1) };
        let px = |sx: usize| {
            let v = u32::from_le_bytes([
                row[sx * 4],
                row[sx * 4 + 1],
                row[sx * 4 + 2],
                row[sx * 4 + 3],
            ]);
            if swap {
                (v & 0xFF00_FF00) | ((v >> 16) & 0xFF) | ((v & 0xFF) << 16)
            } else {
                v
            }
        };
        if same_cols {
            for i in 0..whole {
                out[i * bpp..i * bpp + 4].copy_from_slice(&px(i).to_le_bytes());
            }
        } else {
            // Scaled up, neighbouring screen pixels show the same picture pixel: read it once.
            let (mut last, mut v) = (usize::MAX, 0u32);
            for (i, &sx) in cols[..whole].iter().enumerate() {
                if sx != last {
                    last = sx;
                    v = px(sx);
                }
                out[i * bpp..i * bpp + 4].copy_from_slice(&v.to_le_bytes());
            }
        }
        if whole < n {
            let sx = if same_cols { whole } else { cols[whole] };
            let v = px(sx).to_le_bytes();
            out[whole * bpp..whole * bpp + 3].copy_from_slice(&v[..3]);
        }
    }
    true
}

#[cfg(test)]
mod tests;
