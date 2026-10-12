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

#[path = "shapes/blit.rs"]
mod blit;
#[path = "shapes/glow.rs"]
mod glow;
// Explicit paths, like `fb.rs` does for this file: the bench crate includes `fb.rs` through
// `#[path]`, which changes where nested modules are looked up.
#[path = "shapes/rrect.rs"]
mod rrect;
#[path = "shapes/shadow.rs"]
mod shadow;

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
