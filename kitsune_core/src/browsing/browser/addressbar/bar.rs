//! The bar focus, suggestions, stop, recent pages and failures.

use super::*;

impl Browser {
    /// Select the whole address (Ctrl+L): typing replaces it.
    pub fn select_bar(&mut self) {
        self.bar_focus = true;
        self.bar_selected = self.url_len > 0;
        self.sugg_dismissed = true;
    }

    /// Is the whole address selected?
    pub fn bar_selected(&self) -> bool {
        self.bar_selected
    }

    /// Keyboard focus is in the address bar (true) or on the page (false).
    pub fn bar_focus(&self) -> bool {
        self.bar_focus
    }

    pub fn set_bar_focus(&mut self, on: bool) {
        self.bar_focus = on;
        if !on {
            self.sugg_sel = None;
        }
    }

    /// The history, oldest first (absolute URLs).
    pub fn history_urls(&self) -> impl Iterator<Item = &[u8]> {
        self.history.urls.iter().map(|u| u.as_slice())
    }

    /// Address-bar suggestions for the text typed so far: favourites then history, prefix matches
    /// before substring matches, at most [`MAX_SUGGESTIONS`]. Empty when the bar has no text, does
    /// not have the focus, was dismissed with Esc, or only the typed address itself matches.
    pub fn suggestions(&self) -> Vec<Suggestion> {
        if !self.bar_focus || self.sugg_dismissed || self.url_len == 0 {
            return Vec::new();
        }
        let q = String::from_utf8_lossy(self.url());
        let hist: Vec<String> = self
            .history
            .urls
            .iter()
            .rev()
            .map(|u| String::from_utf8_lossy(u).into_owned())
            .filter(|u| !is_internal_url(u))
            .collect();
        suggest(&q, &self.bookmarks.borrow().all(), &hist)
    }

    /// The highlighted suggestion index.
    pub fn suggestion_selected(&self) -> Option<usize> {
        self.sugg_sel
    }

    /// Choose suggestion `i` (a click): open it.
    pub fn pick_suggestion(&mut self, i: usize) -> bool {
        match self.suggestions().get(i) {
            Some(s) => {
                let url = s.url.clone();
                self.open(url.as_bytes());
                true
            }
            None => false,
        }
    }

    /// Stop loading: a request that was not taken yet is dropped, one in flight is ignored by
    /// the caller. The page on screen (or the start page) stays.
    pub fn stop(&mut self) {
        if self.status == Status::Loading {
            self.pending = false;
            self.history.replay = false;
            self.status = Status::Idle;
        }
    }

    /// Is a navigation in progress?
    pub fn is_loading(&self) -> bool {
        self.status == Status::Loading
    }

    /// Up to `n` most recently visited addresses, newest first, each once, without the
    /// browser's own pages.
    pub fn recent(&self, n: usize) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        for u in self.history.urls.iter().rev() {
            let u = String::from_utf8_lossy(u);
            if is_internal_url(&u) || out.iter().any(|o| *o == u) {
                continue;
            }
            out.push(u.into_owned());
            if out.len() >= n {
                break;
            }
        }
        out
    }

    /// Mark the current fetch as failed (the kernel shows the error state).
    pub fn fail(&mut self) {
        self.fail_with(FailReason::Network);
    }

    /// Mark the current fetch as failed for a specific reason.
    pub fn fail_with(&mut self, reason: FailReason) {
        self.history.replay = false;
        self.status = Status::Error;
        self.fail_reason = reason;
        self.note = None;
        self.home = false;
    }
}
