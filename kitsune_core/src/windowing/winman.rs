//! Dynamic window table: any number of windows (up to a configurable limit),
//! each owning an application payload `A`, with z-order, focus, hit-testing,
//! open / close / minimize / maximize / restore, cascading placement,
//! move / resize and most-recently-used focus cycling (Alt+Tab). What is on screen is
//! planned by [`crate::windowing::compositor`].
//!
//! The table is pure logic: it never draws and knows nothing about what `A`
//! is. The kernel instantiates it with its app-instance type; the tests below
//! use plain integers. Windows are stored back-to-front, so the z-order *is*
//! the vector order and the last window is the topmost.
//!
//! The older free functions in [`crate::windowing::wm`] (which work over a fixed `order`
//! slice) keep their meaning; this module is the dynamic generalisation.

use alloc::vec::Vec;

use crate::ui::anim::{Anim, Zoom};
use crate::windowing::snap::{self, SnapZone};
use crate::windowing::window::{Rect, ResizeEdge, WindowId};

mod focus;
mod geometry;
mod helpers;
mod lifecycle;
mod workspaces;
pub use helpers::*;

/// Default cap on simultaneously open windows.
pub const DEFAULT_MAX_WINDOWS: usize = 32;
/// Offset between consecutive cascaded windows (both axes).
pub const CASCADE_STEP: i32 = 28;
/// Cascade positions before the sequence wraps back to the base position.
pub const CASCADE_WRAP: usize = 8;

/// Placement of a new window.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct WindowSpec {
    pub rect: Rect,
    pub min_w: i32,
    pub min_h: i32,
    /// Whether the window can be resized / maximized.
    pub resizable: bool,
}

/// High-level state of a window, derived from its flags.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum WinState {
    Normal,
    Minimized,
    Maximized,
}

/// What a *closing* animation does when it finishes.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Leaving {
    /// Remove the window (and drop its app).
    Destroy,
    /// Hide it but keep it (and its app) alive.
    Minimize,
    /// It slides away to another workspace.
    Workspace,
}

/// Most workspaces there can be.
pub const MAX_WORKSPACES: u8 = 4;
/// Workspaces always shown (the switcher never offers fewer).
pub const MIN_WORKSPACES: u8 = 2;

/// One window record.
pub struct Window<A> {
    pub id: WindowId,
    /// Current on-screen rectangle (the work area while maximized).
    pub rect: Rect,
    /// The rectangle to return to when un-maximizing.
    pub restore: Rect,
    pub minimized: bool,
    pub maximized: bool,
    /// Tiled to a half or a quarter of the work area (never `Maximize`: that is `maximized`).
    pub snap: Option<SnapZone>,
    pub anim: Option<Anim>,
    /// A maximise / restore in flight: the rectangle to draw travels from the old
    /// one to `rect` (see [`Zoom`]).
    pub zoom: Option<Zoom>,
    pub leaving: Leaving,
    pub min_w: i32,
    pub min_h: i32,
    pub resizable: bool,
    /// The workspace the window lives on, and whether it is hidden because that is not the
    /// current one.
    pub ws: u8,
    pub off_ws: bool,
    pub app: A,
}

impl<A> Window<A> {
    pub fn state(&self) -> WinState {
        if self.minimized {
            WinState::Minimized
        } else if self.maximized {
            WinState::Maximized
        } else {
            WinState::Normal
        }
    }

    /// Where the window sits in the snap model: `None` = free, `Some(Maximize)` = maximised,
    /// otherwise the half or quarter it is tiled to.
    pub fn snap_state(&self) -> Option<SnapZone> {
        if self.maximized {
            Some(SnapZone::Maximize)
        } else {
            self.snap
        }
    }

    /// Animating out (closing or minimizing): no longer takes focus or clicks.
    pub fn is_leaving(&self) -> bool {
        matches!(self.anim, Some(a) if a.is_closing())
    }

    /// Being destroyed (as opposed to merely minimized).
    pub fn is_closing(&self) -> bool {
        self.is_leaving() && self.leaving == Leaving::Destroy
    }

    /// On screen: not minimized and on the current workspace (an animating-out window still is).
    pub fn shown(&self) -> bool {
        !self.minimized && !self.off_ws
    }

    /// Accepts focus and clicks: shown and not animating out.
    pub fn active(&self) -> bool {
        self.shown() && !self.is_leaving()
    }

    /// The rectangle the window occupies on screen right now: `rect`, or the
    /// in-flight rectangle of a maximise / restore.
    pub fn visual_rect(&self) -> Rect {
        self.zoom.as_ref().map_or(self.rect, Zoom::rect)
    }

    /// Begin (or redirect) the maximise / restore travel from `from` to `rect`.
    fn start_zoom(&mut self, from: Rect) {
        if !self.shown() || from == self.rect {
            self.zoom = None;
            return;
        }
        match self.zoom.as_mut() {
            Some(z) => z.retarget(self.rect),
            None => self.zoom = Some(Zoom::new(from, self.rect)),
        }
    }
}

/// The dynamic window table. See the module docs.
pub struct WindowManager<A> {
    wins: Vec<Window<A>>,
    /// Most recently used first.
    mru: Vec<WindowId>,
    next_id: u32,
    limit: usize,
    /// The workspace on screen.
    cur_ws: u8,
}

impl<A> Default for WindowManager<A> {
    fn default() -> Self {
        Self::new(DEFAULT_MAX_WINDOWS)
    }
}

impl<A> WindowManager<A> {
    /// An empty table holding at most `limit` windows.
    pub fn new(limit: usize) -> Self {
        Self {
            wins: Vec::new(),
            mru: Vec::new(),
            next_id: 1,
            cur_ws: 0,
            limit,
        }
    }

    pub fn limit(&self) -> usize {
        self.limit
    }

    pub fn len(&self) -> usize {
        self.wins.len()
    }

    pub fn is_empty(&self) -> bool {
        self.wins.is_empty()
    }

    pub fn is_full(&self) -> bool {
        self.wins.len() >= self.limit
    }

    /// All windows, back to front.
    pub fn windows(&self) -> &[Window<A>] {
        &self.wins
    }

    /// Z-order position of `id` (0 = backmost).
    pub fn z_index(&self, id: WindowId) -> Option<usize> {
        self.wins.iter().position(|w| w.id == id)
    }

    pub fn get(&self, id: WindowId) -> Option<&Window<A>> {
        self.wins.iter().find(|w| w.id == id)
    }

    pub fn get_mut(&mut self, id: WindowId) -> Option<&mut Window<A>> {
        self.wins.iter_mut().find(|w| w.id == id)
    }
}

#[cfg(test)]
mod tests;
