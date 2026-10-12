//! The small monochrome tool glyphs of the toolbars.

use super::*;

/// Small monochrome glyphs for the toolbars of the apps.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Tool {
    ChevronLeft,
    ChevronRight,
    ChevronUp,
    ChevronDown,
    ViewList,
    ViewGrid,
    Sort,
    Eye,
    Home,
    Folder,
    Photo,
    Apps,
    Trash,
    Disk,
    Search,
    Cancel,
    Plus,
    Minus,
    RotateLeft,
    RotateRight,
    Fit,
    Actual,
    Info,
    Play,
    Pause,
    Save,
    FlipH,
    Replace,
    Doc,
    Check,
    ArrowUp,
    ArrowDown,
    Warning,
}

/// Every tool glyph, in a stable order.
pub const TOOLS: [Tool; 33] = [
    Tool::ChevronLeft,
    Tool::ChevronRight,
    Tool::ChevronUp,
    Tool::ChevronDown,
    Tool::ViewList,
    Tool::ViewGrid,
    Tool::Sort,
    Tool::Eye,
    Tool::Home,
    Tool::Folder,
    Tool::Photo,
    Tool::Apps,
    Tool::Trash,
    Tool::Disk,
    Tool::Search,
    Tool::Cancel,
    Tool::Plus,
    Tool::Minus,
    Tool::RotateLeft,
    Tool::RotateRight,
    Tool::Fit,
    Tool::Actual,
    Tool::Info,
    Tool::Play,
    Tool::Pause,
    Tool::Save,
    Tool::FlipH,
    Tool::Replace,
    Tool::Doc,
    Tool::Check,
    Tool::ArrowUp,
    Tool::ArrowDown,
    Tool::Warning,
];

