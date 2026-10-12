//! Colour maths on premultiplied `0xAARRGGBB`, and the paint a path is filled with.

/// Straight (non-premultiplied) colour from components.
#[inline]
pub const fn argb(a: u8, r: u8, g: u8, b: u8) -> u32 {
    ((a as u32) << 24) | ((r as u32) << 16) | ((g as u32) << 8) | b as u32
}

/// Opaque colour from `0xRRGGBB`.
#[inline]
pub const fn rgb(c: u32) -> u32 {
    0xFF00_0000 | (c & 0x00FF_FFFF)
}

/// Straight colour `0xRRGGBB` with alpha `a` (0..=255).
#[inline]
pub const fn rgba(c: u32, a: u8) -> u32 {
    ((a as u32) << 24) | (c & 0x00FF_FFFF)
}

#[inline]
pub fn alpha_of(c: u32) -> u32 {
    c >> 24
}

/// Straight -> premultiplied.
#[inline]
pub fn premul(c: u32) -> u32 {
    let a = c >> 24;
    if a == 255 {
        return c;
    }
    if a == 0 {
        return 0;
    }
    let m = |sh: u32| (((c >> sh) & 0xFF) * a + 127) / 255;
    (a << 24) | (m(16) << 16) | (m(8) << 8) | m(0)
}

/// Premultiplied -> straight (for tests and the odd conversion).
#[inline]
pub fn unpremul(c: u32) -> u32 {
    let a = c >> 24;
    if a == 0 || a == 255 {
        return c;
    }
    let u = |sh: u32| ((((c >> sh) & 0xFF) * 255 + a / 2) / a).min(255);
    (a << 24) | (u(16) << 16) | (u(8) << 8) | u(0)
}

/// Premultiplied source-over: `src` over `dst`.
#[inline]
pub fn over(dst: u32, src: u32) -> u32 {
    let sa = src >> 24;
    if sa == 255 {
        return src;
    }
    if sa == 0 {
        return dst;
    }
    let inv = 256 - (sa + (sa >> 7)); // 1..=255 (256 - 257 would underflow only for sa=255)
    let rb = (((dst & 0x00FF_00FF) * inv + 0x0080_0080) >> 8) & 0x00FF_00FF;
    let ag = ((((dst >> 8) & 0x00FF_00FF) * inv + 0x0080_0080) >> 8) & 0x00FF_00FF;
    src.wrapping_add(rb | (ag << 8))
}

/// Linear blend of two straight colours, `t` in `0..=256` (0 = `a`).
#[inline]
pub fn lerp(a: u32, b: u32, t: u32) -> u32 {
    let t = t.min(256);
    let m = |sh: u32| {
        let (x, y) = ((a >> sh) & 0xFF, (b >> sh) & 0xFF);
        (x * (256 - t) + y * t) >> 8
    };
    (m(24) << 24) | (m(16) << 16) | (m(8) << 8) | m(0)
}

/// Scale a straight colour's alpha by `a256` (0..=256).
#[inline]
pub fn mul_alpha(c: u32, a256: u32) -> u32 {
    let a = ((c >> 24) * a256.min(256)) >> 8;
    (a << 24) | (c & 0x00FF_FFFF)
}

/// How a shape is coloured. Colours are straight ARGB.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Paint {
    Solid(u32),
    /// Top to bottom.
    Vertical(u32, u32),
    /// Top-left to bottom-right.
    Diagonal(u32, u32),
}

impl Paint {
    /// Colour at `(lx, ly)` of a `w x h` shape.
    #[inline]
    pub fn at(self, lx: usize, ly: usize, w: usize, h: usize) -> u32 {
        match self {
            Paint::Solid(c) => c,
            Paint::Vertical(a, b) => lerp(a, b, ((ly * 256 + 128) / h.max(1)) as u32),
            Paint::Diagonal(a, b) => {
                let t = (lx * 256 + 128) / w.max(1) + (ly * 256 + 128) / h.max(1);
                lerp(a, b, (t / 2) as u32)
            }
        }
    }
}
