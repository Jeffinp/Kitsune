//! The Kitsune brand mark: a geometric fox head, as pure polygon data.
//!
//! The mark is **vector first**. [`fox_head`] returns a dozen flat polygons on a 128 x 128
//! design grid (coordinates in quarter units, so `512` is the full width and the symmetry axis
//! is `x = 256`), each tagged with a colour [`Role`]. Everything else is derived from that data:
//!
//! * [`tile`]: the head on a rounded-square indigo tile (22 % radius), the app icon;
//! * [`mono`]: one single-colour silhouette with the eyes and the nose knocked out, for
//!   favicons, symbolic glyphs and anything that is tinted by the UI;
//! * [`halo`]: the head, smaller, in front of a half-circle crown of curved tails (nine in the
//!   source art; see [`HALO_TAILS`] for how many are drawn and why), for the boot splash and
//!   the About page;
//! * [`render`]: the anti-aliased raster at any size (16 to 128 px are the tuned ones), made by
//!   supersampling the polygons and averaging down, with per-size hints ([`hint`]) that merge
//!   facets and thicken the eyes and the nose so that 16 and 24 px stay readable;
//! * [`to_svg`]: the same polygons as hand-readable SVG (`docs/brand/` is generated with
//!   `cargo run -p kitsune_core --example brand_svg`).
//!
//! No floating point, no `unsafe`; the module is pure and host tested. The polygons are
//! painted back to front and overlap where facets meet, so no seam of the background shows
//! through between two flat facets.

use crate::ui::glyph::Path;
use crate::ui::iconart::{cos_q14, sin_q14};
use crate::ui::raster::{Paint, Surface, argb};
use alloc::string::String;
use alloc::vec::Vec;

mod halo;
mod head;
mod hints;
mod render;
mod svg;
pub use halo::*;
pub use head::*;
pub use hints::*;
pub use render::*;
pub use svg::*;

/// Side of the design grid, in grid units.
pub const GRID: i32 = 128;
/// Coordinates are in `1 / UNIT` of a grid unit.
pub const UNIT: i32 = 4;
/// Side of the design grid in coordinate units (the valid range is `0..=EXTENT`).
pub const EXTENT: i32 = GRID * UNIT;
/// Corner radius of the tile: 22 % of the side, in coordinate units.
pub const TILE_RADIUS: i32 = 28 * UNIT;
/// How many of the nine tails of the story are drawn in [`halo`].
///
/// Nine tails over a half circle are 20 degrees apart: at the 96-112 px of the splash the
/// gaps are 3-4 px wide and the crown turns into a noisy comb, and at 48 px it is a smudge.
/// Seven tails (24 degrees apart, the middle one peeking out between the ears) keep every
/// tail, every gap and the cream tips legible down to about 64 px. [`halo_n`] builds any
/// count from 1 to 9 (the nine-tail art is a test and a docs example, not a UI asset).
pub const HALO_TAILS: usize = 7;
/// What a polygon is for; a [`Scheme`] maps it to a colour.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    /// The rounded-square background of the app icon.
    Tile,
    /// Left half of the face.
    LightFace,
    /// Right half of the face and the two cheek facets.
    ShadeFace,
    /// Outer facet of an ear.
    EarOuter,
    /// Darker inner facet of an ear.
    EarInner,
    /// The cream V-shaped muzzle and cheeks.
    Muzzle,
    Eye,
    Nose,
    /// The single colour of the [`mono`] silhouette.
    Mark,
    /// A hole cut out of the [`mono`] silhouette (eye, nose).
    Knockout,
    /// A tail (even ones).
    Tail,
    /// A tail (odd ones, a shade darker so neighbours separate).
    TailDark,
    /// The cream tip of a tail.
    TailTip,
}
/// Straight `0xRRGGBB` colours for every role.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Scheme {
    pub tile: u32,
    pub light_face: u32,
    pub shade_face: u32,
    pub ear_outer: u32,
    pub ear_inner: u32,
    pub muzzle: u32,
    pub eye: u32,
    pub nose: u32,
    pub tail: u32,
    pub tail_dark: u32,
    pub tail_tip: u32,
}
impl Scheme {
    /// The brand palette (see `docs/brand/README.md`).
    pub const KITSUNE: Scheme = Scheme {
        tile: 0x2A2A5C,
        light_face: 0xFF7A33,
        shade_face: 0xC94F1C,
        ear_outer: 0xF76B2A,
        ear_inner: 0xB7371A,
        muzzle: 0xFFF3E6,
        eye: 0x1A1A3A,
        nose: 0x1A1A3A,
        tail: 0xC94F1C,
        tail_dark: 0xB7371A,
        tail_tip: 0xFFF3E6,
    };

    /// The colour of `role` (`Mark`/`Knockout` are not part of a scheme: the caller
    /// chooses them, see [`mono`]).
    pub fn color(&self, role: Role) -> u32 {
        match role {
            Role::Tile => self.tile,
            Role::LightFace => self.light_face,
            Role::ShadeFace => self.shade_face,
            Role::EarOuter => self.ear_outer,
            Role::EarInner => self.ear_inner,
            Role::Muzzle => self.muzzle,
            Role::Eye => self.eye,
            Role::Nose => self.nose,
            Role::Tail => self.tail,
            Role::TailDark => self.tail_dark,
            Role::TailTip => self.tail_tip,
            Role::Mark | Role::Knockout => 0xFFFFFF,
        }
    }
}
/// The corners of a polygon.
type Pts = Vec<(i32, i32)>;
/// One flat polygon: a role, the colour it was given and its corners (coordinate units,
/// y down, implicitly closed, clockwise unless it is a [`Role::Knockout`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Poly {
    pub role: Role,
    pub color: u32,
    pub pts: Vec<(i32, i32)>,
}
/// A drawable mark: an optional tile behind some polygons, painted in order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Mark {
    /// Colour of the rounded-square tile, if the mark has one.
    pub tile: Option<u32>,
    pub polys: Vec<Poly>,
}
impl Mark {
    /// Every polygon corner of the mark.
    pub fn points(&self) -> impl Iterator<Item = (i32, i32)> + '_ {
        self.polys.iter().flat_map(|p| p.pts.iter().copied())
    }
}

#[cfg(test)]
mod tests;
