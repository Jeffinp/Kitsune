use super::lists::{AD_HOSTS, AD_PATHS, TRACKER_HOSTS, TRACKER_PATHS, contains_line};
use super::urlparts::{contains_ci, lower_host, split};
use super::*;
use crate::browsing::web::imgcache::{ImageCache, ImgState};
use crate::browsing::web::sheets::SheetSet;
use crate::browsing::web::{Cmd, Doc, FixedAdvance, Layout};
use alloc::string::String;
use alloc::vec::Vec;

const PAGE: &[u8] = b"news.example.org";

// ---- the data files ----

#[test]
fn host_lists_are_sorted_unique_lowercase_and_clean() {
    for (name, list) in [("ad", AD_HOSTS), ("tracker", TRACKER_HOSTS)] {
        assert!(list.ends_with('\n'), "{name}: no final newline");
        let lines: Vec<&str> = list.lines().collect();
        assert!(lines.len() >= 20, "{name}: too short");
        for w in lines.windows(2) {
            assert!(w[0] < w[1], "{name}: {:?} before {:?}", w[0], w[1]);
        }
        for l in &lines {
            assert!(
                !l.is_empty() && !l.contains('\r') && !l.contains(' '),
                "{name}: {l:?}"
            );
            assert_eq!(*l, l.to_ascii_lowercase(), "{name}: {l:?}");
            assert!(
                l.contains('.'),
                "{name}: a bare TLD would block a whole zone: {l:?}"
            );
        }
    }
}

#[test]
fn no_host_is_in_both_lists() {
    for l in AD_HOSTS.lines() {
        assert!(!contains_line(TRACKER_HOSTS, l.as_bytes()), "{l}");
    }
}

#[test]
fn path_rules_start_with_a_slash_and_are_lowercase() {
    for list in [AD_PATHS, TRACKER_PATHS] {
        assert!(list.lines().count() >= 5);
        for l in list.lines() {
            assert!(l.starts_with('/') && l.len() >= 4, "{l:?}");
            assert_eq!(l, l.to_ascii_lowercase());
            assert!(!l.contains('\r') && !l.contains(' '), "{l:?}");
        }
    }
}

#[test]
fn the_cosmetic_sheet_is_small_and_only_hides() {
    let css = crate::browsing::web::parse_css(COSMETIC_CSS);
    let selectors: usize = css.rules.iter().map(|r| r.selectors.len()).sum();
    assert!(
        selectors > 5 && selectors <= MAX_COSMETIC_SELECTORS,
        "{selectors}"
    );
    for r in &css.rules {
        assert!(
            r.decls
                .iter()
                .all(|d| d.name == "display" && d.value == "none" && d.important)
        );
    }
    // Generic enough to be safe: nothing matches every element or a bare common word.
    assert!(!COSMETIC_CSS.contains("[class*="));
}

#[test]
fn contains_line_finds_every_line_and_nothing_else() {
    for list in [AD_HOSTS, TRACKER_HOSTS] {
        for l in list.lines() {
            assert!(contains_line(list, l.as_bytes()), "{l}");
            // A longer or shorter neighbour is not a hit.
            let mut longer = String::from(l);
            longer.push('x');
            assert!(!contains_line(list, longer.as_bytes()));
            assert!(!contains_line(list, &l.as_bytes()[1..]));
            assert!(!contains_line(list, &l.as_bytes()[..l.len() - 1]));
        }
    }
    assert!(!contains_line(AD_HOSTS, b""));
    assert!(!contains_line("", b"a.com"));
    assert!(contains_line("a.com", b"a.com"));
    assert!(contains_line("a.com\nb.com", b"b.com"));
    assert!(!contains_line("a.com\nb.com", b"c.com"));
    assert!(!contains_line("b.com\nd.com\n", b"a.com"));
    assert!(!contains_line("b.com\nd.com\n", b"e.com"));
}

// ---- classifying a request ----

#[test]
fn ad_and_tracker_hosts_and_their_subdomains_are_blocked() {
    assert_eq!(
        classify(PAGE, b"https://doubleclick.net/x.png"),
        Some(Kind::Ad)
    );
    assert_eq!(
        classify(PAGE, b"https://ad.doubleclick.net/ddm/ad.gif"),
        Some(Kind::Ad)
    );
    assert_eq!(
        classify(PAGE, b"http://a.b.c.googlesyndication.com/pagead/x"),
        Some(Kind::Ad)
    );
    assert_eq!(
        classify(PAGE, b"https://www.google-analytics.com/collect"),
        Some(Kind::Tracker)
    );
    assert_eq!(
        classify(PAGE, b"https://connect.facebook.net/en_US/sdk.js"),
        Some(Kind::Tracker)
    );
}