/// Draw `t` at `px` x `px` in straight ARGB colour `c`. Design grid: 16 units.
pub fn tool_glyph(t: Tool, px: usize, c: u32) -> Surface {
    let px = px.clamp(8, 128);
    let mut s = Surface::new(px, px);
    let u = |v: i32| v * px as i32 * 256 / 16;
    // Half-unit helper for finer details.
    let hu = |v2: i32| v2 * px as i32 * 256 / 32;
    let pt = |x: i32, y: i32| (u(x), u(y));
    let width = |w2: i32| (w2 * px as i32 * 256 / 32).max(256 * 3 / 4);
    let stroke = |s: &mut Surface, pts: &[(i32, i32)], w2: i32| {
        let q8: Vec<(i32, i32)> = pts.iter().map(|&(x, y)| pt(x, y)).collect();
        s.fill_path(&stroke_path(&q8, width(w2), true), Paint::Solid(c));
    };
    let fill_rr = |s: &mut Surface, x: i32, y: i32, w: i32, h: i32, r: i32| {
        let mut p = Path::new();
        p.rrect(u(x), u(y), u(w), u(h), u(r).max(1));
        s.fill_path(&p, Paint::Solid(c));
    };
    let ring = |s: &mut Surface, cx: i32, cy: i32, r: i32, t2: i32| {
        let mut p = Path::new();
        p.ellipse(hu(cx), hu(cy), hu(r), hu(r));
        p.ellipse_hole(hu(cx), hu(cy), hu(r - t2).max(1), hu(r - t2).max(1));
        s.fill_path(&p, Paint::Solid(c));
    };
    match t {
        Tool::ChevronLeft => stroke(&mut s, &[(10, 3), (5, 8), (10, 13)], 4),
        Tool::ChevronRight => stroke(&mut s, &[(6, 3), (11, 8), (6, 13)], 4),
        Tool::ChevronUp => stroke(&mut s, &[(3, 10), (8, 5), (13, 10)], 4),
        Tool::ChevronDown => stroke(&mut s, &[(3, 6), (8, 11), (13, 6)], 4),
        Tool::ViewList => {
            for y in [4, 8, 12] {
                fill_rr(&mut s, 2, y - 1, 2, 2, 1);
                stroke(&mut s, &[(6, y), (14, y)], 3);
            }
        }
        Tool::ViewGrid => {
            for (x, y) in [(2, 2), (9, 2), (2, 9), (9, 9)] {
                fill_rr(&mut s, x, y, 5, 5, 1);
            }
        }
        Tool::Sort => {
            stroke(&mut s, &[(2, 4), (14, 4)], 3);
            stroke(&mut s, &[(2, 8), (11, 8)], 3);
            stroke(&mut s, &[(2, 12), (8, 12)], 3);
        }
        Tool::Eye => {
            let mut p = Path::new();
            p.ellipse(u(8), u(8), u(7), u(4) + hu(1));
            p.ellipse_hole(u(8), u(8), u(7) - hu(3), u(4) - hu(2));
            s.fill_path(&p, Paint::Solid(c));
            let mut d = Path::new();
            d.ellipse(u(8), u(8), hu(4), hu(4));
            s.fill_path(&d, Paint::Solid(c));
        }
        Tool::Home => {
            stroke(&mut s, &[(2, 8), (8, 2), (14, 8)], 3);
            let mut body = Path::new();
            body.polygon(&[
                (u(4), u(8)),
                (u(8), u(4) + hu(1)),
                (u(12), u(8)),
                (u(12), u(14)),
                (u(4), u(14)),
            ]);
            // The door is a hole in the same path.
            body.rrect_hole(u(7), u(10), u(2), u(4), 0);
            s.fill_path(&body, Paint::Solid(c));
        }
        Tool::Folder => {
            fill_rr(&mut s, 1, 3, 6, 4, 1);
            fill_rr(&mut s, 1, 5, 14, 9, 2);
        }
        Tool::Photo => {
            let mut p = Path::new();
            p.rrect(u(1), u(3), u(14), u(10), u(2));
            p.rrect_hole(
                u(1) + hu(3),
                u(3) + hu(3),
                u(14) - hu(6),
                u(10) - hu(6),
                u(1),
            );
            s.fill_path(&p, Paint::Solid(c));
            let mut hills = Path::new();
            hills.polygon(&[
                (u(3), u(11)),
                (u(6), u(7)),
                (u(8), u(9) + hu(1)),
                (u(10), u(7)),
                (u(13), u(11)),
            ]);
            s.fill_path(&hills, Paint::Solid(c));
            let mut sun = Path::new();
            sun.ellipse(u(11), u(6), hu(2), hu(2));
            s.fill_path(&sun, Paint::Solid(c));
        }
        Tool::Apps => {
            for (x, y) in [(2, 2), (9, 2), (2, 9), (9, 9)] {
                fill_rr(&mut s, x, y, 5, 5, 2);
            }
        }
        Tool::Trash => {
            fill_rr(&mut s, 2, 3, 12, 2, 1);
            fill_rr(&mut s, 6, 1, 4, 2, 1);
            let mut bin = Path::new();
            bin.polygon(&[(u(3), u(6)), (u(13), u(6)), (u(12), u(15)), (u(4), u(15))]);
            s.fill_path(&bin, Paint::Solid(c));
        }
        Tool::Disk => {
            let mut p = Path::new();
            p.rrect(u(1), u(4), u(14), u(8), u(2));
            p.ellipse_hole(u(12), u(8), hu(1) + 64, hu(1) + 64);
            p.rrect_hole(u(3), u(7), u(5), hu(3), 0);
            s.fill_path(&p, Paint::Solid(c));
        }
        Tool::Search => {
            ring(&mut s, 14, 14, 11, 4);
            stroke(&mut s, &[(11, 11), (14, 14)], 4);
        }
        Tool::Cancel => {
            // A filled disc with a cross cut out reads at 12 px.
            let mut d = Path::new();
            d.ellipse(u(8), u(8), u(7), u(7));
            s.fill_path(&d, Paint::Solid(c));
            let cross = |a: (i32, i32), b: (i32, i32)| {
                stroke_path(&[pt(a.0, a.1), pt(b.0, b.1)], width(3), true)
            };
            let mut cut = Surface::new(px, px);
            cut.fill_path(&cross((5, 5), (11, 11)), Paint::Solid(rgb(0xFFFFFF)));
            cut.fill_path(&cross((11, 5), (5, 11)), Paint::Solid(rgb(0xFFFFFF)));
            for (d, k) in s.px.iter_mut().zip(&cut.px) {
                let a = k >> 24;
                if a != 0 {
                    *d = crate::ui::raster::scale_premul(*d, 256 - (a + (a >> 7)));
                }
            }
        }
        Tool::Plus => {
            stroke(&mut s, &[(3, 8), (13, 8)], 4);
            stroke(&mut s, &[(8, 3), (8, 13)], 4);
        }
        Tool::Minus => stroke(&mut s, &[(3, 8), (13, 8)], 4),
        Tool::RotateLeft | Tool::RotateRight => {
            // Three quarters of a ring (clockwise from 30 to 270 degrees) with an arrow head
            // at the end; the left variant is the mirror image.
            let pts: Vec<(i32, i32)> = (0..=8)
                .map(|k| crate::ui::iconart::rot(u(8), u(8), 0, -u(5), 30 + k * 30))
                .collect();
            s.fill_path(&stroke_path(&pts, width(3), true), Paint::Solid(c));
            let e = *pts.last().unwrap_or(&(0, 0));
            let mut head = Path::new();
            head.polygon(&[
                (e.0 - u(3), e.1 + u(1)),
                (e.0 + u(3), e.1 + u(1)),
                (e.0, e.1 - u(3)),
            ]);
            s.fill_path(&head, Paint::Solid(c));
            if t == Tool::RotateLeft {
                for row in s.px.chunks_mut(px) {
                    row.reverse();
                }
            }
        }
        Tool::Fit => {
            for (a, b, d) in [
                ((2, 6), (2, 2), (6, 2)),
                ((10, 2), (14, 2), (14, 6)),
                ((14, 10), (14, 14), (10, 14)),
                ((6, 14), (2, 14), (2, 10)),
            ] {
                stroke(&mut s, &[a, b, d], 3);
            }
        }
        Tool::Actual => {
            // A "1" in a frame.
            let mut f = Path::new();
            f.rrect(u(2), u(2), u(12), u(12), u(3));
            f.rrect_hole(
                u(2) + hu(3),
                u(2) + hu(3),
                u(12) - hu(6),
                u(12) - hu(6),
                u(2),
            );
            s.fill_path(&f, Paint::Solid(c));
            stroke(&mut s, &[(6, 6), (8, 5), (8, 11)], 3);
        }
        Tool::Info => {
            ring(&mut s, 16, 16, 14, 3);
            stroke(&mut s, &[(8, 7), (8, 11)], 3);
            let mut dot = Path::new();
            dot.ellipse(u(8), u(5), hu(1) + 96, hu(1) + 96);
            s.fill_path(&dot, Paint::Solid(c));
        }
        Tool::Play => {
            let mut p = Path::new();
            p.polygon(&[(u(5), u(2)), (u(13), u(8)), (u(5), u(14))]);
            s.fill_path(&p, Paint::Solid(c));
        }
        Tool::Pause => {
            fill_rr(&mut s, 3, 2, 3, 12, 1);
            fill_rr(&mut s, 10, 2, 3, 12, 1);
        }
        Tool::Save => {
            stroke(&mut s, &[(8, 2), (8, 10)], 3);
            stroke(&mut s, &[(4, 7), (8, 11), (12, 7)], 3);
            stroke(&mut s, &[(2, 13), (14, 13)], 3);
        }
        Tool::FlipH => {
            stroke(&mut s, &[(8, 1), (8, 15)], 2);
            let mut l = Path::new();
            l.polygon(&[(u(6), u(3)), (u(1), u(13)), (u(6), u(13))]);
            s.fill_path(&l, Paint::Solid(c));
            let mut r = Path::new();
            r.polygon(&[(u(10), u(3)), (u(15), u(13)), (u(10), u(13))]);
            s.fill_path(
                &r,
                Paint::Solid(rgba(c & 0xFF_FFFF, ((c >> 24) * 90 / 255) as u8)),
            );
        }
        Tool::Replace => {
            stroke(&mut s, &[(2, 6), (13, 6)], 3);
            stroke(&mut s, &[(10, 3), (13, 6), (10, 9)], 3);
            stroke(&mut s, &[(14, 11), (3, 11)], 3);
            stroke(&mut s, &[(6, 8), (3, 11), (6, 14)], 3);
        }
        Tool::Doc => {
            let mut p = Path::new();
            p.rrect(u(3), u(1), u(10), u(14), u(2));
            p.rrect_hole(
                u(3) + hu(3),
                u(1) + hu(3),
                u(10) - hu(6),
                u(14) - hu(6),
                u(1),
            );
            s.fill_path(&p, Paint::Solid(c));
            stroke(&mut s, &[(6, 6), (10, 6)], 2);
            stroke(&mut s, &[(6, 9), (10, 9)], 2);
        }
        Tool::Check => stroke(&mut s, &[(3, 8), (6, 11), (13, 4)], 4),
        Tool::ArrowUp => {
            stroke(&mut s, &[(8, 13), (8, 3)], 3);
            stroke(&mut s, &[(4, 7), (8, 3), (12, 7)], 3);
        }
        Tool::ArrowDown => {
            stroke(&mut s, &[(8, 3), (8, 13)], 3);
            stroke(&mut s, &[(4, 9), (8, 13), (12, 9)], 3);
        }
        Tool::Warning => {
            let mut tri = Path::new();
            tri.polygon(&[(u(8), u(1) + hu(1)), (u(15), u(14)), (u(1), u(14))]);
            s.fill_path(&tri, Paint::Solid(c));
            let mut cut = Surface::new(px, px);
            cut.fill_path(
                &stroke_path(&[pt(8, 6), pt(8, 10)], width(3), true),
                Paint::Solid(rgb(0xFFFFFF)),
            );
            let mut dot = Path::new();
            dot.ellipse(u(8), u(12), hu(1) + 96, hu(1) + 96);
            cut.fill_path(&dot, Paint::Solid(rgb(0xFFFFFF)));
            for (d, k) in s.px.iter_mut().zip(&cut.px) {
                let a = k >> 24;
                if a != 0 {
                    *d = crate::ui::raster::scale_premul(*d, 256 - (a + (a >> 7)));
                }
            }
        }
    }
    s
}
