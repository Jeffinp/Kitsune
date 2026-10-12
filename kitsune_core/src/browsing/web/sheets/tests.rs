use super::*;
use crate::web::{Layout, imgcache::NoImages, metrics::FixedAdvance};

const BASE: &[u8] = b"https://example.com/docs/page.html";

fn first_color(doc: &Doc) -> crate::web::Rgb {
    let m = FixedAdvance;
    let p = doc.layout(&Layout {
        width: 600,
        zoom: 100,
        images: &NoImages,
        metrics: &m,
    });
    p.cmds
        .iter()
        .find_map(|c| match c {
            crate::web::Cmd::Text { color, .. } => Some(*color),
            _ => None,
        })
        .unwrap()
}

#[test]
fn links_and_imports_are_queued_in_order_and_resolved() {
    let html = br#"<html><head>
        <link rel="stylesheet" href="/a.css">
        <link rel="alternate stylesheet" href="skip.css">
        <link rel="stylesheet" href="b.css" media="print">
        <link rel="icon" href="x.ico">
        <link rel="stylesheet" href="b.css?v=1&amp;w=2" media="screen">
        <style>@import "c.css";</style></head><body><p>x</p></body></html>"#;
    let doc = Doc::parse(html);
    assert_eq!(doc.linked_sheets(), ["/a.css", "b.css?v=1&w=2", "c.css"]);
    let mut set = SheetSet::default();
    set.begin(&doc, BASE);
    assert_eq!(
        set.next_pending().as_deref(),
        Some("https://example.com/a.css")
    );
    assert_eq!(
        set.next_pending().as_deref(),
        Some("https://example.com/docs/b.css?v=1&w=2")
    );
    assert_eq!(
        set.next_pending().as_deref(),
        Some("https://example.com/docs/c.css")
    );
    assert_eq!(set.next_pending(), None);
    assert!(set.busy());
}

#[test]
fn a_fetched_sheet_restyles_the_page_and_the_page_still_wins() {
    let mut doc = Doc::parse(
        br#"<link rel="stylesheet" href="s.css"><style>.own{color:#00ff00}</style>
        <p class="a">text</p>"#,
    );
    assert_eq!(first_color(&doc), crate::web::Rgb(0x1d, 0x1d, 0x1f));
    let mut set = SheetSet::default();
    set.begin(&doc, BASE);
    let key = set.next_pending().unwrap();
    assert!(set.finish(&mut doc, &key, Some("p{color:#ff0000} .own{color:#0000ff}")));
    assert_eq!(first_color(&doc), crate::web::Rgb(255, 0, 0));
    assert!(!set.busy());
    assert_eq!(set.done(), 1);
}

#[test]
fn imports_of_a_sheet_are_relative_to_it_and_bounded() {
    let mut doc = Doc::parse(br#"<link rel="stylesheet" href="css/main.css"><p>x</p>"#);
    let mut set = SheetSet::default();
    set.begin(&doc, BASE);
    let key = set.next_pending().unwrap();
    assert_eq!(key, "https://example.com/docs/css/main.css");
    set.finish(
        &mut doc,
        &key,
        Some("@import url(base/reset.css); p{margin:0}"),
    );
    assert_eq!(
        set.next_pending().as_deref(),
        Some("https://example.com/docs/css/base/reset.css")
    );
    // A sheet that imports itself or many others cannot grow the queue past the limit.
    let mut doc = Doc::parse(br#"<link rel="stylesheet" href="a.css"><p>x</p>"#);
    set.begin(&doc, BASE);
    let key = set.next_pending().unwrap();
    let many: String = (0..40)
        .map(|i| alloc::format!("@import 'i{i}.css';"))
        .collect();
    set.finish(&mut doc, &key, Some(&many));
    let mut n = 0;
    while set.next_pending().is_some() {
        n += 1;
    }
    assert!(n < MAX_SHEETS);
}

#[test]
fn failures_unknown_keys_and_the_byte_limit() {
    let mut doc = Doc::parse(
        br#"<link rel="stylesheet" href="a.css"><link rel="stylesheet" href="b.css"><p>x</p>"#,
    );
    let mut set = SheetSet::default();
    set.begin(&doc, BASE);
    let a = set.next_pending().unwrap();
    assert!(!set.finish(&mut doc, &a, None));
    // Not handed out: ignored.
    assert!(!set.finish(
        &mut doc,
        "https://example.com/docs/zzz.css",
        Some("p{color:red}")
    ));
    let b = set.next_pending().unwrap();
    let big = alloc::format!("/*{}*/p{{color:red}}", "x".repeat(MAX_TOTAL));
    assert!(!set.finish(&mut doc, &b, Some(&big)));
    assert!(!set.busy());
}

#[test]
fn mixed_content_and_data_links_are_not_fetched() {
    let doc = Doc::parse(br#"<link rel="stylesheet" href="http://example.com/a.css"><link rel="stylesheet" href="data:text/css,p{}"><p>x</p>"#);
    let mut set = SheetSet::default();
    set.begin(&doc, BASE); // an https page
    assert_eq!(set.next_pending(), None);
}
