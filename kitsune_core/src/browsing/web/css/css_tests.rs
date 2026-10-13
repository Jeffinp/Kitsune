use super::super::dom::{Node, parse_html};
use super::*;

fn el_of(html: &str) -> Vec<Node> {
    parse_html(html.as_bytes()).0
}

fn first(nodes: &[Node]) -> &Element {
    match &nodes[0] {
        Node::Element(e) => e,
        _ => panic!("not an element"),
    }
}

fn sel(s: &str) -> Selector {
    parse_complex_selector(s).unwrap_or_else(|| panic!("{s} did not parse"))
}

fn m(s: &str, el: &Element, anc: &[&Element]) -> bool {
    let mut b = 1000;
    sel(s).matches(el, anc, &mut b)
}

#[test]
fn parses_rules_and_decls() {
    let ss = parse_css("h1, .big { color: #fff; font-size: 32px; } p{margin:10px}");
    assert_eq!(ss.rules.len(), 2);
    assert_eq!(ss.rules[0].selectors.len(), 2);
    assert_eq!(ss.rules[0].decls.len(), 2);
    assert_eq!(ss.rules[0].decls[0].name, "color");
}

#[test]
fn selector_matching_and_specificity() {
    let nodes = el_of("<p id=lead class='a b'>x</p>");
    let el = first(&nodes);
    assert!(m("p", el, &[]));
    assert!(m(".a", el, &[]));
    assert!(m("#lead", el, &[]));
    assert!(m("p.a.b", el, &[]));
    assert!(!m(".c", el, &[]));
    assert!(sel("#lead").specificity() > sel("p").specificity());
    assert!(sel("div p").specificity() > sel("p").specificity());
}

/// Regression: unbounded rule and selector counts made the cascade
/// O(rules x elements) with no ceiling.
#[test]
fn rule_and_selector_counts_are_capped() {
    let mut css = String::new();
    for i in 0..(MAX_RULES * 3) {
        css.push_str(&format!(".r{i}{{color:red}}"));
    }
    let ss = parse_css(&css);
    assert_eq!(ss.rules.len(), MAX_RULES);
    // The first rules are the ones kept (document order = cascade order).
    assert_eq!(ss.rules[0].selectors[0].classes, ["r0"]);

    // One rule with a huge selector list cannot dodge the selector budget.
    let list = (0..MAX_SELECTORS * 4)
        .map(|i| format!(".s{i}"))
        .collect::<Vec<_>>()
        .join(",");
    let ss = parse_css(&format!("{list}{{color:red}} p{{margin:1px}}"));
    let total: usize = ss.rules.iter().map(|r| r.selectors.len()).sum();
    assert!(total <= MAX_SELECTORS, "{total} selectors kept");
}

#[test]
fn skips_at_rules_and_comments() {
    let ss = parse_css(
        "@font-face { font-family: x } /* c */ a { color: blue; } @keyframes k { from {top:0} }",
    );
    // Font and animation blocks are skipped; only the `a` rule remains.
    assert_eq!(ss.rules.len(), 1);
    assert_eq!(ss.rules[0].selectors[0].tag.as_deref(), Some("a"));
}

#[test]
fn media_blocks_keep_their_rules_with_the_condition() {
    let ss = parse_css(
        "p{color:red} @media (min-width: 700px) { p{color:blue} } @media print { p{color:green} }\
         @media screen and (max-width: 600px), (orientation: portrait) { a{color:pink} }",
    );
    assert_eq!(ss.rules.len(), 4);
    let on = |i: usize, w: i32, h: i32| {
        let r = &ss.rules[i];
        r.media == 0 || ss.medias[usize::from(r.media) - 1].matches(w, h)
    };
    assert!(on(0, 300, 500));
    assert!(!on(1, 699, 500) && on(1, 700, 500));
    assert!(!on(2, 2000, 500), "print never applies");
    assert!(on(3, 500, 900) && on(3, 900, 1200) && !on(3, 900, 500));
}

#[test]
fn media_features_and_range_syntax() {
    let m = |s: &str| Media::parse(s).unwrap();
    assert!(m("(width >= 600px)").matches(600, 400));
    assert!(!m("(width > 600px)").matches(600, 400));
    assert!(m("(400px <= width <= 800px)").matches(800, 400));
    assert!(!m("(400px <= width < 800px)").matches(800, 400));
    assert!(m("(prefers-color-scheme: light)").matches(500, 500));
    assert!(!m("(prefers-color-scheme: dark)").matches(500, 500));
    assert!(m("not (min-width: 1000px)").matches(500, 500));
    assert!(m("(min-width: 40em)").matches(640, 500));
    assert!(!m("(some-new-feature: 3)").matches(500, 500));
    assert!(m("all").matches(1, 1));
}

