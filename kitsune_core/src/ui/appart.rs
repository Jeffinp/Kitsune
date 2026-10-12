//! Art for the app interiors: the coloured file-type icons of Arquivos (folder, text, image, app,
//! generic file, drive) and the small monochrome tool glyphs of the toolbars (arrows, view
//! switches, sort, eye, house, bin, rotate, play...).
//!
//! Everything is drawn from anti-aliased paths in 24.8 fixed point at the size asked, on a
//! 64-unit (file icons) or 16-unit (glyphs) design grid, so nothing is a scaled bitmap and the
//! same code gives the 16 px sidebar glyph and the 64 px preview. The kernel caches the
//! surfaces per (kind, size, colour). Pure and host tested.

use crate::ui::glyph::Path;
use crate::ui::iconart::stroke_path;
use crate::ui::raster::{Corner, CornerMasks, Paint, Surface, rgb, rgba};
use alloc::vec::Vec;

mod files;
mod tools;
pub use files::*;
pub use tools::*;

#[cfg(test)]
mod tests;
