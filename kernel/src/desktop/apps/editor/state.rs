//! State of an editor window: modals, hit targets and the animation state.

use crate::desktop::kit::appui;
use crate::desktop::services::vfs;
use crate::desktop::*;
use kitsune_core::anim::{Spring, Tween, curves};
use kitsune_core::editor2::ui::FindHit;
use kitsune_core::editor2::{CloseAsk, Editor as Ed2, Picker};
use kitsune_core::t;
use kitsune_core::widgets::ScrollbarFade;

/// Largest file the editor opens (the gap buffer, undo and a copy for saving all live in the heap).
pub(crate) const MAX_OPEN: u64 = 16 * 1024 * 1024;

/// Size of the "save changes?" sheet.
pub(crate) const CLOSE_SIZE: (i32, i32) = (430, 168);

/// A question or dialog over the text.
pub(crate) enum EdModal {
    Open(Picker),
    SaveAs { picker: Picker, then_close: bool },
    Close(CloseAsk),
}

/// What the pointer is over, for the hover wash.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum EdHit {
    Bar(FindHit),
    /// A button of the open sheet, left to right.
    Sheet(usize),
    Place(usize),
}

/// An editor window.
pub(crate) struct EditorState {
    pub ed: Ed2,
    /// The file the buffer belongs to; `None` for a new, unnamed document.
    pub path: Option<Vec<u8>>,
    pub modal: Option<EdModal>,
    /// Last result (saved, error), shown in the status bar until the next key.
    pub msg: Option<(String, bool)>,
    /// "Descartar" was chosen: the next close does not ask again.
    pub force_close: bool,
    /// Last text click: tick, byte offset and how many in a row (double / triple click).
    last_click: (u64, usize, u8),
    /// Where the last text press happened: a "drag" that has not moved leaves a word or line
    /// selection (double / triple click) alone.
    pub(super) press_at: (i32, i32),
    /// Tick of the last key or click: the caret holds still, then blinks (and rests again).
    pub last_input: u64,
    /// Where the caret is drawn: pixels right of the first text column. It glides along a row.
    pub caret_x: Spring,
    /// Top line, left column and screen row the caret target was last computed for: a change
    /// of any of them jumps instead of gliding.
    pub(super) caret_seen: (usize, usize, usize),
    /// The selection fading in.
    pub sel_t: Tween,
    pub(super) had_sel: bool,
    /// The sheet sliding down.
    pub sheet_t: Tween,
    pub hover: Option<EdHit>,
    pub hover_t: Tween,
    pub scroll_fade: ScrollbarFade,
    pub(super) seen_top: usize,
}

impl EditorState {
    pub(crate) fn new() -> Self {
        let mut ed = Ed2::new();
        ed.set_line_numbers(true);
        Self {
            ed,
            path: None,
            modal: None,
            msg: None,
            force_close: false,
            last_click: (0, 0, 0),
            press_at: (-1, -1),
            last_input: 0,
            caret_x: Spring::pixels(0.0, 1100.0, 66.0),
            caret_seen: (0, 0, 0),
            sel_t: Tween::at(1.0),
            had_sel: false,
            sheet_t: Tween::at(1.0),
            hover: None,
            hover_t: Tween::at(1.0),
            scroll_fade: ScrollbarFade::new(),
            seen_top: 0,
        }
    }

    /// Replace the buffer with `data` from `path`.
    pub(super) fn load(&mut self, path: Vec<u8>, data: &[u8]) {
        self.ed.load_file(data);
        self.path = Some(path);
        self.modal = None;
        self.msg = None;
    }

    /// Put a dialog or question over the text, sliding in.
    pub(super) fn raise(&mut self, m: EdModal) {
        self.modal = Some(m);
        self.sheet_t = Tween::at(0.0);
        self.sheet_t.retarget(1.0, 0.24, curves::ENTER);
        self.hover = None;
    }

    /// Count a click at `pos` (a byte offset, or a list row) at `ticks`: 1, then 2
    /// and 3 when it repeats the same spot within half a second.
    pub(super) fn click_count(&mut self, ticks: u64, pos: usize) -> u8 {
        let (t0, p0, n0) = self.last_click;
        let count = if n0 > 0 && p0 == pos && ticks.saturating_sub(t0) <= 125 {
            (n0 % 3) + 1
        } else {
            1
        };
        self.last_click = (ticks, pos, count);
        count
    }

    /// Nothing typed and nothing opened: a new file can reuse this window.
    pub(super) fn pristine(&self) -> bool {
        self.path.is_none() && self.ed.is_empty() && !self.ed.is_modified()
    }

    /// File name for the title and messages.
    pub(crate) fn name(&self) -> String {
        match &self.path {
            Some(p) => String::from_utf8_lossy(vfs::base_name(p)).into_owned(),
            None => String::from(t!("edit.untitled")),
        }
    }

    /// Whether the window needs frames: a blinking or gliding caret, a selection fading in, a
    /// sheet sliding, a fading scrollbar. `focused` windows blink their caret, others rest.
    pub(crate) fn animating(&self, focused: bool) -> bool {
        (focused && self.modal.is_none() && appui::caret_animating(self.last_input))
            || !self.caret_x.at_rest()
            || !self.sel_t.finished()
            || !self.sheet_t.finished()
            || !self.hover_t.finished()
            || self.scroll_fade.active(appui::now_ms())
    }
}
