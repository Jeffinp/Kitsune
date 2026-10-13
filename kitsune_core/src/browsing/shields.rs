//! The native ad and tracker blocker (the browser's "shield").
//!
//! On by default; the person turns it off for everything (Ajustes) or for one site (the shield in
//! the address bar). It decides, before the browser asks the network for a sub-resource of a page
//! (a picture, a style sheet, later a script), whether that address is a known ad or tracker, and
//! it hides the boxes of a page that ad scripts would fill (cosmetic rules). The address of the page
//! itself is never filtered: a link to an ad host is a navigation, and a person who types one gets
//! the page.
//!
//! # Pieces
//!
//! | file | what |
//! |---|---|
//! | `urlparts` | host and path of an absolute address without allocating; the "site" of a host; first or third party |
//! | `lists` | the rules, embedded from `shields/data/*.txt`; sorted host lists searched in `O(log n)` |
//! | `check` | [`classify`]: the verdict for one request |
//! | `page` | [`PageShield`]: the state of the page on screen (on or off, its host, what was blocked) |
//! | `sites` | [`SiteList`]: the sites where the person turned the blocker off, as a text file |
//! | `cosmetic` | the generic "hide this box" style sheet and [`hide_ads`] |
//!
//! # Rules
//!
//! * **Host rules** (`ad_hosts.txt`, `tracker_hosts.txt`): a registrable domain or a host; the rule
//!   matches that host and every subdomain of it. Only for **third-party** requests (the request's
//!   site differs from the page's site), so a site's own subdomains are never blocked by them.
//! * **Path rules** (`ad_paths.txt`, `tracker_paths.txt`): a substring of the path and query
//!   (ASCII case-insensitive), for any request. They start with `/` so `/ads/banner` does not match
//!   `/uploads/banner`.
//! * **Cosmetic rules** (`cosmetic.css`): a short list of generic selectors (`.adsbygoogle`,
//!   `[id^="div-gpt-ad"]`...) with `display: none !important`. At most [`MAX_COSMETIC_SELECTORS`].
//!
//! Matching never allocates: the host is lowercased into a stack buffer and the lists are searched
//! in place. The list files are our own curated lists (see `THIRD-PARTY.md`).
//!
//! # Using it
//!
//! The kernel builds a [`PageShield`] when a page arrives (`PageShield::new(url, on)`), passes it to
//! [`SheetSet`](crate::web::sheets::SheetSet) and the image registration, and shows
//! [`PageShield::tally`] on the shield. A future script loader asks the same
//! [`PageShield::check`] before it fetches a script.

mod check;
mod cosmetic;
mod lists;
mod page;
mod sites;
mod urlparts;

pub use check::classify;
pub use cosmetic::{COSMETIC_CSS, MAX_COSMETIC_SELECTORS, hide_ads};
pub use page::{MAX_LOGGED, PageShield};
pub use sites::{MAX_SITES, SiteList};
pub use urlparts::{is_third_party, site_of};

/// What a blocked address is.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    /// An advertising network or ad path.
    Ad,
    /// An analytics, tag-manager or tracking pixel.
    Tracker,
}

impl Kind {
    /// A short English word for logs.
    pub fn name(self) -> &'static str {
        match self {
            Kind::Ad => "ad",
            Kind::Tracker => "tracker",
        }
    }
}

/// What the blocker removed from one page.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Tally {
    /// Ad requests that were not made.
    pub ads: u32,
    /// Tracker requests that were not made.
    pub trackers: u32,
    /// Boxes hidden by the cosmetic rules.
    pub hidden: u32,
}

impl Tally {
    /// Everything removed: the number on the shield.
    pub fn total(&self) -> u32 {
        self.ads
            .saturating_add(self.trackers)
            .saturating_add(self.hidden)
    }
}

#[cfg(test)]
mod tests;
