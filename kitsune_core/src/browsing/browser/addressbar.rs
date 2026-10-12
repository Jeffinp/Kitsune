//! The address bar and navigation of a [`Browser`]: history, the bar's text and suggestions, internal
//! pages, bookmarks and the HTTP request the page asks for.
//!
//! The `Browser` methods are grouped by subject in the child modules (`state`, `keys`, `navigation`,
//! `internal`, `bookmarks`, `bar`); the request builders are in `request`.

use super::*;

mod request;
pub use request::*;

mod bar;
mod bookmarks;
mod internal;
mod keys;
mod navigation;
mod state;

impl History {
    /// Record a finished navigation to `url`: drops the forward entries, ignores
    /// a reload of the same page and honors a pending back/forward replay.
    pub(super) fn record(&mut self, url: &[u8]) {
        if core::mem::take(&mut self.replay) {
            return;
        }
        if self.urls.get(self.cur).is_some_and(|u| u == url) {
            return;
        }
        if !self.urls.is_empty() {
            self.urls.truncate(self.cur + 1);
        }
        if self.urls.len() == MAX_HISTORY {
            self.urls.remove(0);
        }
        self.urls.push(url.to_vec());
        self.cur = self.urls.len() - 1;
    }
}
impl InsecureHosts {
    pub(super) const fn new() -> Self {
        Self {
            hosts: [[0; HOST_CAP]; MAX_INSECURE_HOSTS],
            lens: [0; MAX_INSECURE_HOSTS],
            n: 0,
        }
    }

    pub(super) fn find(&self, host: &[u8]) -> Option<usize> {
        (0..self.n).find(|&i| self.hosts[i][..usize::from(self.lens[i])].eq_ignore_ascii_case(host))
    }

    /// Remember `host` (the oldest entry is dropped when full).
    pub(super) fn add(&mut self, host: &[u8]) {
        if host.is_empty() || host.len() > HOST_CAP || self.find(host).is_some() {
            return;
        }
        if self.n == MAX_INSECURE_HOSTS {
            for i in 1..MAX_INSECURE_HOSTS {
                self.hosts[i - 1] = self.hosts[i];
                self.lens[i - 1] = self.lens[i];
            }
            self.n -= 1;
        }
        let i = self.n;
        self.hosts[i] = [0; HOST_CAP];
        self.hosts[i][..host.len()].copy_from_slice(host);
        self.lens[i] = host.len() as u8;
        self.n += 1;
    }
}
