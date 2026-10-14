//! Minimal framebuffer drawing primitives. No allocation, writes pixels directly.

use bootloader_api::info::{FrameBufferInfo, PixelFormat};

pub use kitsune_core::gfx::Color;

#[path = "fb/copy.rs"]
mod copy;
#[path = "fb/scale.rs"] // explicit so the bench crate can `#[path]`-include this file
mod scale;
#[path = "fb/shapes.rs"]
mod shapes;
// The shape / shadow / surface toolkit (`Canvas` methods live in `shapes.rs`).
#[allow(unused_imports)]
pub use copy::copy_bytes;
use kitsune_core::gfx::{
    alpha255_to_256, blend_lut, corner_inset, luma, mix256, split_span_around_hole,
};
#[allow(unused_imports)]
pub use shapes::{Corner, Shadow, masks};

/// Minimum `w*h` for the table-driven alpha fill (building the table costs
/// ~3k instructions, which only pays off on large areas).
const LUT_MIN_PIXELS: usize = 4096;

/// A saved clip rectangle (see [`Canvas::set_clip`]).
#[derive(Clone, Copy)]
#[allow(dead_code)] // used by the window chrome and the widgets from the next commits
pub struct ClipState([usize; 4]);

pub struct Canvas<'a> {
    buf: &'a mut [u8],
    info: FrameBufferInfo,
    /// Drawing is limited to `[cx0, cx1) x [cy0, cy1)` (the whole canvas by default).
    cx0: usize,
    cy0: usize,
    cx1: usize,
    cy1: usize,
}

impl<'a> Canvas<'a> {
    pub fn new(buf: &'a mut [u8], info: FrameBufferInfo) -> Self {
        Self {
            buf,
            info,
            cx0: 0,
            cy0: 0,
            cx1: info.width,
            cy1: info.height,
        }
    }

    /// Limit all further drawing to `r` (intersected with the canvas). Returns the
    /// previous clip so callers can restore it with [`Canvas::restore_clip`].
    #[allow(dead_code)]
    pub fn set_clip(&mut self, r: kitsune_core::Rect) -> ClipState {
        let old = ClipState([self.cx0, self.cy0, self.cx1, self.cy1]);
        self.cx0 = (r.x.max(0) as usize).min(self.info.width);
        self.cy0 = (r.y.max(0) as usize).min(self.info.height);
        self.cx1 = (r.right().max(0) as usize)
            .min(self.info.width)
            .max(self.cx0);
        self.cy1 = (r.bottom().max(0) as usize)
            .min(self.info.height)
            .max(self.cy0);
        old
    }

    #[allow(dead_code)]
    pub fn restore_clip(&mut self, c: ClipState) {
        [self.cx0, self.cy0, self.cx1, self.cy1] = c.0;
    }

    /// The current clip rectangle.
    pub fn clip_rect(&self) -> kitsune_core::Rect {
        kitsune_core::Rect::new(
            self.cx0 as i32,
            self.cy0 as i32,
            (self.cx1 - self.cx0) as i32,
            (self.cy1 - self.cy0) as i32,
        )
    }

    #[inline]
    pub fn width(&self) -> usize {
        self.info.width
    }

    #[inline]
    pub fn height(&self) -> usize {
        self.info.height
    }

    #[inline]
    pub fn bpp(&self) -> usize {
        self.info.bytes_per_pixel
    }

    /// The framebuffer layout backing this canvas.
    #[inline]
    pub fn fb_info(&self) -> FrameBufferInfo {
        self.info
    }

    /// Mutable access to the raw pixel buffer. Lets the WASM app engine draw a
    /// guest's window content into the live compositor buffer through its own
    /// `Canvas` over the same memory.
    #[inline]
    pub fn buffer_mut(&mut self) -> &mut [u8] {
        self.buf
    }

