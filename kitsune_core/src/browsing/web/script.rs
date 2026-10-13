//! The page's JavaScript, as the browser sees it: which scripts a page has and fetching them
//! ([`ScriptSet`]), the messages exchanged with the script process ([`init_message`],
//! [`click_message`], [`parse_message`]) and the clicks waiting for the page's verdict ([`Session`]).
//!
//! The engine itself runs in a ring 3 process (`user-apps/jsrun`, QuickJS) that holds its own copy of
//! the DOM: the browser sends it the parsed tree and the script sources once, then events; the process
//! answers with the whole document as HTML whenever a task changed it, every element carrying its id in
//! [`super::UID_ATTR`], and the browser takes it with [`super::Doc::replace_from_script`] and lays the
//! page out again. Nothing here trusts the process: what it sends goes through the same tolerant HTML
//! parser as a page from the network. See `docs/design/tls-browser.md` (§12).

use super::Doc;
use super::dom::{Node, decode_attr};
use super::imgcache::image_key;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

mod collect;
mod session;
mod wire;
pub use collect::*;
pub use session::*;
pub use wire::*;

/// What scripts see as `navigator.userAgent`.
pub const USER_AGENT: &str = concat!(
    "Mozilla/5.0 (X11; Kitsune x86_64) Kitsune/",
    env!("CARGO_PKG_VERSION")
);

#[cfg(test)]
mod tests;
