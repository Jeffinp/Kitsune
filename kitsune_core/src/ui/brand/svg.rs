//! The mark as hand-readable SVG.

use super::*;

pub(super) fn coord(out: &mut String, v: i32) {
    use core::fmt::Write;
    let neg = v < 0;
    let a = v.abs();
    let (i, f) = (a / UNIT, a % UNIT * 100 / UNIT);
    let _ = match (neg, f) {
        (false, 0) => write!(out, "{i}"),
        (true, 0) => write!(out, "-{i}"),
        (false, _) if f % 10 == 0 => write!(out, "{}.{}", i, f / 10),
        (true, _) if f % 10 == 0 => write!(out, "-{}.{}", i, f / 10),
        (false, _) => write!(out, "{i}.{f:02}"),
        (true, _) => write!(out, "-{i}.{f:02}"),
    };
}

pub(super) fn points_attr(pts: &[(i32, i32)]) -> String {
    let mut s = String::new();
    for (i, &(x, y)) in pts.iter().enumerate() {
        if i > 0 {
            s.push(' ');
        }
        coord(&mut s, x);
        s.push(',');
        coord(&mut s, y);
    }
    s
}

pub(super) fn hex(c: u32) -> String {
    use core::fmt::Write;
    let mut s = String::new();
    let _ = write!(s, "#{:06X}", c & 0xFF_FFFF);
    s
}

pub(super) fn role_name(r: Role) -> &'static str {
    match r {
        Role::Tile => "tile",
        Role::LightFace => "light face",
        Role::ShadeFace => "shade face",
        Role::EarOuter => "ear outer",
        Role::EarInner => "ear inner",
        Role::Muzzle => "muzzle",
        Role::Eye => "eye",
        Role::Nose => "nose",
        Role::Mark => "mark",
        Role::Knockout => "knockout",
        Role::Tail => "tail",
        Role::TailDark => "tail (dark)",
        Role::TailTip => "tail tip",
    }
}

/// The mark as an SVG document on the 128 x 128 design grid (one element per polygon, in
/// paint order, each with the role as a comment; the mono mark is a single even-odd path).
pub fn to_svg(mark: &Mark, title: &str) -> String {
    use core::fmt::Write;
    let mut s = String::new();
    let _ = writeln!(
        s,
        "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 {GRID} {GRID}\" width=\"{GRID}\" height=\"{GRID}\" role=\"img\" aria-label=\"{title}\">"
    );
    let _ = writeln!(s, "  <title>{title}</title>");
    if let Some(c) = mark.tile {
        let _ = writeln!(
            s,
            "  <rect width=\"{GRID}\" height=\"{GRID}\" rx=\"{}\" fill=\"{}\"/> <!-- tile -->",
            TILE_RADIUS / UNIT,
            hex(c)
        );
    }
    let is_mono = mark.polys.iter().any(|p| p.role == Role::Mark);
    if is_mono {
        let color = mark.polys[0].color;
        let mut d = String::new();
        for p in &mark.polys {
            d.push_str(if d.is_empty() { "M" } else { " M" });
            d.push_str(&points_attr(&p.pts).replace(' ', " L"));
            d.push_str(" Z");
        }
        let _ = writeln!(
            s,
            "  <path fill=\"{}\" fill-rule=\"evenodd\" d=\"{d}\"/> <!-- silhouette; eyes and nose are holes -->",
            hex(color)
        );
    } else {
        for p in &mark.polys {
            let _ = writeln!(
                s,
                "  <polygon fill=\"{}\" points=\"{}\"/> <!-- {} -->",
                hex(p.color),
                points_attr(&p.pts),
                role_name(p.role)
            );
        }
    }
    s.push_str("</svg>\n");
    s
}