#[test]
fn matching_ignores_case_port_userinfo_and_a_trailing_dot() {
    for u in [
        &b"HTTPS://ADS.DoubleClick.NET/x"[..],
        b"https://doubleclick.net:8443/x",
        b"https://user:pw@doubleclick.net/x",
        b"https://doubleclick.net./x",
        b"https://doubleclick.net",
        b"https://doubleclick.net?x=1",
        b"  https://doubleclick.net/x",
    ] {
        assert_eq!(
            classify(PAGE, u),
            Some(Kind::Ad),
            "{}",
            String::from_utf8_lossy(u)
        );
    }
}

#[test]
fn near_misses_are_not_blocked() {
    for u in [
        &b"https://notdoubleclick.net/x.png"[..],
        b"https://doubleclick.net.example.com/x.png",
        b"https://example.com/doubleclick.net/x.png",
        b"https://example.com/x.png",
        b"https://net/x.png",
        b"https://cdn.example.com/uploads/banner.png",
        b"https://example.com/downloads/ads.zip",
        b"https://example.com/roads/ad.png",
        b"https://google.com/search",
        b"https://www.googletagmanager.com.evil.example/x",
    ] {
        assert_eq!(classify(PAGE, u), None, "{}", String::from_utf8_lossy(u));
    }
}

#[test]
fn only_http_and_https_are_considered() {
    for u in [
        &b"data:image/png;base64,AAAA"[..],
        b"ftp://doubleclick.net/x",
        b"//doubleclick.net/x",
        b"doubleclick.net/x",
        b"",
        b"https://",
        b"https:///x",
        b"https://:80/x",
    ] {
        assert_eq!(classify(PAGE, u), None, "{}", String::from_utf8_lossy(u));
    }
}

#[test]
fn host_rules_leave_a_sites_own_requests_alone() {
    // The page is the ad host's own site: its own resources are first party.
    assert_eq!(
        classify(b"doubleclick.net", b"https://static.doubleclick.net/x.png"),
        None
    );
    assert_eq!(
        classify(b"www.doubleclick.net", b"https://doubleclick.net/x.png"),
        None
    );
    // Another site still gets it blocked.
    assert_eq!(
        classify(b"example.com", b"https://static.doubleclick.net/x.png"),
        Some(Kind::Ad)
    );
}

#[test]
fn path_rules_block_first_party_ads_and_pixels() {
    let local = b"203.0.113.5";
    assert_eq!(
        classify(local, b"http://203.0.113.5:8079/ads/banner-728.png"),
        Some(Kind::Ad)
    );
    assert_eq!(
        classify(local, b"http://203.0.113.5/PAGEAD/x.png"),
        Some(Kind::Ad)
    );
    assert_eq!(
        classify(local, b"http://203.0.113.5/tracking/pixel.png?id=7"),
        Some(Kind::Tracker)
    );
    assert_eq!(
        classify(local, b"http://203.0.113.5/x/analytics.js"),
        Some(Kind::Tracker)
    );
    assert_eq!(
        classify(local, b"http://203.0.113.5/g.png?ref=/pagead/"),
        Some(Kind::Ad)
    );
    // The fragment is not sent to the server, so it does not count.
    assert_eq!(classify(local, b"http://203.0.113.5/g.png#/pagead/"), None);
    assert_eq!(classify(local, b"http://203.0.113.5/g.png"), None);
    // The host part is not searched for path rules.
    assert_eq!(classify(local, b"http://pagead.example.com/g.png"), None);
}

#[test]
fn sites_are_registrable_domains() {
    let site = |h: &str| String::from_utf8(site_of(h.as_bytes()).to_vec()).unwrap();
    assert_eq!(site("example.com"), "example.com");
    assert_eq!(site("www.example.com"), "example.com");
    assert_eq!(site("a.b.example.com"), "example.com");
    assert_eq!(site("news.bbc.co.uk"), "bbc.co.uk");
    assert_eq!(site("www.uol.com.br"), "uol.com.br");
    assert_eq!(site("co.uk"), "co.uk");
    assert_eq!(site("localhost"), "localhost");
    assert_eq!(site("203.0.113.5"), "203.0.113.5");
    assert_eq!(site("[::1]"), "[::1]");
    assert!(!is_third_party(b"WWW.Example.com", b"cdn.example.COM"));
    assert!(is_third_party(b"example.com", b"example.org"));
    assert!(is_third_party(b"example.co.uk", b"other.co.uk"));
    assert!(is_third_party(b"203.0.113.5", b"203.0.113.6"));
    assert!(is_third_party(b"", b"example.com"));
}

