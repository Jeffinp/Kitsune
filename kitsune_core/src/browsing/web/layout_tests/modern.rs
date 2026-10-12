//! Flexbox, grid, inline-block rows, floats and the hiding rules, with the fixed 8 px/character metrics.

use super::*;

const A: &str = "<div style='background:#aa0000;width:100px'>aaaa</div>";

#[test]
fn flex_row_places_boxes_side_by_side_by_their_content() {
    let p = lay(
        &body0("<div style='display:flex'><span>aa</span><span>bbbb</span><span>c</span></div>"),
        600,
    );
    assert_eq!(x_of(&p, "aa"), 0);
    assert_eq!(x_of(&p, "bbbb"), 16);
    assert_eq!(x_of(&p, "c"), 16 + 32);
    assert_eq!(distinct_ys(&p).len(), 1);
}

#[test]
fn flex_grow_shares_the_free_room() {
    let p = lay(
        &body0(
            "<div style='display:flex'>\
             <div style='flex:1;background:#010101'>a</div>\
             <div style='flex:2;background:#020202'>b</div></div>",
        ),
        600,
    );
    let (x1, _, w1, _) = rect_of(&p, Rgb(1, 1, 1)).unwrap();
    let (x2, _, w2, _) = rect_of(&p, Rgb(2, 2, 2)).unwrap();
    assert_eq!((x1, w1), (0, 200));
    assert_eq!((x2, w2), (200, 400));
}

#[test]
fn flex_shrinks_wide_boxes_and_justify_spreads_the_rest() {
    // Two 400 px boxes in 600 px shrink to 300 each.
    let p = lay(
        &body0(
            "<div style='display:flex'>\
             <div style='width:400px;background:#010101'>a</div>\
             <div style='width:400px;background:#020202'>b</div></div>",
        ),
        600,
    );
    let (_, _, w1, _) = rect_of(&p, Rgb(1, 1, 1)).unwrap();
    assert_eq!(w1, 300);
    // space-between: first at the left edge, last at the right edge.
    let p = lay(
        &body0(
            "<div style='display:flex;justify-content:space-between'>\
             <div style='width:100px;background:#010101'>a</div>\
             <div style='width:100px;background:#020202'>b</div></div>",
        ),
        600,
    );
    assert_eq!(rect_of(&p, Rgb(1, 1, 1)).unwrap().0, 0);
    assert_eq!(rect_of(&p, Rgb(2, 2, 2)).unwrap().0, 500);
    // center.
    let p = lay(
        &body0(
            "<div style='display:flex;justify-content:center'>\
             <div style='width:100px;background:#010101'>a</div></div>",
        ),
        600,
    );
    assert_eq!(rect_of(&p, Rgb(1, 1, 1)).unwrap().0, 250);
}

#[test]
fn auto_margins_push_a_box_to_the_far_side() {
    let p = lay(
        &body0(
            "<div style='display:flex'><div style='width:100px;background:#010101'>a</div>\
             <div style='margin-left:auto;width:100px;background:#020202'>b</div></div>",
        ),
        600,
    );
    assert_eq!(rect_of(&p, Rgb(2, 2, 2)).unwrap().0, 500);
}

#[test]
fn flex_gap_wrap_and_stretch() {
    let p = lay(
        &body0(
            "<div style='display:flex;flex-wrap:wrap;gap:10px'>\
             <div style='width:250px;background:#010101'>a</div>\
             <div style='width:250px;background:#020202'>b</div>\
             <div style='width:250px;background:#030303'>c<br>d<br>e</div></div>",
        ),
        600,
    );
    // Two fit (250 + 10 + 250); the third wraps below, 10 px lower than the first line's bottom.
    let a = rect_of(&p, Rgb(1, 1, 1)).unwrap();
    let b = rect_of(&p, Rgb(2, 2, 2)).unwrap();
    let c = rect_of(&p, Rgb(3, 3, 3)).unwrap();
    assert_eq!((a.0, b.0), (0, 260));
    assert_eq!(c.0, 0);
    assert_eq!(c.1, a.1 + a.3 + 10);
    // Stretch: a box in a line with a taller one is as tall as the line.
    let p = lay(
        &body0(
            "<div style='display:flex'><div style='background:#010101'>a</div>\
             <div style='background:#020202'>b<br>b<br>b</div></div>",
        ),
        600,
    );
    assert_eq!(
        rect_of(&p, Rgb(1, 1, 1)).unwrap().3,
        rect_of(&p, Rgb(2, 2, 2)).unwrap().3
    );
    // align-items:center moves the short one down instead.
    let p = lay(
        &body0(
            "<div style='display:flex;align-items:center'><div>a</div>\
             <div>b<br>b<br>b</div></div>",
        ),
        600,
    );
    assert!(y_of(&p, "a") > 0);
}

