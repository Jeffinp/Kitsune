//! State and accessors of a `Browser`: constructors, the current URL, status and failure reason.

use super::*;

impl Browser {
    pub fn new() -> Self {
        let mut b = Self {
            url: [0; URL_CAP],
            url_len: 0,
            caret: 0,
            status: Status::Idle,
            pending: false,
            post: None,
            nav: [0; URL_CAP],
            nav_len: 0,
            home: true,
            security: Security::None,
            note: None,
            fail_reason: FailReason::Network,
            insecure: Rc::new(RefCell::new(InsecureHosts::new())),
            history: History::default(),
            bookmarks: Rc::new(RefCell::new(Box::new(MemoryBookmarks::default()))),
            bar_focus: true,
            sugg_sel: None,
            sugg_dismissed: false,
            internal: false,
            internal_ready: false,
            page_title: String::new(),
            bar_selected: false,
        };
        b.set_url(b"");
        b
    }

    /// A browser whose favourites live in `store` (the one place the kernel plugs persistence in).
    pub fn with_store(store: Box<dyn BookmarkStore>) -> Self {
        let mut b = Self::new();
        b.bookmarks = Rc::new(RefCell::new(store));
        b
    }

    /// A fresh browser (a new tab) that shares this one's favourites and the hosts the user
    /// allowed past a certificate error: both belong to the window, not to a tab.
    pub fn sibling(&self) -> Self {
        let mut b = Self::new();
        b.bookmarks = Rc::clone(&self.bookmarks);
        b.insecure = Rc::clone(&self.insecure);
        b
    }

    /// True while the native start page (logo + shortcuts) is shown.
    pub fn is_home(&self) -> bool {
        self.home
    }

    /// Return to the start page, clearing the address bar.
    pub fn go_home(&mut self) {
        self.home = true;
        self.status = Status::Idle;
        self.security = Security::None;
        self.note = None;
        self.set_url(b"");
    }

    /// Re-fetch the page that is shown (no-op on the start page). It is the address that was
    /// navigated to, not whatever has been typed in the bar since; an unsent edit is dropped.
    pub fn reload(&mut self) {
        if self.home || self.nav_len == 0 {
            return;
        }
        let nav = self.nav[..self.nav_len].to_vec();
        self.set_url(&nav);
        self.submit();
    }

    /// "Tentar novamente" on an error page: the same navigation again.
    pub fn retry(&mut self) {
        self.reload();
    }

    /// Navigate straight to `url` (used by the start-page shortcuts).
    pub fn open(&mut self, url: &[u8]) {
        self.set_url(url);
        self.submit();
    }

    pub(in super::super) fn set_url(&mut self, s: &[u8]) {
        self.bar_selected = false;
        // Every way to navigate goes through here, so only `open_post` leaves a body behind.
        self.post = None;
        self.url_len = s.len().min(URL_CAP);
        self.url[..self.url_len].copy_from_slice(&s[..self.url_len]);
        self.caret = self.url_len;
    }

    pub fn url(&self) -> &[u8] {
        &self.url[..self.url_len]
    }
    pub fn caret(&self) -> usize {
        self.caret
    }
    pub fn status(&self) -> Status {
        self.status
    }

    /// What can be said about the connection behind the current page. While a
    /// load is in flight this reflects the *requested* scheme; once it
    /// completes ([`Browser::loaded_with`]) it reflects the final one, after
    /// any redirects. Never reports a verified connection (see [`Security`]).
    pub fn security(&self) -> Security {
        self.security
    }

    /// True when the loaded page is not the whole document (see [`Browser::note`]).
    pub fn truncated(&self) -> bool {
        self.note.is_some()
    }

    /// Why the loaded page is only part of the document, if it is.
    pub fn note(&self) -> Option<PageNote> {
        self.note
    }

    /// Why the last navigation failed (meaningful when `status()` is `Error`).
    pub fn fail_reason(&self) -> FailReason {
        self.fail_reason
    }
}
