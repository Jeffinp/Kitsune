use super::*;
use alloc::format;
use alloc::string::ToString;

const NOW: i64 = 1_790_000_000_000; // 2026-09

fn site<'a>(https: bool, host: &'a str, path: &'a str) -> Site<'a> {
    Site { https, host, path }
}

fn nav() -> Context {
    Context::NAVIGATION
}

#[test]
fn set_and_send() {
    let mut j = Jar::new();
    let s = site(true, "www.example.com", "/a/b");
    assert!(j.set_from_header(s, "sid=abc; Path=/; HttpOnly; Secure", nav(), NOW));
    assert!(j.set_from_header(s, "theme=dark", nav(), NOW));
    assert_eq!(j.header_for(s, nav(), NOW), "theme=dark; sid=abc");
    // The default path is the directory: /a.
    assert_eq!(
        j.header_for(site(true, "www.example.com", "/"), nav(), NOW),
        "sid=abc"
    );
    assert_eq!(
        j.header_for(site(true, "www.example.com", "/a"), nav(), NOW),
        "theme=dark; sid=abc"
    );
    assert_eq!(
        j.header_for(site(true, "www.example.com", "/ab"), nav(), NOW),
        "sid=abc"
    );
    // Not to another host, not over http (Secure).
    assert_eq!(
        j.header_for(site(true, "example.com", "/a/b"), nav(), NOW),
        ""
    );
    assert_eq!(
        j.header_for(site(false, "www.example.com", "/a/b"), nav(), NOW),
        "theme=dark"
    );
}

#[test]
fn longer_paths_first_then_older() {
    let mut j = Jar::new();
    let s = site(true, "h.com", "/");
    j.set_from_header(s, "a=1; Path=/", nav(), NOW);
    j.set_from_header(s, "b=2; Path=/x/y", nav(), NOW);
    j.set_from_header(s, "c=3; Path=/x", nav(), NOW);
    j.set_from_header(s, "d=4; Path=/", nav(), NOW);
    assert_eq!(
        j.header_for(site(true, "h.com", "/x/y/z"), nav(), NOW),
        "b=2; c=3; a=1; d=4"
    );
}

#[test]
fn domain_attribute() {
    let mut j = Jar::new();
    let s = site(true, "www.example.com", "/");
    assert!(j.set_from_header(s, "a=1; Domain=.Example.com", nav(), NOW));
    assert_eq!(
        j.header_for(site(true, "api.example.com", "/"), nav(), NOW),
        "a=1"
    );
    assert_eq!(
        j.header_for(site(true, "example.com", "/"), nav(), NOW),
        "a=1"
    );
    assert_eq!(
        j.header_for(site(true, "badexample.com", "/"), nav(), NOW),
        ""
    );
    // A suffix, another site or a sibling is refused.
    assert!(!j.set_from_header(s, "b=1; Domain=com", nav(), NOW));
    assert!(!j.set_from_header(s, "b=1; Domain=other.com", nav(), NOW));
    assert!(!j.set_from_header(s, "b=1; Domain=api.example.com", nav(), NOW));
    let uk = site(true, "www.shop.co.uk", "/");
    assert!(!j.set_from_header(uk, "b=1; Domain=co.uk", nav(), NOW));
    assert!(j.set_from_header(uk, "b=1; Domain=shop.co.uk", nav(), NOW));
    // An IP address: only itself.
    let ip = site(false, "203.0.113.5", "/");
    assert!(!j.set_from_header(ip, "c=1; Domain=113.5", nav(), NOW));
    assert!(j.set_from_header(ip, "c=1; Domain=203.0.113.5", nav(), NOW));
    assert_eq!(j.header_for(ip, nav(), NOW), "c=1");
}

#[test]
fn replace_and_delete() {
    let mut j = Jar::new();
    let s = site(true, "h.com", "/");
    j.set_from_header(s, "a=1; Max-Age=100", nav(), NOW);
    j.set_from_header(s, "a=2; Max-Age=100", nav(), NOW);
    assert_eq!(j.len(), 1);
    assert_eq!(j.header_for(s, nav(), NOW), "a=2");
    assert!(j.set_from_header(s, "a=; Max-Age=0", nav(), NOW));
    assert_eq!(j.len(), 0);
    j.set_from_header(s, "b=1", nav(), NOW);
    j.set_from_header(s, "b=1; Expires=Thu, 01 Jan 1970 00:00:00 GMT", nav(), NOW);
    assert!(j.is_empty());
}

