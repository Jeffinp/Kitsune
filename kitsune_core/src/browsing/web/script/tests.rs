use super::*;
use crate::browsing::web::{
    FixedAdvance, Layout, NO_LINK, UID_ATTR, imgcache::NoImages, parse_html_full,
};

const BASE: &[u8] = b"http://example.test/dir/page.html";

fn layout(doc: &Doc) -> crate::browsing::web::Page {
    doc.layout(&Layout {
        width: 600,
        zoom: 100,
        images: &NoImages,
        metrics: &FixedAdvance,
    })
}

// ---- the parser keeps the scripts ----

#[test]
fn parser_keeps_classic_scripts_in_order() {
    let p = parse_html_full(
        b"<script>a()</script><p>x</p><script src='/s.js?a=1&amp;b=2'></script>\
          <script type=module>import x</script><script type='text/javascript'>b()</script>\
          <script type=application/ld+json>{}</script><script>   </script>",
    );
    let got: Vec<(Option<&str>, &str)> = p
        .scripts
        .iter()
        .map(|s| (s.src.as_deref(), s.text.as_str()))
        .collect();
    assert_eq!(
        got,
        [(None, "a()"), (Some("/s.js?a=1&b=2"), ""), (None, "b()"),]
    );
    // Scripts are still not part of the tree.
    assert_eq!(p.nodes.len(), 1);
}

#[test]
fn script_count_is_bounded() {
    let html = "<script>x()</script>".repeat(crate::browsing::web::dom::MAX_SCRIPTS + 10);
    let p = parse_html_full(html.as_bytes());
    assert_eq!(p.scripts.len(), crate::browsing::web::dom::MAX_SCRIPTS);
}

// ---- element ids ----

#[test]
fn elements_are_numbered_in_document_order() {
    let doc = Doc::parse(
        b"<!doctype html><html><body><div id=a><p>t</p></div><span></span></body></html>",
    );
    let mut seen = Vec::new();
    fn walk(n: &[Node], out: &mut Vec<(String, u32)>) {
        for x in n {
            if let Node::Element(e) = x {
                out.push((e.tag.clone(), e.uid));
                walk(&e.children, out);
            }
        }
    }
    walk(&doc.dom, &mut seen);
    let tags: Vec<&str> = seen.iter().map(|s| s.0.as_str()).collect();
    assert_eq!(tags, ["html", "body", "div", "p", "span"]);
    let ids: Vec<u32> = seen.iter().map(|s| s.1).collect();
    assert_eq!(ids, [1, 2, 3, 4, 5]);
}

#[test]
fn replace_from_script_reads_ids_and_keeps_external_sheets() {
    let mut doc = Doc::parse(b"<!doctype html><style>p{color:#00ff00}</style><p>a</p>");
    doc.add_sheet("h1{color:#0000ff}");
    doc.replace_from_script(
        b"<!doctype html><html data-kjs=1><head data-kjs=7><style>p{color:#00ff00}</style></head>\
          <body data-kjs=2><h1 data-kjs=9>T</h1><p data-kjs=12 class=x>b</p></body></html>",
    );
    let mut ids = Vec::new();
    fn walk(n: &[Node], out: &mut Vec<(String, u32, bool)>) {
        for x in n {
            if let Node::Element(e) = x {
                out.push((e.tag.clone(), e.uid, e.attrs.contains_key(UID_ATTR)));
                walk(&e.children, out);
            }
        }
    }
    walk(&doc.dom, &mut ids);
    assert!(ids.iter().all(|i| !i.2), "the id attribute is taken out");
    assert!(ids.contains(&("p".into(), 12, false)));
    assert!(ids.contains(&("h1".into(), 9, false)));
    // The external sheet still colours the heading, the page's own sheet the paragraph.
    let page = layout(&doc);
    let colour_of = |text: &str| {
        page.cmds.iter().find_map(|c| match c {
            crate::browsing::web::Cmd::Text { text: t, color, .. } if t == text => Some(*color),
            _ => None,
        })
    };
    assert_eq!(colour_of("T"), Some(crate::browsing::web::Rgb(0, 0, 255)));
    assert_eq!(colour_of("b"), Some(crate::browsing::web::Rgb(0, 255, 0)));
}

