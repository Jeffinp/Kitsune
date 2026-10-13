use super::*;

#[test]
fn push_state_adds_entries_of_the_same_document() {
    let mut b = browser_with_history(&["http://a.test/", "http://a.test/app"]);
    assert!(b.push_state(b"http://a.test/app#/active", false));
    assert_eq!(b.nav_url(), b"http://a.test/app#/active");
    assert_eq!(
        b.url(),
        b"http://a.test/app#/active",
        "the address bar follows"
    );
    assert_eq!(b.history_len(), 3);
    assert!(b.take_request().is_none(), "nothing is loaded");
    // Back: the same document, no load, the page is told.
    b.back();
    assert!(b.take_request().is_none());
    assert_eq!(b.take_pop(), Some((b"http://a.test/app".to_vec(), 1)));
    assert_eq!(b.take_pop(), None, "once");
    assert_eq!(b.nav_url(), b"http://a.test/app");
    // Back again: another document, a load.
    b.back();
    assert_eq!(b.take_request(), Some(&b"http://a.test/"[..]));
    assert_eq!(b.take_pop(), None);
}

#[test]
fn replace_state_and_go() {
    let mut b = browser_with_history(&["http://a.test/x"]);
    assert!(b.push_state(b"http://a.test/x/1", false));
    assert!(b.push_state(b"http://a.test/x/2", false));
    assert!(b.push_state(b"http://a.test/x/3", true), "replace");
    assert_eq!(b.history_len(), 3);
    b.go(-2);
    assert_eq!(b.take_pop(), Some((b"http://a.test/x".to_vec(), 0)));
    b.go(2);
    assert_eq!(b.take_pop(), Some((b"http://a.test/x/3".to_vec(), 2)));
    b.go(5);
    assert_eq!(b.take_pop(), None, "past the end: nothing");
    assert!(b.take_request().is_none());
}

#[test]
fn push_state_refuses_another_origin_and_odd_states() {
    let mut b = browser_with_history(&["https://a.test/"]);
    assert!(!b.push_state(b"https://evil.test/", false));
    assert!(!b.push_state(b"http://a.test/", false), "another scheme");
    assert!(!b.push_state(b"https://a.test:444/", false), "another port");
    assert!(!b.push_state(&[b'x'; URL_CAP + 1], false));
    assert_eq!(b.history_len(), 1);
    b.open(b"https://a.test/next");
    assert!(
        !b.push_state(b"https://a.test/z", false),
        "not while a page loads"
    );
}

#[test]
fn a_reloaded_page_is_a_new_document() {
    let mut b = browser_with_history(&["http://a.test/app"]);
    assert!(b.push_state(b"http://a.test/app/1", false));
    b.reload();
    let _ = b.take_request();
    b.loaded_with(Conn::Plain, false);
    // The entry before belongs to the document that was replaced: going back loads it.
    b.back();
    assert_eq!(b.take_pop(), None);
    assert_eq!(b.take_request(), Some(&b"http://a.test/app"[..]));
}
