//! Keys typed in the address bar, submitting its text and taking the request to send.

use super::*;

impl Browser {
    /// Handle a key while the address bar has focus. Returns `true` if anything
    /// changed (so the caller repaints). ENTER submits a navigation/search.
    pub fn on_key(&mut self, key: Key) -> bool {
        // Suggestion list: Up/Down move through it, Enter opens the highlighted one.
        let n = self.suggestions().len();
        match key {
            Key::Down if n > 0 => {
                self.sugg_sel = Some(self.sugg_sel.map_or(0, |i| (i + 1).min(n - 1)));
                return true;
            }
            Key::Up if n > 0 => {
                self.sugg_sel = match self.sugg_sel {
                    Some(0) | None => None,
                    Some(i) => Some(i - 1),
                };
                return true;
            }
            Key::Enter if self.sugg_sel.is_some() => {
                let i = self.sugg_sel.unwrap_or(0);
                if let Some(s) = self.suggestions().get(i) {
                    let url = s.url.clone();
                    self.open(url.as_bytes());
                }
                return true;
            }
            Key::Esc if n > 0 => {
                self.sugg_dismissed = true;
                self.sugg_sel = None;
                return true;
            }
            Key::Char(_) | Key::Backspace | Key::Delete => {
                self.sugg_sel = None;
                self.sugg_dismissed = false;
                // A selected address is replaced by what is typed (or just deleted).
                if core::mem::take(&mut self.bar_selected) {
                    self.url_len = 0;
                    self.caret = 0;
                    if matches!(key, Key::Backspace | Key::Delete) {
                        return true;
                    }
                }
            }
            Key::Left | Key::Right | Key::Home | Key::End => self.bar_selected = false,
            _ => {}
        }
        match key {
            Key::Char(c) => {
                if self.url_len < URL_CAP {
                    // insert at caret
                    let mut i = self.url_len;
                    while i > self.caret {
                        self.url[i] = self.url[i - 1];
                        i -= 1;
                    }
                    self.url[self.caret] = c;
                    self.url_len += 1;
                    self.caret += 1;
                }
                true
            }
            Key::Backspace => {
                if self.caret > 0 {
                    for i in self.caret..self.url_len {
                        self.url[i - 1] = self.url[i];
                    }
                    self.url_len -= 1;
                    self.caret -= 1;
                }
                true
            }
            Key::Delete => {
                if self.caret < self.url_len {
                    for i in self.caret + 1..self.url_len {
                        self.url[i - 1] = self.url[i];
                    }
                    self.url_len -= 1;
                }
                true
            }
            Key::Left => {
                self.caret = self.caret.saturating_sub(1);
                true
            }
            Key::Right => {
                if self.caret < self.url_len {
                    self.caret += 1;
                }
                true
            }
            Key::Home => {
                self.caret = 0;
                true
            }
            Key::End => {
                self.caret = self.url_len;
                true
            }
            Key::Enter => {
                self.submit();
                true
            }
            _ => false,
        }
    }

    /// Resolve the address bar into a navigation target and mark a fetch pending.
    pub fn submit(&mut self) {
        let input = &self.url[..self.url_len];
        if input.trim_ascii().is_empty() {
            return;
        }
        self.sugg_sel = None;
        self.sugg_dismissed = true;
        // Only a link or a form of the page sets one again (`open_link`, `open_post`, after this).
        self.referrer = None;
        if is_internal_input(input.trim_ascii()) {
            let url = input.trim_ascii().to_vec();
            self.open_internal(&url);
            return;
        }
        self.internal = false;
        let mut nav = [0u8; URL_CAP];
        let n = if looks_like_url(input) {
            // Normalize: prepend https:// if no scheme was given.
            if starts_with_ci(input, b"http://") || starts_with_ci(input, b"https://") {
                let n = input.len().min(URL_CAP);
                nav[..n].copy_from_slice(&input[..n]);
                n
            } else {
                let pre = b"https://";
                let mut n = pre.len();
                nav[..n].copy_from_slice(pre);
                let take = input.len().min(URL_CAP - n);
                nav[n..n + take].copy_from_slice(&input[..take]);
                n += take;
                n
            }
        } else {
            build_search_url(input, &mut nav)
        };
        self.nav_len = n;
        self.nav[..n].copy_from_slice(&nav[..n]);
        // A verified connection is only ever reported by `loaded_with`.
        self.security = if starts_with_ci(&nav[..n], b"http://") {
            Security::Http
        } else {
            Security::None
        };
        self.note = None;
        self.status = Status::Loading;
        self.pending = true;
        self.home = false;
    }

    /// Pull a pending navigation target (clears the pending flag). The kernel
    /// fetches it and reports back with [`loaded`] / [`fail`].
    pub fn take_request(&mut self) -> Option<&[u8]> {
        if self.pending {
            self.pending = false;
            Some(&self.nav[..self.nav_len])
        } else {
            None
        }
    }
}