    /// Pack a color into a native-endian 32-bit pixel for the 4-byte-per-pixel
    /// formats. `None` for layouts we can't pack (grayscale / unknown), which
    /// fall back to the byte path. The padding byte is left 0.
    #[inline]
    fn pack32(&self, c: Color) -> Option<u32> {
        let (b0, b1, b2) = match self.info.pixel_format {
            PixelFormat::Rgb => (c.r, c.g, c.b),
            PixelFormat::Bgr => (c.b, c.g, c.r),
            _ => return None,
        };
        Some(u32::from_le_bytes([b0, b1, b2, 0]))
    }

    #[inline]
    pub fn put(&mut self, x: usize, y: usize, c: Color) {
        if x < self.cx0 || x >= self.cx1 || y < self.cy0 || y >= self.cy1 {
            return;
        }
        let bpp = self.info.bytes_per_pixel;
        let offset = (y * self.info.stride + x) * bpp;
        let px = &mut self.buf[offset..offset + bpp];
        match self.info.pixel_format {
            PixelFormat::Rgb => {
                px[0] = c.r;
                px[1] = c.g;
                px[2] = c.b;
            }
            PixelFormat::Bgr => {
                px[0] = c.b;
                px[1] = c.g;
                px[2] = c.r;
            }
            PixelFormat::U8 => {
                // Grayscale: luminance approximation.
                px[0] = luma(c);
            }
            _ => {
                // Unknown layout: best-effort RGB.
                if bpp >= 3 {
                    px[0] = c.r;
                    px[1] = c.g;
                    px[2] = c.b;
                }
            }
        }
    }

    /// Write one row of opaque `0xAARRGGBB` pixels (alpha ignored) starting at
    /// `(x, y)`, clipped to the framebuffer. The image viewer's fast path: the 4-byte
    /// formats are converted straight into the buffer.
    pub fn put_row(&mut self, x: usize, y: usize, px: &[u32]) {
        if y >= self.info.height || x >= self.info.width {
            return;
        }
        let n = px.len().min(self.info.width - x);
        let bpp = self.info.bytes_per_pixel;
        let fmt = self.info.pixel_format;
        if bpp == 4 && matches!(fmt, PixelFormat::Rgb | PixelFormat::Bgr) {
            let off = (y * self.info.stride + x) * bpp;
            let (dst, _) = self.buf[off..off + n * 4].as_chunks_mut::<4>();
            let bgr = matches!(fmt, PixelFormat::Bgr);
            for (d, p) in dst.iter_mut().zip(px) {
                let (r, g, b) = ((*p >> 16) as u8, (*p >> 8) as u8, *p as u8);
                if bgr {
                    d[0] = b;
                    d[1] = g;
                    d[2] = r;
                } else {
                    d[0] = r;
                    d[1] = g;
                    d[2] = b;
                }
            }
            return;
        }
        for (i, p) in px.iter().take(n).enumerate() {
            self.put(
                x + i,
                y,
                Color::rgb((*p >> 16) as u8, (*p >> 8) as u8, *p as u8),
            );
        }
    }

    /// Alpha-blit a tightly-packed RGBA image (`iw*ih*4` bytes) at `(x0, y0)`.
    #[allow(dead_code)]
    pub fn draw_rgba(&mut self, data: &[u8], iw: usize, ih: usize, x0: usize, y0: usize) {
        for y in 0..ih {
            for x in 0..iw {
                let o = (y * iw + x) * 4;
                let a = data[o + 3];
                if a == 0 {
                    continue;
                }
                let col = Color::rgb(data[o], data[o + 1], data[o + 2]);
                // Scale 0..255 alpha to the 0..256 range blend_pixel expects.
                self.blend_pixel(x0 + x, y0 + y, col, alpha255_to_256(a));
            }
        }
    }

    pub fn fill_rect(&mut self, x0: usize, y0: usize, w: usize, h: usize, c: Color) {
        let t0 = crate::trace::t();
        self.fill_rect_inner(x0, y0, w, h, c);
        crate::trace::prim(crate::trace::Prim::FillRect, t0);
    }

