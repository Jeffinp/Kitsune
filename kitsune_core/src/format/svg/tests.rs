use super::*;

fn px(img: &Image, x: usize, y: usize) -> u32 {
    img.get(x, y).unwrap()
}

#[test]
fn recognises_svg_documents() {
    assert!(looks_like(
        b"<svg xmlns='http://www.w3.org/2000/svg' width='10' height='10'/>"
    ));
    assert!(looks_like(
        b"<?xml version=\"1.0\"?>\n<!-- c --><svg></svg>"
    ));
    assert!(looks_like(b"\xef\xbb\xbf  <SVG></SVG>"));
    assert!(!looks_like(b"<html><body>hi</body></html>"));
    assert!(!looks_like(b"\x89PNG\r\n\x1a\n"));
    assert!(!looks_like(b""));
}

#[test]
fn sizes_come_from_width_height_or_view_box() {
    assert_eq!(dims(b"<svg width='40' height='20'/>"), Some((40, 20)));
    assert_eq!(dims(b"<svg viewBox='0 0 100 50'/>"), Some((100, 50)));
    assert_eq!(
        dims(b"<svg width='200' viewBox='0 0 100 50'/>"),
        Some((200, 100))
    );
    assert_eq!(
        dims(b"<svg height='10pt' viewBox='0 0 2 1'/>"),
        Some((27, 13))
    );
    assert_eq!(dims(b"<svg/>"), Some((300, 150)));
    assert_eq!(dims(b"<svg width='0' height='5'/>"), None);
    assert_eq!(dims(b"<svg width='99999' height='5'/>"), None);
    assert_eq!(dims(b"<p/>"), None);
}

#[test]
fn a_filled_rect_is_solid_inside_and_clear_outside() {
    let img = render(b"<svg width='20' height='20'><rect x='5' y='5' width='10' height='10' fill='#ff0000'/></svg>", 100).unwrap();
    assert_eq!((img.width(), img.height()), (20, 20));
    assert_eq!(px(&img, 10, 10), 0xffff0000);
    assert_eq!(px(&img, 2, 2) >> 24, 0);
    assert_eq!(px(&img, 17, 17) >> 24, 0);
}

#[test]
fn edges_are_antialiased() {
    // A rect that starts half a pixel in covers that column half.
    let img = render(b"<svg width='10' height='10'><rect x='2.5' y='0' width='5' height='10' fill='#000'/></svg>", 100).unwrap();
    let a = px(&img, 2, 5) >> 24;
    assert!((100..=160).contains(&a), "half coverage, got {a}");
    assert_eq!(px(&img, 4, 5) >> 24, 255);
}

#[test]
fn view_box_scales_and_the_output_never_exceeds_the_width() {
    let img = render(b"<svg viewBox='0 0 10 10' width='100' height='100'><rect width='10' height='10' fill='#00ff00'/></svg>", 40).unwrap();
    assert_eq!((img.width(), img.height()), (40, 40));
    assert_eq!(px(&img, 39, 39), 0xff00ff00);
}

#[test]
fn circles_ellipses_polygons_and_paths() {
    let img = render(
        b"<svg width='40' height='20'><circle cx='10' cy='10' r='8' fill='blue'/>\
          <polygon points='22,2 38,2 30,18' fill='#00f000'/>\
          <path d='M2 2 L4 2 L4 4 Z' fill='red'/></svg>",
        100,
    )
    .unwrap();
    assert_eq!(px(&img, 10, 10), 0xff0000ff);
    assert_eq!(px(&img, 1, 18) >> 24, 0, "outside the circle");
    assert_eq!(px(&img, 30, 5), 0xff00f000);
    assert_eq!(px(&img, 22, 15) >> 24, 0, "outside the triangle");
}

#[test]
fn path_commands_relative_curves_and_implicit_linetos() {
    // A relative square written with implicit linetos and h/v.
    let img = render(
        b"<svg width='10' height='10'><path d='m1 1 8 0 0 8 -8 0z' fill='#112233'/></svg>",
        50,
    )
    .unwrap();
    assert_eq!(px(&img, 5, 5), 0xff112233);
    // A closed curve (two cubic arcs) covers its middle.
    let img = render(
        b"<svg width='20' height='20'><path d='M2 10 C2 2 18 2 18 10 C18 18 2 18 2 10 Z' fill='#445566'/></svg>",
        50,
    )
    .unwrap();
    assert_eq!(px(&img, 10, 10), 0xff445566);
    assert_eq!(px(&img, 0, 0) >> 24, 0);
    // Smooth and quadratic commands run.
    let img = render(b"<svg width='20' height='20'><path d='M2 18 Q10 0 18 18 T18 2 S4 4 2 18' fill='none' stroke='#000' stroke-width='2'/></svg>", 50).unwrap();
    assert!(img.pixels().iter().any(|p| p >> 24 != 0));
}

#[test]
fn strokes_have_width_and_fill_none_leaves_the_inside() {
    let img = render(b"<svg width='20' height='20'><rect x='4' y='4' width='12' height='12' fill='none' stroke='#ff0000' stroke-width='2'/></svg>", 50).unwrap();
    assert_eq!(px(&img, 4, 10) >> 24, 255, "on the stroke");
    assert_eq!(px(&img, 10, 10) >> 24, 0, "inside, not filled");
    let img = render(b"<svg width='20' height='20'><line x1='2' y1='10' x2='18' y2='10' stroke='#000' stroke-width='4'/></svg>", 50).unwrap();
    assert_eq!(px(&img, 10, 10) >> 24, 255);
    assert_eq!(px(&img, 10, 3) >> 24, 0);
}

