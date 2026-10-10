//! File manager logic: everything about the Arquivos window that is not pixels.
//!
//! The kernel draws and routes input; this module decides:
//!
//! * [`FileView`]: one window's state (current path, rows, sort, search filter, selection,
//!   history) and how it reloads from a [`Backend`]. The trash is the pseudo path
//!   [`TRASH_PATH`], so navigation history, the path bar and the sidebar treat it like any
//!   other place.
//! * Sorting ([`natural_cmp`], [`Sort`]), multi-selection ([`Selection`]: click,
//!   Ctrl+click, Shift+click, Ctrl+A, Shift+arrows), breadcrumbs, [`History`], [`Place`].
//! * [`ui`]: the window's geometry and hit-testing, the list and icon-grid models, the
//!   path bar, rubber band, drag-and-drop rules, the smooth scroller and the preview
//!   helpers, shared by the renderer and the mouse handler so they cannot disagree.
//! * Small pure helpers: [`format_size`], [`format_datetime`], [`display_ascii`]
//!   (folding names for the accent-insensitive search), [`TextInput`] (the inline name
//!   editor with selection), [`PathClip`] (the shared copy/cut clipboard), [`classify`]
//!   (what "open" means for a file), [`context_menu`].

pub mod apps;
pub mod ui;

use crate::storage::vfs::{self, Backend, Entry, EntryKind, VfsError};
use alloc::string::String;
use alloc::vec::Vec;
use core::cmp::Ordering;

pub mod archive;
mod clipboard;
mod commands;
mod format;
mod input;
mod open;
mod paths;
mod places;
mod rows;
mod selection;
mod view;
pub use clipboard::*;
pub use commands::*;
pub use format::*;
pub use input::*;
pub use open::*;
pub use paths::*;
pub use places::*;
pub use rows::*;
pub use selection::*;
pub use view::*;

/// The trash as a location.
pub const TRASH_PATH: &[u8] = b"/.trash";
/// The Apps place (installed and bundled packages) as a location, like the trash a
/// pseudo path: history, the address bar and the sidebar treat it as any other place.
pub const APPS_PATH: &[u8] = b"/.apps";
/// Most rows a view loads (a folder with more shows the first ones).
pub const MAX_ROWS: usize = 20_000;

// ---------------------------------------------------------------------------
// Rows and sorting
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// Selection
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// Paths, breadcrumbs, history
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// Formatting
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// Inline text input
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// Clipboard of paths
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// What "open" means
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// Commands and context menu
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// Places
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// The view
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests;