    fn fill_rect_inner(&mut self, x0: usize, y0: usize, w: usize, h: usize, c: Color) {
        let x_end = x0.saturating_add(w).min(self.cx1);
        let y_end = y0.saturating_add(h).min(self.cy1);
        let (x0, y0) = (x0.max(self.cx0), y0.max(self.cy0));
        if x_end <= x0 || y_end <= y0 {
            return;
        }
        let bpp = self.info.bytes_per_pixel;
        let stride = self.info.stride;
        let count = x_end - x0;

        // Fastest path: 4-byte pixels written 32 bits at a time. One store per
        // pixel instead of three, which the backend lowers to `rep stosd` / SSE.
        // The render buffers are 64-byte aligned and rows start on a 4-byte
        // boundary, so `align_to_mut::<u32>` yields no scalar prefix/suffix.
        if bpp == 4
            && let Some(packed) = self.pack32(c)
        {
            for y in y0..y_end {
                let o = (y * stride + x0) * 4;
                let row = &mut self.buf[o..o + count * 4];
                // SAFETY: every initialized `[u8]` is valid as `[u32]` (no invalid bit patterns); `align_to_mut`
                // itself returns the unaligned prefix/suffix, so alignment is handled.
                let (pre, mid, suf) = unsafe { row.align_to_mut::<u32>() };
                if pre.is_empty() && suf.is_empty() {
                    mid.fill(packed);
                } else {
                    let b = packed.to_ne_bytes();
                    for (i, px) in row.iter_mut().enumerate() {
                        *px = b[i & 3];
                    }
                }
            }
            return;
        }

        // 3-byte RGB/BGR ordering written directly per row, skipping the
        // per-pixel bounds check + format match that `put` performs.
        let order = match self.info.pixel_format {
            PixelFormat::Rgb => Some((c.r, c.g, c.b)),
            PixelFormat::Bgr => Some((c.b, c.g, c.r)),
            _ => None,
        };
        let Some((p0, p1, p2)) = order else {
            for y in y0..y_end {
                for x in x0..x_end {
                    self.put(x, y, c);
                }
            }
            return;
        };
        // 24-bit framebuffers (BIOS/VBE, `bytes_per_pixel == 3`): store four
        // pixels (12 bytes) per step from a prebuilt pattern instead of three
        // bounds-checked byte stores per pixel. Same bytes written.
        if bpp == 3 {
            let quad = [p0, p1, p2, p0, p1, p2, p0, p1, p2, p0, p1, p2];
            for y in y0..y_end {
                let o = (y * stride + x0) * 3;
                let row = &mut self.buf[o..o + count * 3];
                let (quads, rest) = row.as_chunks_mut::<12>();
                for q in quads {
                    *q = quad;
                }
                for px in rest.as_chunks_mut::<3>().0 {
                    *px = [p0, p1, p2];
                }
            }
            return;
        }
        for y in y0..y_end {
            let mut o = (y * stride + x0) * bpp;
            for _ in x0..x_end {
                self.buf[o] = p0;
                self.buf[o + 1] = p1;
                self.buf[o + 2] = p2;
                o += bpp;
            }
        }
    }

    /// Blend a solid color into one pixel. `alpha` in `0..=256`
    /// (0 = unchanged, 256 = fully `c`). Format-aware.
    #[inline]
    #[allow(dead_code)]
    pub fn blend_pixel(&mut self, x: usize, y: usize, c: Color, alpha: u16) {
        if x < self.cx0 || x >= self.cx1 || y < self.cy0 || y >= self.cy1 {
            return;
        }
        let a = alpha.min(256);
        let bpp = self.info.bytes_per_pixel;
        let o = (y * self.info.stride + x) * bpp;
        let (c0, c1, c2) = match self.info.pixel_format {
            PixelFormat::Rgb => (c.r, c.g, c.b),
            PixelFormat::Bgr => (c.b, c.g, c.r),
            _ => return,
        };
        self.buf[o] = mix256(self.buf[o], c0, a);
        self.buf[o + 1] = mix256(self.buf[o + 1], c1, a);
        self.buf[o + 2] = mix256(self.buf[o + 2], c2, a);
    }