#[test]
fn replace_from_script_keeps_the_pages_own_style() {
    // The page's <style> is not in the tree the process gets, so its document has none.
    let mut doc = Doc::parse(b"<style>.x{color:#ff0000}</style><p class=x>a</p>");
    doc.replace_from_script(
        b"<html data-kjs=1><body data-kjs=2><p data-kjs=3 class=x>b</p></body></html>",
    );
    let page = layout(&doc);
    let red = page.cmds.iter().any(|c| {
        matches!(c, crate::browsing::web::Cmd::Text { text, color, .. }
            if text == "b" && *color == crate::browsing::web::Rgb(255, 0, 0))
    });
    assert!(red, "{:?}", page.cmds);
}

#[test]
fn replace_from_script_rebuilds_the_sheet_when_style_text_changes() {
    let mut doc = Doc::parse(b"<p>a</p>");
    doc.add_sheet("p{color:#0000ff}");
    doc.replace_from_script(b"<style>p{font-weight:bold}</style><p data-kjs=3>a</p>");
    let page = layout(&doc);
    let run = page
        .cmds
        .iter()
        .find_map(|c| match c {
            crate::browsing::web::Cmd::Text { color, font, .. } => Some((*color, font.bold)),
            _ => None,
        })
        .unwrap();
    assert_eq!(run, (crate::browsing::web::Rgb(0, 0, 255), true));
    // Replacing again with the same style text does not add the external sheet twice.
    doc.replace_from_script(b"<style>p{font-weight:bold}</style><p data-kjs=3>c</p>");
    assert_eq!(doc.ext_css.len(), 1);
}

#[test]
fn replace_from_script_is_tolerant_of_anything() {
    let mut doc = Doc::parse(b"<p>a</p>");
    for junk in [
        &b""[..],
        b"<<<>>>",
        b"<p data-kjs=x>bad id</p>",
        b"<p data-kjs=99999999999999>huge</p>",
        &[0xff, 0xfe, b'<', b'b'],
    ] {
        doc.replace_from_script(junk);
        let _ = layout(&doc);
    }
}

// ---- which element is under the pointer ----

#[test]
fn node_at_finds_the_innermost_element() {
    let mut doc = Doc::parse(
        b"<!doctype html><body><div id=card style='padding:20px'><p>Hello <b>bold</b></p>\
          <button id=go>Go</button></div></body>",
    );
    let off = layout(&doc);
    assert!(off.hits.is_empty(), "no element boxes unless asked");
    doc.set_track(true);
    let page = layout(&doc);
    let uid_of = |tag: &str| {
        fn find(n: &[Node], tag: &str) -> Option<u32> {
            n.iter().find_map(|x| match x {
                Node::Element(e) if e.tag == tag => Some(e.uid),
                Node::Element(e) => find(&e.children, tag),
                _ => None,
            })
        }
        find(&doc.dom, tag).unwrap()
    };
    let center = |uid: u32| {
        let h = page
            .hits
            .iter()
            .filter(|h| h.node == uid)
            .min_by_key(|h| h.w * h.h)
            .unwrap();
        (h.x + h.w / 2, h.y + h.h / 2)
    };
    let (bx, by) = center(uid_of("b"));
    assert_eq!(page.node_at(bx, by), Some(uid_of("b")));
    let (gx, gy) = center(uid_of("button"));
    assert_eq!(page.node_at(gx, gy), Some(uid_of("button")));
    // The padding of the card is the card.
    let card = uid_of("div");
    let h = page.hits.iter().find(|h| h.node == card).unwrap();
    assert_eq!(page.node_at(h.x + 3, h.y + 3), Some(card));
    // Element boxes are not links.
    assert!(page.hits.iter().all(|h| h.link == NO_LINK));
    assert_eq!(page.link_index_at(bx, by), None);
}

