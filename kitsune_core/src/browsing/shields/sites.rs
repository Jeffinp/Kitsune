//! The sites where the person turned the blocker off.
//!
//! The list is kept by site (the registrable domain, see [`site_of`](super::site_of)), so turning it
//! off on `www.example.com` also covers `example.com` and `news.example.com`. The text form is one
//! site per line (`#` starts a comment); parsing is total, like the settings file: a damaged file
//! yields a shorter list, never an error.

use super::urlparts::{HOST_MAX, lower_host, site_of};
use alloc::string::String;
use alloc::vec::Vec;

/// Most sites remembered.
pub const MAX_SITES: usize = 64;
/// Longest site name kept (bytes).
const SITE_CAP: usize = 63;

/// Sites with the blocker off.
#[derive(Default, Clone, PartialEq, Eq, Debug)]
pub struct SiteList {
    sites: Vec<String>,
}

fn valid_site(s: &[u8]) -> bool {
    // A name starts and ends with a letter or digit: "." or "-" alone is not a site (and the text form
    // would drop it again, since a trailing dot is cut when a line is read).
    s.first().is_some_and(u8::is_ascii_alphanumeric)
        && s.last().is_some_and(u8::is_ascii_alphanumeric)
        && s.len() <= SITE_CAP
        && s.iter()
            .all(|&c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, b'.' | b'-'))
}

impl SiteList {
    /// An empty list: the blocker is on everywhere.
    pub const fn new() -> SiteList {
        SiteList { sites: Vec::new() }
    }

    /// Read the text form. Lines that are not a plausible site are skipped; at most
    /// [`MAX_SITES`] are kept.
    pub fn parse(text: &[u8]) -> SiteList {
        let mut list = SiteList::new();
        for line in text.split(|&c| c == b'\n') {
            let line = line.trim_ascii();
            if line.is_empty() || line[0] == b'#' || list.sites.len() >= MAX_SITES {
                continue;
            }
            let mut buf = [0u8; HOST_MAX];
            let Some(site) = lower_host(line, &mut buf).filter(|s| valid_site(s)) else {
                continue;
            };
            let site = String::from_utf8_lossy(site).into_owned();
            if !list.sites.contains(&site) {
                list.sites.push(site);
            }
        }
        list
    }

    /// The text form.
    pub fn to_text(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(48 + self.sites.len() * 16);
        out.extend_from_slice(b"# Kitsune: sites where the ad blocker is off\n");
        for s in &self.sites {
            out.extend_from_slice(s.as_bytes());
            out.push(b'\n');
        }
        out
    }

    /// The site key of `host` (any case), or `None` when it cannot be kept.
    fn key(host: &[u8]) -> Option<String> {
        let mut buf = [0u8; HOST_MAX];
        let host = lower_host(host, &mut buf)?;
        let site = site_of(host);
        valid_site(site).then(|| String::from_utf8_lossy(site).into_owned())
    }

    /// Is the blocker off for `host` (a host or a site, any case)?
    pub fn is_off(&self, host: &[u8]) -> bool {
        let mut buf = [0u8; HOST_MAX];
        let Some(host) = lower_host(host, &mut buf) else {
            return false;
        };
        let site = site_of(host);
        self.sites.iter().any(|s| s.as_bytes() == site)
    }

    /// Turn the blocker off (`off`) or back on for the site of `host`. Returns whether the list
    /// changed; `false` too when the host cannot be kept or the list is full.
    pub fn set_off(&mut self, host: &[u8], off: bool) -> bool {
        let Some(site) = Self::key(host) else {
            return false;
        };
        let at = self.sites.iter().position(|s| *s == site);
        match (off, at) {
            (true, None) if self.sites.len() < MAX_SITES => {
                self.sites.push(site);
                true
            }
            (false, Some(i)) => {
                self.sites.remove(i);
                true
            }
            _ => false,
        }
    }

    /// The sites, oldest first.
    pub fn sites(&self) -> &[String] {
        &self.sites
    }

    /// How many sites are listed.
    pub fn len(&self) -> usize {
        self.sites.len()
    }

    /// Is the list empty?
    pub fn is_empty(&self) -> bool {
        self.sites.is_empty()
    }
}
