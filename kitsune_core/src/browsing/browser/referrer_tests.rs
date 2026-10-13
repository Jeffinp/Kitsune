//! The `Referer` of a navigation (and so `document.referrer`): a link or a form of the page sends it
//! (`strict-origin-when-cross-origin`), a reload repeats it, an address typed has none. YouTube's embedded
//! player, for one, refuses to play when it is opened without one ("Error 153").

use super::*;

fn load(b: &mut Browser, url: &[u8]) {
    b.open(url);
    assert!(b.take_request().is_some());
    b.loaded_with(Conn::Verified, false);
}

#[test]
fn links_send_the_page_as_referrer() {
    let mut b = Browser::new();
    load(&mut b, b"https://site.example/dir/page.html?q=1#frag");
    assert_eq!(b.referrer(), None, "an address typed has none");
    // The same origin: the whole address, without the fragment.
    assert!(b.open_link(b"other.html"));
    assert_eq!(b.referrer(), Some("https://site.example/dir/page.html?q=1"));
    let _ = b.take_request();
    b.loaded_with(Conn::Verified, false);
    // Another origin: only the origin.
    assert!(b.open_link(b"https://www.youtube.com/embed/x"));
    assert_eq!(b.referrer(), Some("https://site.example/"));
    let _ = b.take_request();
    b.loaded_with(Conn::Verified, false);
    // A reload asks again with the same one; an address typed has none.
    b.reload();
    assert_eq!(b.referrer(), Some("https://site.example/"));
    b.open(b"https://typed.example/");
    assert_eq!(b.referrer(), None);
}

#[test]
fn forms_send_it_and_plain_http_gets_the_origin() {
    let mut b = Browser::new();
    load(&mut b, b"http://10.0.2.2:8479/yt");
    assert!(b.open_post(b"https://other.example/send", b"a=1".to_vec()));
    assert_eq!(b.referrer(), Some("http://10.0.2.2:8479/"));
    let _ = b.take_request();
    b.loaded_with(Conn::Verified, false);
    // https to http is a downgrade: refused as a link, and never a referrer.
    assert!(!b.open_link(b"http://plain.example/"));
}

#[test]
fn the_start_page_sends_none() {
    let mut b = Browser::new();
    b.open(b"https://a.example/");
    assert_eq!(b.referrer(), None, "from the start page");
}