#[test]
fn urls_split_into_host_and_rest() {
    let s = |u: &str| {
        split(u.as_bytes()).map(|(h, r)| {
            (
                String::from_utf8_lossy(h).into_owned(),
                String::from_utf8_lossy(r).into_owned(),
            )
        })
    };
    assert_eq!(s("http://a.com"), Some(("a.com".into(), "".into())));
    assert_eq!(
        s("https://a.com:81/p?q=1#f"),
        Some(("a.com".into(), "/p?q=1".into()))
    );
    assert_eq!(s("https://u@a.com/p"), Some(("a.com".into(), "/p".into())));
    assert_eq!(
        s("https://[::1]:8080/p"),
        Some(("[::1]".into(), "/p".into()))
    );
    assert_eq!(s("https://a.com#frag"), Some(("a.com".into(), "".into())));
    assert_eq!(s("mailto:a@b.c"), None);
    let mut buf = [0u8; 255];
    assert_eq!(lower_host(b"A.Com.", &mut buf), Some(&b"a.com"[..]));
    assert_eq!(lower_host(&[b'a'; 300], &mut buf), None);
    assert!(contains_ci(b"/Ads/Banner.PNG", b"/ads/banner"));
    assert!(!contains_ci(b"/ad", b"/ads/"));
}

#[test]
fn hostile_addresses_do_not_panic() {
    let long = alloc::vec![b'a'; 100_000];
    let mut u = b"https://".to_vec();
    u.extend_from_slice(&long);
    assert_eq!(classify(PAGE, &u), None);
    let mut u = b"https://x.com/".to_vec();
    u.extend_from_slice(&long);
    assert_eq!(classify(PAGE, &u), None);
    for bytes in [
        &[0xff, 0xfe, b':'][..],
        b"https://\xff\xfe.com/\x00",
        b"http://[",
        b"http://@@@@/",
    ] {
        let _ = classify(PAGE, bytes);
        let _ = classify(bytes, b"https://doubleclick.net/x");
    }
}

// ---- the page's blocker ----

#[test]
fn a_page_counts_each_blocked_address_once() {
    let mut s = PageShield::new(b"https://News.Example.org/article", true);
    assert!(s.is_on() && s.has_host());
    assert_eq!(s.host(), "news.example.org");
    assert_eq!(s.check(b"https://example.org/a.png"), None);
    assert_eq!(s.check(b"https://ad.doubleclick.net/a.png"), Some(Kind::Ad));
    assert_eq!(s.check(b"https://ad.doubleclick.net/a.png"), Some(Kind::Ad));
    assert_eq!(
        s.check(b"https://www.google-analytics.com/ga.js"),
        Some(Kind::Tracker)
    );
    s.add_hidden(2);
    let t = s.tally();
    assert_eq!((t.ads, t.trackers, t.hidden, t.total()), (1, 1, 2, 4));
    assert_eq!(s.blocked().len(), 2);
    assert_eq!(s.blocked()[0].1, Kind::Ad);
}

#[test]
fn deciding_asks_for_the_pages_host() {
    let mut asked = String::new();
    let s = PageShield::decide(b"https://WWW.Example.com:8443/p", |h| {
        asked = String::from(h);
        true
    });
    assert!(s.is_on());
    assert_eq!(asked, "www.example.com");
    assert!(!PageShield::decide(b"https://example.com/", |_| false).is_on());
    // No host: the question is not even asked, and the blocker stays off.
    let s = PageShield::decide(b"kitsune://inicio", |_| panic!("asked"));
    assert!(!s.is_on() && !s.has_host());
}

#[test]
fn an_off_blocker_lets_everything_through_and_counts_nothing() {
    let mut s = PageShield::new(b"https://news.example.org/", false);
    assert!(!s.is_on());
    assert_eq!(s.check(b"https://doubleclick.net/x.png"), None);
    assert_eq!(s.tally(), Tally::default());
    let mut s = PageShield::off();
    assert_eq!(s.check(b"https://doubleclick.net/x.png"), None);
    // A page with no host (internal, data) is off whatever was asked.
    assert!(!PageShield::new(b"kitsune://inicio", true).is_on());
    assert!(!PageShield::new(b"", true).has_host());
}

