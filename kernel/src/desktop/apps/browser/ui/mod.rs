//! Drawing the browser window: the toolbar with the omnibox, the tab strip, the new-tab page,
//! error pages, and what floats over the page (suggestions, the security popover, the find bar,
//! the context menu, notices). The page itself is `paint`.
//!
//! The chrome follows the system appearance; the page area keeps the page's own colours.
//! Geometry comes from `kitsune_core::layout` (the same functions hit-test clicks), colours from
//! the palette, type from `text::*`, widgets and glyphs from the toolkit.

mod errors;
mod frame;
mod helpers;
mod overlay;
mod popups;
mod shield;
mod start;
mod tabstrip;
mod toolbar;
