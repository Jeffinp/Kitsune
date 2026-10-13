use super::*;

fn masks() -> CornerMasks {
    CornerMasks::with_max_radius(48)
}

#[test]
fn sine_table_is_accurate_and_wraps() {
    assert_eq!(sin_q14(0), 0);
    assert_eq!(sin_q14(90), 16384);
    assert_eq!(sin_q14(30), 8192);
    assert_eq!(sin_q14(180), 0);
    assert_eq!(sin_q14(270), -16384);
    assert_eq!(sin_q14(-90), -16384);
    assert_eq!(sin_q14(450), 16384);
    assert_eq!(cos_q14(0), 16384);
    assert_eq!(cos_q14(60), 8192);
    for d in 0..360 {
        let (s, c) = (sin_q14(d) as i64, cos_q14(d) as i64);
        let n = s * s + c * c;
        assert!((n - 16384 * 16384).abs() < 16384 * 8, "deg {d}");
    }
}

#[test]
fn every_glyph_draws_ink_inside_its_box_at_every_size() {
    let all = [
        Glyph::Search,
        Glyph::Network,
        Glyph::NetworkOff,
        Glyph::Control,
        Glyph::Check,
        Glyph::ChevronRight,
        Glyph::ChevronDown,
        Glyph::Close,
        Glyph::Plus,
        Glyph::Minus,
        Glyph::Brand,
        Glyph::Sun,
        Glyph::Moon,
        Glyph::Info,
        Glyph::Power,
        Glyph::Bell,
        Glyph::Wave,
        Glyph::Clock,
        Glyph::User,
    ];
    for g in all {
        for px in [12usize, 16, 20, 24] {
            let s = glyph(g, px, 0xFFFF_FFFF);
            assert_eq!((s.w, s.h), (px, px));
            let ink = s.px.iter().filter(|&&p| p >> 24 > 24).count();
            assert!(ink >= px / 2, "{g:?} at {px}: {ink} inked pixels");
            // Not a filled box: the glyph leaves room around itself.
            assert!(ink < px * px * 3 / 4, "{g:?} at {px}: {ink}");
        }
    }
}

#[test]
fn rotation_moves_points_on_a_circle() {
    assert_eq!(rot(0, 0, 0, -256, 90), (256, 0));
    let (x, y) = rot(100, 100, 0, -2560, 45);
    assert!(
        (x - 100 - 1810).abs() < 4 && (y - 100 + 1810).abs() < 4,
        "{x} {y}"
    );
}

#[test]
fn every_icon_is_a_squircle_with_content() {
    let m = masks();
    // The halo is the one mark without a tile: transparent background, no squircle.
    for id in ALL.into_iter().filter(|&i| i != IconId::Halo) {
        let s = render(id, &m);
        assert_eq!((s.w, s.h), (SRC, SRC));
        // Transparent outside the rounded corner, opaque in the middle area.
        assert!(s.get(0, 0) >> 24 < 30, "{id:?} corner");
        assert!(s.get(127, 127) >> 24 < 30, "{id:?} corner br");
        let opaque = s.px.iter().filter(|&&p| p >> 24 == 255).count();
        assert!(opaque > SRC * SRC * 7 / 10, "{id:?} opaque {opaque}");
        // Not a flat colour: it has a glyph.
        let first = s.get(64, 20);
        let distinct = s.px.iter().filter(|&&p| p != first).count();
        assert!(distinct > 2000, "{id:?}");
    }
}

#[test]
fn icons_are_deterministic_and_distinct() {
    let m = masks();
    let a = render(IconId::Files, &m);
    let b = render(IconId::Files, &m);
    assert_eq!(a.px, b.px);
    let mut seen: Vec<Vec<u32>> = Vec::new();
    for id in ALL {
        let s = render(id, &m).resized(16, 16);
        assert!(!seen.contains(&s.px), "{id:?} duplicates another icon");
        seen.push(s.px);
    }
}

#[test]
fn downscaled_icons_keep_their_shape() {
    let m = masks();
    for id in ALL {
        for side in [24usize, 48, 72] {
            let s = render(id, &m).resized(side, side);
            assert!(s.get(0, 0) >> 24 < 40);
            assert!(s.get(side / 2, side / 2) >> 24 > 200);
        }
    }
}

#[test]
fn app_icons_wrap_on_a_tile() {
    let m = masks();
    let mut px = alloc::vec![0u8; 24 * 24 * 4];
    for i in 0..24 * 24 {
        px[i * 4..i * 4 + 4].copy_from_slice(&[255, 0, 0, 255]);
    }
    let s = wrap_app_icon(&px, 24, 24, &m);
    // Red in the middle, light tile around it, rounded corner.
    let c = s.get(64, 64);
    assert!(((c >> 16) & 0xFF) > 200 && (c & 0xFF) < 60);
    let edge = s.get(12, 64);
    assert!((edge & 0xFF) > 200);
    assert!(s.get(0, 0) >> 24 < 30);
    // Garbage in: the generic icon out.
    assert_eq!(
        wrap_app_icon(&[1, 2, 3], 24, 24, &m).px,
        render(IconId::Apps, &m).px
    );
}

