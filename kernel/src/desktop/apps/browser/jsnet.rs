//! The page's scripts on the network: their requests (`fetch`, `XMLHttpRequest`, `sendBeacon`: the `F`
//! and `X` messages of the script process) and `document.cookie` (`C`), and the cookie file.
//!
//! A request first goes past the ad blocker (`PageShield::check`, the same verdict as for sheets,
//! pictures and scripts: a blocked address is a network error for the script and counts on the
//! shield), then to the fetcher's queue (`fetch::post_script_fetch`), which answers the process
//! directly. Cookies live in the fetcher's jar (`fetch::cookies`); the persistent ones are kept in
//! `~/.cookies` (`/home/<name>/.cookies`; plain `/home` is not writable by a signed-in account),
//! read when a browser window opens and written at most every few seconds after a change.

use crate::desktop::*;
use core::sync::atomic::{AtomicU64, Ordering};
use kitsune_core::web::script::{FetchRequest, cookie_message, error_message};

/// Where the cookies are kept.
fn path() -> Vec<u8> {
    let mut p = crate::desktop::services::accounts::home();
    p.extend_from_slice(b"/.cookies");
    p
}

/// Load the signed-in person's cookies into the jar (once per person).
pub(super) fn load_cookies() {
    let home = crate::desktop::services::accounts::home();
    if crate::fetch::cookies::owner() == home {
        return;
    }
    let text = crate::desktop::services::vfs::read_file(&path()).unwrap_or_default();
    crate::fetch::cookies::load(&home, &text);
}

/// When the cookie file was last written (ms since boot).
static LAST_SAVE: AtomicU64 = AtomicU64::new(0);
/// Shortest time between two writes of the cookie file (ms).
const SAVE_EVERY_MS: u64 = 3000;

/// Write the cookie file if persistent cookies changed (at most every [`SAVE_EVERY_MS`]).
pub(super) fn save_cookies() {
    let now = crate::netd::now_ms();
    if now.saturating_sub(LAST_SAVE.load(Ordering::Relaxed)) < SAVE_EVERY_MS {
        return;
    }
    // Only the owner's jar is written to the owner's home.
    if crate::fetch::cookies::owner() != crate::desktop::services::accounts::home() {
        return;
    }
    let Some(text) = crate::fetch::cookies::take_save() else {
        return;
    };
    LAST_SAVE.store(now, Ordering::Relaxed);
    match crate::desktop::services::vfs::write_file(&path(), &text) {
        Ok(_) => crate::serial_println!("cookies: saved ({} bytes)", text.len()),
        Err(_) => crate::serial_println!("cookies: could not write the cookie file"),
    }
}

/// `document.cookie` for the page of `t` when its scripts start.
pub(super) fn init_cookie(t: &TabData) -> String {
    crate::fetch::cookies::document_get(&String::from_utf8_lossy(t.browser.nav_url()))
}

/// A request from the scripts of the page shown by `browser` (process `pid`), whose blocker is
/// `shield`.
pub(super) fn on_fetch(
    shield: &mut kitsune_core::shields::PageShield,
    browser: &kitsune_core::Browser,
    pid: u32,
    req: FetchRequest,
) {
    let id = req.id;
    let n0 = shield.blocked().len();
    if shield.check(req.url.as_bytes()).is_some() {
        super::shield::log_new(shield, n0, "fetch");
        crate::userproc::page_post(pid, error_message(id, "blocked by the ad blocker"));
        return;
    }
    let job = crate::fetch::ScriptFetch {
        pid,
        page_url: String::from_utf8_lossy(browser.nav_url()).into_owned(),
        shield_on: shield.is_on(),
        insecure_host: browser.insecure_host().unwrap_or_default(),
        req,
    };
    if !crate::fetch::post_script_fetch(job) {
        crate::userproc::page_post(pid, error_message(id, "the network is unavailable"));
    }
}

/// `document.cookie = text` from the page shown by `browser`; the process gets the new `document.cookie`.
pub(super) fn on_set_cookie(browser: &kitsune_core::Browser, pid: u32, text: &str) {
    let url = String::from_utf8_lossy(browser.nav_url()).into_owned();
    crate::fetch::cookies::document_set(&url, text);
    let now = crate::fetch::cookies::document_get(&url);
    crate::userproc::page_post(pid, cookie_message(&now));
}

impl Desktop {
    /// Tell the cookie jar which page the sub-requests about to be made belong to (the active tab's).
    pub fn browser_cookie_context(&mut self) {
        let Some(id) = self.browser_id() else {
            return;
        };
        let Some(b) = self.browser_state_mut(id) else {
            return;
        };
        let t = b.tabs.active();
        crate::fetch::cookies::set_page(t.browser.nav_url(), t.shield.is_on());
    }
}
