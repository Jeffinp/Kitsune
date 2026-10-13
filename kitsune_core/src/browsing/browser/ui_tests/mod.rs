//! Tests for the browser UI model: favourites, suggestions, internal pages,
//! focus, history interplay.

use super::*;

fn browser_with_history(urls: &[&str]) -> Browser {
    let mut b = Browser::new();
    for u in urls {
        b.open(u.as_bytes());
        b.take_request();
        b.loaded_with(Conn::Plain, false);
    }
    b
}

fn type_str(b: &mut Browser, s: &str) {
    for c in s.bytes() {
        b.on_key(Key::Char(c));
    }
}

fn clear_bar(b: &mut Browser) {
    while b.url_len > 0 {
        b.on_key(Key::Backspace);
    }
}

fn bm(url: &str, title: &str) -> Bookmark {
    Bookmark {
        url: url.into(),
        title: title.into(),
    }
}

fn open_internal(b: &mut Browser, url: &str) -> String {
    b.open(url.as_bytes());
    String::from_utf8(b.take_internal().expect("internal html")).unwrap()
}

mod address_bar_selection;
mod favourites;
mod internal_pages;
mod same_document;
mod suggestions;
mod tabs_share_window;
mod title_url_cap;
