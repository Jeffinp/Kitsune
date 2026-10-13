use super::*;

#[test]
fn css_color_and_background_applied() {
    let p = lay(
        "<style>.hl{color:#ff0000} body{background:#102030}</style><p class=hl>red</p>",
        600,
    );
    assert!(p.cmds.iter().any(
        |c| matches!(c, Cmd::Text { text, color, .. } if text == "red" && *color == Rgb(255, 0, 0))
    ));
    assert_eq!(p.background, Rgb(0x10, 0x20, 0x30));
}

#[test]
fn the_page_background_defaults_to_white_and_html_wins() {
    assert_eq!(lay("<p>x</p>", 300).background, Rgb(255, 255, 255));
    let p = lay(
        "<html style='background:#111'><body style='background:#eee'>x",
        300,
    );
    assert_eq!(p.background, Rgb(0x11, 0x11, 0x11));
    // The canvas colour is not also painted as a box.
    assert!(!p.cmds.iter().any(|c| matches!(c, Cmd::Rect { .. })));
}

#[test]
fn block_background_border_and_radius_are_emitted() {
    let p = lay(
        "<body style='margin:0'><div style='background:#eee;border:2px solid #333;padding:10px;border-radius:8px'>x</div></body>",
        300,
    );
    let rect = p.cmds.iter().find_map(|c| match c {
        Cmd::Rect {
            x, y, w, h, radius, ..
        } => Some((*x, *y, *w, *h, *radius)),
        _ => None,
    });
    let (x, y, w, h, r) = rect.expect("background");
    assert_eq!((x, y, w), (0, 0, 300));
    assert_eq!(r, 8);
    // 2 border + 10 padding + a 24 px line + 10 + 2.
    assert_eq!(h, 2 + 10 + 24 + 10 + 2);
    assert!(p.cmds.iter().any(|c| matches!(
        c,
        Cmd::Border {
            widths: [2, 2, 2, 2],
            radius: 8,
            ..
        }
    )));
    // The text sits inside border + padding.
    assert_eq!(x_of(&p, "x"), 12);
}

#[test]
fn display_none_hides_content() {
    let p = lay("<p>shown</p><div style='display:none'>hidden</div>", 600);
    assert!(texts(&p).iter().any(|s| s == "shown"));
    assert!(!texts(&p).iter().any(|s| s == "hidden"));
}

#[test]
fn noscript_is_hidden_and_scripts_take_no_room() {
    // The browser runs scripts: what a page has for browsers without them is not shown, and the
    // (empty) <script> elements kept in the tree lay out to nothing.
    let p = lay(
        "<p>a <script>var x</script>b</p><noscript><p>enable JavaScript</p></noscript>",
        600,
    );
    assert!(!texts(&p).iter().any(|s| s.contains("JavaScript")));
    assert!(texts(&p).iter().any(|s| s.contains('b')));
}

#[test]
fn text_align_center_and_right() {
    let body = "<body style='margin:0'>";
    let p = lay(
        &format!("{body}<p style='margin:0;text-align:center'>abcd</p></body>"),
        200,
    );
    assert_eq!(x_of(&p, "abcd"), (200 - 32) / 2);
    let p = lay(
        &format!("{body}<p style='margin:0;text-align:right'>abcd</p></body>"),
        200,
    );
    assert_eq!(x_of(&p, "abcd"), 200 - 32);
    let p = lay(&format!("{body}<center>abcd</center></body>"), 200);
    assert_eq!(x_of(&p, "abcd"), (200 - 32) / 2);
}

#[test]
fn text_align_is_inherited_by_children() {
    let p = lay(
        "<body style='margin:0'><div style='text-align:center'><p style='margin:0'>ab</p></div></body>",
        100,
    );
    assert_eq!(x_of(&p, "ab"), (100 - 16) / 2);
}

