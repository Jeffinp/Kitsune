//! Framebuffer arithmetic that does not touch memory: colour mixing, integer
//! square root and rounded-corner insets.
//!
//! The kernel's `Canvas` owns the pixel buffer and the hot loops; it calls these
//! tiny `#[inline]` helpers so their results (and edge cases) are covered by
//! host tests.

/// An 8-bit-per-channel RGB colour.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Color {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

impl Color {
    pub const fn rgb(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b }
    }

    /// Linear blend between `self` and `other`. `t` in 0..=255 (0 = self);
    /// larger values are treated as 255.
    #[inline]
    pub fn lerp(self, other: Color, t: u16) -> Color {
        let t = t.min(255);
        let mix = |a: u8, b: u8| -> u8 {
            let a = a as u16;
            let b = b as u16;
            ((a * (255 - t) + b * t) / 255) as u8
        };
        Color::rgb(
            mix(self.r, other.r),
            mix(self.g, other.g),
            mix(self.b, other.b),
        )
    }
}

/// Integer square root (floor). Used to inset rounded-rectangle corner rows.
#[inline]
pub fn isqrt(n: usize) -> usize {
    if n == 0 {
        return 0;
    }
    let mut x = n;
    let mut y = x.div_ceil(2);
    while y < x {
        x = y;
        y = (x + n / x) / 2;
    }
    x
}

/// How many pixels row `y` (0-based, of `h` rows) of a rounded rectangle with
/// corner radius `r` is inset from each side. `r` is clamped to `h / 2`.
#[inline]
pub fn corner_inset(r: usize, y: usize, h: usize) -> usize {
    let r = r.min(h / 2);
    if r == 0 {
        0
    } else if y < r {
        let dy = r - y; // 1..=r
        r - isqrt(r * r - dy * dy)
    } else if y >= h - r {
        let dy = y - (h - 1 - r); // 0..=r
        r - isqrt(r.saturating_mul(r).saturating_sub(dy * dy))
    } else {
        0
    }
}

/// `(dst * (256 - a) + src * a) / 256` with `a` clamped to `0..=256`
/// (0 = `dst` unchanged, 256 = `src`).
#[inline]
pub fn mix256(dst: u8, src: u8, a: u16) -> u8 {
    let a = a.min(256);
    ((dst as u16 * (256 - a) + src as u16 * a) / 256) as u8
}

/// Converts an 8-bit alpha (0..=255) to the 0..=256 range `mix256` expects, so
/// 255 maps to exactly 256 (opaque).
#[inline]
pub fn alpha255_to_256(a: u8) -> u16 {
    a as u16 + (a >> 7) as u16
}

/// Per-channel destination -> result tables for blending the constant colour
/// `c` at `alpha` (0..=256) over any destination byte. Bit-identical to calling
/// [`mix256`] per pixel.
#[inline]
pub fn blend_lut(c: [u8; 3], alpha: u16) -> [[u8; 256]; 3] {
    let a = alpha.min(256);
    let ia = 256 - a;
    let sa = [c[0] as u16 * a, c[1] as u16 * a, c[2] as u16 * a];
    let mut t = [[0u8; 256]; 3];
    for (table, &src) in t.iter_mut().zip(sa.iter()) {
        for (v, out) in table.iter_mut().enumerate() {
            *out = ((v as u16 * ia + src) / 256) as u8;
        }
    }
    t
}

/// Grayscale luminance of an RGB triple (for 8-bit framebuffers).
#[inline]
pub fn luma(c: Color) -> u8 {
    ((c.r as u16 * 54 + c.g as u16 * 183 + c.b as u16 * 19) >> 8) as u8
}

/// A half-open pixel span `[start, end)` within a row.
pub type Span = (usize, usize);

/// The parts of the span `[xs, xe)` left after removing the hole
/// `[hx, hx + hw)`: `(left, right)`, each `None` when empty.
pub fn split_span_around_hole(
    xs: usize,
    xe: usize,
    hx: usize,
    hw: usize,
) -> (Option<Span>, Option<Span>) {
    let left_end = xe.min(hx);
    let left = (xs < left_end).then_some((xs, left_end));
    let right_start = xs.max(hx + hw);
    let right = (right_start < xe).then_some((right_start, xe));
    (left, right)
}

#[cfg(test)]
mod tests;
pub mod videoblit;