#[test]
fn links_still_hit_with_element_boxes_on() {
    let mut doc = Doc::parse(b"<p>see <a href=/x>this link</a> now</p>");
    doc.set_track(true);
    let page = layout(&doc);
    let h = page
        .hits
        .iter()
        .find(|h| h.link != NO_LINK)
        .copied()
        .unwrap();
    assert_eq!(page.link_at(h.x + 1, h.y + 1), Some("/x"));
    assert!(page.node_at(h.x + 1, h.y + 1).is_some());
}

// ---- the script set ----

#[test]
fn script_set_fetches_externals_and_keeps_order() {
    let doc = Doc::parse(b"<script>one()</script><script src=two.js></script><script>three()</script><script src='//cdn.test/four.js'></script>");
    let mut s = ScriptSet::default();
    s.begin(&doc, BASE);
    assert_eq!(s.len(), 4);
    assert!(!s.ready(), "waits for the downloads");
    let a = s.next_pending().unwrap();
    assert_eq!(a, "http://example.test/dir/two.js");
    let b = s.next_pending().unwrap();
    assert_eq!(b, "http://cdn.test/four.js");
    assert_eq!(s.next_pending(), None);
    assert!(s.is_loading(&a));
    s.finish(&b, Some(b"four()"));
    assert!(!s.ready());
    s.finish(&a, None); // a failed download is skipped
    assert!(s.ready());
    let got = s.take();
    let names: Vec<&str> = got.iter().map(|g| g.0.as_str()).collect();
    assert_eq!(names, ["inline-1", "inline-2", "http://cdn.test/four.js"]);
    assert_eq!(got[2].1, "four()");
    assert!(!s.ready() && s.take().is_empty(), "scripts run once");
}

#[test]
fn script_set_without_scripts_is_never_ready() {
    let doc = Doc::parse(b"<p>no scripts</p>");
    let mut s = ScriptSet::default();
    s.begin(&doc, BASE);
    assert!(s.is_empty() && !s.ready() && !s.busy());
}

#[test]
fn a_page_with_media_gets_its_process_without_scripts() {
    let doc = Doc::parse(b"<video src=v.webm controls></video>");
    let mut s = ScriptSet::default();
    s.begin(&doc, BASE);
    assert!(s.is_empty() && s.ready());
    assert!(s.take().is_empty());
    assert!(!s.ready(), "handed over once");
}

#[test]
fn script_set_bounds_the_bytes() {
    let doc = Doc::parse(b"<script src=a.js></script><script src=b.js></script>");
    let mut s = ScriptSet::default();
    s.begin(&doc, BASE);
    let a = s.next_pending().unwrap();
    let b = s.next_pending().unwrap();
    s.finish(&a, Some(&alloc::vec![b'1'; MAX_SCRIPT_TOTAL - 10]));
    s.finish(&b, Some(&[b'2'; 100]));
    let got = s.take();
    assert_eq!(got.len(), 1, "the one past the budget is dropped");
}

// ---- messages ----

#[test]
fn init_message_carries_tree_ids_and_scripts() {
    let doc =
        Doc::parse(b"<!doctype html><title>T</title><p class=\"a&amp;b\">x \"y\"</p> <i>z</i>");
    let m = init_message(
        &doc,
        &InitInfo {
            cookie: "",
            url: "http://h/",
            lang: "pt-BR",
            width: 800,
            height: 600,
        },
        &[("inline-1".into(), "console.log(\"a\\n\")\n".into())],
    );
    let s = core::str::from_utf8(&m).unwrap();
    assert!(s.starts_with("{\"t\":\"init\",\"url\":\"http://h/\",\"ua\":\"Mozilla/5.0"));
    assert!(s.contains("\"w\":800,\"h\":600,\"quirks\":false,\"dom\":[[\"html\",1,{},["));
    assert!(
        s.contains("[\"p\",4,{\"class\":\"a&b\"},[\"x \\\"y\\\"\"]],\" \",[\"i\",5,{},[\"z\"]]"),
        "{s}"
    );
    assert!(s.ends_with(",\"scripts\":[[\"inline-1\",\"console.log(\\\"a\\\\n\\\")\\n\"]]}"));
}

