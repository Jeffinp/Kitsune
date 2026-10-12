//! Corner masks: the coverage of a rounded corner (circular or "continuous"), cached per radius.

use super::*;

/// Shape of a rounded corner.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Corner {
    /// Quarter circle.
    Circle,
    /// "Continuous" corner: a superellipse (n = 4), what app icons and the dock use.
    Squircle,
}

/// Largest radius with a precomputed mask.
pub const MAX_RADIUS: usize = 64;

/// Coverage masks for the top-left corner of every radius `0..=MAX_RADIUS`.
pub struct CornerMasks {
    pub(super) circle: Vec<Vec<u8>>,
    pub(super) squircle: Vec<Vec<u8>>,
}

impl CornerMasks {
    pub fn new() -> CornerMasks {
        Self::with_max_radius(MAX_RADIUS)
    }

    /// Masks for radii `0..=max` (at most [`MAX_RADIUS`]); larger requests use the largest.
    pub fn with_max_radius(max: usize) -> CornerMasks {
        let max = max.min(MAX_RADIUS);
        let mut circle = Vec::with_capacity(max + 1);
        let mut squircle = Vec::with_capacity(max + 1);
        for r in 0..=max {
            circle.push(build_mask(r, Corner::Circle));
            squircle.push(build_mask(r, Corner::Squircle));
        }
        CornerMasks { circle, squircle }
    }

    /// `r * r` coverage bytes, row-major; `(0, 0)` is the outermost corner pixel.
    pub fn get(&self, style: Corner, r: usize) -> &[u8] {
        let r = r.min(self.circle.len() - 1);
        match style {
            Corner::Circle => &self.circle[r],
            Corner::Squircle => &self.squircle[r],
        }
    }
}

impl Default for CornerMasks {
    fn default() -> Self {
        Self::new()
    }
}

/// Coverage of pixel `(x, y)` of the corner square of radius `r`.
pub(super) fn build_mask(r: usize, style: Corner) -> Vec<u8> {
    let mut m = vec![0u8; r * r];
    if r == 0 {
        return m;
    }
    for y in 0..r {
        for x in 0..r {
            // Pixel centre offset from the arc centre (at (r, r)), in 1/256 px.
            let dx = (r * 256) as i64 - (x * 256) as i64 - 128;
            let dy = (r * 256) as i64 - (y * 256) as i64 - 128;
            let cov256: i64 = match style {
                Corner::Circle => {
                    let d = isqrt((dx * dx + dy * dy) as usize) as i64;
                    ((r * 256) as i64 + 128 - d).clamp(0, 256)
                }
                Corner::Squircle => {
                    // u, v in Q16 (0..1), f = u^4 + v^4, signed distance ~ (1-f) r / |grad f|.
                    let uq = dx * 256 / r as i64;
                    let vq = dy * 256 / r as i64;
                    let (u2, v2) = ((uq * uq) >> 16, (vq * vq) >> 16);
                    let (u3, v3) = ((u2 * uq) >> 16, (v2 * vq) >> 16);
                    let (u4, v4) = ((u3 * uq) >> 16, (v3 * vq) >> 16);
                    let f = u4 + v4;
                    let g2 = 16 * ((u3 * u3 + v3 * v3) >> 16); // |grad f|^2, Q16
                    let g = isqrt((g2.max(1) as usize) << 16) as i64; // Q16
                    if g == 0 {
                        256
                    } else {
                        let sd = (65536 - f) * r as i64 * 256 / g; // pixels in 1/256
                        (sd + 128).clamp(0, 256)
                    }
                }
            };
            m[y * r + x] = ((cov256 * 255 + 128) >> 8) as u8;
        }
    }
    m
}

/// Coverage (0..=255) of pixel `(lx, ly)` of a `w x h` rounded rectangle with
/// corner radius `r` (already clamped to `min(w, h) / 2`) using `mask`.
#[inline]
pub fn rrect_cov(mask: &[u8], r: usize, w: usize, h: usize, lx: usize, ly: usize) -> u8 {
    if r == 0 {
        return 255;
    }
    let cx = if lx < r {
        lx
    } else if lx + r >= w {
        w - 1 - lx
    } else {
        return 255;
    };
    let cy = if ly < r {
        ly
    } else if ly + r >= h {
        h - 1 - ly
    } else {
        return 255;
    };
    mask[cy * r + cx]
}