#[test]
fn the_log_is_bounded_but_the_count_keeps_going() {
    let mut s = PageShield::new(b"https://example.org/", true);
    for i in 0..(MAX_LOGGED + 10) {
        let u = alloc::format!("https://doubleclick.net/{i}.png");
        assert_eq!(s.check(u.as_bytes()), Some(Kind::Ad));
    }
    assert_eq!(s.blocked().len(), MAX_LOGGED);
    assert_eq!(s.tally().ads as usize, MAX_LOGGED + 10);
}

// ---- the sites list ----

#[test]
fn the_site_list_works_by_site_and_round_trips() {
    let mut l = SiteList::new();
    assert!(!l.is_off(b"www.example.com"));
    assert!(l.set_off(b"WWW.Example.com", true));
    assert!(!l.set_off(b"example.com", true), "already listed");
    assert!(l.is_off(b"example.com") && l.is_off(b"news.example.com") && l.is_off(b"EXAMPLE.com"));
    assert!(!l.is_off(b"example.org") && !l.is_off(b"notexample.com"));
    assert!(l.set_off(b"203.0.113.5", true));
    let back = SiteList::parse(&l.to_text());
    assert_eq!(back, l);
    assert_eq!(back.len(), 2);
    assert!(l.set_off(b"cdn.example.com", false));
    assert!(!l.is_off(b"example.com"));
    assert!(!l.set_off(b"example.com", false), "already on");
}

#[test]
fn the_site_list_parses_a_damaged_file_and_is_bounded() {
    let text = b"# c\n\nexample.com\r\nEXAMPLE.com\n  spaced.org  \nnot a host\n\xff\xfe\nhttp://x.com/\nbad_underscore.com\n";
    let l = SiteList::parse(text);
    assert_eq!(l.sites(), ["example.com", "spaced.org"]);
    let mut big = String::new();
    for i in 0..200 {
        big.push_str(&alloc::format!("site{i}.example\n"));
    }
    assert_eq!(SiteList::parse(big.as_bytes()).len(), MAX_SITES);
    let mut l = SiteList::new();
    for i in 0..(MAX_SITES + 5) {
        let h = alloc::format!("host{i}.test");
        assert_eq!(l.set_off(h.as_bytes(), true), i < MAX_SITES);
    }
    assert!(!l.set_off(&[b'a'; 300], true));
    assert!(!SiteList::new().is_off(&[0xff; 4]));
}

// ---- in the pipeline ----

fn lay(doc: &Doc, cache: &ImageCache) -> crate::browsing::web::Page {
    let base = b"http://203.0.113.5:8079/page";
    doc.layout(&Layout {
        width: 600,
        zoom: 100,
        images: &crate::browsing::web::imgcache::PageImages { cache, base },
        metrics: &FixedAdvance,
    })
}

#[test]
fn a_blocked_picture_has_no_box_and_the_others_keep_theirs() {
    let html = br#"<p>x</p><img src="/ads/banner-728.png" width=728 height=90>
        <img src="/g.png" width=100 height=50><img src="http://doubleclick.net/p.gif" width=1 height=1>"#;
    let doc = Doc::parse(html);
    let mut cache = ImageCache::new();
    let base = b"http://203.0.113.5:8079/page";
    let mut shield = PageShield::new(base, true);
    cache.begin_page();
    let page = lay(&doc, &cache);
    assert_eq!(page.images.len(), 3, "indices stay aligned with the layout");
    for r in &page.images {
        let key = crate::browsing::web::imgcache::image_key(base, &r.src).unwrap();
        if shield.check(key.as_bytes()).is_some() {
            cache.block(&key);
        } else {
            cache.want(&key, None);
        }
    }
    assert_eq!(shield.tally().ads, 2);
    let after = lay(&doc, &cache);
    assert_eq!(after.images.len(), 3);
    // Only the allowed picture is left: a pending 100x50 placeholder.
    let boxes: Vec<(i32, i32)> = after
        .cmds
        .iter()
        .filter_map(|c| match c {
            Cmd::Rect { w, h, .. } if *w == 100 && *h == 50 => Some((*w, *h)),
            _ => None,
        })
        .collect();
    assert_eq!(boxes.len(), 1);
    assert!(
        !after
            .cmds
            .iter()
            .any(|c| matches!(c, Cmd::Rect { w: 728, .. }))
    );
    assert!(after.height < page.height, "the ad boxes took room before");
    assert_eq!(
        cache.state("http://203.0.113.5:8079/ads/banner-728.png"),
        ImgState::Blocked
    );
    assert_eq!(
        cache.next_pending().map(|(k, _)| k).as_deref(),
        Some("http://203.0.113.5:8079/g.png")
    );
    assert_eq!(
        cache.next_pending(),
        None,
        "blocked pictures are never handed out"
    );
    // A new page forgets the blocked list.
    cache.begin_page();
    assert_ne!(
        cache.state("http://203.0.113.5:8079/ads/banner-728.png"),
        ImgState::Blocked
    );
}

