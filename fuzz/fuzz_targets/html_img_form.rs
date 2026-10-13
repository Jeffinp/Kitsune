//! Fuzz target: the parts of the browser that read hostile page content beyond
//! plain text: `<img>` (sizes, `src`/`alt`, `data:` URIs, base64), `<form>`
//! controls and their editing / query encoding, the layout with images and
//! zoom, find in page, selection, and the image cache.
//!
//! Input layout: `[mode, zoom, viewport_lo, viewport_hi, ...bytes]`.
//!
//! * `mode & 3`: how the bytes are used as HTML: raw; inside `<img src=...>`
//!   / `alt=...` / `width=...`; inside `<form action=...><input name=... value=...>`;
//!   as a `data:image/png;base64,` payload.
//! * `mode >> 2`: the state every `<img>` is reported in (pending, ready with
//!   a size taken from the bytes, unsupported, failed, too big, too many, blocked by the ad blocker).
//! * `zoom`: mapped onto `[0, 400]` (the engine clamps to 50..=300).
//!
//! Invariants checked after every layout: no coordinate overflows, every
//! `Cmd::Image` has a positive box and a valid `idx`, every link hit and
//! control box refers to something that exists, the display list is bounded.
#![no_main]

use libfuzzer_sys::fuzz_target;
use kitsune_core::Key;
use kitsune_core::browser::{self, Browser, Conn};
use kitsune_core::base64;
use kitsune_core::web::form::{FormState, MAX_QUERY, MAX_VALUE};
use kitsune_core::web::imgcache::{
    self, ImageCache, ImageLookup, ImgFail, ImgState, Loaded, NoImages, PageImages,
};
use kitsune_core::web::{Cmd, Doc, FixedAdvance, Font, Layout, Page, TextMetrics};

const STACK: usize = 512 * 1024;

struct Fixed(ImgState);

impl ImageLookup for Fixed {
    fn lookup(&self, _src: &str) -> ImgState {
        self.0
    }
}

/// Metrics that misbehave: widths from the input, zero, or absurdly large. The layout must
/// stay bounded whatever the metrics say.
struct Weird(u8);

impl TextMetrics for Weird {
    fn width(&self, t: &str, f: Font) -> i32 {
        let n = t.chars().count() as i32;
        match self.0 % 5 {
            0 => 0,
            1 => n.saturating_mul(1_000_000),
            2 => i32::MAX,
            3 => n * i32::from(f.size) / 3,
            _ => -n,
        }
    }
    fn line_height(&self, f: Font) -> i32 {
        match self.0 % 3 {
            0 => 0,
            1 => -5,
            _ => i32::from(f.size) * 100_000,
        }
    }
    fn ascent(&self, f: Font) -> i32 {
        match self.0 % 3 {
            0 => 0,
            1 => i32::MIN,
            _ => i32::from(f.size) * 50,
        }
    }
}

fn state_from(sel: u8, a: u8, b: u8) -> ImgState {
    match sel % 7 {
        0 => ImgState::Pending,
        1 => ImgState::Ready {
            w: i32::from(a) * 40 + 1,
            h: i32::from(b) * 40 + 1,
        },
        2 => ImgState::Unsupported,
        3 => ImgState::Failed,
        4 => ImgState::TooBig,
        5 => ImgState::TooMany,
        _ => ImgState::Blocked,
    }
}

/// The ad blocker on hostile bytes: addresses, page hosts, the site list file, the cosmetic rules on
/// the page itself and the sheet queue with a blocking page. Never panics, counts stay sane.
fn shields_exercise(raw: &[u8], html: &str) {
    use kitsune_core::shields::{self, PageShield, SiteList};
    let _ = shields::classify(raw, raw);
    let mut url = b"https://".to_vec();
    url.extend_from_slice(raw);
    let _ = shields::classify(b"example.org", &url);
    let mut shield = PageShield::new(&url, true);
    let _ = shield.check(&url);
    let _ = shield.check(raw);
    assert!(shield.tally().ads + shield.tally().trackers <= 2);
    let mut sites = SiteList::parse(raw);
    sites.set_off(raw, true);
    assert_eq!(SiteList::parse(&sites.to_text()), sites);
    let mut doc = Doc::parse(html.as_bytes());
    let hidden = shields::hide_ads(&mut doc);
    shield.add_hidden(hidden);
    check_page(&doc.layout(&Layout {
        width: 400,
        zoom: 100,
        images: &NoImages,
        metrics: &FixedAdvance,
    }));
    let mut set = kitsune_core::web::sheets::SheetSet::default();
    set.begin_with(&doc, b"https://example.org/", &mut shield);
    let _ = set.finish_with(&mut doc, "https://example.org/a.css", Some(&html[..html.len().min(2000)]), &mut shield);
    let mut cache = ImageCache::new();
    cache.block(html);
    let _ = cache.state(html);
}

