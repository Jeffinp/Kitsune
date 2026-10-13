use super::*;

#[test]
fn a_sibling_shares_favourites_and_allowed_hosts_but_not_history() {
    let mut a = browser_with_history(&["http://a.test/x"]);
    a.toggle_bookmark();
    let mut b = a.sibling();
    assert!(b.is_home(), "a new tab starts on the start page");
    assert_eq!(b.bookmarks().len(), 1, "favourites are the window's");
    assert_eq!(b.history_len(), 0, "history is the tab's");
    // A favourite added in one tab shows in the other.
    b.open(b"http://b.test/y");
    b.take_request();
    b.loaded_with(Conn::Plain, false);
    b.toggle_bookmark();
    assert_eq!(a.bookmarks().len(), 2);
    // So does the one-site-one-session certificate override.
    a.open(b"https://expired.example.com/");
    a.take_request();
    a.fail_with(FailReason::Cert(
        crate::network::tlsverify::CertError::Expired,
    ));
    a.continue_insecure();
    b.open(b"https://expired.example.com/");
    assert_eq!(
        b.insecure_host().as_deref(),
        Some(&b"expired.example.com"[..])
    );
}

#[test]
fn stop_drops_a_pending_load_and_keeps_the_page() {
    let mut b = browser_with_history(&["http://a.test/x"]);
    b.open(b"http://b.test/y");
    assert!(b.is_loading());
    b.stop();
    assert!(!b.is_loading());
    assert!(b.take_request().is_none(), "nothing left to fetch");
    assert_eq!(b.history_len(), 1);
    b.stop(); // nothing to stop: harmless
}

#[test]
fn recent_lists_each_address_once_newest_first_without_internal_pages() {
    let mut b = browser_with_history(&["http://a.test/", "http://b.test/", "http://a.test/"]);
    b.open(b"kitsune://sobre");
    let r = b.recent(10);
    assert_eq!(r, ["http://a.test/", "http://b.test/"]);
    assert_eq!(b.recent(1), ["http://a.test/"]);
    assert!(Browser::new().recent(5).is_empty());
}

#[test]
fn reload_refetches_the_navigated_address_not_the_typed_text() {
    let mut b = Browser::new();
    b.open(b"https://example.com/a");
    assert!(b.take_request().is_some());
    type_str(&mut b, "typed junk");
    b.reload();
    assert_eq!(b.take_request(), Some(&b"https://example.com/a"[..]));
}

#[test]
fn a_post_request_carries_its_body_and_length() {
    let mut r = Vec::new();
    build_request(
        &mut r,
        Lang::En,
        "example.com",
        "/p",
        80,
        false,
        Some(b"a=1&b=2"),
    );
    let t = String::from_utf8(r).unwrap();
    assert!(t.starts_with("POST /p HTTP/1.1\r\nHost: example.com\r\n"));
    assert!(t.contains("\r\nContent-Type: application/x-www-form-urlencoded\r\n"));
    assert!(t.contains("\r\nContent-Length: 7\r\n"));
    assert!(t.ends_with("\r\n\r\na=1&b=2"));
}

#[test]
fn open_post_navigates_with_a_body_that_reload_and_back_drop() {
    let mut b = Browser::new();
    b.open(b"https://example.com/form");
    assert!(b.take_request().is_some());
    b.loaded();
    assert!(b.open_post(b"/send?x=1", b"k=v".to_vec()));
    assert_eq!(b.take_request(), Some(&b"https://example.com/send?x=1"[..]));
    assert_eq!(b.post_body(), Some(&b"k=v"[..]));
    b.loaded();
    b.reload();
    assert!(b.take_request().is_some());
    assert_eq!(b.post_body(), None);
    assert!(!b.open_post(b"javascript:x", Vec::new()));
}

#[test]
fn the_request_asks_for_the_language_of_the_interface() {
    let ask = |l: Lang, host: &str, port: u16, tls: bool| {
        let mut r = Vec::new();
        build_get_request(&mut r, l, host, "/a?b=1", port, tls);
        String::from_utf8(r).unwrap()
    };
    let pt = ask(Lang::Pt, "example.com", 443, true);
    assert!(pt.starts_with("GET /a?b=1 HTTP/1.1\r\nHost: example.com\r\n"));
    assert!(pt.contains("\r\nAccept-Language: pt-BR,pt;q=0.9,en;q=0.8\r\n"));
    assert!(pt.ends_with("Connection: close\r\n\r\n"));
    assert!(pt.contains(concat!(
        "\r\nUser-Agent: Mozilla/5.0 (X11; Kitsune x86_64) Kitsune/",
        env!("CARGO_PKG_VERSION"),
        "\r\n"
    )));
    let en = ask(Lang::En, "example.com", 80, false);
    assert!(en.contains("\r\nAccept-Language: en;q=1\r\n"));
    assert!(!en.contains("pt-BR"));
    // The port shows only when it is not the default of the scheme.
    assert!(ask(Lang::En, "h.test", 8080, false).contains("Host: h.test:8080\r\n"));
    assert!(ask(Lang::En, "h.test", 8080, true).contains("Host: h.test:8080\r\n"));
    assert!(ask(Lang::En, "h.test", 443, false).contains("Host: h.test:443\r\n"));
    assert!(ask(Lang::En, "h.test", 443, true).contains("Host: h.test\r\n"));
    // One header line each, in a fixed place.
    for l in Lang::ALL {
        let r = ask(l, "x.test", 80, false);
        assert_eq!(r.matches("Accept-Language:").count(), 1);
        assert_eq!(r.matches("\r\n").count(), 8);
    }
}

#[test]
fn labels_and_banners_have_both_languages() {
    for s in [
        Security::Http,
        Security::HttpsVerified,
        Security::HttpsInvalid,
    ] {
        let k = s.label_key().unwrap();
        assert_ne!(i18n::tr_in(Lang::Pt, k), i18n::tr_in(Lang::En, k));
    }
    for n in [
        PageNote::Truncated,
        PageNote::Incomplete,
        PageNote::Damaged,
        PageNote::BadChecksum,
    ] {
        for l in Lang::ALL {
            let t = i18n::tr_in(l, n.label_key());
            assert!(!t.is_empty() && !t.starts_with("web."), "{l:?} {n:?}");
        }
    }
    for (k, _) in QUICK_LINKS {
        assert!(!i18n::tr_in(Lang::En, k).starts_with("web."), "{k}");
    }
    assert_eq!(i18n::tr_in(Lang::Pt, QUICK_LINKS[3].0), "Exemplo");
    assert_eq!(i18n::tr_in(Lang::En, QUICK_LINKS[3].0), "Example");
}

#[test]
fn an_internal_page_can_be_built_again_in_another_language() {
    let mut b = Browser::new();
    assert!(b.internal_html_in(Lang::En).is_none());
    b.open(b"kitsune://sobre");
    let first = b.take_internal().unwrap();
    assert!(b.take_internal().is_none(), "delivered once");
    let en = String::from_utf8(b.internal_html_in(Lang::En).unwrap()).unwrap();
    let pt = String::from_utf8(b.internal_html_in(Lang::Pt).unwrap()).unwrap();
    assert!(en.contains("<html lang=\"en\">") && en.contains("Shortcuts"));
    assert!(pt.contains("<html lang=\"pt-BR\">") && pt.contains("Atalhos"));
    assert_eq!(first, pt.into_bytes(), "the default language is Portuguese");
}
