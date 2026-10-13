//! The browser's cookie jar (`kitsune_core::cookies`) in the kernel: one jar for the browser, used by
//! the fetcher for every request it makes for a page (a `Cookie` line out, `Set-Cookie` lines in)
//! and by the browser for `document.cookie`. Apps (`net_http_get`) and the shell (`curl`) never see
//! it.
//!
//! The context of a request: a navigation is first-party to itself; a sub-request (sheet, picture,
//! script, `fetch`) is same-site or not with the page shown in the active tab ([`set_page`]). When
//! that page's ad blocker is on, a cross-site sub-request neither sends nor stores cookies
//! (third-party cookies blocked). The persistent cookies are saved by the browser in `~/.cookies`
//! ([`take_save`], [`load`]).

use super::*;
use crate::allocator::SpinLock;
use kitsune_core::cookies::{Context, Jar, Site, same_site};
use kitsune_core::http1::{Target, header_lines};

static JAR: SpinLock<Option<Jar>> = SpinLock::new(None);
/// The page of the active tab and whether its blocker is on (third-party cookies blocked).
static PAGE: SpinLock<Option<(String, bool)>> = SpinLock::new(None);
/// Whose `~/.cookies` the jar was loaded from.
static OWNER: SpinLock<Vec<u8>> = SpinLock::new(Vec::new());

/// Unix time in milliseconds for expiry decisions (0 when the clock is unknown).
pub fn unix_ms() -> i64 {
    crate::clock::trusted_unix_ms()
        .or_else(crate::clock::local_unix_ms)
        .unwrap_or(0) as i64
}

fn with<R>(f: impl FnOnce(&mut Jar) -> R) -> R {
    let mut g = JAR.lock();
    f(g.get_or_insert_with(Jar::new))
}

fn site(t: &Target) -> Site<'_> {
    Site {
        https: t.https,
        host: &t.host,
        path: t.path_only(),
    }
}

/// The page shown in the active tab (its address) and whether its blocker is on.
pub fn set_page(url: &[u8], shield_on: bool) {
    let url = String::from_utf8_lossy(url).into_owned();
    *PAGE.lock() = Some((url, shield_on));
}

/// What a request is for, as far as cookies go.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Use {
    /// No cookies (apps, the shell).
    None,
    /// The page itself.
    Navigation,
    /// A sub-request of the page in the active tab.
    Subresource,
}

/// The cookie context of a request to `t` (`None`: no cookies at all). `page` overrides the active
/// tab's page (a script's `fetch` names its own page).
pub(super) fn context(
    t: &Target,
    what: Use,
    safe_method: bool,
    page: Option<(&str, bool)>,
) -> Option<Context> {
    match what {
        Use::None => None,
        Use::Navigation => Some(Context {
            safe_method,
            ..Context::NAVIGATION
        }),
        Use::Subresource => {
            let (page_host, shield) = match page {
                Some((u, s)) => (Target::parse(u).map(|p| p.host), s),
                None => {
                    let g = PAGE.lock();
                    let (u, s) = g.as_ref()?;
                    (Target::parse(u).map(|p| p.host), *s)
                }
            };
            let same = page_host.is_some_and(|h| same_site(&h, &t.host));
            if !same && shield {
                serial_println!("cookies: third-party cookies blocked for {}", t.host);
                return None;
            }
            Some(Context::subresource(same, safe_method))
        }
    }
}

/// The `Cookie` value for a request to `t` in `ctx` (empty: none).
pub(super) fn header_for(t: &Target, ctx: Context) -> String {
    with(|j| j.header_for(site(t), ctx, unix_ms()))
}

/// Take every `Set-Cookie` of the header block `head`, answered by `t` in `ctx`.
pub(super) fn store(t: &Target, head: &[u8], ctx: Context) {
    let now = unix_ms();
    for (n, v) in header_lines(head) {
        if !n.eq_ignore_ascii_case(b"set-cookie") {
            continue;
        }
        let Ok(v) = core::str::from_utf8(v) else {
            continue;
        };
        let took = with(|j| j.set_from_header(site(t), v, ctx, now));
        let name = v.split('=').next().unwrap_or("");
        serial_println!(
            "cookies: {} {} from {}",
            if took { "stored" } else { "refused" },
            name,
            t.host
        );
    }
}

/// `document.cookie` of the page at `url`.
pub fn document_get(url: &str) -> String {
    match Target::parse(url) {
        Some(t) => with(|j| j.document_get(site(&t), unix_ms())),
        None => String::new(),
    }
}

/// `document.cookie = text` on the page at `url`.
pub fn document_set(url: &str, text: &str) -> bool {
    match Target::parse(url) {
        Some(t) => with(|j| j.document_set(site(&t), text, unix_ms())),
        None => false,
    }
}

/// Load the saved cookies of the person whose home is `home` (from `text`), unless the jar already
/// belongs to them. Returns whether it loaded.
pub fn load(home: &[u8], text: &[u8]) -> bool {
    let mut owner = OWNER.lock();
    if owner.as_slice() == home {
        return false;
    }
    owner.clear();
    owner.extend_from_slice(home);
    drop(owner);
    let jar = Jar::parse_text(text, unix_ms());
    serial_println!("cookies: {} saved cookie(s) loaded", jar.len());
    *JAR.lock() = Some(jar);
    true
}

/// Whose cookies the jar holds (the home folder they were loaded from; empty before any load).
pub fn owner() -> Vec<u8> {
    OWNER.lock().clone()
}

/// The text of the persistent cookies when they changed since the last call.
pub fn take_save() -> Option<Vec<u8>> {
    let now = unix_ms();
    with(|j| j.take_dirty().then(|| j.to_text(now)))
}