fn check_page(p: &Page) {
    assert!(p.height >= 0);
    assert!(p.cmds.len() <= 200_000);
    for c in &p.cmds {
        match c {
            Cmd::Image { w, h, idx, x, y } => {
                assert!(*w >= 1 && *h >= 1 && *x >= 0 && *y >= 0);
                assert!(*idx < p.images.len());
            }
            Cmd::Rect { x, y, w, h, .. } => assert!(*x >= 0 && *y >= 0 && *w >= 0 && *h >= 0, "rect {x} {y} {w} {h}"),
            Cmd::Border { w, h, .. } => assert!(*w >= 0 && *h >= 0),
            Cmd::Text { x, y, .. } => assert!(*x >= 0 && *y >= 0),
            Cmd::Video {
                x, y, w, h, poster, ..
            } => {
                assert!(*w >= 1 && *h >= 1 && *x >= 0 && *y >= 0);
                assert!(poster.is_none_or(|i| i < p.images.len()));
            }
        }
    }
    for h in &p.hits {
        assert!(h.link < p.links.len());
    }
    for f in &p.fields {
        let form = p.forms.get(f.form).expect("form index");
        assert!(f.field < form.fields.len());
        assert!(f.w >= 1 && f.h >= 1);
    }
}

fn exercise(data: &[u8]) {
    if data.len() < 4 {
        return;
    }
    let mode = data[0];
    let zoom = u32::from(data[1]) * 400 / 255;
    let viewport = i32::from(u16::from_le_bytes([data[2], data[3]]) % 3000);
    let raw = &data[4..];
    let text = String::from_utf8_lossy(raw).into_owned();
    let esc: String = text
        .chars()
        .filter(|c| !matches!(c, '\'' | '"' | '<' | '>'))
        .take(600)
        .collect();

    let html = match mode & 3 {
        0 => text.clone(),
        1 => format!(
            "<p>a <img src='{esc}' alt='{esc}' width='{esc}' height='{}'> b <a href='{esc}'><img src='{esc}'></a></p>",
            raw.len()
        ),
        2 => format!(
            "<form action='{esc}' method='{}'><input name='{esc}' value='{esc}' size='{}'><input type=hidden name=h value='{esc}'><button name=b value='{esc}'>{esc}</button><input type=password name=p></form>",
            if raw.len() % 2 == 0 { "get" } else { "post" },
            raw.len()
        ),
        _ => {
            let b64 = base64::encode(raw);
            format!("<img src='data:image/png;base64,{b64}' alt='x'><img src='data:image/png;base64,{esc}'>")
        }
    };

    // Shapes that reach the table, list, style-box and cascade code with the same bytes.
    let html = match data[3] >> 5 {
        1 => format!("<table border=1><tr><td>{html}</td><td colspan=3>{esc}</td></tr><tr><td rowspan=2>x</td></tr></table>"),
        2 => format!("<table><tr><td><table><tr><td><table><tr><td>{html}</td></tr></table></td></tr></table></td></tr></table>"),
        3 => format!("<style>{esc}{{color:red;margin:1em auto;border:1px solid}} .a>b,p+p{{padding:2px}}</style>{html}"),
        4 => format!("<div style=\"{esc}\"><span style=\"background:#f00;padding:0 4px;border-radius:3px\">{html}</span></div>"),
        5 => format!("<ul><li>{html}<ol><li>{html}</li></ol></li></ul><pre>{esc}\n  x</pre><blockquote>{html}</blockquote>"),
        6 => format!("<b><i><u><s><a href=x><code><small><big><sub><sup>{html}</sup></sub></big></small></code></a></s></u></i></b>"),
        _ => html,
    };

    let state = state_from(mode >> 2, raw.first().copied().unwrap_or(0), raw.get(1).copied().unwrap_or(0));
    shields_exercise(raw, &html);
    let doc = Doc::parse(html.as_bytes());
    let _ = doc.title();
    let mut page = None;
    for z in [zoom as u16, 50, 100, 300] {
        let lookups: [&dyn ImageLookup; 2] = [&Fixed(state), &NoImages];
        for l in lookups {
            let p = doc.layout(&Layout {
                width: viewport,
                zoom: z,
                images: l,
                metrics: &FixedAdvance,
            });
            check_page(&p);
            page = Some(p);
        }
    }
    // Misbehaving metrics: no checks on the geometry (it is garbage in, garbage out), only that
    // layout terminates without a panic or an overflow.
    let _ = doc.layout(&Layout {
        width: viewport,
        zoom: zoom as u16,
        images: &NoImages,
        metrics: &Weird(mode),
    });
    let page = page.expect("a page");

    // ---- forms: edit with the bytes as keys, then submit ----
    let mut st = FormState::new(&page.forms);
    for (fi, f) in page.forms.iter().enumerate() {
        for i in 0..f.fields.len() {
            if st.set_focus(&page.forms, fi, i) {
                for &b in raw.iter().take(64) {
                    let key = match b % 16 {
                        0 => Key::Backspace,
                        1 => Key::Delete,
                        2 => Key::Left,
                        3 => Key::Right,
                        4 => Key::Home,
                        5 => Key::End,
                        6 => Key::Tab,
                        7 => Key::Esc,
                        8 => Key::Enter,
                        _ => Key::Char(b),
                    };
                    let _ = st.on_key(&page.forms, key);
                    assert!(st.value(fi, i).len() <= MAX_VALUE);
                    assert!(st.value(fi, i).is_char_boundary(st.caret().min(st.value(fi, i).len())) || st.focus().is_none());
                }
                st.insert_str(&page.forms, &text);
                let _ = st.toggle(&page.forms, fi, i);
            }
            if let Ok(q) = st.query(&page.forms, fi, Some(i)) {
                assert!(q.len() <= MAX_QUERY + 2000);
                assert!(q.is_ascii());
            }
        }
        if let Ok(t) = st.target(&page.forms, fi, None) {
            // A GET puts the query in the address; a POST keeps it in the body.
            assert!(t.contains('?') || page.forms.get(fi).is_some_and(|f| f.post));
        }
        let _ = st.tab(&page.forms, mode & 1 == 0);
    }

    // ---- find / selection ----
    let _ = page.find(&text.chars().take(8).collect::<String>(), &FixedAdvance);
    let a = (i32::from(raw.first().copied().unwrap_or(0)) * 4, i32::from(raw.get(1).copied().unwrap_or(0)) * 4);
    let b = (i32::from(raw.get(2).copied().unwrap_or(9)) * 4, i32::from(raw.get(3).copied().unwrap_or(9)) * 8);
    if let Some(r) = page.select(a, b, &FixedAdvance) {
        let _ = page.selection_spans(&r, &FixedAdvance);
        let _ = page.selection_text(&r);
    }
    let _ = page.select_word(a.0, a.1, &FixedAdvance);

    // ---- decoders and the cache ----
    let _ = base64::decode(raw, 4096);
    let _ = base64::parse_data_uri(&text);
    let _ = base64::decode_data_image(&text, 4096);
    let _ = imgcache::decode_for_page(raw, viewport as usize);
    let _ = imgcache::decode_data_uri(&text, viewport as usize);
    let _ = imgcache::peek_dims(raw);
    let _ = imgcache::image_key(b"http://h.test/dir/page.html", &text);
    let _ = imgcache::image_key(b"https://h.test/", &text);

    let mut cache = ImageCache::new();
    for (i, &b) in raw.iter().take(200).enumerate() {
        let key = format!("k{}", b % 20);
        match b % 7 {
            0 => cache.begin_page(),
            1 | 2 => {
                cache.want(&key, None);
            }
            3 => {
                if let Some((k, _)) = cache.next_pending() {
                    let res = if b & 1 == 0 {
                        Ok(Loaded {
                            orig_w: 10,
                            orig_h: 10,
                            img: kitsune_core::image::Image::new(
                                (b as usize % 200) + 1,
                                (i % 150) + 1,
                                0xFF00_0000,
                            )
                            .expect("small image"),
                        })
                    } else {
                        Err(ImgFail::Failed)
                    };
                    cache.finish(&k, res);
                }
            }
            4 => {
                let _ = cache.fit_to(&key, b as usize % 300, i % 300);
            }
            5 => cache.requeue_loading(),
            _ => {
                let _ = cache.state(&key);
                let _ = cache.image(&key);
            }
        }
        assert!(cache.bytes() <= imgcache::CACHE_BYTES + 4 * 4096 * 4096 / 16);
        assert!(cache.len() <= imgcache::MAX_ENTRIES + 1);
    }
    let pi = PageImages {
        cache: &cache,
        base: b"http://h.test/",
    };
    let _ = pi.lookup(&text);

    // ---- tabs, tab text and the browser's own pages ----
    {
        use kitsune_core::browser::{Bookmark, pages, tabs};
        let (hs, he) = tabs::host_range(&text);
        assert!(hs <= he && he <= text.len() && text.is_char_boundary(hs) && text.is_char_boundary(he));
        let _ = tabs::host_of(&text);
        let _ = tabs::tab_title(&text, &text);
        let _ = tabs::tab_badge(&text, &text);
        let mut list = tabs::TabList::new(0usize);
        for (i, &b) in raw.iter().take(64).enumerate() {
            match b % 9 {
                0 | 1 => {
                    let _ = list.open(i);
                }
                2 => {
                    let _ = list.close(usize::from(b) % 10);
                }
                3 => {
                    let _ = list.select(usize::from(b) % 10);
                }
                4 => list.next(),
                5 => list.prev(),
                6 => {
                    let _ = list.select_number(usize::from(b) % 11);
                }
                _ => {
                    let _ = list.move_active(b & 1 == 0);
                }
            }
            assert!(list.len() >= 1 && list.len() <= tabs::MAX_TABS);
            assert!(list.active_index() < list.len());
        }
        let bm = [
            Bookmark { url: text.clone(), title: text.clone() },
            Bookmark { url: esc.clone(), title: String::new() },
        ];
        for html in [
            pages::bookmarks(&bm),
            pages::bookmarks(&[]),
            pages::history(&[raw.to_vec(), text.clone().into_bytes(), b"kitsune://sobre".to_vec()]),
            pages::about(),
        ] {
            check_page(&Doc::parse(&html).layout(&Layout {
                width: viewport,
                zoom: 100,
                images: &NoImages,
                metrics: &FixedAdvance,
            }));
        }
    }

    // ---- the browser model: address bar, history, favourites, suggestions, internal pages ----
    let mut br = Browser::new();
    for (i, &b) in raw.iter().take(120).enumerate() {
        match b % 10 {
            0 => br.open(&raw[i..]),
            1 => {
                let _ = br.take_request();
                br.loaded_with(Conn::Plain, b & 1 == 0);
            }
            2 | 3 => {
                let _ = br.on_key(Key::Char(b));
            }
            4 => {
                let _ = br.toggle_bookmark();
            }
            5 => br.select_bar(),
            6 => br.back(),
            7 => br.forward(),
            8 => {
                br.open(b"kitsune://historico");
                if let Some(h) = br.take_internal() {
                    let d = Doc::parse(&h);
                    check_page(&d.layout(&Layout {
                        width: 400,
                        zoom: 100,
                        images: &NoImages,
                        metrics: &FixedAdvance,
                    }));
                }
            }
            _ => {
                br.set_page_title(&text);
                let _ = br.on_key(if b & 1 == 0 { Key::Down } else { Key::Up });
                let _ = br.suggestions();
                let _ = br.open_link(&raw[i..]);
            }
        }
        assert!(br.history_len() <= browser::MAX_HISTORY);
        assert!(br.bookmarks().len() <= browser::MAX_BOOKMARKS);
        assert!(br.suggestions().len() <= browser::MAX_SUGGESTIONS);
    }
    let _ = browser::suggest(&text, &br.bookmarks(), &[text.clone()]);
    let _ = browser::html_escape(&text);
}

fuzz_target!(|data: &[u8]| {
    let data = data.to_vec();
    // A small stack, like the kernel's, so unbounded recursion is a crash here too.
    let h = std::thread::Builder::new()
        .stack_size(STACK)
        .spawn(move || exercise(&data))
        .expect("spawn");
    if let Err(e) = h.join() {
        std::panic::resume_unwind(e);
    }
});
