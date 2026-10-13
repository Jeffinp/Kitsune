//! Entries of one document in the history: `history.pushState`/`replaceState` of a page's scripts
//! (single-page apps), and moving back and forth between them without loading anything (the page
//! gets `popstate`).

use super::*;

impl Browser {
    /// The page's scripts changed the address to `url` without loading (`pushState`, or
    /// `replaceState` when `replace`). Only an address of the page's own origin is taken (the script
    /// process is not trusted); the address bar shows it unless the person is typing there. Returns
    /// whether it was taken.
    pub fn push_state(&mut self, url: &[u8], replace: bool) -> bool {
        if self.internal || self.home || url.len() > URL_CAP || self.status == Status::Loading {
            return false;
        }
        let (Ok(new), Ok(cur)) = (
            core::str::from_utf8(url),
            core::str::from_utf8(self.nav_url()),
        ) else {
            return false;
        };
        let origin = |u: &str| crate::browsing::http1::Target::parse(u).map(|t| t.origin());
        match (origin(new), origin(cur)) {
            (Some(a), Some(b)) if a == b => {}
            _ => return false,
        }
        // The bar follows unless the person changed its text.
        let follow = !self.bar_focus || self.url() == self.nav_url();
        let h = &mut self.history;
        h.docs.resize(h.urls.len(), 0);
        if replace || h.urls.is_empty() {
            if let Some(u) = h.urls.get_mut(h.cur) {
                *u = url.to_vec();
            }
        } else {
            let doc = h.docs.get(h.cur).copied().unwrap_or(0);
            h.push(url, doc);
        }
        self.nav[..url.len()].copy_from_slice(url);
        self.nav_len = url.len();
        if follow {
            self.set_url(url);
        }
        true
    }

    /// Back/forward reached an entry of the document on screen: its address and index, once (the
    /// page's scripts get `popstate`).
    pub fn take_pop(&mut self) -> Option<(Vec<u8>, usize)> {
        let i = self.history.pop.take()?;
        Some((self.history.urls.get(i)?.clone(), i))
    }

    /// The index of the current entry of the history.
    pub fn history_index(&self) -> usize {
        self.history.cur
    }

    /// The current entry is one of the document on screen: show its address, load nothing.
    pub(in super::super) fn same_doc_move(&mut self) {
        let i = self.history.cur;
        let url = self.history.urls[i].clone();
        let n = url.len().min(URL_CAP);
        self.nav[..n].copy_from_slice(&url[..n]);
        self.nav_len = n;
        self.set_url(&url[..n]);
        self.history.pop = Some(i);
    }
}