#[test]
fn line_height_changes_the_line_pitch() {
    let one = |lh: &str| {
        let p = lay(
            &format!(
                "<body style='margin:0'><p style='margin:0;width:40px;line-height:{lh}'>aaaa bbbb</p></body>"
            ),
            600,
        );
        let ys = distinct_ys(&p);
        assert_eq!(ys.len(), 2);
        ys[1] - ys[0]
    };
    assert_eq!(one("1"), 16);
    assert_eq!(one("2"), 32);
    assert_eq!(one("30px"), 30);
    assert_eq!(one("150%"), 24);
}

#[test]
fn font_size_weight_and_style_from_css() {
    let p = lay(
        "<p style='font-size:20px;font-weight:700'>big</p><p style='font-style:italic'>it</p><p style='font-family:monospace'>mono</p>",
        600,
    );
    let f = font_of(&p, "big");
    assert!(f.size == 20 && f.bold && !f.italic);
    assert!(font_of(&p, "it").italic);
    assert!(font_of(&p, "mono").mono);
}

#[test]
fn nested_inline_styles_compose() {
    let p = lay(
        "<p>a <b>b <i>bi <u>biu</u></i></b> <span style='color:#00ff00'>g <s>gs</s></span></p>",
        600,
    );
    let f = font_of(&p, "bi");
    assert!(f.bold && f.italic);
    match run_named(&p, "biu") {
        Cmd::Text { font, deco, .. } => {
            assert!(font.bold && font.italic);
            assert_eq!(*deco & DECO_UNDERLINE, DECO_UNDERLINE);
        }
        _ => unreachable!(),
    }
    match run_named(&p, "gs") {
        Cmd::Text { color, deco, .. } => {
            assert_eq!(*color, Rgb(0, 255, 0));
            assert_eq!(*deco & DECO_STRIKE, DECO_STRIKE);
        }
        _ => unreachable!(),
    }
}

#[test]
fn code_and_pre_use_the_monospace_face() {
    let p = lay("<p>use <code>cargo</code></p><pre>a  b\n  c</pre>", 600);
    assert!(font_of(&p, "cargo").mono);
    // Preformatted text keeps its spaces and its line break.
    let t = texts(&p);
    assert!(t.contains(&String::from("a  b")), "{t:?}");
    assert!(t.contains(&String::from("  c")), "{t:?}");
    assert!(font_of(&p, "a  b").mono);
    // pre gets a box.
    assert!(p.cmds.iter().any(|c| matches!(
        c,
        Cmd::Rect {
            color: Rgb(0xf5, 0xf5, 0xf7),
            ..
        }
    )));
}

#[test]
fn a_leading_newline_in_pre_is_dropped_and_tabs_expand() {
    let p = lay("<pre>\nx\ty</pre>", 600);
    assert_eq!(texts(&p), ["x   y"]);
}

#[test]
fn long_pre_lines_wrap_instead_of_overflowing() {
    let line = "w".repeat(200);
    let p = lay(&format!("<pre>{line}</pre>"), 300);
    for c in &p.cmds {
        if let Cmd::Text { x, w, .. } = c {
            assert!(*x + *w <= 300);
        }
    }
    assert!(runs(&p).len() > 1);
}

#[test]
fn white_space_nowrap_keeps_words_together() {
    let p = lay(
        "<body style='margin:0'><p style='margin:0;width:40px;white-space:nowrap'>aaaa bbbb cccc</p></body>",
        600,
    );
    assert_eq!(distinct_ys(&p).len(), 1);
}

#[test]
fn text_transform_and_decoration() {
    let p = lay(
        "<p style='text-transform:uppercase'>abc</p><p style='text-decoration:underline'>u</p>",
        600,
    );
    assert!(texts(&p).contains(&String::from("ABC")));
    match run_named(&p, "u") {
        Cmd::Text { deco, .. } => assert_eq!(*deco & DECO_UNDERLINE, DECO_UNDERLINE),
        _ => unreachable!(),
    }
}
