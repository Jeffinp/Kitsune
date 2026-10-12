use super::*;
use crate::browsing::web::css::parse_css;
use crate::browsing::web::dom::{Node, parse_html};

fn style_of(html: &str, css: &str) -> Computed {
    let (nodes, page_css) = parse_html(html.as_bytes());
    let mut sheet = parse_css(UA_CSS);
    sheet.rules.extend(parse_css(css).rules);
    sheet.rules.extend(parse_css(&page_css).rules);
    let Node::Element(el) = &nodes[0] else {
        panic!("no element")
    };
    let mut b = 100_000;
    compute(el, &sheet, &Computed::root(), &[], &mut b)
}

#[test]
fn lengths_in_every_unit() {
    assert_eq!(parse_len("12px", 16), Some(Len::Px(12)));
    assert_eq!(parse_len("1.5em", 20), Some(Len::Px(30)));
    assert_eq!(parse_len("2rem", 20), Some(Len::Px(32)));
    assert_eq!(parse_len("12pt", 16), Some(Len::Px(16)));
    assert_eq!(parse_len("50%", 16), Some(Len::Pct(50)));
    assert_eq!(parse_len("0", 16), Some(Len::Px(0)));
    assert_eq!(parse_len("auto", 16), Some(Len::Auto));
    assert_eq!(parse_len("-4px", 16), Some(Len::Px(-4)));
    assert_eq!(parse_len(".5em", 10), Some(Len::Px(5)));
    assert_eq!(parse_len("calc(1px + 2px)", 16), Some(Len::Px(3)));
    assert!(matches!(parse_len("10vw", 16), Some(Len::Px(p)) if p > 0));
    assert_eq!(parse_len("", 16), None);
    // Hostile numbers saturate.
    assert_eq!(
        parse_len("99999999999999999999999px", 16),
        Some(Len::Px(MAX_PX))
    );
}

#[test]
fn shorthand_margins_expand_to_four_sides() {
    let c = style_of("<p style='margin:1px 2px 3px 4px'>x</p>", "");
    assert_eq!(c.margin, [Len::Px(1), Len::Px(2), Len::Px(3), Len::Px(4)]);
    let c = style_of("<p style='margin:0 auto'>x</p>", "");
    assert_eq!(c.margin, [Len::Px(0), Len::Auto, Len::Px(0), Len::Auto]);
    let c = style_of("<p style='margin:5px 6px;margin-left:9px'>x</p>", "");
    assert_eq!(c.margin, [Len::Px(5), Len::Px(6), Len::Px(5), Len::Px(9)]);
}

#[test]
fn ua_sizes_and_styles() {
    let h1 = style_of("<h1>x</h1>", "");
    assert_eq!(h1.font_px, 32);
    assert!(h1.bold);
    let em = style_of("<em>x</em>", "");
    assert!(em.italic && !em.bold);
    let code = style_of("<code>x</code>", "");
    assert!(code.mono);
    assert_eq!(code.font_px, 15); // .92 * 16 = 14.72, rounded
    let a = style_of("<a href=x>x</a>", "");
    assert_eq!(a.color, Rgb(0x4f, 0x46, 0xe5));
}

#[test]
fn font_shorthand_and_keywords() {
    let c = style_of(
        "<p style='font:italic 600 20px/1.2 Georgia, serif'>x</p>",
        "",
    );
    assert!(c.italic && c.bold);
    assert_eq!(c.font_px, 20);
    assert_eq!(c.line_h, LineH::Mult(120));
    let c = style_of("<p style='font-size:large'>x</p>", "");
    assert_eq!(c.font_px, 18);
    let c = style_of("<p style='font-size:200%'>x</p>", "");
    assert_eq!(c.font_px, 32);
    let c = style_of("<p style='font: 13px monospace'>x</p>", "");
    assert!(c.mono && c.font_px == 13);
}

#[test]
fn em_lengths_follow_the_elements_own_font_size() {
    let c = style_of("<p style='margin-top:2em;font-size:10px'>x</p>", "");
    assert_eq!(c.margin[0], Len::Px(20));
}

#[test]
fn border_shorthand_and_sides() {
    let c = style_of("<p style='border:2px solid #ff0000'>x</p>", "");
    assert_eq!(c.border_w, [2; 4]);
    assert_eq!(c.border_color, Rgb(255, 0, 0));
    let c = style_of("<p style='border-bottom:1px solid #ccc'>x</p>", "");
    assert_eq!(c.border_w, [0, 0, 1, 0]);
    let c = style_of("<p style='border:none'>x</p>", "");
    assert_eq!(c.border_w, [0; 4]);
    let c = style_of("<p style='border-radius:8px 2px'>x</p>", "");
    assert_eq!(c.radius, 8);
}

#[test]
fn background_shorthand_picks_the_colour_or_a_gradient_stop() {
    let c = style_of("<p style='background:#eee url(x.png) no-repeat'>x</p>", "");
    assert_eq!(c.bg, Some(Rgb(0xee, 0xee, 0xee)));
    let c = style_of(
        "<p style='background:linear-gradient(90deg, #ff0000, #0000ff)'>x</p>",
        "",
    );
    assert_eq!(c.bg, Some(Rgb(255, 0, 0)));
    let c = style_of("<p style='background:transparent'>x</p>", "");
    assert_eq!(c.bg, None);
}

