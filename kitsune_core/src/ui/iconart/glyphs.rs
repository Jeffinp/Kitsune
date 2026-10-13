//! glyphs (split out of `iconart.rs`).

use super::*;

/// Monochrome glyphs for the menu bar, menus and controls.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Glyph {
    Search,
    Network,
    NetworkOff,
    Control,
    Check,
    ChevronRight,
    ChevronDown,
    Close,
    Plus,
    Minus,
    Brand,
    Sun,
    Moon,
    Info,
    ChevronUp,
    Trash,
    Save,
    Copy,
    Keyboard,
    Clock,
    Disk,
    Power,
    Image,
    Dock,
    Bell,
    Wave,
    ChevronLeft,
    Reload,
    Lock,
    Warning,
    Star,
    StarFill,
    Globe,
    Home,
    /// A head and shoulders: accounts.
    User,
    /// A shield with a check: the ad blocker is on.
    Shield,
    /// A shield crossed out: the ad blocker is off.
    ShieldOff,
}

/// Draw `g` at `px` x `px` in straight ARGB colour `c`.
pub fn glyph(g: Glyph, px: usize, c: u32) -> Surface {
    let mut s = Surface::new(px, px);
    // Work in a 16 x 16 design grid scaled to the size asked.
    let u = |v: i32| (v * px as i32 * 256) / 16;
    let w = |v: i32| (v * px as i32 * 256) / 16; // stroke widths scale the same way
    let pt = |x: i32, y: i32| (u(x), u(y));
    let stroke = |s: &mut Surface, pts: &[(i32, i32)], width: i32| {
        let q8: Vec<(i32, i32)> = pts.iter().map(|&(x, y)| pt(x, y)).collect();
        s.fill_path(
            &stroke_path(&q8, w(width).max(256 * 3 / 4), true),
            Paint::Solid(c),
        );
    };
    match g {
        Glyph::Search => {
            let mut ring = Path::new();
            ring.ellipse(u(7), u(7), u(4) + u(1) / 4, u(4) + u(1) / 4);
            ring.ellipse_hole(u(7), u(7), u(3) - u(1) / 4, u(3) - u(1) / 4);
            s.fill_path(&ring, Paint::Solid(c));
            stroke(&mut s, &[(10, 10), (14, 14)], 2);
        }
        Glyph::Network | Glyph::NetworkOff => {
            // Three arcs and a dot (wi-fi), or a dashed variant.
            let col = if g == Glyph::NetworkOff {
                rgba(c & 0xFFFFFF, ((c >> 24) * 90 / 255) as u8)
            } else {
                c
            };
            let arc = |s: &mut Surface, r: i32| {
                let pts = quad_points(
                    pt(8 - r, 8 - r / 3 + 1),
                    pt(8, 8 - r * 2 + 1),
                    pt(8 + r, 8 - r / 3 + 1),
                    10,
                );
                s.fill_path(&stroke_path(&pts, w(1) + 64, true), Paint::Solid(col));
            };
            arc(&mut s, 6);
            arc(&mut s, 4);
            arc(&mut s, 2);
            let mut dot = Path::new();
            dot.ellipse(u(8), u(12), u(1) + 32, u(1) + 32);
            s.fill_path(&dot, Paint::Solid(col));
        }
        Glyph::Control => {
            // Two toggles: pill outlines with knobs.
            for (y, kx) in [(5, 11), (11, 5)] {
                let mut p = Path::new();
                p.ellipse(u(8), u(y), u(6) + 32, u(2) + 64);
                p.ellipse_hole(u(8), u(y), u(6) - 80, u(2) - 80);
                s.fill_path(&p, Paint::Solid(c));
                let mut k = Path::new();
                k.ellipse(u(kx), u(y), u(1) + 128, u(1) + 128);
                s.fill_path(&k, Paint::Solid(c));
            }
        }
        Glyph::Check => stroke(&mut s, &[(3, 8), (6, 11), (13, 4)], 2),
        Glyph::ChevronRight => stroke(&mut s, &[(6, 3), (11, 8), (6, 13)], 2),
        Glyph::ChevronDown => stroke(&mut s, &[(3, 6), (8, 11), (13, 6)], 2),
        Glyph::ChevronUp => stroke(&mut s, &[(3, 10), (8, 5), (13, 10)], 2),
        Glyph::Trash => {
            stroke(&mut s, &[(2, 4), (14, 4)], 2);
            stroke(&mut s, &[(6, 4), (6, 2), (10, 2), (10, 4)], 1);
            stroke(&mut s, &[(4, 6), (5, 14), (11, 14), (12, 6)], 2);
        }
        Glyph::Save => {
            stroke(&mut s, &[(8, 2), (8, 10)], 2);
            stroke(&mut s, &[(4, 7), (8, 11), (12, 7)], 2);
            stroke(&mut s, &[(3, 11), (3, 14), (13, 14), (13, 11)], 2);
        }
        Glyph::Keyboard => {
            stroke(&mut s, &[(2, 4), (14, 4), (14, 12), (2, 12), (2, 4)], 1);
            for x in [5, 8, 11] {
                let mut d = Path::new();
                d.ellipse(u(x), u(7), u(1) - 32, u(1) - 32);
                s.fill_path(&d, Paint::Solid(c));
            }
            stroke(&mut s, &[(5, 10), (11, 10)], 1);
        }
        Glyph::Clock => {
            let mut p = Path::new();
            p.ellipse(u(8), u(8), u(7), u(7));
            p.ellipse_hole(u(8), u(8), u(7) - 96, u(7) - 96);
            s.fill_path(&p, Paint::Solid(c));
            stroke(&mut s, &[(8, 4), (8, 8), (11, 10)], 1);
        }
        Glyph::Disk => {
            stroke(&mut s, &[(2, 5), (14, 5), (14, 11), (2, 11), (2, 5)], 1);
            stroke(&mut s, &[(4, 8), (7, 8)], 1);
            let mut d = Path::new();
            d.ellipse(u(11), u(8), u(1) - 32, u(1) - 32);
            s.fill_path(&d, Paint::Solid(c));
        }
        Glyph::Power => {
            // An open ring (the gap at the top) and the bar through it.
            let pts: Vec<(i32, i32)> = (0..=9)
                .map(|k| rot(u(8), u(9), 0, -u(5), 40 + k * 31))
                .collect();
            s.fill_path(&stroke_path(&pts, w(1) + 96, true), Paint::Solid(c));
            stroke(&mut s, &[(8, 2), (8, 8)], 2);
        }
        Glyph::Image => {
            stroke(&mut s, &[(2, 3), (14, 3), (14, 13), (2, 13), (2, 3)], 1);
            stroke(&mut s, &[(2, 12), (6, 8), (9, 11), (11, 9), (14, 12)], 1);
            let mut d = Path::new();
            d.ellipse(u(11), u(6), u(1) - 32, u(1) - 32);
            s.fill_path(&d, Paint::Solid(c));
        }
        Glyph::Dock => {
            stroke(&mut s, &[(1, 9), (15, 9), (15, 14), (1, 14), (1, 9)], 1);
            for x in [4, 8, 12] {
                let mut d = Path::new();
                d.ellipse(u(x), u(11) + 128, u(1) - 48, u(1) - 48);
                s.fill_path(&d, Paint::Solid(c));
            }
            stroke(&mut s, &[(4, 4), (12, 4)], 1);
        }
        Glyph::Copy => {
            stroke(&mut s, &[(3, 6), (10, 6), (10, 13), (3, 13), (3, 6)], 2);
            stroke(&mut s, &[(6, 3), (13, 3), (13, 10), (11, 10)], 2);
        }
        Glyph::Close => {
            stroke(&mut s, &[(4, 4), (12, 12)], 2);
            stroke(&mut s, &[(12, 4), (4, 12)], 2);
        }
        Glyph::Plus => {
            stroke(&mut s, &[(3, 8), (13, 8)], 2);
            stroke(&mut s, &[(8, 3), (8, 13)], 2);
        }
        Glyph::Minus => stroke(&mut s, &[(3, 8), (13, 8)], 2),
        Glyph::Brand => {
            // The mono fox head, tuned for this size ([`crate::ui::brand`]), in the colour asked
            // (its alpha included).
            s = crate::ui::brand::render(&crate::ui::brand::mono(c & 0x00FF_FFFF), px);
            s.fade(((c >> 24) * 256 + 127) / 255);
        }
        Glyph::Sun => {
            let mut d = Path::new();
            d.ellipse(u(8), u(8), u(3) + 64, u(3) + 64);
            s.fill_path(&d, Paint::Solid(c));
            for k in 0..8 {
                let a = rot(u(8), u(8), 0, -u(5) - 64, k * 45);
                let b = rot(u(8), u(8), 0, -u(7) - 32, k * 45);
                s.fill_path(&stroke_path(&[a, b], w(1) + 64, true), Paint::Solid(c));
            }
        }
        Glyph::Moon => {
            let mut p = Path::new();
            p.ellipse(u(8), u(8), u(5) + 128, u(5) + 128);
            p.ellipse_hole(u(11), u(6), u(4) + 128, u(4) + 128);
            s.fill_path(&p, Paint::Solid(c));
        }
        Glyph::Bell => {
            let mut pts = quad_points(pt(4, 11), pt(4, 3), pt(8, 3), 8);
            pts.extend(quad_points(pt(8, 3), pt(12, 3), pt(12, 11), 8));
            pts.push(pt(14, 12));
            pts.push(pt(2, 12));
            pts.push(pt(4, 11));
            s.fill_path(&stroke_path(&pts, w(1) + 96, true), Paint::Solid(c));
            let mut clap = Path::new();
            clap.ellipse(u(8), u(14) + 64, u(1) + 64, u(1));
            s.fill_path(&clap, Paint::Solid(c));
        }
        Glyph::Wave => {
            let mut pts = quad_points(pt(1, 8), pt(4, 2), pt(8, 8), 8);
            pts.extend(quad_points(pt(8, 8), pt(12, 14), pt(15, 8), 8));
            s.fill_path(&stroke_path(&pts, w(2) - 32, true), Paint::Solid(c));
        }
        Glyph::Info => {
            let mut p = Path::new();
            p.ellipse(u(8), u(8), u(7), u(7));
            p.ellipse_hole(u(8), u(8), u(7) - 80, u(7) - 80);
            s.fill_path(&p, Paint::Solid(c));
            stroke(&mut s, &[(8, 7), (8, 11)], 2);
            let mut dot = Path::new();
            dot.ellipse(u(8), u(5), u(1) - 32, u(1) - 32);
            s.fill_path(&dot, Paint::Solid(c));
        }
        Glyph::User => {
            let mut head = Path::new();
            head.ellipse(u(8), u(5), u(3) + 32, u(3) + 32);
            s.fill_path(&head, Paint::Solid(c));
            let body = quad_points(pt(2, 15), pt(8, 5), pt(14, 15), 10);
            s.fill_path(&stroke_path(&body, w(2) + 32, true), Paint::Solid(c));
        }
        Glyph::ChevronLeft => stroke(&mut s, &[(10, 3), (5, 8), (10, 13)], 2),
        Glyph::Reload => {
            // An open ring (from 40 to 330 degrees, clockwise from the top) with an arrow
            // head at its end.
            let centre = (u(8), u(8));
            let radius = u(5) + 32;
            let at = |deg: i32| rot(centre.0, centre.1, 0, -radius, deg);
            let pts: Vec<(i32, i32)> = (0..=15).map(|k| at(40 + k * 20)).collect();
            s.fill_path(&stroke_path(&pts, w(2) - 16, true), Paint::Solid(c));
            // The tip leads along the direction of travel at the end of the arc.
            let end = at(330);
            let t = rot(0, 0, 0, -256, 330 + 90);
            let n = rot(0, 0, 0, -256, 330);
            let k = u(3) + 64;
            let tip = (end.0 + t.0 * k / 256 * 3 / 2, end.1 + t.1 * k / 256 * 3 / 2);
            let a = (end.0 + n.0 * k / 256, end.1 + n.1 * k / 256);
            let b = (end.0 - n.0 * k / 256, end.1 - n.1 * k / 256);
            let mut head = Path::new();
            head.polygon(&[tip, a, b]);
            s.fill_path(&head, Paint::Solid(c));
        }
        Glyph::Lock => {
            let mut body = Path::new();
            body.rrect(u(3), u(7), u(10), u(7) + 128, u(2));
            s.fill_path(&body, Paint::Solid(c));
            let sh = [
                pt(5, 7),
                pt(5, 5),
                (u(6) - 40, u(3) + 80),
                pt(8, 2),
                (u(10) + 40, u(3) + 80),
                pt(11, 5),
                pt(11, 7),
            ];
            s.fill_path(&stroke_path(&sh, w(2) - 32, true), Paint::Solid(c));
        }
        Glyph::Warning => {
            let tri = [pt(8, 2), pt(14, 13), pt(2, 13), pt(8, 2)];
            s.fill_path(&stroke_path(&tri, w(2) - 24, true), Paint::Solid(c));
            stroke(&mut s, &[(8, 6), (8, 9)], 2);
            let mut dot = Path::new();
            dot.ellipse(u(8), u(11) + 40, u(1) - 24, u(1) - 24);
            s.fill_path(&dot, Paint::Solid(c));
        }
        Glyph::Star | Glyph::StarFill => {
            let centre = (u(8), u(8) + 40);
            let outer = u(6) + 128;
            let inner = u(3);
            let pts: Vec<(i32, i32)> = (0..10)
                .map(|k| {
                    let r = if k % 2 == 0 { outer } else { inner };
                    rot(centre.0, centre.1, 0, -r, k * 36)
                })
                .collect();
            if g == Glyph::StarFill {
                let mut p = Path::new();
                p.polygon(&pts);
                s.fill_path(&p, Paint::Solid(c));
            } else {
                let mut closed = pts.clone();
                closed.push(pts[0]);
                s.fill_path(&stroke_path(&closed, w(1) + 96, true), Paint::Solid(c));
            }
        }
        Glyph::Globe => {
            let mut ring = Path::new();
            ring.ellipse(u(8), u(8), u(7), u(7));
            ring.ellipse_hole(u(8), u(8), u(7) - 72, u(7) - 72);
            s.fill_path(&ring, Paint::Solid(c));
            let mut mer = Path::new();
            mer.ellipse(u(8), u(8), u(3) + 40, u(7) - 20);
            mer.ellipse_hole(u(8), u(8), u(3) - 32, u(7) - 90);
            s.fill_path(&mer, Paint::Solid(c));
            stroke(&mut s, &[(1, 8), (15, 8)], 1);
        }
        Glyph::Shield | Glyph::ShieldOff => {
            let outline = [
                pt(8, 2),
                pt(13, 4),
                pt(13, 8),
                pt(11, 11),
                pt(8, 14),
                pt(5, 11),
                pt(3, 8),
                pt(3, 4),
                pt(8, 2),
            ];
            s.fill_path(&stroke_path(&outline, w(2) - 32, true), Paint::Solid(c));
            if g == Glyph::Shield {
                stroke(&mut s, &[(6, 8), (7, 9), (10, 6)], 1);
            } else {
                stroke(&mut s, &[(4, 3), (12, 13)], 2);
            }
        }
        Glyph::Home => {
            stroke(&mut s, &[(2, 8), (8, 2), (14, 8)], 2);
            let body = [pt(4, 7), pt(4, 14), pt(12, 14), pt(12, 7)];
            s.fill_path(&stroke_path(&body, w(2) - 32, true), Paint::Solid(c));
        }
    }
    s
}
