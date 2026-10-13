//! Native browser logic: URL parsing, search-URL building, HTML→text
//! extraction, and the address-bar + content model.
//!
//! Pure and allocation-free (fixed buffers), so the whole parser and editing
//! model is host-testable. The kernel supplies only the networking: it pulls a
//! pending request out with [`Browser::take_request`], fetches the bytes, and
//! fetches the bytes; the kernel renders them with the `web` engine.

pub mod cert;
pub mod errors;
pub mod motion;
pub mod pages;
pub mod tabs;

pub use cert::CertInfo;

use crate::Key;
use crate::i18n::{self, Lang};
use crate::tk;
use alloc::boxed::Box;
use alloc::rc::Rc;
use alloc::string::String;
use alloc::vec::Vec;
use core::cell::RefCell;

mod addressbar;
mod http;
mod internal;
mod url;
pub use addressbar::*;
pub use http::*;
pub use internal::*;
pub use url::*;

/// Max bytes of a URL (address bar + resolved navigation target).
pub const URL_CAP: usize = 480;
/// Max host length.
pub const HOST_CAP: usize = 80;

/// Maximum bytes of one HTTP(S) response accepted from the network (headers +
/// body, still compressed). Shared by the plain-HTTP and the TLS path so neither
/// can be talked into exhausting the kernel's single heap. An oversized response
/// is cut at this size and the page is flagged as truncated (a cut gzip/deflate
/// body still renders its decoded prefix, see [`page_body_partial`]).
///
/// Memory budget of one page load, worst case, of the 64 MiB heap (freed once the
/// DOM exists; the DOM itself is bounded by `web::MAX_NODES`): the raw response
/// (this, 1 MiB) + its de-chunked copy (1 MiB) + the decoded body
/// ([`crate::format::gzip::MAX_DECODED_BYTES`], 4 MiB, up to 2x transient `Vec` slack)
/// ~ 12 MiB. It was 256 KiB, which real pages (a gzip home page of a CDN vendor
/// is ~300 KiB on the wire) overflowed.
pub const MAX_RESPONSE_BYTES: usize = 1024 * 1024;

/// Append `data` to `out`, never letting `out` grow past `cap` bytes. Returns
/// `true` when some of `data` had to be dropped (the response is truncated).
pub fn append_capped(out: &mut alloc::vec::Vec<u8>, data: &[u8], cap: usize) -> bool {
    let room = cap.saturating_sub(out.len());
    let take = data.len().min(room);
    out.extend_from_slice(&data[..take]);
    take < data.len()
}

/// What the browser can honestly say about the connection that produced the
/// page on screen. "Secure" exists only as [`Security::HttpsVerified`], which
/// the kernel may report only after the server's certificate chain was
/// validated against the embedded trust store, the host name matched and the
/// handshake signature verified; there is no way to reach it without that.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Security {
    /// Nothing loaded (start page), or an `https://` load still in flight.
    None,
    /// Plain `http://`: not encrypted.
    Http,
    /// `https://` with a verified certificate chain for this host.
    HttpsVerified,
    /// `https://` where verification failed and the user chose to continue
    /// anyway for this origin, for this session only: encrypted but the peer
    /// is not authenticated.
    HttpsInvalid,
}

impl Security {
    /// Catalog key of the short label for the address bar, `None` when there is nothing to
    /// say.
    pub fn label_key(self) -> Option<&'static str> {
        match self {
            Security::None => None,
            Security::Http => Some(tk!("web.sec.insecure")),
            Security::HttpsVerified => Some(tk!("web.sec.secure")),
            Security::HttpsInvalid => Some(tk!("web.sec.invalid")),
        }
    }

    /// The label in the language in effect.
    pub fn label(self) -> Option<&'static str> {
        self.label_key().map(crate::i18n::tr)
    }
}

/// How a loaded page actually arrived, reported by the kernel's fetcher.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Conn {
    /// Plain `http://`.
    Plain,
    /// TLS with a validated chain and a matching host name.
    Verified,
    /// TLS where validation failed and the user allowed this origin.
    Insecure,
}

/// Why a navigation failed, so the UI can say more than "failed".
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum FailReason {
    /// DNS, connect, TLS or timeout: no usable response (cause not known).
    Network,
    /// The host name does not resolve.
    Dns,
    /// The server refused the TCP connection.
    Refused,
    /// No answer in time (connect, handshake or response).
    Timeout,
    /// The TLS handshake failed for a reason other than the certificate.
    Tls,
    /// The server certificate was refused.
    Cert(crate::network::tlsverify::CertError),
    /// A redirect tried to move from `https://` to `http://`; blocked.
    RedirectDowngrade,
    /// A redirect `Location` was malformed or unsupported.
    RedirectInvalid,
    /// A redirect pointed back at a URL already visited in this navigation.
    RedirectLoop,
    /// More than [`crate::browsing::redirect::MAX_REDIRECTS`] redirects.
    TooManyRedirects,
    /// The background fetcher thread died (panic or CPU fault) and cannot serve requests.
    WorkerDied,
}

