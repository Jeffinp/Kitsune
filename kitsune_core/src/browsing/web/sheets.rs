//! The external style sheets of the page on screen: which ones the document asks for, which are
//! downloaded, and the order they are applied in.
//!
//! The browser asks [`SheetSet::next_pending`] for the next address to fetch (one at a time, like
//! the pictures) and reports back with [`SheetSet::finish`], which adds the rules to the document and
//! queues the sheet's own `@import`s. Bounded: at most [`MAX_SHEETS`] sheets and [`MAX_TOTAL`] bytes
//! of CSS per page.

use super::Doc;
use super::imgcache::image_key;
use alloc::string::String;
use alloc::vec::Vec;

/// Most sheets (links and imports together) fetched for one page.
pub const MAX_SHEETS: usize = 12;
/// Most bytes of CSS taken from the network for one page.
pub const MAX_TOTAL: usize = 1024 * 1024;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum State {
    Pending,
    Loading,
    Done,
}

struct Entry {
    key: String,
    state: State,
}

/// The sheets of one page.
#[derive(Default)]
pub struct SheetSet {
    base: Vec<u8>,
    entries: Vec<Entry>,
    bytes: usize,
}

impl SheetSet {
    /// A new page at `base`: forget the old sheets and queue the ones `doc` asks for.
    pub fn begin(&mut self, doc: &Doc, base: &[u8]) {
        self.base = base.to_vec();
        self.entries.clear();
        self.bytes = 0;
        for href in doc.linked_sheets() {
            self.add(&href);
        }
    }

    fn add(&mut self, href: &str) {
        if self.entries.len() >= MAX_SHEETS {
            return;
        }
        let Some(key) = image_key(&self.base, href) else {
            return;
        };
        if key.starts_with("data:") || self.entries.iter().any(|e| e.key == key) {
            return;
        }
        self.entries.push(Entry {
            key,
            state: State::Pending,
        });
    }

    /// The next address to download (it is marked as loading), in document order.
    pub fn next_pending(&mut self) -> Option<String> {
        let e = self
            .entries
            .iter_mut()
            .find(|e| e.state == State::Pending)?;
        e.state = State::Loading;
        Some(e.key.clone())
    }

    /// Is a download queued or in progress?
    pub fn busy(&self) -> bool {
        self.entries.iter().any(|e| e.state != State::Done)
    }

    /// Is `key` one of this page's sheets that was handed out?
    pub fn is_loading(&self, key: &str) -> bool {
        self.entries
            .iter()
            .any(|e| e.key == key && e.state == State::Loading)
    }

    /// The download of `key` ended: `css` is its text (`None` when it failed). The rules join `doc`
    /// and the sheet's imports are queued. Returns whether `doc` changed.
    pub fn finish(&mut self, doc: &mut Doc, key: &str, css: Option<&str>) -> bool {
        let Some(e) = self
            .entries
            .iter_mut()
            .find(|e| e.key == key && e.state == State::Loading)
        else {
            return false;
        };
        e.state = State::Done;
        let Some(css) = css else { return false };
        if self.bytes.saturating_add(css.len()) > MAX_TOTAL {
            return false;
        }
        self.bytes += css.len();
        // A relative `@import` is relative to the sheet that holds it.
        let sheet_base = key.as_bytes().to_vec();
        let imports = doc.add_sheet(css);
        let page_base = core::mem::replace(&mut self.base, sheet_base);
        for href in &imports {
            self.add(href);
        }
        self.base = page_base;
        true
    }

    /// The sheets of the page that were fetched (for tests and the log).
    pub fn done(&self) -> usize {
        self.entries
            .iter()
            .filter(|e| e.state == State::Done)
            .count()
    }
}

#[cfg(test)]
mod tests;
