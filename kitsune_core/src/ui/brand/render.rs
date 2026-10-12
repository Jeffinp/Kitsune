//! The anti-aliased raster of a mark at any size, by supersampling the polygons.

use super::*;

/// Add the polygon to `out` keeping its winding (a knockout winds the other way and cuts a
/// hole; `Path::polygon` would straighten it out).
pub(super) fn path_of(pts: &[(i32, i32)], scale: i64, out: &mut Path) {
    // Coordinate units -> 24.8 pixels: x * scale * 256 / EXTENT.
    let conv = |v: i32| ((v as i64 * scale * 256 + (EXTENT as i64) / 2) / EXTENT as i64) as i32;
    for (i, &(x, y)) in pts.iter().enumerate() {
        if i == 0 {
            out.move_to(conv(x), conv(y));
        } else {
            out.line_to(conv(x), conv(y));
        }
    }
    out.close();
}

/// Fill the rounded-square tile path.
pub(super) fn tile_path(side: usize) -> Path {
    let mut p = Path::new();
    let s = (side as i32) * 256;
    let r = (TILE_RADIUS as i64 * side as i64 * 256 / EXTENT as i64) as i32;
    p.rrect(0, 0, s, s, r);
    p
}

/// The mark at `px` x `px`, anti-aliased (straight colours, premultiplied surface), with the
/// per-size [`hint`] applied.
pub fn render(mark: &Mark, px: usize) -> Surface {
    let px = px.clamp(8, 512);
    let h = hint(px);
    let m = hinted(mark, px);
    let ss = h.supersample;
    let side = px * ss;
    let mut big = Surface::new(side, side);
    if let Some(c) = m.tile {
        big.fill_path(
            &tile_path(side),
            Paint::Solid(argb(255, (c >> 16) as u8, (c >> 8) as u8, c as u8)),
        );
    }
    // A mono mark is one path (outline + holes) so the holes really cut.
    let mono_color = m
        .polys
        .iter()
        .find(|p| p.role == Role::Mark)
        .map(|p| p.color);
    if let Some(c) = mono_color {
        let mut path = Path::new();
        for p in m
            .polys
            .iter()
            .filter(|p| matches!(p.role, Role::Mark | Role::Knockout))
        {
            path_of(&p.pts, side as i64, &mut path);
        }
        big.fill_path(
            &path,
            Paint::Solid(argb(255, (c >> 16) as u8, (c >> 8) as u8, c as u8)),
        );
    } else {
        for p in &m.polys {
            let mut path = Path::new();
            path_of(&p.pts, side as i64, &mut path);
            big.fill_path(
                &path,
                Paint::Solid(argb(
                    255,
                    (p.color >> 16) as u8,
                    (p.color >> 8) as u8,
                    p.color as u8,
                )),
            );
        }
    }
    if ss == 1 { big } else { big.resized(px, px) }
}