impl FailReason {
    /// Map a refused redirect onto the reason shown to the user.
    pub fn from_redirect(e: crate::browsing::redirect::RedirectError) -> Self {
        use crate::browsing::redirect::RedirectError as E;
        match e {
            E::Invalid => FailReason::RedirectInvalid,
            E::Downgrade => FailReason::RedirectDowngrade,
            E::Loop => FailReason::RedirectLoop,
            E::TooMany => FailReason::TooManyRedirects,
        }
    }
}

/// Where a fetch stands, surfaced in the UI.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Status {
    Idle,
    Loading,
    Done,
    Error,
}

// ---- URL parsing ----

/// A parsed absolute URL split into fixed buffers.
pub struct Url {
    pub https: bool,
    pub port: u16,
    host: [u8; HOST_CAP],
    host_len: usize,
    path: [u8; URL_CAP],
    path_len: usize,
}

// ---- HTTP / HTML ----

/// Why a page body on screen is not the whole document. Shown as a banner.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PageNote {
    /// Cut at [`MAX_RESPONSE_BYTES`] (or the decoded size limit).
    Truncated,
    /// The connection ended before the body did.
    Incomplete,
    /// The compressed data went bad; the part decoded before that is shown.
    Damaged,
    /// Fully decoded, but the gzip/zlib checksum did not match.
    BadChecksum,
}

/// A decoded page body plus, when it is not the whole document, why.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PageBody {
    pub body: alloc::vec::Vec<u8>,
    pub note: Option<PageNote>,
}

// ---- favourites, suggestions, internal pages ----

/// Scheme of the browser's own pages.
pub const INTERNAL_SCHEME: &[u8] = b"kitsune:";
/// Scheme the browser's pages had when the system was called OSjeff. Still recognised
/// (typed, clicked or found in an old history) and redirected to `kitsune://`.
pub const LEGACY_INTERNAL_SCHEME: &[u8] = b"osjeff:";

/// Most favourites kept.
pub const MAX_BOOKMARKS: usize = 64;
/// Most suggestions shown under the address bar.
pub const MAX_SUGGESTIONS: usize = 6;

/// A favourite: an absolute URL and the page's title.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Bookmark {
    pub url: String,
    pub title: String,
}

/// Where favourites live. The browser only talks to this trait, so persistence
/// (a file on the filesystem) is a matter of giving [`Browser::with_store`] another
/// implementation; [`MemoryBookmarks`] forgets everything at power-off.
pub trait BookmarkStore {
    /// Every favourite, oldest first.
    fn all(&self) -> Vec<Bookmark>;
    /// Add one; `false` when it is already there or the store is full.
    fn add(&mut self, b: Bookmark) -> bool;
    /// Remove the favourite with this URL; `false` when there is none.
    fn remove(&mut self, url: &str) -> bool;
    fn contains(&self, url: &str) -> bool {
        self.all().iter().any(|b| b.url == url)
    }
}

/// Favourites in memory, at most [`MAX_BOOKMARKS`].
#[derive(Default)]
pub struct MemoryBookmarks {
    items: Vec<Bookmark>,
}

impl BookmarkStore for MemoryBookmarks {
    fn all(&self) -> Vec<Bookmark> {
        self.items.clone()
    }
    fn add(&mut self, b: Bookmark) -> bool {
        if self.items.len() >= MAX_BOOKMARKS || self.items.iter().any(|x| x.url == b.url) {
            return false;
        }
        self.items.push(b);
        true
    }
    fn remove(&mut self, url: &str) -> bool {
        let n = self.items.len();
        self.items.retain(|b| b.url != url);
        self.items.len() != n
    }
    fn contains(&self, url: &str) -> bool {
        self.items.iter().any(|b| b.url == url)
    }
}

/// Favourites that write themselves out after every change: `save` receives the
/// text form of the whole list (the kernel stores it as a file). A failed save
/// keeps the change in memory.
pub struct SavedBookmarks<F: FnMut(&[u8])> {
    inner: MemoryBookmarks,
    save: F,
}

