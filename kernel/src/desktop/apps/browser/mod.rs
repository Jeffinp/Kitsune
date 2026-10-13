//! Navegador, the browser window: tabs, the hand-off with the network fetcher, laying pages out
//! and scrolling them, drawing, keys and clicks.
//!
//! # Tabs and the single fetcher
//!
//! One window holds up to [`MAX_TABS`](kitsune_core::browser::tabs::MAX_TABS) tabs, each with
//! its own history, page, scroll, forms and find state ([`TabData`]). There is one fetcher
//! thread and it serves one request at a time, so tabs share it by queueing: the main loop asks
//! for a request, the active tab is asked first and the others after it in order, and the
//! answer goes back to the tab that asked (`req_tab`), active or not. A tab that is not active
//! when its page arrives keeps the parsed document and is laid out when it is shown (inactive
//! tabs do not hold a display list or pictures). Pictures are fetched for the active tab only.
//!
//! - `state`, `model` the window state and its methods
//! - `fetch`, `page`, `tabs`, `layout` loading, scrolling, tabs and page layout
//! - `script` the page's JavaScript (a ring 3 process per tab with scripts); `jsnet` its network
//!   requests (`fetch`, XHR) and cookies; `jsinput` keys, focus, mouse and the layout snapshot for it;
//!   `jsstore` its `localStorage`; `jsworker` its Web Workers; `media` the players of its
//!   `<video>`/`<audio>` elements
//! - `shield` the ad blocker's side of it: the per-site list on disk, the toggle, the popover's geometry
//! - `keys`, `mouse`, `hover` input
//! - `ui` the chrome (toolbar, tab strip, start and error pages, popups); `paint` the page

mod fetch;
mod hover;
mod jsinput;
mod jsnet;
mod jsstore;
mod jsworker;
mod keys;
mod layout;
mod media;
mod model;
mod mouse;
mod page;
pub(super) mod paint;
mod script;
mod shield;
mod state;
mod tabs;
mod ui;

pub(crate) use media::note_present;
pub(crate) use model::SecurityTone;
pub(crate) use paint::PaintCache;
pub(crate) use state::*;
