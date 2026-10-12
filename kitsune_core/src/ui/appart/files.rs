//! The coloured file-type icons of Arquivos and the drawing helpers they share (grid, rounded rectangle, line, page).

use super::*;

/// What a file icon shows.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum FileKind {
    Folder,
    Text,
    Image,
    App,
    Generic,
    Drive,
}

/// Every file kind, in a stable order.
pub const FILE_KINDS: [FileKind; 6] = [
    FileKind::Folder,
    FileKind::Text,
    FileKind::Image,
    FileKind::App,
    FileKind::Generic,
    FileKind::Drive,
];

pub(super) fn mix(a: u32, b: u32, t: u32) -> u32 {
    crate::ui::raster::lerp(a, b, t)
}

/// A scaler from the 64-unit design grid to 24.8 pixels for a `px`-wide icon.
pub(super) struct Grid {
    pub(super) px: i32,
}

impl Grid {
    /// 24.8 coordinate of design unit `v` (64 units span the whole icon).
    pub(super) fn u(&self, v: i32) -> i32 {
        v * self.px * 256 / 64
    }

    /// Same with a half-unit fraction (`v` in half units).
    pub(super) fn h(&self, v2: i32) -> i32 {
        v2 * self.px * 256 / 128
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) fn rrect(
    s: &mut Surface,
    g: &Grid,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    r: i32,
    paint: Paint,
) {
    let mut p = Path::new();
    p.rrect(g.u(x), g.u(y), g.u(w), g.u(h), g.u(r));
    s.fill_path(&p, paint);
}

pub(super) fn line(s: &mut Surface, g: &Grid, a: (i32, i32), b: (i32, i32), w2: i32, c: u32) {
    s.fill_path(
        &stroke_path(&[(g.u(a.0), g.u(a.1)), (g.u(b.0), g.u(b.1))], g.h(w2), true),
        Paint::Solid(c),
    );
}

/// A page with the top-right corner folded: the shared body of the document icons.
pub(super) fn page(s: &mut Surface, g: &Grid, x: i32, y: i32, w: i32, h: i32, fold: i32) {
    let (x2, y2) = (x + w, y + h);
    let r = 5;
    let mut outline = Path::new();
    outline.move_to(g.u(x + r), g.u(y));
    outline.line_to(g.u(x2 - fold), g.u(y));
    outline.line_to(g.u(x2), g.u(y + fold));
    outline.line_to(g.u(x2), g.u(y2 - r));
    outline.quad_to(g.u(x2), g.u(y2), g.u(x2 - r), g.u(y2));
    outline.line_to(g.u(x + r), g.u(y2));
    outline.quad_to(g.u(x), g.u(y2), g.u(x), g.u(y2 - r));
    outline.line_to(g.u(x), g.u(y + r));
    outline.quad_to(g.u(x), g.u(y), g.u(x + r), g.u(y));
    outline.close();
    // A hairline of shade around the page, then the paper on top.
    s.fill_path(&outline, Paint::Solid(rgba(0x000000, 70)));
    let mut inner = Path::new();
    let (ix, iy, ix2, iy2) = (x, y, x2, y2);
    let d = g.h(1).max(64); // inset: about half a design unit, at least a quarter pixel
    let f = fold;
    inner.move_to(g.u(ix + r) + d, g.u(iy) + d);
    inner.line_to(g.u(ix2 - f) - d / 2, g.u(iy) + d);
    inner.line_to(g.u(ix2) - d, g.u(iy + f) + d / 2);
    inner.line_to(g.u(ix2) - d, g.u(iy2 - r));
    inner.quad_to(g.u(ix2) - d, g.u(iy2) - d, g.u(ix2 - r), g.u(iy2) - d);
    inner.line_to(g.u(ix + r), g.u(iy2) - d);
    inner.quad_to(g.u(ix) + d, g.u(iy2) - d, g.u(ix) + d, g.u(iy2 - r));
    inner.line_to(g.u(ix) + d, g.u(iy + r));
    inner.quad_to(g.u(ix) + d, g.u(iy) + d, g.u(ix + r), g.u(iy) + d);
    inner.close();
    // The outline above already put the shade down; paint the paper through a fresh surface so
    // the hairline stays only at the edge.
    let mut paper = Surface::new(s.w, s.h);
    paper.fill_path(&inner, Paint::Vertical(rgb(0xFFFFFF), rgb(0xEEEEF3)));
    s.blit(&paper, 0, 0, 256);
    // The folded corner.
    let mut tri = Path::new();
    tri.polygon(&[
        (g.u(x2 - fold), g.u(y)),
        (g.u(x2), g.u(y + fold)),
        (g.u(x2 - fold), g.u(y + fold)),
    ]);
    s.fill_path(&tri, Paint::Solid(rgb(0xD9D9E2)));
}

/// Render the icon of `kind` at `px` x `px`. `accent` (`0xRRGGBB`) tints folders, apps and
/// the drive. `masks` provides the squircle corners of the app tile.
pub fn file_icon(kind: FileKind, px: usize, accent: u32, masks: &CornerMasks) -> Surface {
    let px = px.clamp(8, 256);
    let mut s = Surface::new(px, px);
    let g = Grid { px: px as i32 };
    let acc = 0xFF00_0000 | (accent & 0xFF_FFFF);
    match kind {
        FileKind::Folder => {
            let dark = mix(acc, 0xFF00_0000, 70);
            let light = mix(acc, 0xFFFF_FFFF, 90);
            // Back plate with its tab.
            rrect(&mut s, &g, 5, 10, 54, 44, 6, Paint::Solid(dark));
            rrect(&mut s, &g, 5, 8, 24, 12, 4, Paint::Solid(dark));
            // The paper peeking out.
            rrect(&mut s, &g, 10, 16, 44, 20, 3, Paint::Solid(rgb(0xF4F4F8)));
            // Front plate.
            rrect(&mut s, &g, 5, 20, 54, 36, 6, Paint::Vertical(light, acc));
            line(&mut s, &g, (10, 22), (54, 22), 3, rgba(0xFFFFFF, 90));
        }
        FileKind::Text => {
            page(&mut s, &g, 12, 4, 40, 56, 12);
            let ink = rgb(0x9A9AAE);
            for (i, w) in [24, 24, 24, 15].iter().enumerate() {
                let y = 26 + i as i32 * 8;
                line(&mut s, &g, (19, y), (19 + w, y), 5, ink);
            }
            line(&mut s, &g, (19, 17), (30, 17), 5, mix(acc, 0xFFFF_FFFF, 40));
        }
        FileKind::Image => {
            rrect(
                &mut s,
                &g,
                5,
                11,
                54,
                42,
                7,
                Paint::Solid(rgba(0x000000, 60)),
            );
            rrect(&mut s, &g, 6, 12, 52, 40, 6, Paint::Solid(rgb(0xFFFFFF)));
            // The picture: sky, sun, two hills.
            let mut pic = Surface::new(px, px);
            rrect(
                &mut pic,
                &g,
                10,
                16,
                44,
                32,
                3,
                Paint::Vertical(rgb(0x8FD3FF), rgb(0x4C8DF6)),
            );
            let mut sun = Path::new();
            sun.ellipse(g.u(42), g.u(25), g.u(5), g.u(5));
            pic.fill_path(&sun, Paint::Solid(rgb(0xFFE08A)));
            let mut hills = Path::new();
            hills.polygon(&[
                (g.u(10), g.u(48)),
                (g.u(10), g.u(40)),
                (g.u(22), g.u(28)),
                (g.u(34), g.u(42)),
                (g.u(40), g.u(36)),
                (g.u(54), g.u(46)),
                (g.u(54), g.u(48)),
            ]);
            pic.fill_path(&hills, Paint::Vertical(rgb(0x4FD08B), rgb(0x1C9D63)));
            // Keep the picture inside its rounded frame.
            let mut clip = Surface::new(px, px);
            rrect(
                &mut clip,
                &g,
                10,
                16,
                44,
                32,
                3,
                Paint::Solid(rgb(0xFFFFFF)),
            );
            for (d, c) in pic.px.iter_mut().zip(&clip.px) {
                *d = crate::ui::raster::scale_premul(*d, (*c >> 24) + (*c >> 31));
            }
            s.blit(&pic, 0, 0, 256);
        }
        FileKind::App => {
            let top = mix(acc, 0xFFFF_FFFF, 60);
            let bottom = mix(acc, 0xFF00_0000, 60);
            let side = (px * 52 / 64).max(4);
            let off = ((px - side) / 2) as i32;
            s.fill_rrect(
                off,
                off,
                side,
                side,
                side * 23 / 100,
                Corner::Squircle,
                Paint::Vertical(top, bottom),
                masks,
            );
            let chev = [(g.u(23), g.u(22)), (g.u(37), g.u(32)), (g.u(23), g.u(42))];
            s.fill_path(
                &stroke_path(&chev, g.h(10), true),
                Paint::Solid(rgb(0xFFFFFF)),
            );
            line(&mut s, &g, (40, 44), (49, 44), 8, rgba(0xFFFFFF, 220));
        }
        FileKind::Generic => {
            page(&mut s, &g, 12, 4, 40, 56, 12);
            // A neutral badge where a document would show text.
            rrect(&mut s, &g, 22, 28, 20, 20, 5, Paint::Solid(rgb(0xC9CAD8)));
            line(&mut s, &g, (27, 38), (37, 38), 4, rgb(0xFFFFFF));
        }
        FileKind::Drive => {
            rrect(
                &mut s,
                &g,
                4,
                18,
                56,
                30,
                8,
                Paint::Solid(rgba(0x000000, 70)),
            );
            rrect(
                &mut s,
                &g,
                5,
                18,
                54,
                28,
                7,
                Paint::Vertical(rgb(0xD7D8E2), rgb(0x9FA2B5)),
            );
            line(&mut s, &g, (11, 27), (53, 27), 3, rgba(0xFFFFFF, 130));
            let mut led = Path::new();
            led.ellipse(g.u(48), g.u(37), g.u(3), g.u(3));
            s.fill_path(&led, Paint::Solid(mix(acc, 0xFFFF_FFFF, 80)));
            line(&mut s, &g, (11, 37), (28, 37), 4, rgba(0x000000, 70));
        }
    }
    s
}