#[test]
fn important_and_specificity_order() {
    let c = style_of(
        "<p class=a id=b>x</p>",
        "#b{color:#00ff00} .a{color:#ff0000 !important} p{color:#0000ff}",
    );
    assert_eq!(c.color, Rgb(255, 0, 0));
    let c = style_of("<p id=b style='color:#112233'>x</p>", "#b{color:#00ff00}");
    assert_eq!(c.color, Rgb(0x11, 0x22, 0x33));
}

#[test]
fn inheritance_and_reset() {
    let (nodes, _) = parse_html(b"<div style='color:#102030;font-weight:bold;background:#fff;margin:9px'><span>x</span></div>");
    let sheet = parse_css(UA_CSS);
    let Node::Element(div) = &nodes[0] else {
        panic!()
    };
    let mut b = 1000;
    let dc = compute(div, &sheet, &Computed::root(), &[], &mut b);
    let Node::Element(span) = &div.children[0] else {
        panic!()
    };
    let sc = compute(span, &sheet, &dc, &[div], &mut b);
    assert_eq!(sc.color, Rgb(0x10, 0x20, 0x30));
    assert!(sc.bold);
    assert_eq!(sc.bg, None);
    assert_eq!(sc.margin, [Len::Px(0); 4]);
}

#[test]
fn text_transform_cases() {
    assert_eq!(transform_text("ação", Transform::Upper), "AÇÃO");
    assert_eq!(transform_text("a b", Transform::Capital), "A B");
    assert_eq!(transform_text("ABC", Transform::Lower), "abc");
}

#[test]
fn calc_min_max_clamp() {
    let l = |v: &str| parse_len(v, 16);
    assert_eq!(
        l("calc(100% - 40px)"),
        Some(Len::Calc {
            pct: 100_000,
            px: -40,
            mode: 0
        })
    );
    assert_eq!(l("calc(2 * (10px + 1em))"), Some(Len::Px(52)));
    assert_eq!(l("calc(100% / 4)"), Some(Len::Pct(25)));
    assert_eq!(l("calc(1rem + 2px)"), Some(Len::Px(18)));
    assert_eq!(l("min(10px, 20px)"), Some(Len::Px(10)));
    assert_eq!(l("max(10px, 2em)"), Some(Len::Px(32)));
    assert_eq!(l("clamp(10px, 50px, 30px)"), Some(Len::Px(30)));
    assert_eq!(
        l("clamp(1rem, 2.5vw, 2rem)").map(|v| matches!(v, Len::Px(16..=32))),
        Some(true)
    );
    assert_eq!(
        l("min(100%, 600px)"),
        Some(Len::Calc {
            pct: 100_000,
            px: 600,
            mode: 1
        })
    );
    assert_eq!(
        l("max(50%, 300px)"),
        Some(Len::Calc {
            pct: 50_000,
            px: 300,
            mode: 2
        })
    );
    // Resolved against the container at layout time.
    assert_eq!(l("min(100%, 600px)").unwrap().resolve(400, 100), Some(400));
    assert_eq!(l("min(100%, 600px)").unwrap().resolve(900, 100), Some(600));
    assert_eq!(l("calc(100% - 40px)").unwrap().resolve(500, 100), Some(460));
    // Malformed or mixed up: no value.
    assert_eq!(l("calc(1px +)"), None);
    assert_eq!(l("calc(1px * 2px)"), None);
    assert_eq!(l("calc(1px / 0)"), None);
    assert_eq!(l("calc(5 + 1px)"), None);
    assert_eq!(l("calc(((((((((1px)))))))))"), None);
}

#[test]
fn custom_properties_substitute() {
    let mut vars = Vars::new();
    vars.insert("--a".into(), "10px".into());
    vars.insert("--b".into(), "var(--a)".into());
    assert_eq!(subst_vars("var(--a)", &vars).as_deref(), Some("10px"));
    assert_eq!(
        subst_vars("calc(var(--b) * 2)", &vars).as_deref(),
        Some("calc(10px * 2)")
    );
    assert_eq!(
        subst_vars("var(--missing, 7px)", &vars).as_deref(),
        Some("7px")
    );
    assert_eq!(
        subst_vars("var(--missing, var(--a))", &vars).as_deref(),
        Some("10px")
    );
    assert_eq!(subst_vars("var(--missing)", &vars), None);
    assert_eq!(
        subst_vars("1px var(--a) var(--a)", &vars).as_deref(),
        Some("1px 10px 10px")
    );
    // A loop of definitions ends instead of growing.
    vars.insert("--x".into(), "var(--y)".into());
    vars.insert("--y".into(), "var(--x)".into());
    assert_eq!(subst_vars("var(--x)", &vars), None);
}

#[test]
fn functions_that_pick_an_argument() {
    assert_eq!(pick_args_for_test("light-dark(#fff, #000)"), "#fff");
    assert_eq!(
        pick_args_for_test("1px solid light-dark(red, blue)"),
        "1px solid red"
    );
    assert_eq!(
        pick_args_for_test("color-mix(in srgb, #ff0000 50%, white)"),
        "#ff0000"
    );
    assert_eq!(pick_args_for_test("env(safe-area-inset-top, 12px)"), "12px");
    assert_eq!(
        pick_args_for_test("light-dark(rgb(1,2,3), rgb(4,5,6)) x env(a, 3px)"),
        "rgb(1,2,3) x 3px"
    );
}