#[test]
fn flex_column_stacks_and_text_in_a_flex_container_stays_text() {
    let p = lay(
        &body0(
            "<div style='display:flex;flex-direction:column;gap:20px'><div>a</div><div>b</div></div>",
        ),
        600,
    );
    assert_eq!(x_of(&p, "a"), 0);
    assert!(y_of(&p, "b") - y_of(&p, "a") >= 20);
    let p = lay(&body0("<div style='display:flex'>just text</div>"), 600);
    assert_eq!(texts(&p), ["just text"]);
}

#[test]
fn grid_columns_fr_px_and_repeat() {
    let p = lay(
        &body0(
            "<div style='display:grid;grid-template-columns:100px 1fr 2fr;gap:0'>\
             <div style='background:#010101'>a</div><div style='background:#020202'>b</div>\
             <div style='background:#030303'>c</div><div style='background:#040404'>d</div></div>",
        ),
        700,
    );
    let w = |n: u8| rect_of(&p, Rgb(n, n, n)).unwrap();
    assert_eq!((w(1).0, w(1).2), (0, 100));
    assert_eq!((w(2).0, w(2).2), (100, 200));
    assert_eq!((w(3).0, w(3).2), (300, 400));
    // The fourth wraps to the second row, first column.
    assert_eq!(w(4).0, 0);
    assert!(w(4).1 > w(1).1);
    let p = lay(
        &body0(
            "<div style='display:grid;grid-template-columns:repeat(3,1fr);gap:10px'>\
             <div style='background:#010101'>a</div><div style='background:#020202'>b</div>\
             <div style='background:#030303'>c</div></div>",
        ),
        620,
    );
    assert_eq!(rect_of(&p, Rgb(2, 2, 2)).unwrap().0, 200 + 10);
    // auto-fill with a minimum size.
    let p = lay(
        &body0(
            "<div style='display:grid;grid-template-columns:repeat(auto-fill,minmax(150px,1fr))'>\
             <div style='background:#010101'>a</div><div style='background:#020202'>b</div>\
             <div style='background:#030303'>c</div></div>",
        ),
        500,
    );
    assert_eq!(rect_of(&p, Rgb(1, 1, 1)).unwrap().2, 500 / 3);
}

#[test]
fn a_run_of_inline_blocks_is_a_row_of_boxes() {
    let p = lay(
        &body0(
            "<ul style='margin:0;padding:0;list-style:none'>\
             <li style='display:inline-block;padding:4px 10px;background:#010101'>aa</li>\
             <li style='display:inline-block;padding:4px 10px;background:#020202'>bbbb</li></ul>",
        ),
        600,
    );
    let a = rect_of(&p, Rgb(1, 1, 1)).unwrap();
    let b = rect_of(&p, Rgb(2, 2, 2)).unwrap();
    assert_eq!(a.2, 16 + 20);
    assert!(b.0 >= a.0 + a.2, "the second box starts after the first");
    assert_eq!(a.1, b.1);
    // Among text they stay in the line.
    let p = lay(
        &body0("<p>one <span style='display:inline-block'>two</span> three</p>"),
        600,
    );
    assert_eq!(distinct_ys(&p).len(), 1);
}

#[test]
fn floats_sit_at_the_side_and_text_wraps_beside_them() {
    let p = lay(
        &body0(
            "<div style='float:left;width:100px;height:20px;background:#010101'>f</div>\
             <p style='margin:0'>aaaa bbbb cccc dddd eeee ffff gggg hhhh iiii jjjj</p>",
        ),
        300,
    );
    let f = rect_of(&p, Rgb(1, 1, 1)).unwrap();
    assert_eq!((f.0, f.2), (0, 100));
    // The first line of text starts to the right of the float.
    let first = runs(&p).into_iter().find(|r| r.2 != "f").unwrap();
    assert!(first.0 >= 100, "{:?}", runs(&p));
    // Lines below the float's bottom use the full width again.
    let last = runs(&p).into_iter().last().unwrap();
    assert!(last.1 >= 20 && last.0 == 0, "{:?}", runs(&p));
    // Right floats go to the right edge.
    let p = lay(
        &body0("<div style='float:right;width:100px;background:#010101'>f</div><p>x</p>"),
        300,
    );
    assert_eq!(rect_of(&p, Rgb(1, 1, 1)).unwrap().0, 200);
}