#[test]
fn imports_are_collected() {
    let ss = parse_css(
        "@import url(\"a.css\"); @import 'b.css' screen; @import url(c.css) print; p{color:red}",
    );
    assert_eq!(ss.imports, ["a.css", "b.css"]);
    assert_eq!(ss.rules.len(), 1);
}

#[test]
fn semicolons_inside_urls_and_strings_do_not_split() {
    let d = parse_decls("background:url(data:image/png;base64,AAAA); content:\"a;b\"; color:red");
    assert_eq!(d.len(), 3);
    assert_eq!(d[2].value, "red");
}

#[test]
fn complex_selectors_keep_their_ancestors() {
    let ss = parse_css("div.box > p span.hl { color: red }");
    let s = &ss.rules[0].selectors[0];
    assert_eq!(s.tag.as_deref(), Some("span"));
    assert_eq!(s.classes, ["hl"]);
    assert_eq!(s.ancestors.len(), 2);
    assert_eq!(s.ancestors[0].0, Comb::Descendant);
    assert_eq!(s.ancestors[0].1.tag.as_deref(), Some("p"));
    assert_eq!(s.ancestors[1].0, Comb::Child);
    assert_eq!(s.ancestors[1].1.classes, ["box"]);
}

#[test]
fn descendant_and_child_combinators_match_the_chain() {
    let nodes = el_of("<div class=card><section><p class=t>x</p></section></div>");
    let div = first(&nodes);
    let Node::Element(section) = &div.children[0] else {
        panic!()
    };
    let Node::Element(p) = &section.children[0] else {
        panic!()
    };
    let anc = [div, section];
    assert!(m(".card p", p, &anc));
    assert!(m("div section > p.t", p, &anc));
    assert!(!m(".card > p", p, &anc));
    assert!(!m(".other p", p, &anc));
    assert!(!m(".card p", p, &[]));
}

#[test]
fn unsupported_selectors_never_match() {
    for s in [
        "a:hover",
        "p::marker",
        "p::before:hover",
        "a:focus",
        "a:visited",
        "a >",
        "a +",
        "p:unknown-thing",
        "p:not(.a .b)",
    ] {
        assert!(parse_complex_selector(s).is_none(), "{s}");
    }
    // The whole rule is dropped, not widened to `a`.
    assert!(parse_css("a:hover{color:red}").rules.is_empty());
    // A list keeps its good members.
    let ss = parse_css("a:hover, b{color:red}");
    assert_eq!(ss.rules[0].selectors.len(), 1);
}

#[test]
fn attribute_selectors() {
    let nodes = el_of("<input type=Submit class=go data-x='a b c'>");
    let el = first(&nodes);
    assert!(m("input[type=submit]", el, &[]));
    assert!(m("[type=\"SUBMIT\"]", el, &[]));
    assert!(m("[class]", el, &[]));
    assert!(m("[data-x~=b]", el, &[]));
    assert!(m("[data-x^='a ']", el, &[]));
    assert!(m("[data-x$=c]", el, &[]));
    assert!(m("[data-x*=' b ']", el, &[]));
    assert!(!m("[type=text]", el, &[]));
    assert!(!m("[nope]", el, &[]));
}

#[test]
fn root_selects_html_and_important_is_flagged() {
    let ss = parse_css(":root{color:red} p{margin:0 !important;color:blue}");
    assert_eq!(ss.rules[0].selectors[0].tag.as_deref(), Some("html"));
    let d = &ss.rules[1].decls;
    assert!(d[0].important && d[0].value == "0");
    assert!(!d[1].important);
}

#[test]
fn comments_inside_blocks_and_the_ancestor_budget() {
    let ss = parse_css("p{ /* x */ color:red; /* y */ margin:1px }");
    assert_eq!(ss.rules[0].decls.len(), 2);
    let nodes = el_of("<div><p>x</p></div>");
    let div = first(&nodes);
    let Node::Element(p) = &div.children[0] else {
        panic!()
    };
    let mut none = 0;
    assert!(!sel("div p").matches(p, &[div], &mut none));
    let mut some = 5;
    assert!(sel("div p").matches(p, &[div], &mut some));
}

