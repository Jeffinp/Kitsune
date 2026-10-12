//! The page title and bookmarks.

use super::*;

impl Browser {
    /// Remember the `<title>` of the page on screen (for favourites and the window title).
    pub fn set_page_title(&mut self, title: &str) {
        self.page_title.clear();
        // Cut at a character boundary: a title is page-controlled UTF-8.
        let mut end = title.len().min(80);
        while !title.is_char_boundary(end) {
            end -= 1;
        }
        self.page_title.push_str(&title[..end]);
    }

    /// `<title>` of the page on screen (empty when none).
    pub fn page_title(&self) -> &str {
        &self.page_title
    }

    /// Is the page on screen a favourite?
    pub fn is_bookmarked(&self) -> bool {
        !self.home
            && !self.nav_url().is_empty()
            && self
                .bookmarks
                .borrow()
                .contains(&String::from_utf8_lossy(self.nav_url()))
    }

    /// Ctrl+D / the star: add the page to the favourites, or remove it. Returns the new state
    /// (`true` = now a favourite) or `None` when there is nothing to bookmark (start page, full).
    pub fn toggle_bookmark(&mut self) -> Option<bool> {
        if self.home || self.nav_url().is_empty() {
            return None;
        }
        let url = String::from_utf8_lossy(self.nav_url()).into_owned();
        if self.bookmarks.borrow().contains(&url) {
            self.bookmarks.borrow_mut().remove(&url);
            return Some(false);
        }
        let title = if self.page_title.is_empty() {
            url.clone()
        } else {
            self.page_title.clone()
        };
        self.bookmarks
            .borrow_mut()
            .add(Bookmark { url, title })
            .then_some(true)
    }

    /// The favourites.
    pub fn bookmarks(&self) -> Vec<Bookmark> {
        self.bookmarks.borrow().all()
    }
}