#[test]
fn glyphs_draw_ink_in_the_colour_asked() {
    for g in [
        Glyph::Search,
        Glyph::Network,
        Glyph::NetworkOff,
        Glyph::Control,
        Glyph::Check,
        Glyph::ChevronRight,
        Glyph::ChevronDown,
        Glyph::Close,
        Glyph::Plus,
        Glyph::Minus,
        Glyph::Brand,
        Glyph::Sun,
        Glyph::Moon,
        Glyph::Info,
        Glyph::ChevronUp,
        Glyph::Trash,
        Glyph::Save,
        Glyph::Copy,
        Glyph::Keyboard,
        Glyph::Clock,
        Glyph::Disk,
        Glyph::Power,
        Glyph::Image,
        Glyph::Dock,
        Glyph::ChevronLeft,
        Glyph::Reload,
        Glyph::Lock,
        Glyph::Warning,
        Glyph::Star,
        Glyph::StarFill,
        Glyph::Globe,
        Glyph::Home,
        Glyph::Shield,
        Glyph::ShieldOff,
    ] {
        for px in [12usize, 16, 20] {
            let s = glyph(g, px, rgb(0xFF0000));
            let ink: u32 = s.px.iter().map(|&p| p >> 24).sum();
            assert!(ink > 255 * (px as u32) / 2, "{g:?} {px}px has no ink");
            // Only red ink (premultiplied: g and b stay zero).
            assert!(s.px.iter().all(|&p| p & 0xFFFF == 0), "{g:?}");
            // It stays inside its box with a margin to spare on all sides.
            assert!(
                s.px[..px].iter().all(|&p| p >> 24 < 200)
                    || matches!(g, Glyph::Info | Glyph::Globe),
                "{g:?} touches the top edge"
            );
        }
    }
}

#[test]
#[ignore]
fn dump_icons() {
    // Writes a contact sheet (PPM) of all icons and glyphs: ICON_SHEET=/tmp/x.ppm
    let Ok(path) = std::env::var("ICON_SHEET") else {
        return;
    };
    let m = masks();
    let sizes = [128usize, 64, 32];
    let (w, h) = (13 * 136 + 8, 128 + 8 + 64 + 8 + 32 + 8 + 48);
    let mut img = std::vec![0xE8u8; w * h * 3];
    let mut put = |s: &Surface, x0: usize, y0: usize, dark: bool| {
        for y in 0..s.h {
            for x in 0..s.w {
                let p = s.px[y * s.w + x];
                let a = p >> 24;
                let o = ((y0 + y) * w + x0 + x) * 3;
                if o + 2 >= img.len() {
                    continue;
                }
                let bg = if dark { 0x20u32 } else { 0xE8 };
                for (k, sh) in [16, 8, 0].iter().enumerate() {
                    let c = (p >> sh) & 0xFF;
                    img[o + k] = (c + bg * (255 - a) / 255).min(255) as u8;
                }
            }
        }
    };
    for (i, id) in ALL.iter().enumerate() {
        let src = render(*id, &m);
        let mut y = 4;
        for &sz in &sizes {
            let s = if sz == SRC {
                src.clone()
            } else {
                src.resized(sz, sz)
            };
            put(&s, 4 + i * 136, y, false);
            y += sz + 8;
        }
    }
    let gl = [
        Glyph::Search,
        Glyph::Network,
        Glyph::NetworkOff,
        Glyph::Control,
        Glyph::Check,
        Glyph::ChevronRight,
        Glyph::ChevronDown,
        Glyph::Close,
        Glyph::Plus,
        Glyph::Minus,
        Glyph::Brand,
        Glyph::Sun,
        Glyph::Moon,
        Glyph::Info,
        Glyph::ChevronUp,
        Glyph::Trash,
        Glyph::Save,
        Glyph::Copy,
        Glyph::Keyboard,
        Glyph::Clock,
        Glyph::Disk,
        Glyph::Power,
        Glyph::Image,
        Glyph::Dock,
        Glyph::ChevronLeft,
        Glyph::Reload,
        Glyph::Lock,
        Glyph::Warning,
        Glyph::Star,
        Glyph::StarFill,
        Glyph::Globe,
        Glyph::Home,
        Glyph::Shield,
        Glyph::ShieldOff,
    ];
    for (i, g) in gl.iter().enumerate() {
        put(
            &glyph(*g, 16, rgb(0x1D1D1F)),
            4 + i * 40,
            128 + 8 + 64 + 8 + 32 + 16,
            false,
        );
        put(
            &glyph(*g, 32, rgb(0x1D1D1F)),
            4 + i * 40,
            128 + 8 + 64 + 8 + 32 + 40,
            false,
        );
    }
    let mut out = std::format!("P6\n{w} {h}\n255\n").into_bytes();
    out.extend_from_slice(&img);
    std::fs::write(path, out).unwrap();
}