#[test]
fn json_strings_escape_controls() {
    let mut out = String::new();
    json_str(&mut out, "a\u{1}b\u{2028}\\\"é");
    assert_eq!(out, "\"a\\u0001b\\u2028\\\\\\\"é\"");
}

#[test]
fn messages_from_the_process_are_checked() {
    assert_eq!(
        parse_message(b"D<p>x</p>"),
        FromScript::Document(b"<p>x</p>")
    );
    assert_eq!(
        parse_message(b"A7 1"),
        FromScript::Answer {
            seq: 7,
            allow: true
        }
    );
    assert_eq!(
        parse_message(b"A7 0"),
        FromScript::Answer {
            seq: 7,
            allow: false
        }
    );
    assert_eq!(parse_message(b"A7 2"), FromScript::Unknown);
    assert_eq!(parse_message(b"Ax 1"), FromScript::Unknown);
    assert_eq!(parse_message(b"A7 1 9"), FromScript::Unknown);
    assert_eq!(parse_message(b"N /next "), FromScript::Navigate("/next"));
    assert_eq!(parse_message(b"N "), FromScript::Unknown);
    assert_eq!(parse_message(&[b'N', 0xff]), FromScript::Unknown);
    assert_eq!(parse_message(b""), FromScript::Unknown);
    assert_eq!(parse_message(b"Zzz"), FromScript::Unknown);
}

// ---- clicks waiting for the page ----

#[test]
fn session_waits_for_the_verdict() {
    let mut s: Session<&str> = Session::default();
    let m = s.click(4, 10, 20, None, 0);
    assert_eq!(
        m,
        b"{\"t\":\"click\",\"uid\":4,\"x\":10,\"y\":20,\"seq\":0}"
    );
    assert!(!s.waiting());
    let m = s.click(4, 10, 20, Some("follow"), 1000);
    assert!(core::str::from_utf8(&m).unwrap().ends_with("\"seq\":1}"));
    assert!(s.waiting());
    assert_eq!(s.answer(2, true), None, "another click's verdict");
    assert_eq!(s.expire(1000 + ANSWER_TIMEOUT_MS - 1), None);
    assert_eq!(s.answer(1, true), Some("follow"));
    assert!(!s.waiting());
    // Prevented: nothing happens.
    s.click(4, 0, 0, Some("submit"), 5000);
    assert_eq!(s.answer(2, false), None);
    assert!(!s.waiting());
}

#[test]
fn session_gives_up_on_a_silent_page() {
    let mut s: Session<u8> = Session::default();
    s.click(1, 0, 0, Some(9), 100);
    assert_eq!(s.expire(100 + ANSWER_TIMEOUT_MS), Some(9));
    assert_eq!(s.answer(1, true), None, "too late");
    s.click(1, 0, 0, Some(8), 0);
    assert_eq!(s.abandon(), Some(8));
}

#[test]
fn controls_outside_forms_show_on_pages_with_scripts() {
    use crate::browsing::web::form::{FieldKind, FormError, FormState};
    let mut doc =
        Doc::parse(b"<button>Go</button><input name=q><form><button>Send</button></form>");
    assert_eq!(
        layout(&doc).fields.len(),
        1,
        "without scripts only the form's button"
    );
    doc.set_track(true);
    let page = layout(&doc);
    let kinds: Vec<FieldKind> = page.fields.iter().map(|f| f.kind).collect();
    assert_eq!(
        kinds,
        [FieldKind::PushButton, FieldKind::Text, FieldKind::Submit]
    );
    let loose = page.fields[0].form;
    assert!(page.forms[loose].formless);
    let st = FormState::new(&page.forms);
    assert_eq!(
        st.submission(&page.forms, loose, None).err(),
        Some(FormError::NoForm),
        "never submitted"
    );
}
