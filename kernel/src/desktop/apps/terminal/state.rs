//! State of a terminal window and the character grid that fits it.

use crate::desktop::kit::appui;
use crate::desktop::services::shellhost::{self, Ctx};
use crate::desktop::*;
use crate::text::{self};
use alloc::boxed::Box;
use core::sync::atomic::{AtomicU32, Ordering};
use kitsune_core::settings::{FONT_MAX, FONT_MIN};
use kitsune_core::shell::Term;
use kitsune_core::t;
use kitsune_core::termui::{self, Grid, Metrics, Selection};
use kitsune_core::widgets::ScrollbarFade;

static NEXT_UID: AtomicU32 = AtomicU32::new(1);

/// A terminal window: the interactive state and the shell it talks to.
pub(crate) struct TermState {
    /// Identity the worker thread knows this terminal by.
    pub uid: u32,
    pub term: Term,
    /// The shell and its working directory; `None` while a command line runs
    /// (the worker owns them then).
    pub ctx: Option<Box<Ctx>>,
    /// The text selected with the mouse, as cells of the visible rows. It lives until the next
    /// key or output moves the rows.
    pub sel: Option<Selection>,
    /// Tick of the last key or click: the caret holds still, then blinks (and rests again).
    pub last_input: u64,
    pub scroll_fade: ScrollbarFade,
    /// The window's grid as of the last input (what a copy reads the rows with).
    pub(super) grid: (usize, usize),
    /// Last press: tick, cell and how many in a row (double: word, triple: line).
    pub(super) last_click: (u64, (usize, usize), u8),
    pub(super) press_at: (i32, i32),
}

impl TermState {
    pub(crate) fn new() -> Self {
        let ctx = Ctx::new();
        let mut term = Term::new(&shellhost::prompt_of(&ctx));
        term.print(t!("term.welcome1"));
        term.print(t!("term.welcome2"));
        Self {
            uid: NEXT_UID.fetch_add(1, Ordering::Relaxed),
            term,
            ctx: Some(ctx),
            sel: None,
            last_input: 0,
            scroll_fade: ScrollbarFade::new(),
            grid: (80, 24),
            last_click: (0, (0, 0), 0),
            press_at: (-1, -1),
        }
    }

    /// Text on the live line (what Ctrl+Shift+C copies when nothing is selected).
    pub(crate) fn input(&self) -> String {
        self.term.input()
    }

    /// The text selected with the mouse, if any.
    pub(crate) fn selection_text(&self) -> Option<String> {
        let sel = self.sel.filter(|s| !s.is_empty())?;
        let v = self.term.view_in(self.grid.0, self.grid.1);
        let t = termui::extract(&v.rows, &sel);
        (!t.is_empty()).then_some(t)
    }

    /// Whether the window needs frames beyond a running command: a blinking caret, a fading
    /// scrollbar.
    pub(crate) fn animating(&self, focused: bool) -> bool {
        self.blinking(focused) || self.moving()
    }

    /// Only the caret's blink needs frames (an ambient animation: it can run at a low rate).
    pub(crate) fn blinking(&self, focused: bool) -> bool {
        focused && appui::caret_animating(self.last_input)
    }

    /// The scrollbar fades: frames at the full animation rate.
    pub(crate) fn moving(&self) -> bool {
        self.scroll_fade.active(appui::now_ms())
    }

    pub(super) fn click_count(&mut self, ticks: u64, cell: (usize, usize)) -> u8 {
        let (t0, c0, n0) = self.last_click;
        let n = if n0 > 0 && c0 == cell && ticks.saturating_sub(t0) <= 125 {
            (n0 % 3) + 1
        } else {
            1
        };
        self.last_click = (ticks, cell, n);
        n
    }
}

/// The text size in pixels (a setting shared by all terminals).
pub(super) fn font_px() -> u16 {
    crate::settings::get()
        .terminal_font
        .clamp(FONT_MIN, FONT_MAX) as u16
}

/// The character cell: the face's pitch and a line a little taller than its natural height.
fn metrics() -> Metrics {
    let (cw, lh) = text::mono_cell_px(font_px());
    Metrics { cw, lh: lh + 2 }
}

/// The geometry of a terminal window of rectangle `r`.
pub(crate) fn term_grid(r: Rect) -> Grid {
    termui::layout(r, metrics())
}
