//! A small SVG renderer for pictures on web pages (logos, icons, simple illustrations).
//!
//! Integer arithmetic only (the kernel is soft-float): coordinates are 16.16 fixed point while
//! parsing, 24.8 in device space. Shapes are filled by a scanline rasteriser with four sub-scanlines
//! per pixel and exact horizontal coverage; strokes are drawn as unions of thick segments and
//! round-ish joints.
//!
//! Supported: `svg` (`viewBox`, `width`, `height`), `g`, `path` (all commands; an elliptical arc is
//! flattened to its chord), `rect` (with `rx`), `circle`, `ellipse`, `line`, `polyline`, `polygon`,
//! `fill`, `stroke`, `stroke-width`, the three opacities, `fill-rule`, `transform` (`matrix`,
//! `translate`, `scale`, `rotate`, `skewX/Y`), `style="..."`, solid colours (`#rgb`, `#rrggbb`,
//! `rgb()`, names) and gradients (the first stop stands for the whole gradient). Not drawn: text,
//! `use`, filters, masks, clip paths, dashes, `<style>` classes.
//!
//! Hostile input is bounded: element count, path segments, nesting and output pixels.

// Fixed-point maths reads best as `a * b >> 16`.
#![allow(clippy::precedence)]

use super::image::{Image, over, rgba};
use alloc::vec::Vec;

/// Largest picture drawn, in pixels.
pub const MAX_PIXELS: usize = 4 * 1024 * 1024;
const MAX_ELEMENTS: usize = 6000;
const MAX_SEGMENTS: usize = 60_000;
const MAX_DEPTH: usize = 24;
/// Sub-scanlines per pixel.
const SS: i64 = 4;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SvgError {
    /// Not an SVG document.
    NotSvg,
    /// A size of zero, or a picture over [`MAX_PIXELS`].
    BadSize,
    OutOfMemory,
}

/// Does `bytes` look like an SVG document (an `<svg` element near the start)?
pub fn looks_like(bytes: &[u8]) -> bool {
    let head = &bytes[..bytes.len().min(2048)];
    let lower: Vec<u8> = head.iter().map(u8::to_ascii_lowercase).collect();
    let t = lower
        .iter()
        .position(|b| !b.is_ascii_whitespace() && *b != 0xef && *b != 0xbb && *b != 0xbf)
        .unwrap_or(0);
    let start = &lower[t..];
    (start.starts_with(b"<svg")
        || start.starts_with(b"<?xml")
        || start.starts_with(b"<!doctype svg")
        || start.starts_with(b"<!--"))
        && find(&lower, b"<svg").is_some()
}

fn find(h: &[u8], n: &[u8]) -> Option<usize> {
    h.windows(n.len()).position(|w| w == n)
}

// ---- the document ----

/// Render `bytes` to an image at most `max_w` pixels wide (the aspect ratio kept; never enlarged
/// past the declared size).
pub fn render(bytes: &[u8], max_w: usize) -> Result<Image, SvgError> {
    let s = core::str::from_utf8(bytes).map_err(|_| SvgError::NotSvg)?;
    // Gradients first: they may be defined after their use.
    let mut grads = Gradients { list: Vec::new() };
    {
        let mut pos = 0;
        let mut cur_id: Option<&str> = None;
        let mut n = 0;
        while let Some(t) = next_tag(s, &mut pos) {
            n += 1;
            if n > MAX_ELEMENTS {
                break;
            }
            if t.name.ends_with("Gradient") && t.open {
                cur_id = t.get("id");
            } else if t.name == "stop"
                && let Some(id) = cur_id
                && !grads.list.iter().any(|(g, _)| *g == id)
            {
                let mut c = t.get("stop-color").and_then(parse_color);
                if c.is_none()
                    && let Some(st) = t.get("style")
                {
                    for d in st.split(';') {
                        if let Some(("stop-color", v)) =
                            d.split_once(':').map(|(k, v)| (k.trim(), v.trim()))
                        {
                            c = parse_color(v);
                        }
                    }
                }
                if let Some(c) = c {
                    grads.list.push((id, c));
                }
            } else if t.name.ends_with("Gradient") && t.close {
                cur_id = None;
            }
        }
    }
    let mut pos = 0;
    let (mut root, mut found) = (None, false);
    while let Some(t) = next_tag(s, &mut pos) {
        if t.name.eq_ignore_ascii_case("svg") && t.open {
            root = Some(root_size(&t).ok_or(SvgError::BadSize)?);
            found = true;
            break;
        }
    }
    if !found {
        return Err(SvgError::NotSvg);
    }
    let (nat_w, nat_h, view) = root.ok_or(SvgError::BadSize)?;
    let out_w = nat_w.min(max_w.max(1));
    let out_h = ((nat_h as u64 * out_w as u64 + nat_w as u64 / 2) / nat_w as u64).max(1) as usize;
    if out_w.checked_mul(out_h).is_none_or(|n| n > MAX_PIXELS) {
        return Err(SvgError::BadSize);
    }
    let mut cv = Canvas {
        w: out_w,
        h: out_h,
        px: Vec::new(),
    };
    cv.px
        .try_reserve_exact(out_w * out_h)
        .map_err(|_| SvgError::OutOfMemory)?;
    cv.px.resize(out_w * out_h, 0);
    // View box to device pixels (preserveAspectRatio xMidYMid meet).
    let sx = ((out_w as i64) << 16 << 16) / view[2].max(1);
    let sy = ((out_h as i64) << 16 << 16) / view[3].max(1);
    let k = sx.min(sy);
    let ox = (((out_w as i64) << 16) - (view[2] * k >> 16)) / 2;
    let oy = (((out_h as i64) << 16) - (view[3] * k >> 16)) / 2;
    let base = Mat {
        a: k,
        b: 0,
        c: 0,
        d: k,
        e: ox - (view[0] * k >> 16),
        f: oy - (view[1] * k >> 16),
    };
    walk(s, pos, &mut cv, base, &grads);
    Image::from_pixels(out_w, out_h, cv.px).map_err(|_| SvgError::BadSize)
}

mod draw;
mod number;
mod paint;
mod path;
mod raster;
mod size;
mod transform;
mod xml;

use draw::*;
use number::*;
use paint::*;
use path::*;
use raster::*;
pub use size::dims;
use size::*;
use transform::*;
use xml::*;

#[cfg(test)]
mod tests;
