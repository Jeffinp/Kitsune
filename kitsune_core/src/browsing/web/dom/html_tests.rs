use super::*;

fn first_element(nodes: &[Node]) -> &Element {
    nodes
        .iter()
        .find_map(|n| match n {
            Node::Element(e) => Some(e),
            _ => None,
        })
        .expect("an element")
}

#[test]
fn parses_nested_elements_and_text() {
    let (nodes, _) = parse_html(b"<div id=x class='a b'><p>Hello <b>world</b></p></div>");
    let div = first_element(&nodes);
    assert_eq!(div.tag, "div");
    assert_eq!(div.id(), Some("x"));
    let classes: Vec<_> = div.classes().collect();
    assert_eq!(classes, ["a", "b"]);
    let p = first_element(&div.children);
    assert_eq!(p.tag, "p");
}

#[test]
fn captures_style_css_and_skips_script() {
    let (nodes, css) =
        parse_html(b"<style>p { color: red; }</style><script>var x=1<2;</script><p>hi</p>");
    assert!(css.contains("color: red"));
    // <style> produces no node; <script> stays as an empty element (scripts look themselves up).
    let tags: Vec<(&str, usize)> = nodes
        .iter()
        .filter_map(|n| match n {
            Node::Element(e) => Some((e.tag.as_str(), e.children.len())),
            Node::Text(_) => None,
        })
        .collect();
    assert_eq!(tags, [("script", 0), ("p", 1)]);
}

#[test]
fn void_and_self_closing_tags() {
    let (nodes, _) = parse_html(b"<div>a<br>b<img src=x/>c</div>");
    let div = first_element(&nodes);
    // text "a", br, text "b", img, text "c"
    assert!(div.children.len() >= 3);
}

fn count_elements(nodes: &[Node]) -> usize {
    nodes
        .iter()
        .map(|n| match n {
            Node::Element(e) => 1 + count_elements(&e.children),
            Node::Text(_) => 0,
        })
        .sum()
}

fn depth(nodes: &[Node]) -> usize {
    nodes
        .iter()
        .map(|n| match n {
            Node::Element(e) => 1 + depth(&e.children),
            Node::Text(_) => 0,
        })
        .max()
        .unwrap_or(0)
}

fn contains_text(nodes: &[Node], needle: &str) -> bool {
    nodes.iter().any(|n| match n {
        Node::Element(e) => contains_text(&e.children, needle),
        Node::Text(t) => t.contains(needle),
    })
}

/// Regression: nesting depth was unbounded, so a page of 20k `<div>` blew
/// the native stack in parse_html / render / Drop (~150 levels were enough
/// for the kernel's 80 KiB stack). Run on a small stack so an unbounded
/// recursion is caught here too.
#[test]
fn deeply_nested_markup_does_not_overflow_the_stack() {
    let mut html = String::new();
    for _ in 0..20_000 {
        html.push_str("<div><b>");
    }
    html.push_str("deep text");
    let h = std::thread::Builder::new()
        .stack_size(256 * 1024)
        .spawn(move || {
            let (nodes, _) = parse_html(html.as_bytes());
            assert!(depth(&nodes) <= MAX_DEPTH, "depth {}", depth(&nodes));
            // Content past the limit is kept, just not nested further.
            assert!(contains_text(&nodes, "deep text"));
            drop(nodes);
            let page = super::super::render(html.as_bytes(), 600);
            assert!(page.height > 0);
        })
        .unwrap();
    h.join().unwrap();
}

/// Regression: the DOM had no node budget, so a page of hundreds of
/// thousands of tiny elements (one `<i>` is 3 bytes) allocated without
/// bound and made every later pass slow. Parsing stops at `MAX_NODES`.
#[test]
fn node_count_is_capped() {
    let html = "<i>x</i>".repeat(MAX_NODES * 3);
    let (nodes, _) = parse_html(html.as_bytes());
    let total = count_nodes(&nodes);
    assert!(total <= MAX_NODES, "{total} nodes kept");
    // ...but a normal document is untouched.
    let (nodes, _) = parse_html("<p>a</p>".repeat(100).as_bytes());
    assert_eq!(count_elements(&nodes), 100);
}

#[test]
fn node_cap_holds_for_siblings_and_nested_text() {
    let html = "<div>t<b>u</b>v</div>".repeat(MAX_NODES);
    let (nodes, _) = parse_html(html.as_bytes());
    assert!(count_nodes(&nodes) <= MAX_NODES);
    let html = "a<br>".repeat(MAX_NODES * 2); // void elements + text
    let (nodes, _) = parse_html(html.as_bytes());
    assert!(count_nodes(&nodes) <= MAX_NODES);
}

fn count_nodes(nodes: &[Node]) -> usize {
    nodes
        .iter()
        .map(|n| match n {
            Node::Element(e) => 1 + count_nodes(&e.children),
            Node::Text(_) => 1,
        })
        .sum()
}

#[test]
fn tolerates_unclosed_tags() {
    // Both <p> elements are recovered (nested rather than siblings — full
    // optional-end-tag handling is out of scope, but nothing is lost).
    let (nodes, _) = parse_html(b"<p>one<p>two");
    assert_eq!(count_elements(&nodes), 2);
}

#[test]
fn decodes_entities_in_text() {
    let (nodes, _) = parse_html(b"<p>a &amp; b &#233;</p>");
    let p = first_element(&nodes);
    if let Node::Text(t) = &p.children[0] {
        assert!(t.contains("a & b"));
        assert!(t.contains('\u{e9}')); // &#233; stays an e with acute
    } else {
        panic!("expected text");
    }
}

#[test]
fn attribute_names_of_framework_templates_survive() {
    let (nodes, _) = parse_html(
        b"<input @click='go()' :Class=\"{ on: x }\" v-model.trim=name @keyup.enter=\"add\" data-a/b=1 x=\"y\"/>",
    );
    let el = first_element(&nodes);
    let get = |k: &str| el.attrs.get(k).map(String::as_str);
    assert_eq!(get("@click"), Some("go()"));
    assert_eq!(get(":class"), Some("{ on: x }"), "lower case");
    assert_eq!(get("v-model.trim"), Some("name"));
    assert_eq!(get("@keyup.enter"), Some("add"));
    assert_eq!(get("x"), Some("y"));
    assert_eq!(get("data-a"), Some(""), "a '/' ends a name");
}