    /// Blend an 8-bit coverage bitmap (`w x h`, tightly packed) with its top-left
    /// at `(x, y)` in colour `c`. `lut` maps coverage to opacity (text gamma),
    /// `alpha` (0..=256) scales it (fades). Clipped; only the 3-colour-byte formats.
    #[allow(clippy::too_many_arguments)]
    pub fn blend_coverage(
        &mut self,
        x: i32,
        y: i32,
        cov: &[u8],
        w: usize,
        h: usize,
        c: Color,
        alpha: u16,
        lut: &[u8; 256],
    ) {
        if w == 0 || h == 0 || cov.len() < w * h || alpha == 0 {
            return;
        }
        let (c0, c1, c2) = match self.info.pixel_format {
            PixelFormat::Rgb => (c.r as u32, c.g as u32, c.b as u32),
            PixelFormat::Bgr => (c.b as u32, c.g as u32, c.r as u32),
            _ => return,
        };
        let bpp = self.info.bytes_per_pixel;
        let stride = self.info.stride;
        // Visible part of the bitmap.
        let gx0 = (self.cx0 as i32 - x).max(0) as usize;
        let gy0 = (self.cy0 as i32 - y).max(0) as usize;
        let gx1 = ((self.cx1 as i32 - x).max(0) as usize).min(w);
        let gy1 = ((self.cy1 as i32 - y).max(0) as usize).min(h);
        if gx0 >= gx1 || gy0 >= gy1 {
            return;
        }
        for gy in gy0..gy1 {
            let row = &cov[gy * w + gx0..gy * w + gx1];
            let py = (y + gy as i32) as usize;
            let mut o = (py * stride + (x + gx0 as i32) as usize) * bpp;
            for &cv in row {
                if cv != 0 {
                    let mut a = lut[cv as usize] as u32;
                    a += a >> 7; // 0..=256
                    a = (a * alpha.min(256) as u32) >> 8;
                    let px = &mut self.buf[o..o + 3];
                    px[0] = ((px[0] as u32 * (256 - a) + c0 * a) >> 8) as u8;
                    px[1] = ((px[1] as u32 * (256 - a) + c1 * a) >> 8) as u8;
                    px[2] = ((px[2] as u32 * (256 - a) + c2 * a) >> 8) as u8;
                }
                o += bpp;
            }
        }
    }

    /// Rounded rectangle blended over existing pixels at `alpha` (0..=256).
    /// Used for soft drop shadows and translucent surfaces.
    #[allow(clippy::too_many_arguments)]
    pub fn fill_round_rect_alpha(
        &mut self,
        x0: usize,
        y0: usize,
        w: usize,
        h: usize,
        r: usize,
        c: Color,
        alpha: u16,
    ) {
        let t0 = crate::trace::t();
        self.fill_round_rect_alpha_inner(x0, y0, w, h, r, c, alpha, (0, 0, 0, 0));
        crate::trace::prim(crate::trace::Prim::Alpha, t0);
    }

