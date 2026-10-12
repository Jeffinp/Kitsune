//! Navigation: loading, back and forward, links and POST, insecure-host warnings.

use super::*;

impl Browser {
    /// Mark a navigation as successfully loaded (the kernel renders the page via
    /// the `web` engine and owns the display list).
    pub fn loaded(&mut self) {
        self.status = Status::Done;
        self.home = false;
        let url = self.nav[..self.nav_len].to_vec();
        self.history.record(&url);
    }

    /// True when there is an earlier page in the history.
    pub fn can_back(&self) -> bool {
        self.history.cur > 0 && !self.history.urls.is_empty()
    }

    /// True when there is a later page in the history.
    pub fn can_forward(&self) -> bool {
        self.history.cur + 1 < self.history.urls.len()
    }

    /// Number of pages in the history.
    pub fn history_len(&self) -> usize {
        self.history.urls.len()
    }

    /// Go to the previous page of the history (no-op at the start).
    pub fn back(&mut self) {
        if self.can_back() {
            self.history.cur -= 1;
            self.replay_current();
        }
    }

    /// Go to the next page of the history (no-op at the end).
    pub fn forward(&mut self) {
        if self.can_forward() {
            self.history.cur += 1;
            self.replay_current();
        }
    }

    pub(in super::super) fn replay_current(&mut self) {
        let url = self.history.urls[self.history.cur].clone();
        // Set before `submit`: an `kitsune://` page is "loaded" inside it.
        self.history.replay = true;
        self.set_url(&url);
        self.submit();
    }

    /// The user clicked a link whose `href` is `href` on the current page:
    /// resolve it against the page URL (relative, absolute and protocol-relative
    /// forms; `javascript:`, `data:` and an https -> http downgrade are refused)
    /// and navigate there. Returns `false` when the link was refused.
    pub fn open_link(&mut self, href: &[u8]) -> bool {
        if is_internal_input(href.trim_ascii()) {
            self.set_url(href.trim_ascii());
            self.submit();
            return true;
        }
        // The browser's own pages have no real address: resolve against a neutral http base so
        // their absolute links are not mistaken for an https -> http downgrade.
        let base = if self.internal {
            parse_url(b"http://kitsune.local/")
        } else {
            parse_url(self.nav_url())
        };
        let Some(base) = base else {
            return false;
        };
        match crate::browsing::redirect::resolve_redirect(&base, href) {
            Ok(target) => {
                self.set_url(&target);
                self.submit();
                true
            }
            Err(_) => false,
        }
    }

    /// A form with `method=post` was submitted: resolve `href` like a link and navigate there
    /// with `body` (urlencoded). Returns `false` when the address was refused. Reloading or
    /// going back to the result asks for the address with GET (a silent second POST could
    /// repeat an action), so the body is not kept.
    pub fn open_post(&mut self, href: &[u8], body: Vec<u8>) -> bool {
        let base = if self.internal {
            parse_url(b"http://kitsune.local/")
        } else {
            parse_url(self.nav_url())
        };
        let Some(base) = base else {
            return false;
        };
        match crate::browsing::redirect::resolve_redirect(&base, href) {
            Ok(target) => {
                self.set_url(&target);
                self.post = Some(body);
                self.submit();
                true
            }
            Err(_) => false,
        }
    }

    /// The body to send with the request just taken by [`Browser::take_request`] (`None` for a GET).
    pub fn post_body(&self) -> Option<&[u8]> {
        self.post.as_deref()
    }

    /// Like [`Browser::loaded`], recording how the page actually arrived: `conn`
    /// describes the *final* connection (after redirects) and `truncated` says
    /// the response hit [`MAX_RESPONSE_BYTES`].
    pub fn loaded_with(&mut self, conn: Conn, truncated: bool) {
        self.loaded_with_note(conn, truncated.then_some(PageNote::Truncated));
    }

    /// [`Browser::loaded_with`] with the precise reason the page is partial
    /// (from [`page_body_partial`]), `None` for a whole page.
    pub fn loaded_with_note(&mut self, conn: Conn, note: Option<PageNote>) {
        self.security = match conn {
            Conn::Plain => Security::Http,
            Conn::Verified => Security::HttpsVerified,
            Conn::Insecure => Security::HttpsInvalid,
        };
        self.note = note;
        self.loaded();
    }

    /// The navigation target of the last (or pending) request.
    pub fn nav_url(&self) -> &[u8] {
        &self.nav[..self.nav_len]
    }

    /// The host of the current navigation when the user allowed it to proceed
    /// despite a certificate error (the fetcher then skips validation for that
    /// host only, on every hop of this navigation).
    pub fn insecure_host(&self) -> Option<Vec<u8>> {
        let u = parse_url(self.nav_url())?;
        if !u.https {
            return None;
        }
        let ins = self.insecure.borrow();
        let i = ins.find(u.host())?;
        Some(ins.hosts[i][..usize::from(ins.lens[i])].to_vec())
    }

    /// True when the failed navigation can be retried anyway: the failure is a
    /// certificate error (nothing else offers an unsafe override).
    pub fn can_continue_insecure(&self) -> bool {
        self.status == Status::Error && matches!(self.fail_reason, FailReason::Cert(_))
    }

    /// The user's explicit "continue anyway (insecure)": remember this host for
    /// the session and load the page again.
    pub fn continue_insecure(&mut self) {
        if !self.can_continue_insecure() {
            return;
        }
        if let Some(u) = parse_url(self.nav_url()) {
            self.insecure.borrow_mut().add(u.host());
        }
        self.pending = true;
        self.status = Status::Loading;
    }

    // ---- the browser's own pages, favourites and suggestions ----
}
