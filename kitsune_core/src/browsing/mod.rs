//! The browser engine: the HTML / CSS / layout engine (`web`), the browser model (`browser`: URLs,
//! tabs, history, bookmarks, error pages, TLS status), HTTP redirect policy (`redirect`), HTTP/1.1 framing (`http1`), cookies
//! (`cookies`), the same-origin rules for scripts (`cors`) and the ad and tracker blocker (`shields`).
//!
//! May depend on: `format` (decoders), `network` (TLS and redirects), `ui` (motion preferences),
//! `i18n`. See `docs/design/code-structure.md`.

pub mod browser;
pub mod cookies;
pub mod cors;
pub mod http1;
pub mod media;
pub mod redirect;
pub mod shields;
pub mod web;