#[test]
fn style_transform_opacity_and_inheritance() {
    let img = render(
        b"<svg width='20' height='20'><g fill='#ff0000' transform='translate(10 0)'>\
          <rect width='5' height='5'/><rect y='10' width='5' height='5' style='fill:#0000ff;opacity:.5'/></g></svg>",
        50,
    )
    .unwrap();
    assert_eq!(px(&img, 12, 2), 0xffff0000);
    assert_eq!(px(&img, 1, 2) >> 24, 0, "moved right");
    let half = px(&img, 12, 12);
    assert!((120..=135).contains(&(half >> 24)), "{half:x}");
    assert_eq!(half & 0xffffff, 0x0000ff);
    // Rotate by 90 degrees about the origin: (x, y) -> (-y, x).
    let img = render(b"<svg width='20' height='20'><rect width='5' height='10' fill='#000' transform='translate(10 0) rotate(90)'/></svg>", 50).unwrap();
    assert_eq!(px(&img, 6, 2) >> 24, 255);
}

#[test]
fn gradients_use_their_first_stop_and_hidden_things_are_not_drawn() {
    let img = render(
        b"<svg width='10' height='10'><defs><linearGradient id='g'><stop offset='0' stop-color='#ff8000'/>\
          <stop offset='1' stop-color='#000'/></linearGradient><rect width='10' height='10' fill='#fff'/></defs>\
          <rect width='10' height='10' fill='url(#g)'/>\
          <rect width='10' height='10' fill='#0f0' display='none'/>\
          <text>no</text></svg>",
        50,
    )
    .unwrap();
    assert_eq!(px(&img, 5, 5), 0xffff8000);
}

#[test]
fn even_odd_makes_a_hole() {
    let d = "M0 0H20V20H0Z M5 5H15V15H5Z";
    let a = render(
        alloc::format!("<svg width='20' height='20'><path d='{d}' fill-rule='evenodd'/></svg>")
            .as_bytes(),
        50,
    )
    .unwrap();
    assert_eq!(px(&a, 10, 10) >> 24, 0);
    assert_eq!(px(&a, 2, 2) >> 24, 255);
    let b = render(
        alloc::format!("<svg width='20' height='20'><path d='{d}'/></svg>").as_bytes(),
        50,
    )
    .unwrap();
    assert_eq!(
        px(&b, 10, 10) >> 24,
        255,
        "nonzero fills the inner square too (same winding)"
    );
}

#[test]
fn hostile_documents_are_bounded() {
    assert_eq!(render(b"not svg", 50).err(), Some(SvgError::NotSvg));
    assert_eq!(render(b"<html/>", 50).err(), Some(SvgError::NotSvg));
    assert_eq!(
        render(b"<svg width='0' height='0'/>", 50).err(),
        Some(SvgError::BadSize)
    );
    // Deep nesting, many elements, a huge path and a huge polygon finish.
    let mut s = String::from("<svg width='10' height='10'>");
    for _ in 0..200 {
        s.push_str("<g>");
    }
    s.push_str("<rect width='5' height='5'/>");
    assert!(render(s.as_bytes(), 50).is_ok());
    let mut s = String::from("<svg width='50' height='50'>");
    for i in 0..9000 {
        s.push_str(&alloc::format!(
            "<rect x='{}' y='1' width='2' height='2'/>",
            i % 40
        ));
    }
    assert!(render(s.as_bytes(), 50).is_ok());
    let mut d = String::from("M0 0");
    for i in 0..100_000 {
        d.push_str(&alloc::format!(" L{} {}", i % 50, (i * 7) % 50));
    }
    let doc = alloc::format!("<svg width='50' height='50'><path d='{d}' stroke='#000'/></svg>");
    assert!(render(doc.as_bytes(), 50).is_ok());
    // Garbage inside tags and numbers does not panic.
    for junk in [
        "<svg width='1e999' height='5'/>",
        "<svg><path d='M 1e5e5 L - - Z a'/></svg>",
        "<svg><rect width='-5' height='5'/><circle r='x'/></svg>",
        "<svg viewBox='0 0 0 0'/>",
        "<svg><polygon points='1,2,3'/></svg>",
    ] {
        let _ = render(junk.as_bytes(), 50);
    }
    // A big declared size is refused before allocating.
    assert_eq!(
        render(b"<svg width='16000' height='16000'/>", 20_000).err(),
        Some(SvgError::BadSize)
    );
}

#[test]
fn sine_table_is_close() {
    for (deg, s, c) in [
        (0, 0, 65536),
        (30, 32768, 56756),
        (45, 46341, 46341),
        (90, 65536, 0),
        (180, 0, -65536),
        (270, -65536, 0),
    ] {
        let (ss, cc) = sin_cos(deg);
        assert!(
            (ss - s).abs() < 400 && (cc - c).abs() < 400,
            "{deg}: {ss} {cc}"
        );
    }
}
