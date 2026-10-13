//! Cookies: the browser's cookie jar (RFC 6265 and the parts of RFC 6265bis that browsers apply).
//!
//! The kernel's fetcher keeps one [`Jar`] for the browser. Every response's `Set-Cookie` goes through
//! [`Jar::set_from_header`] and every request asks [`Jar::header_for`] for its `Cookie` line; a page's
//! scripts read and write `document.cookie` through [`Jar::document_get`] / [`Jar::document_set`]
//! (which never see or write an `HttpOnly` cookie). Persistent cookies are saved as text
//! ([`Jar::to_text`], [`Jar::parse_text`]) in `~/.cookies`.
//!
//! What a request may carry depends on its [`Context`]: whether it is same-site with the page shown
//! (the "site" is `shields::site_of`, a registrable domain without a public-suffix list), whether it
//! is a top-level navigation, and whether it comes from HTTP or from a script. `SameSite=Strict`
//! cookies only go to same-site requests, `Lax` (and cookies without the attribute, as in current
//! browsers) also to top-level `GET` navigations, `None` everywhere (and must be `Secure`). A
//! cross-site response may not set `Strict` or `Lax` cookies. Blocking third-party cookies (the ad
//! blocker's shield) is the caller's choice: it simply does not ask the jar for cross-site requests.
//!
//! Limits: [`MAX_COOKIE_BYTES`] for a name and value, [`MAX_PER_DOMAIN`] per domain and
//! [`MAX_COOKIES`] in all (the oldest go first), expiry at most [`MAX_AGE_MS`] ahead.
//!
//! Pure and host-tested.

use alloc::string::String;
use alloc::vec::Vec;

mod jar;
mod parse;
mod store;
pub use jar::*;
pub use parse::*;

/// The largest name plus value of one cookie, in bytes.
pub const MAX_COOKIE_BYTES: usize = 4096;
/// Most cookies for one domain.
pub const MAX_PER_DOMAIN: usize = 50;
/// Most cookies in the jar.
pub const MAX_COOKIES: usize = 1000;
/// The furthest expiry accepted: 400 days, as RFC 6265bis caps it.
pub const MAX_AGE_MS: i64 = 400 * 24 * 3600 * 1000;

/// The `SameSite` attribute.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SameSite {
    Strict,
    Lax,
    None,
    /// Not given: treated as `Lax`.
    Unset,
}

impl SameSite {
    fn code(self) -> char {
        match self {
            SameSite::Strict => 'S',
            SameSite::Lax => 'L',
            SameSite::None => 'N',
            SameSite::Unset => 'U',
        }
    }

    fn from_code(c: &str) -> Option<SameSite> {
        Some(match c {
            "S" => SameSite::Strict,
            "L" => SameSite::Lax,
            "N" => SameSite::None,
            "U" => SameSite::Unset,
            _ => return None,
        })
    }
}

/// One stored cookie.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Cookie {
    pub name: String,
    pub value: String,
    /// Lowercase host (host-only) or the `Domain` attribute without its leading dot.
    pub domain: String,
    /// Sent only to `domain` itself, not to its subdomains (no `Domain` attribute).
    pub host_only: bool,
    pub path: String,
    /// Unix time in milliseconds; `None` for a session cookie (gone when the system restarts).
    pub expires: Option<i64>,
    pub secure: bool,
    pub http_only: bool,
    pub same_site: SameSite,
    /// Creation order (older cookies come first among equal paths, and are evicted first).
    pub created: u64,
}

/// The address a cookie is read for or written from.
#[derive(Clone, Copy, Debug)]
pub struct Site<'a> {
    pub https: bool,
    /// Lowercase host.
    pub host: &'a str,
    /// The path without the query.
    pub path: &'a str,
}

/// The circumstances of a request.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Context {
    /// The request's site is the site of the page shown (or it is a navigation of the page itself).
    pub same_site: bool,
    /// A top-level navigation (the address bar, a link, a form).
    pub top_level: bool,
    /// `GET` or `HEAD`.
    pub safe_method: bool,
    /// From HTTP (`false`: `document.cookie`).
    pub http: bool,
}

impl Context {
    /// The page itself (a navigation): everything the site set is sent.
    pub const NAVIGATION: Context = Context {
        same_site: true,
        top_level: true,
        safe_method: true,
        http: true,
    };

    /// A sub-request (picture, sheet, script, `fetch`) of a page: same-site or not, by method.
    pub fn subresource(same_site: bool, safe_method: bool) -> Context {
        Context {
            same_site,
            top_level: false,
            safe_method,
            http: true,
        }
    }

    /// `document.cookie` of the page.
    pub const SCRIPT: Context = Context {
        same_site: true,
        top_level: false,
        safe_method: true,
        http: false,
    };
}

#[cfg(test)]
mod tests;