#[test]
fn expiry() {
    let mut j = Jar::new();
    let s = site(true, "h.com", "/");
    j.set_from_header(s, "a=1; Max-Age=10", nav(), NOW);
    j.set_from_header(
        s,
        "b=1; Expires=Wed, 21 Oct 2099 07:28:00 GMT; Max-Age=20",
        nav(),
        NOW,
    );
    j.set_from_header(s, "c=1; expires=Wed, 21-Oct-2099 07:28:00 GMT", nav(), NOW);
    assert_eq!(j.header_for(s, nav(), NOW + 5_000), "a=1; b=1; c=1");
    assert_eq!(j.header_for(s, nav(), NOW + 15_000), "b=1; c=1");
    assert_eq!(j.header_for(s, nav(), NOW + 25_000), "c=1");
    // Capped at 400 days.
    let c = &j.cookies()[0];
    assert!(c.expires.unwrap() <= NOW + MAX_AGE_MS);
}

#[test]
fn cookie_dates() {
    let d = parse_cookie_date("Wed, 21 Oct 2015 07:28:00 GMT").unwrap();
    assert_eq!(d, 1_445_412_480_000);
    assert_eq!(
        parse_cookie_date("Wednesday, 21-Oct-15 07:28:00 GMT"),
        Some(d)
    );
    assert_eq!(parse_cookie_date("Wed Oct 21 07:28:00 2015"), Some(d));
    assert_eq!(
        parse_cookie_date("21 Oct 1970 00:00:00"),
        Some(25_315_200_000)
    );
    assert_eq!(parse_cookie_date("31 Feb 2020 00:00:00"), None);
    assert_eq!(parse_cookie_date("tomorrow"), None);
    assert_eq!(parse_cookie_date("21 Oct 2015 25:00:00"), None);
}

#[test]
fn http_only_hidden_from_scripts() {
    let mut j = Jar::new();
    let s = site(true, "h.com", "/");
    j.set_from_header(s, "sid=1; HttpOnly", nav(), NOW);
    j.set_from_header(s, "ui=2", nav(), NOW);
    assert_eq!(j.document_get(s, NOW), "ui=2");
    // A script cannot set HttpOnly, nor overwrite an HttpOnly cookie.
    assert!(!j.document_set(s, "x=1; HttpOnly", NOW));
    assert!(!j.document_set(s, "sid=hacked", NOW));
    assert!(j.document_set(s, "lang=pt; Path=/", NOW));
    assert_eq!(j.document_get(s, NOW), "ui=2; lang=pt");
    assert_eq!(j.header_for(s, nav(), NOW), "sid=1; ui=2; lang=pt");
}

#[test]
fn same_site_rules() {
    let mut j = Jar::new();
    let s = site(true, "a.com", "/");
    j.set_from_header(s, "strict=1; SameSite=Strict", nav(), NOW);
    j.set_from_header(s, "lax=1; SameSite=Lax", nav(), NOW);
    j.set_from_header(s, "def=1", nav(), NOW);
    j.set_from_header(s, "none=1; SameSite=None; Secure", nav(), NOW);
    assert!(!j.set_from_header(s, "bad=1; SameSite=None", nav(), NOW));
    let cross_sub = Context::subresource(false, true);
    assert_eq!(j.header_for(s, cross_sub, NOW), "none=1");
    let cross_nav = Context {
        same_site: false,
        top_level: true,
        safe_method: true,
        http: true,
    };
    assert_eq!(j.header_for(s, cross_nav, NOW), "lax=1; def=1; none=1");
    let cross_post = Context {
        safe_method: false,
        ..cross_nav
    };
    assert_eq!(j.header_for(s, cross_post, NOW), "none=1");
    assert_eq!(
        j.header_for(s, Context::subresource(true, false), NOW),
        "strict=1; lax=1; def=1; none=1"
    );
    // A cross-site response sets only SameSite=None cookies.
    let mut k = Jar::new();
    assert!(!k.set_from_header(s, "t=1", cross_sub, NOW));
    assert!(!k.set_from_header(s, "t=1; SameSite=Lax", cross_sub, NOW));
    assert!(k.set_from_header(s, "t=1; SameSite=None; Secure", cross_sub, NOW));
}

