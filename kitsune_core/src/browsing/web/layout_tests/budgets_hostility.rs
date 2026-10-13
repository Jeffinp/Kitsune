use super::*;

#[test]
fn huge_css_lengths_do_not_overflow_layout() {
    let html = "<div style='margin:2147483647;padding:2147483647'>\
        <p style='margin:2147483647;padding:2147483647'>x</p>\
        <p style='margin:2147483647'>y</p></div>\
        <p style='font-size:2147483647px;margin:99999999999999999999'>z</p>";
    for z in [50, 100, 300] {
        let p = lay_zoom(html, 600, z);
        assert!(p.height >= 0);
        for c in &p.cmds {
            match c {
                Cmd::Rect { x, y, w, h, .. } => assert!(*x >= 0 && *y >= 0 && *w >= 0 && *h >= 0),
                Cmd::Text { x, y, .. } => assert!(*x >= 0 && *y >= 0),
                Cmd::Image { x, y, w, h, .. } | Cmd::Video { x, y, w, h, .. } => {
                    assert!(*x >= 0 && *y >= 0 && *w >= 0 && *h >= 0)
                }
                Cmd::Border { w, h, .. } => assert!(*w >= 0 && *h >= 0),
            }
        }
    }
}

#[test]
fn negative_and_absurd_values_do_not_panic() {
    for css in [
        "margin:-99999px",
        "padding:-5px",
        "width:-10px",
        "max-width:0",
        "font-size:0",
        "font-size:-3px",
        "line-height:0",
        "line-height:99999",
        "border:99999px solid red",
        "border-radius:99999px",
        "height:99999px",
        "margin:0 auto 0 auto;width:1px",
    ] {
        let html = format!("<div style='{css}'><p>text <b>bold</b></p><ul><li>x</li></ul></div>");
        for z in [50, 300] {
            let p = lay_zoom(&html, 300, z);
            assert!(p.height >= 0, "{css}");
        }
    }
}

#[test]
fn huge_document_renders_fast_and_bounded() {
    let mut html = String::from("<style>");
    for i in 0..3000 {
        html.push_str(&format!(
            ".c{i}, div p.x{i} {{ color: red; margin: 1px }}\n"
        ));
    }
    html.push_str("</style><body>");
    for i in 0..6000 {
        html.push_str(&format!("<div><p class='c{i}'>row {i}</p></div>"));
    }
    html.push_str("</body>");
    let start = std::time::Instant::now();
    let p = lay(&html, 600);
    assert!(start.elapsed() < std::time::Duration::from_secs(30));
    assert!(p.height > 0);
    assert!(p.cmds.len() <= MAX_CMDS);
}

#[test]
fn selector_work_is_bounded_by_the_budget() {
    // Descendant selectors on deep chains cannot take unbounded time.
    let mut css = String::new();
    for _ in 0..900 {
        css.push_str("div div div div p { color: red }\n");
    }
    let html = format!(
        "<style>{css}</style>{}<p>x</p>{}",
        "<div>".repeat(35),
        "</div>".repeat(35)
    );
    let start = std::time::Instant::now();
    let p = lay(&html, 600);
    assert!(start.elapsed() < std::time::Duration::from_secs(20));
    assert!(p.height >= 0);
}

#[test]
fn deep_nesting_lays_out_on_a_small_stack() {
    let mut html = String::new();
    for _ in 0..5_000 {
        html.push_str("<div style='padding:1px'><ul><li><b><i>");
    }
    html.push_str("deep text");
    let h = std::thread::Builder::new()
        .stack_size(256 * 1024)
        .spawn(move || {
            let p = render(html.as_bytes(), 600);
            assert!(p.height > 0);
        })
        .unwrap();
    h.join().unwrap();
}

#[test]
fn many_words_are_bounded() {
    let html = format!("<p>{}</p>", "ab ".repeat(600_000));
    let p = lay(&html, 500);
    let chars: usize = runs(&p).iter().map(|r| r.2.chars().count()).sum();
    assert!(chars <= 2 * MAX_TEXT_CHARS, "{chars}"); // spaces between words are not counted
}

#[test]
fn layout_is_deterministic() {
    let html = "<h1>T</h1><p>some <b>text</b> <a href=x>link</a> and more words to wrap around</p><ul><li>a</li></ul>";
    let a = lay(html, 300);
    let b = lay(html, 300);
    assert_eq!(a.height, b.height);
    assert_eq!(format!("{:?}", a.cmds), format!("{:?}", b.cmds));
}

#[test]
fn relayout_at_another_width_reflows() {
    let html = "<p>one two three four five six seven eight nine ten eleven twelve</p>";
    let doc = Doc::parse(html.as_bytes());
    let at = |w| {
        doc.layout(&Layout {
            width: w,
            zoom: 100,
            images: &NoImages,
            metrics: &FixedAdvance,
        })
    };
    assert!(at(200).height > at(900).height);
    assert!(at(40).height > 0);
    assert!(at(0).height > 0);
    assert!(at(-5).height > 0);
}

#[test]
fn metrics_are_used_not_character_counts() {
    // The same text in a narrower face takes fewer lines.
    struct Narrow;
    impl TextMetrics for Narrow {
        fn width(&self, t: &str, f: Font) -> i32 {
            t.chars().count() as i32 * i32::from(f.size) / 4
        }
        fn line_height(&self, f: Font) -> i32 {
            i32::from(f.size) * 6 / 5
        }
        fn ascent(&self, f: Font) -> i32 {
            i32::from(f.size)
        }
    }
    let html = "<p>one two three four five six seven eight nine ten eleven twelve</p>";
    let wide = render_with(html.as_bytes(), 200, &FixedAdvance);
    let narrow = render_with(html.as_bytes(), 200, &Narrow);
    assert!(narrow.height < wide.height);
}