impl<F: FnMut(&[u8])> BookmarkStore for SavedBookmarks<F> {
    fn all(&self) -> Vec<Bookmark> {
        self.inner.all()
    }
    fn add(&mut self, b: Bookmark) -> bool {
        let ok = self.inner.add(b);
        if ok {
            self.flush();
        }
        ok
    }
    fn remove(&mut self, url: &str) -> bool {
        let ok = self.inner.remove(url);
        if ok {
            self.flush();
        }
        ok
    }
    fn contains(&self, url: &str) -> bool {
        self.inner.contains(url)
    }
}

/// One line of the suggestion list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Suggestion {
    pub url: String,
    /// Text shown: the page title for a favourite, else the URL.
    pub label: String,
    pub bookmark: bool,
}

// ---- the address-bar model ----

/// The browser app's editable address bar and navigation state. The rendered
/// page itself is produced by the `web` engine and owned by the kernel; this
/// only tracks the URL input, status, and start-page flag. The kernel drives
/// networking via [`take_request`].
pub struct Browser {
    url: [u8; URL_CAP],
    url_len: usize,
    caret: usize,
    status: Status,
    pending: bool,
    /// Body of the POST being navigated to (`application/x-www-form-urlencoded`); `None` = GET.
    post: Option<Vec<u8>>,
    /// The `Referer` of the navigation being made (a link or a form of the page on screen); `None`
    /// for an address typed, a bookmark, a reload or the history. It is also `document.referrer`.
    referrer: Option<String>,
    nav: [u8; URL_CAP],
    nav_len: usize,
    home: bool, // showing the native start page (no page loaded)
    security: Security,
    note: Option<PageNote>,
    fail_reason: FailReason,
    insecure: Rc<RefCell<InsecureHosts>>,
    history: History,
    bookmarks: Rc<RefCell<Box<dyn BookmarkStore>>>,
    /// Keyboard focus is in the address bar (else on the page).
    bar_focus: bool,
    /// Highlighted suggestion (Up/Down in the address bar).
    sugg_sel: Option<usize>,
    /// Esc closed the suggestion list; it stays closed until the text changes.
    sugg_dismissed: bool,
    /// The page on screen is an `kitsune://` page generated by the browser.
    internal: bool,
    /// An internal page was just opened: the kernel must fetch its HTML with
    /// [`Browser::take_internal`].
    internal_ready: bool,
    /// `<title>` of the page on screen (set by the kernel after layout).
    page_title: String,
    /// The whole address is selected (Ctrl+L, a click on the bar): the next edit replaces it.
    bar_selected: bool,
}

/// Pages kept in the in-memory history (the oldest is dropped when full).
pub const MAX_HISTORY: usize = 64;

/// Back/forward list: absolute URLs, the cursor on the page being shown. Never
/// persisted (a store behind a trait comes with the persistent filesystem).
#[derive(Default)]
struct History {
    urls: alloc::vec::Vec<alloc::vec::Vec<u8>>,
    /// The document of each entry: entries a page's scripts added (`history.pushState`) share the
    /// number of the entry they were added from, and moving between them loads nothing.
    docs: alloc::vec::Vec<u32>,
    /// The last document number given.
    doc_seq: u32,
    /// Index of the current entry (meaningful when `urls` is not empty).
    cur: usize,
    /// The next successful load comes from back/forward: do not record it.
    replay: bool,
    /// Back/forward reached an entry of the same document: its index, for the page (`popstate`).
    pop: Option<usize>,
}

/// Most origins the user can allow to continue past a certificate error in one
/// session.
pub const MAX_INSECURE_HOSTS: usize = 8;

/// Hosts the user chose to open despite a certificate error. Lives only in
/// memory: it is never saved, so it ends with the browser window's state.
struct InsecureHosts {
    hosts: [[u8; HOST_CAP]; MAX_INSECURE_HOSTS],
    lens: [u8; MAX_INSECURE_HOSTS],
    n: usize,
}

/// Quick-link shortcuts shown on the start page (label key, URL). All chosen to
/// accept our P-256 TLS 1.3 handshake.
pub const QUICK_LINKS: [(&str, &str); 4] = [
    (tk!("web.quick.bing"), "www.bing.com"),
    (
        tk!("web.quick.wikipedia"),
        "en.wikipedia.org/wiki/Operating_system",
    ),
    (tk!("web.quick.cloudflare"), "www.cloudflare.com"),
    (tk!("web.quick.example"), "example.com"),
];

impl Default for Browser {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod ui_tests;

#[cfg(test)]
mod body_tests;

#[cfg(test)]
mod referrer_tests;