#[test]
fn secure_rules_and_prefixes() {
    let mut j = Jar::new();
    let plain = site(false, "h.com", "/");
    let tls = site(true, "h.com", "/");
    assert!(!j.set_from_header(plain, "a=1; Secure", nav(), NOW));
    assert!(j.set_from_header(tls, "a=1; Secure", nav(), NOW));
    // http: may not shadow a Secure cookie.
    assert!(!j.set_from_header(plain, "a=evil", nav(), NOW));
    assert!(!j.set_from_header(tls, "__Secure-x=1", nav(), NOW));
    assert!(j.set_from_header(tls, "__Secure-x=1; Secure", nav(), NOW));
    assert!(!j.set_from_header(tls, "__Host-y=1; Secure; Path=/a", nav(), NOW));
    assert!(!j.set_from_header(tls, "__Host-y=1; Secure; Domain=h.com", nav(), NOW));
    assert!(j.set_from_header(tls, "__Host-y=1; Secure; Path=/", nav(), NOW));
}

#[test]
fn malformed_values_are_refused() {
    let mut j = Jar::new();
    let s = site(true, "h.com", "/");
    for bad in ["novalue", "=x", "a\tb=1", "a=b\u{7}", " =1", "a b=1"] {
        assert!(!j.set_from_header(s, bad, nav(), NOW), "{bad:?}");
    }
    let big = format!("a={}", "x".repeat(MAX_COOKIE_BYTES));
    assert!(!j.set_from_header(s, &big, nav(), NOW));
    // Unknown attributes and odd spacing are fine.
    assert!(j.set_from_header(s, " k = v ; foo ; Path = /p ", nav(), NOW));
    assert_eq!(j.header_for(site(true, "h.com", "/p"), nav(), NOW), "k=v");
}

#[test]
fn limits_evict_the_oldest() {
    let mut j = Jar::new();
    let s = site(true, "h.com", "/");
    for i in 0..(MAX_PER_DOMAIN + 5) {
        j.set_from_header(s, &format!("c{i}=1; Max-Age=999"), nav(), NOW);
    }
    assert_eq!(j.len(), MAX_PER_DOMAIN);
    assert!(!j.header_for(s, nav(), NOW).contains("c0="));
    assert!(
        j.header_for(s, nav(), NOW)
            .contains(&format!("c{}=", MAX_PER_DOMAIN + 4))
    );
    let mut j = Jar::new();
    for d in 0..(MAX_COOKIES / 10 + 3) {
        let host = format!("h{d}.com");
        for i in 0..10 {
            j.set_from_header(site(true, &host, "/"), &format!("c{i}=1"), nav(), NOW);
        }
    }
    assert_eq!(j.len(), MAX_COOKIES);
    assert_eq!(j.header_for(site(true, "h0.com", "/"), nav(), NOW), "");
}

#[test]
fn text_round_trip() {
    let mut j = Jar::new();
    let s = site(true, "www.h.com", "/x");
    j.set_from_header(
        s,
        "keep=1; Max-Age=3600; Domain=h.com; Path=/; Secure; HttpOnly; SameSite=Strict",
        nav(),
        NOW,
    );
    j.set_from_header(s, "session=1", nav(), NOW);
    j.set_from_header(s, "soon=1; Max-Age=5", nav(), NOW);
    assert!(j.take_dirty());
    assert!(!j.take_dirty());
    let text = j.to_text(NOW);
    let back = Jar::parse_text(&text, NOW + 10_000);
    assert_eq!(back.len(), 1);
    let c = &back.cookies()[0];
    assert_eq!(
        (
            c.name.as_str(),
            c.domain.as_str(),
            c.host_only,
            c.secure,
            c.http_only,
            c.same_site
        ),
        ("keep", "h.com", false, true, true, SameSite::Strict)
    );
    // Damaged lines are skipped.
    let hostile = b"# x\nh.com\t1\t/\t0\t0\tU\t99999999999999\ta\tb\nbad line\nh.com\t2\t/\t0\t0\tU\t1\ta\tb\n\xff\xfe";
    assert_eq!(Jar::parse_text(hostile, NOW).len(), 0);
    let ok = b"h.com\t1\t/\t0\t0\tU\t1890000000000\ta\tb\n";
    assert_eq!(Jar::parse_text(ok, NOW).len(), 1);
}

#[test]
fn helpers() {
    assert!(path_match("/a/b", "/a"));
    assert!(path_match("/a/", "/a/"));
    assert!(!path_match("/ab", "/a"));
    assert_eq!(default_path("/a/b/c"), "/a/b");
    assert_eq!(default_path("/a"), "/");
    assert_eq!(default_path("x"), "/");
    assert!(domain_match("a.b.com", "b.com"));
    assert!(!domain_match("ab.com", "b.com"));
    assert!(!domain_match("1.2.3.4", "3.4"));
    assert!(same_site("a.example.com", "b.example.com"));
    assert!(!same_site("a.com", "b.com"));
    let _ = "x".to_string();
}
