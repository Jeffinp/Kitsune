//! The user's input for the script process, the layout snapshot, `localStorage` and the messages of
//! the page that go with them.

use super::*;
use crate::browsing::web::form::{FieldKind, FormState, field_by_uid};
use crate::browsing::web::{FixedAdvance, Layout, STYLE_BOLD, STYLE_BOX, imgcache::NoImages};

fn layout(doc: &Doc) -> crate::browsing::web::Page {
    doc.layout(&Layout {
        width: 600,
        zoom: 100,
        images: &NoImages,
        metrics: &FixedAdvance,
    })
}

fn text(m: &[u8]) -> &str {
    core::str::from_utf8(m).unwrap()
}

#[test]
fn key_and_focus_messages() {
    let (key, code, kc) = dom_key(crate::Key::Char(b'A'));
    assert_eq!((key.as_str(), code, kc), ("A", "KeyA", 65));
    assert_eq!(dom_key(crate::Key::Enter).1, "Enter");
    assert_eq!(dom_key(crate::Key::Left).0, "ArrowLeft");
    assert_eq!(dom_key(crate::Key::Char(b'?')).1, "Slash");
    let m = key_message(
        &KeyInfo {
            key: "\"",
            code: "Quote",
            key_code: 0,
            mods: MOD_SHIFT,
            uid: 9,
            value: Some("a\"b"),
            seq: 3,
        },
        12,
    );
    assert_eq!(
        text(&m),
        r#"{"t":"key","k":"\"","c":"Quote","kc":0,"m":1,"uid":9,"seq":3,"n":12,"v":"a\"b"}"#
    );
    assert_eq!(text(&focus_message(4, 2)), r#"{"t":"focus","uid":4,"n":2}"#);
    assert_eq!(
        text(&field_message(4, FieldChange::Selected(2), 3)),
        r#"{"t":"field","uid":4,"n":3,"si":2}"#
    );
    assert_eq!(
        text(&field_message(4, FieldChange::Checked(true), 3)),
        r#"{"t":"field","uid":4,"n":3,"ck":1}"#
    );
    assert_eq!(
        text(&mouse_message(MouseKind::Wheel(-48), 7, 10, 20, 0, 5)),
        r#"{"t":"mouse","e":"wheel","uid":7,"x":10,"y":20,"b":0,"dy":-48,"m":0,"n":5}"#
    );
    assert_eq!(
        text(&store_message([("a", "1"), ("b\n", "x")].into_iter())),
        r#"{"t":"store","items":{"a":"1","b\n":"x"}}"#
    );
}

#[test]
fn view_is_in_css_pixels() {
    let v = View::from_device(1000, 600, 300, 3000, 200);
    assert_eq!(
        (v.w, v.h, v.sy, v.page_h, v.zoom),
        (500, 300, 150, 1500, 200)
    );
    assert_eq!(
        text(&view_message(&v)),
        r#"{"t":"view","w":500,"h":300,"sx":0,"sy":150,"pw":500,"ph":1500,"z":200}"#
    );
}

#[test]
fn inputs_are_acknowledged_in_order() {
    let mut i = Inputs::default();
    assert!(i.settled());
    let a = i.next();
    let b = i.next();
    assert!(!i.settled());
    i.ack(a);
    assert!(!i.settled());
    i.ack(b + 5);
    assert!(!i.settled(), "an input never sent is not acknowledged");
    i.ack(b);
    assert!(i.settled());
}

#[test]
fn page_messages_for_input_history_and_storage() {
    assert_eq!(parse_message(b"Q17"), FromScript::Ack(17));
    assert_eq!(parse_message(b"G0"), FromScript::Focus(0));
    assert_eq!(parse_message(b"Gx"), FromScript::Unknown);
    assert_eq!(
        parse_message(b"Wkey,move,bogus"),
        FromScript::Want(Subs(Subs::KEY | Subs::MOVE))
    );
    assert_eq!(parse_message(b"S0 120"), FromScript::Scroll(0, 120));
    assert_eq!(parse_message(b"S0 120 4"), FromScript::Unknown);
    assert_eq!(
        parse_message(b"Hpush http://a.test/x#y"),
        FromScript::History {
            replace: false,
            url: "http://a.test/x#y"
        }
    );
    assert_eq!(parse_message(b"Hjump http://a.test/"), FromScript::Unknown);
    assert_eq!(parse_message(b"B-1"), FromScript::Go(-1));
    assert_eq!(parse_message(b"B-1000"), FromScript::Unknown);
    assert_eq!(
        parse_message(b"Ohttp://a.test/n"),
        FromScript::Open("http://a.test/n")
    );
    assert_eq!(
        parse_message(
            alloc::format!(
                r#"L["set","k","v {b}"{b}u00e9{b}ud83d{b}ude00{b}n"]"#,
                b = '\\'
            )
            .as_bytes()
        ),
        FromScript::Storage(StorageOp::Set("k".into(), "v \"\u{e9}\u{1f600}\n".into()))
    );
    assert_eq!(
        parse_message(br#"L["del","k"]"#),
        FromScript::Storage(StorageOp::Remove("k".into()))
    );
    assert_eq!(
        parse_message(br#"L["clear"]"#),
        FromScript::Storage(StorageOp::Clear)
    );
    for bad in [
        &br#"L["set","k"]"#[..],
        br#"L["set",1,2]"#,
        br#"L["clear"]x"#,
        br#"L["#,
        b"L",
    ] {
        assert_eq!(parse_message(bad), FromScript::Unknown, "{bad:?}");
    }
}

#[test]
fn web_storage_quotas_and_file() {
    let mut s = WebStorage::default();
    let o = "http://a.test:8279";
    assert!(s.apply(o, StorageOp::Set("k".into(), "v".into())));
    assert!(s.apply(
        o,
        StorageOp::Set("tab\there".into(), "line\nbreak\\".into())
    ));
    assert!(s.apply("https://b.test", StorageOp::Set("x".into(), "1".into())));
    assert!(s.take_dirty());
    let big = "x".repeat(ORIGIN_BYTES);
    assert!(
        !s.apply(o, StorageOp::Set("big".into(), big)),
        "over the quota"
    );
    let file = s.to_text();
    let back = WebStorage::from_text(&file);
    let items: Vec<(&str, &str)> = back.items(o).collect();
    assert_eq!(items, [("k", "v"), ("tab\there", "line\nbreak\\")]);
    assert_eq!(back.items("https://b.test").count(), 1);
    let mut back = back;
    assert!(back.apply(o, StorageOp::Remove("k".into())));
    assert!(back.apply("https://b.test", StorageOp::Clear));
    assert_eq!(back.items(o).count(), 1);
    assert_eq!(back.items("https://b.test").count(), 0);
    assert_eq!(
        WebStorage::from_text("junk\nhttp://a\tk\tv\n")
            .items("http://a")
            .count(),
        0
    );
    assert_eq!(
        origin_of("http://a.test:8279/x?y").as_deref(),
        Some("http://a.test:8279")
    );
    assert_eq!(
        origin_of("https://a.test:443/").as_deref(),
        Some("https://a.test")
    );
    assert_eq!(origin_of("about:blank"), None);
}

#[test]
fn web_storage_keeps_at_most_max_origins() {
    let mut s = WebStorage::default();
    for i in 0..MAX_ORIGINS + 3 {
        let o = alloc::format!("http://h{i}.test");
        assert!(s.apply(&o, StorageOp::Set("k".into(), "v".into())));
    }
    assert_eq!(s.items("http://h0.test").count(), 0, "the oldest went");
    assert_eq!(
        s.items(&alloc::format!("http://h{}.test", MAX_ORIGINS + 2))
            .count(),
        1
    );
}

#[test]
fn controls_carry_their_element_and_text_can_be_set() {
    let mut doc =
        Doc::parse(b"<p><input name=a value=1><textarea>t</textarea><input type=checkbox></p>");
    doc.set_track(true);
    let page = layout(&doc);
    let uids: Vec<u32> = page.forms[0].fields.iter().map(|f| f.uid).collect();
    assert!(uids.iter().all(|&u| u != 0));
    assert_eq!(field_by_uid(&page.forms, uids[1]), Some((0, 1)));
    assert_eq!(field_by_uid(&page.forms, 0), None);
    let mut st = FormState::new(&page.forms);
    st.set_focus(&page.forms, 0, 0);
    st.set_text(&page.forms, 0, 0, "kept");
    assert_eq!((st.value(0, 0), st.caret()), ("kept", 4));
    st.set_caret(2);
    assert_eq!(st.caret(), 2);
    st.set_text(&page.forms, 0, 2, "x");
    assert_eq!(st.value(0, 2), "", "not a text control");
    assert_eq!(page.forms[0].fields[2].kind, FieldKind::Checkbox);
}

#[test]
fn boxes_snapshot_has_boxes_and_styles() {
    let mut doc = Doc::parse(
        b"<style>#a{font-weight:bold;margin:3px 4px;padding:5px;border:2px solid red;background:#102030}\
          #h{display:none}</style><div id=a>hello</div><p id=h>x</p><span id=s>t</span>",
    );
    doc.set_track(true);
    let page = layout(&doc);
    let v = View::from_device(600, 400, 0, page.height, 200);
    let m = boxes_message(&page, &v);
    assert_eq!(m[0], DATA_TAG);
    assert_eq!(&m[1..5], &BOXES_ID.to_le_bytes());
    let w: Vec<i32> = m[5..]
        .chunks(4)
        .map(|c| i32::from_le_bytes([c[0], c[1], c[2], c[3]]))
        .collect();
    assert_eq!(&w[..3], &[1, 300, 200]);
    let n = w[8] as usize;
    assert_eq!(w.len(), 9 + n * RECORD_WORDS);
    let uid_of = |id: &str| {
        let mut found = 0;
        fn walk(nodes: &[crate::browsing::web::dom::Node], id: &str, out: &mut u32) {
            for n in nodes {
                if let crate::browsing::web::dom::Node::Element(e) = n {
                    if e.attrs.get("id").map(String::as_str) == Some(id) {
                        *out = e.uid;
                    }
                    walk(&e.children, id, out);
                }
            }
        }
        walk(&doc.dom, id, &mut found);
        found
    };
    let rec = |uid: u32| {
        (0..n)
            .map(|i| &w[9 + i * RECORD_WORDS..9 + (i + 1) * RECORD_WORDS])
            .find(|r| r[0] as u32 == uid)
            .map(<[i32]>::to_vec)
    };
    let a = rec(uid_of("a")).expect("the div has a record");
    let packed = a[7] as u32;
    assert_eq!(packed >> 16, 16, "font px");
    assert_eq!((packed >> 11) & 31, 1, "display block");
    assert!(packed as u16 & STYLE_BOLD != 0 && packed as u16 & STYLE_BOX != 0);
    assert!(a[3] > 0 && a[4] > 0, "a box, in CSS px");
    assert_eq!(a[6] as u32, 0xff10_2030, "background");
    assert_eq!(
        (a[8] >> 16, (a[8] << 16) >> 16),
        (3, 4),
        "margin top, right"
    );
    assert_eq!(a[10] >> 16, 5, "padding top");
    assert_eq!(a[12] & 255, 2, "border top");
    let h = rec(uid_of("h")).expect("display:none is recorded");
    assert_eq!((h[7] as u32 >> 11) & 31, 0);
    assert_eq!(h[7] as u16 & STYLE_BOX, 0, "no box");
    let s = rec(uid_of("s")).expect("the span");
    assert_eq!((s[7] as u32 >> 11) & 31, 2, "inline");
}