#[test]
fn blocked_sheets_and_imports_are_never_queued() {
    let html = br#"<link rel=stylesheet href="/a.css"><link rel=stylesheet href="https://doubleclick.net/ads.css">
        <link rel=stylesheet href="/ads/banner.css"><style>@import url("https://www.googletagmanager.com/x.css");</style><p>x</p>"#;
    let mut doc = Doc::parse(html);
    let base = b"https://example.org/";
    let mut shield = PageShield::new(base, true);
    let mut set = SheetSet::default();
    set.begin_with(&doc, base, &mut shield);
    assert_eq!(
        set.next_pending().as_deref(),
        Some("https://example.org/a.css")
    );
    assert_eq!(set.next_pending(), None);
    assert_eq!(shield.tally().ads, 2);
    assert_eq!(shield.tally().trackers, 1);
    // The sheet's own @import is filtered too.
    assert!(set.finish_with(
        &mut doc,
        "https://example.org/a.css",
        Some("@import url(\"https://ad.doubleclick.net/more.css\"); @import \"b.css\"; p { color: red }"),
        &mut shield
    ));
    assert_eq!(
        set.next_pending().as_deref(),
        Some("https://example.org/b.css")
    );
    assert_eq!(set.next_pending(), None);
    assert_eq!(shield.tally().ads, 3);
}

#[test]
fn the_same_sheets_load_with_the_blocker_off() {
    let html = br#"<link rel=stylesheet href="https://doubleclick.net/ads.css"><link rel=stylesheet href="/ads/banner.css">"#;
    let doc = Doc::parse(html);
    let base = b"https://example.org/";
    let mut shield = PageShield::new(base, false);
    let mut set = SheetSet::default();
    set.begin_with(&doc, base, &mut shield);
    assert_eq!(
        set.next_pending().as_deref(),
        Some("https://doubleclick.net/ads.css")
    );
    assert_eq!(
        set.next_pending().as_deref(),
        Some("https://example.org/ads/banner.css")
    );
    assert_eq!(shield.tally(), Tally::default());
}

fn count_text(page: &crate::browsing::web::Page, needle: &str) -> usize {
    page.cmds
        .iter()
        .filter(|c| matches!(c, Cmd::Text { text, .. } if text.contains(needle)))
        .count()
}

#[test]
fn the_cosmetic_rules_hide_ad_boxes_and_count_the_outermost() {
    let html = br#"<body><p>real article text</p>
        <div class="ad-banner">buy now <span class="ad-slot">inner</span></div>
        <ins class="adsbygoogle">google ad</ins>
        <div id="div-gpt-ad-123">gpt</div><div id="google_ads_iframe_1">g2</div>
        <div class="headline">keep me</div><p class="loader">uploads</p></body>"#;
    let mut doc = Doc::parse(html);
    let before = lay(&doc, &ImageCache::new());
    assert_eq!(count_text(&before, "buy"), 1);
    let n = hide_ads(&mut doc);
    assert_eq!(n, 4, "banner (with its inner slot), ins, gpt, google_ads");
    let after = lay(&doc, &ImageCache::new());
    for gone in ["buy", "inner", "google ad", "gpt", "g2"] {
        assert_eq!(count_text(&after, gone), 0, "{gone}");
    }
    for kept in ["real", "keep", "uploads"] {
        assert_eq!(count_text(&after, kept), 1, "{kept}");
    }
    assert!(after.height < before.height);
}

#[test]
fn the_cosmetic_rules_win_over_the_pages_own_display() {
    let html = br#"<style>.ad-banner { display: block; }</style><div class="ad-banner">x ad</div><p>y</p>"#;
    let mut doc = Doc::parse(html);
    assert_eq!(hide_ads(&mut doc), 1);
    assert_eq!(count_text(&lay(&doc, &ImageCache::new()), "x ad"), 0);
}

#[test]
fn a_deep_hostile_page_is_counted_within_bounds() {
    let mut html = String::from("<body>");
    for _ in 0..300 {
        html.push_str("<div>");
    }
    html.push_str("<div class=\"ad-slot\">deep</div>");
    for _ in 0..50_000 {
        html.push_str("<i class=\"ad-slot\"></i>");
    }
    let mut doc = Doc::parse(html.as_bytes());
    let n = hide_ads(&mut doc);
    assert!(n <= crate::browsing::web::MAX_NODES as u32);
    let _ = lay(&doc, &ImageCache::new());
}
