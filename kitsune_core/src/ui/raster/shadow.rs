//! The 1-D shadow profile of window shadows and the glow lookup table.

use super::*;

/// One axis of a blurred-rectangle shadow: coverage (0..=255) of `len + 2 * blur`
/// positions for a segment of `len` pixels starting at index `blur`, with a
/// smoothstep transition `blur` pixels either side of each edge. The 2-D shadow of
/// a rectangle is the product of two such profiles (the blur is separable).
pub fn shadow_profile(len: usize, blur: usize) -> Vec<u8> {
    let total = len + 2 * blur;
    let mut p = vec![0u8; total];
    if blur == 0 {
        for v in p.iter_mut().take(len) {
            *v = 255;
        }
        return p;
    }
    let cdf = |x2: i64| -> i64 {
        // x2 = 2 * (distance past the edge); transition covers -2b..2b.
        let s =
            (((x2 + 2 * blur as i64) * 256 + 2 * blur as i64) / (4 * blur as i64)).clamp(0, 256);
        s * s * (768 - 2 * s) / (256 * 256) // 0..=256
    };
    for (i, v) in p.iter_mut().enumerate() {
        let x2 = 2 * i as i64 + 1 - 2 * blur as i64;
        let cov = cdf(x2) - cdf(x2 - 2 * len as i64);
        *v = (cov.clamp(0, 256) * 255 / 256) as u8;
    }
    p
}

/// Opacity (0..=255) of a shadow pixel from the two axis profile values and the
/// shadow's overall opacity (0..=256).
#[inline]
pub fn shadow_alpha(px: u8, py: u8, opacity: u32) -> u32 {
    (px as u32 * py as u32 * opacity.min(256)) >> 16
}

/// Falloff table for a soft radial glow: entry `i` is the opacity (0..=255) at a
/// squared distance of `i / 255` of the squared radius (smoothstep fall-off).
pub fn glow_lut() -> [u8; 256] {
    let mut t = [0u8; 256];
    for (i, v) in t.iter_mut().enumerate() {
        // Distance fraction d = sqrt(i/255): approximate with i itself in d^2 space,
        // then smoothstep(1 - d^2) for a long soft tail.
        let s = 256 - i as i64 * 256 / 255;
        *v = (s * s * (768 - 2 * s) / (256 * 256) * 255 / 256) as u8;
    }
    t
}