#[test]
fn the_index_offers_every_rule_that_can_match_in_rule_order() {
    let mut ss = parse_css(
        "p{a:1} .x{a:2} div p{a:3} h1,p{a:4} *{a:5} [id]{a:6} span > b{a:7} a, .y{a:8} #k{a:9} p.z{a:10}",
    );
    let nodes =
        el_of("<p id=k class='x z'>t</p><section></section><h1></h1><b></b><a class=y></a>");
    let el = |i: usize| match &nodes[i] {
        Node::Element(e) => e,
        _ => panic!(),
    };
    let mut out = Vec::new();
    // Without an index every rule is a candidate.
    assert!(!ss.candidates(el(0), &mut out));
    ss.build_index();
    let mut names = |e: &Element| -> Vec<usize> {
        assert!(ss.candidates(e, &mut out));
        out.iter().map(|&i| i as usize).collect()
    };
    // `p`: its tag rules, the `.x` and `#k` rules by class and id, `p.z`, and the generic ones (`*`, `[id]`).
    assert_eq!(names(el(0)), [0, 1, 2, 3, 4, 5, 8, 9]);
    // An element no rule names still gets the generic ones, in order.
    assert_eq!(names(el(1)), [4, 5]);
    assert_eq!(names(el(2)), [3, 4, 5]);
    assert_eq!(names(el(3)), [4, 5, 6]);
    assert_eq!(names(el(4)), [4, 5, 7]);
    // Every rule that matches the element is among the candidates.
    for (k, e) in (0..5).map(|k| (k, el(k))) {
        let cand = names(e);
        for (i, r) in ss.rules.iter().enumerate() {
            let hit = r
                .selectors
                .iter()
                .any(|s| s.matches_element(e) && s.ancestors.is_empty());
            assert!(!hit || cand.contains(&i), "element {k} rule {i}");
        }
    }
}

#[test]
fn structural_and_logical_pseudo_classes() {
    let nodes =
        el_of("<ul><li class=a>1</li><li>2</li><li class=a>3</li><li>4</li><li hidden>5</li></ul>");
    let Node::Element(ul) = &nodes[0] else {
        panic!()
    };
    let li = |i: usize| match &ul.children[i] {
        Node::Element(e) => e,
        _ => panic!(),
    };
    let anc = [ul];
    assert!(m("li:first-child", li(0), &anc) && !m("li:first-child", li(1), &anc));
    assert!(m("li:last-child", li(4), &anc) && !m("li:last-child", li(3), &anc));
    assert!(m("li:nth-child(2)", li(1), &anc));
    assert!(m("li:nth-child(odd)", li(2), &anc) && !m("li:nth-child(odd)", li(3), &anc));
    assert!(m("li:nth-child(2n)", li(3), &anc));
    assert!(m("li:nth-child(n+4)", li(3), &anc) && !m("li:nth-child(n+4)", li(2), &anc));
    assert!(m("li:nth-child(-n+2)", li(1), &anc) && !m("li:nth-child(-n+2)", li(2), &anc));
    assert!(m("li:nth-last-child(1)", li(4), &anc));
    assert!(m("li:first-of-type", li(0), &anc) && m("li:last-of-type", li(4), &anc));
    assert!(m("li:not(.a)", li(1), &anc) && !m("li:not(.a)", li(0), &anc));
    assert!(m("li:not(.a, [hidden])", li(3), &anc) && !m("li:not(.a, [hidden])", li(4), &anc));
    assert!(m(":is(ul, ol) > li", li(0), &anc));
    assert!(m(":where(.a, .b)", li(0), &anc) && !m(":where(.a, .b)", li(1), &anc));
    // Pseudo-classes weigh like classes.
    let s = parse_complex_selector("li:first-child").unwrap();
    assert_eq!(s.specificity(), (0, 1, 1));
    // A selector list is split outside the parentheses.
    assert_eq!(
        parse_css("a:not(.x, .y), b{color:red}").rules[0]
            .selectors
            .len(),
        2
    );
}

#[test]
fn sibling_combinators() {
    let nodes =
        el_of("<div><h1>t</h1><p id=a>1</p><p id=b>2</p><span id=c>3</span><p id=d>4</p></div>");
    let Node::Element(div) = &nodes[0] else {
        panic!()
    };
    let child = |i: usize| match &div.children[i] {
        Node::Element(e) => e,
        _ => panic!(),
    };
    let anc = [div];
    assert!(m("h1 + p", child(1), &anc));
    assert!(!m("h1 + p", child(2), &anc), "not adjacent");
    assert!(m("h1 ~ p", child(2), &anc) && m("h1 ~ p", child(4), &anc));
    assert!(m("p + span", child(3), &anc) && !m("p + span", child(4), &anc));
    assert!(m("p + p", child(2), &anc) && !m("p + p", child(1), &anc));
    assert!(m("span + p", child(4), &anc));
    // Mixed with the other combinators.
    assert!(m("div > h1 + p", child(1), &anc));
    assert!(m("div h1 ~ p#d", child(4), &anc));
    assert!(!m("h1 + p", child(0), &anc));
    // The index still finds the rule (it is keyed by the rightmost compound).
    let ss = parse_css("h1 + p{color:red}");
    assert_eq!(ss.rules.len(), 1);
}

#[test]
fn attribute_prefix_on_non_ascii_values_does_not_panic() {
    // Wikipedia: a prefix selector against a value whose cut falls inside "ō" took the kernel down.
    let nodes = el_of("<a title='Kitsunō monogatari'>x</a>");
    let el = first(&nodes);
    assert!(!m("[title^='Kitsune']", el, &[]));
    assert!(m("[title^='Kitsun']", el, &[]));
}