    #[allow(clippy::too_many_arguments)]
    fn fill_round_rect_alpha_inner(
        &mut self,
        x0: usize,
        y0: usize,
        w: usize,
        h: usize,
        r: usize,
        c: Color,
        alpha: u16,
        hole: (usize, usize, usize, usize),
    ) {
        let (hx, hy, hw, hh) = hole;
        let r = r.min(w / 2).min(h / 2);
        let a = alpha.min(256);
        if a == 0 {
            return;
        }
        let ia = 256 - a;

        // Hoist everything that does not change per pixel out of the loop. The
        // old path called `inside_round` (an isqrt per pixel) and `blend_pixel`
        // (a format match + bounds check per pixel) across the whole w*h box, so
        // a single window shadow was ~hundreds of thousands of isqrts. Now isqrt
        // runs once per row (corner inset, identical geometry to inside_round)
        // and the inner loop is a straight blend over a clamped span -- the same
        // span strategy `fill_round_rect` already uses. Same pixels, ~orders of
        // magnitude fewer ops; this is what tanked FPS with several windows up.
        let bpp = self.info.bytes_per_pixel;
        let stride = self.info.stride;
        let (c0, c1, c2) = match self.info.pixel_format {
            PixelFormat::Rgb => (c.r, c.g, c.b),
            PixelFormat::Bgr => (c.b, c.g, c.r),
            _ => return,
        };
        // Pre-multiply the source contribution (src * a) once per channel.
        let (sa0, sa1, sa2) = (c0 as u16 * a, c1 as u16 * a, c2 as u16 * a);

        // For large areas (window shadows are ~170k pixels, two layers per
        // window) the blend is a pure function of the destination byte, because
        // colour and alpha are constant for the whole call. Tabulate it once
        // (3 x 256 entries, same formula as the scalar loop => bit-identical
        // output) so the per-pixel work is 3 loads + 3 stores instead of 3
        // multiplies, 3 divides-by-256 and 6 bounds-checked accesses.
        let lut = if w * h >= LUT_MIN_PIXELS && (bpp == 3 || bpp == 4) {
            Some(blend_lut([c0, c1, c2], a))
        } else {
            None
        };

        for y in 0..h {
            let py = y0 + y;
            if py >= self.cy1 {
                break;
            }
            if py < self.cy0 {
                continue;
            }
            let inset = corner_inset(r, y, h);
            if w <= 2 * inset {
                continue;
            }
            let xs = (x0 + inset).max(self.cx0);
            let xe = (x0 + w - inset).min(self.cx1);
            if xs >= xe {
                continue;
            }
            let row = py * stride;
            // Blend one horizontal span `[a, b)` of this row.
            let blend = |buf: &mut [u8], a: usize, b: usize| {
                if let Some(t) = &lut {
                    let lo = (row + a) * bpp;
                    let span = &mut buf[lo..lo + (b - a) * bpp];
                    if bpp == 3 {
                        for px in span.as_chunks_mut::<3>().0.iter_mut() {
                            px[0] = t[0][px[0] as usize];
                            px[1] = t[1][px[1] as usize];
                            px[2] = t[2][px[2] as usize];
                        }
                    } else {
                        // 4 bytes/pixel: the 4th (padding) byte is left
                        // untouched, exactly like the scalar path.
                        for px in span.as_chunks_mut::<4>().0.iter_mut() {
                            px[0] = t[0][px[0] as usize];
                            px[1] = t[1][px[1] as usize];
                            px[2] = t[2][px[2] as usize];
                        }
                    }
                    return;
                }
                for px in a..b {
                    let o = (row + px) * bpp;
                    buf[o] = ((buf[o] as u16 * ia + sa0) / 256) as u8;
                    buf[o + 1] = ((buf[o + 1] as u16 * ia + sa1) / 256) as u8;
                    buf[o + 2] = ((buf[o + 2] as u16 * ia + sa2) / 256) as u8;
                }
            };
            if hw > 0 && py >= hy && py < hy + hh {
                // Rows crossing the hole: blend only left of / right of it.
                let (left, right) = split_span_around_hole(xs, xe, hx, hw);
                if let Some((a, b)) = left {
                    blend(self.buf, a, b);
                }
                if let Some((a, b)) = right {
                    blend(self.buf, a, b);
                }
            } else {
                blend(self.buf, xs, xe);
            }
        }
    }

    /// Rounded rectangle (filled). `r` = corner radius in pixels.
    pub fn fill_round_rect(
        &mut self,
        x0: usize,
        y0: usize,
        w: usize,
        h: usize,
        r: usize,
        c: Color,
    ) {
        let t0 = crate::trace::t();
        self.fill_round_rect_inner(x0, y0, w, h, r, c);
        crate::trace::prim(crate::trace::Prim::RoundRect, t0);
    }

    fn fill_round_rect_inner(
        &mut self,
        x0: usize,
        y0: usize,
        w: usize,
        h: usize,
        r: usize,
        c: Color,
    ) {
        let r = r.min(w / 2).min(h / 2);
        // Per-row spans: corner rows inset by the circle, middle rows full
        // width. Each span is one fast `fill_rect` instead of per-pixel `put`.
        for y in 0..h {
            let inset = corner_inset(r, y, h);
            if w > 2 * inset {
                self.fill_rect(x0 + inset, y0 + y, w - 2 * inset, 1, c);
            }
        }
    }
}
