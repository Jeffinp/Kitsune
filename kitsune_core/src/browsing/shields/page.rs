//! The blocker's state for the page on screen.

use super::urlparts::{HOST_MAX, lower_host, split};
use super::{Kind, Tally, classify};
use alloc::string::String;
use alloc::vec::Vec;

/// Most distinct blocked addresses remembered per page (to count each once and to log them). Past
/// it a blocked address still is not requested, but it is counted every time it is asked about.
pub const MAX_LOGGED: usize = 64;

/// One page's blocker: whether it is on, the host the page came from and what was blocked. Each tab
/// has one; a new page replaces it.
#[derive(Default)]
pub struct PageShield {
    on: bool,
    host: String,
    tally: Tally,
    /// Blocked addresses, in the order they were first asked for.
    seen: Vec<(String, Kind)>,
}

impl PageShield {
    /// A blocker that is off: lets everything through (the start page, internal pages, tests).
    pub const fn off() -> PageShield {
        PageShield {
            on: false,
            host: String::new(),
            tally: Tally {
                ads: 0,
                trackers: 0,
                hidden: 0,
            },
            seen: Vec::new(),
        }
    }

    /// The blocker for a page loaded from `page_url`, on or off as the person chose. An address that
    /// is not `http(s)` has no host, so the blocker is off for it.
    pub fn new(page_url: &[u8], on: bool) -> PageShield {
        let mut buf = [0u8; HOST_MAX];
        let host = split(page_url)
            .and_then(|(h, _)| lower_host(h, &mut buf))
            .map(|h| String::from_utf8_lossy(h).into_owned());
        match host {
            Some(host) => PageShield {
                on,
                host,
                tally: Tally::default(),
                seen: Vec::new(),
            },
            None => PageShield::off(),
        }
    }

    /// The blocker for a page loaded from `page_url`, on when `wanted(host)` says so for the page's
    /// host (the kernel asks the settings and the site list there).
    pub fn decide(page_url: &[u8], wanted: impl FnOnce(&str) -> bool) -> PageShield {
        let mut s = PageShield::new(page_url, false);
        s.on = s.has_host() && wanted(&s.host);
        s
    }

    /// Is the blocker filtering this page?
    pub fn is_on(&self) -> bool {
        self.on
    }

    /// Does the page have a host at all (so a shield can be shown for it)?
    pub fn has_host(&self) -> bool {
        !self.host.is_empty()
    }

    /// The host (lowercase) the page was loaded from.
    pub fn host(&self) -> &str {
        &self.host
    }

    /// What was blocked so far.
    pub fn tally(&self) -> Tally {
        self.tally
    }

    /// The blocked addresses in order, with their kind (at most [`MAX_LOGGED`]).
    pub fn blocked(&self) -> &[(String, Kind)] {
        &self.seen
    }

    /// Should the sub-resource `url` be left out? `Some(kind)` when it is blocked (and counted, the
    /// first time this address is asked about), `None` when it may be fetched. Always `None` when the
    /// blocker is off.
    pub fn check(&mut self, url: &[u8]) -> Option<Kind> {
        if !self.on {
            return None;
        }
        let kind = classify(self.host.as_bytes(), url)?;
        let first = !self.seen.iter().any(|(u, _)| u.as_bytes() == url);
        if first {
            match kind {
                Kind::Ad => self.tally.ads = self.tally.ads.saturating_add(1),
                Kind::Tracker => self.tally.trackers = self.tally.trackers.saturating_add(1),
            }
            if self.seen.len() < MAX_LOGGED {
                self.seen
                    .push((String::from_utf8_lossy(url).into_owned(), kind));
            }
        }
        Some(kind)
    }

    /// Record `n` boxes hidden by the cosmetic rules.
    pub fn add_hidden(&mut self, n: u32) {
        self.tally.hidden = self.tally.hidden.saturating_add(n);
    }
}