#[test]
fn floated_columns_in_a_row_and_clear() {
    let p = lay(
        &body0(
            "<div class=row><div style='float:left;width:50%;background:#010101'>a</div>\
             <div style='float:left;width:50%;background:#020202'>b</div></div>\
             <div style='clear:both;background:#030303'>after</div>",
        ),
        600,
    );
    assert_eq!(rect_of(&p, Rgb(1, 1, 1)).unwrap().0, 0);
    assert_eq!(rect_of(&p, Rgb(2, 2, 2)).unwrap().0, 300);
    // The row had only floats: it holds them, so the next block starts below.
    assert!(rect_of(&p, Rgb(3, 3, 3)).unwrap().1 >= rect_of(&p, Rgb(1, 1, 1)).unwrap().1 + 19);
}

#[test]
fn hidden_on_purpose_is_not_shown() {
    for style in [
        "visibility:hidden",
        "opacity:0",
        "position:absolute;left:-9999px",
        "position:absolute;width:1px;height:1px;overflow:hidden;clip:rect(0,0,0,0)",
        "text-indent:-9999px",
    ] {
        let p = lay(
            &body0(&format!("<p>a</p><p style='{style}'>hidden</p><p>b</p>")),
            600,
        );
        assert_eq!(texts(&p), ["a", "b"], "{style}");
    }
    let p = lay(&body0("<p>a</p><p hidden>hidden</p><p>b</p>"), 600);
    assert_eq!(texts(&p), ["a", "b"]);
    let _ = A;
}

#[test]
fn media_queries_follow_the_viewport() {
    let css = "<style>.x{color:#010101}@media (min-width: 700px){.x{color:#020202}}</style>";
    let color = |w: i32| match run_named(&lay(&format!("{css}<p class=x>t</p>"), w), "t") {
        Cmd::Text { color, .. } => *color,
        _ => unreachable!(),
    };
    assert_eq!(color(500), Rgb(1, 1, 1));
    assert_eq!(color(800), Rgb(2, 2, 2));
}

#[test]
fn custom_properties_and_calc_reach_the_layout() {
    let p = lay(
        "<style>:root{--w:200px;--gap:10px}.b{width:calc(var(--w) + var(--gap));background:#010101}</style>\
         <body style='margin:0'><div class=b>x</div></body>",
        600,
    );
    assert_eq!(rect_of(&p, Rgb(1, 1, 1)).unwrap().2, 210);
    let p = lay(
        "<body style='margin:0'><div style='width:min(100%, 300px);background:#010101'>x</div></body>",
        200,
    );
    assert_eq!(rect_of(&p, Rgb(1, 1, 1)).unwrap().2, 200);
}

#[test]
fn quirks_mode_tables_do_not_inherit_center() {
    // No doctype: the cells stay left under <center>; with one they inherit it.
    let quirks = lay(
        "<body style='margin:0'><center><table><tr><td>x</td></tr></table></center></body>",
        600,
    );
    let std = lay(
        "<!DOCTYPE html><body style='margin:0'><center><table><tr><td>x</td></tr></table></center></body>",
        600,
    );
    assert!(x_of(&quirks, "x") < x_of(&std, "x") || x_of(&quirks, "x") <= 12);
}

#[test]
fn hostile_nesting_of_flex_and_floats_terminates() {
    let mut html = String::new();
    for _ in 0..30 {
        html.push_str("<div style='display:flex'><div style='float:left'><div style='display:grid;grid-template-columns:1fr 1fr'>");
    }
    html.push_str("text");
    let p = lay(&html, 400);
    assert!(p.height >= 0);
}

#[test]
fn translucent_colours_blend_over_the_ancestor_background() {
    let p = lay(
        &body0(
            "<div style='background:#102030'><div style='background:rgba(255,255,255,.5)'>x</div></div>",
        ),
        300,
    );
    let rects: Vec<Rgb> = p
        .cmds
        .iter()
        .filter_map(|c| match c {
            Cmd::Rect { color, .. } => Some(*color),
            _ => None,
        })
        .collect();
    assert!(rects.contains(&Rgb(0x10, 0x20, 0x30)));
    // Half white over #102030, not over white.
    let blended = rects.iter().find(|c| **c != Rgb(0x10, 0x20, 0x30)).unwrap();
    assert!(
        (130..=140).contains(&blended.0) && blended.2 > blended.0,
        "{blended:?}"
    );
}
