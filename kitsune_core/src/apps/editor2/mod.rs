//! `editor2` — a real text editor model: gap buffer, UTF-8, selection,
//! unlimited undo/redo, search/replace, soft wrap and a scrolling viewport.
//!
//! Pure logic only (no drawing, no I/O): the kernel feeds [`KeyEvent`]s and mouse
//! coordinates in, and draws what [`Editor::visible_rows`] yields. It is the v2
//! replacement for the old fixed 44x18 grid editor, which is gone.
//!
//! # Buffer choice: gap buffer + line-start index
//!
//! The text is a [`GapBuffer`] (`Vec<u8>` with a movable hole) and a sorted
//! `Vec<usize>` with the byte offset of every line. Typing is O(1) amortised
//! (the gap sits at the cursor), a jump moves the gap in one `memmove`, and the
//! text of a 2 MB file stays contiguous except for the gap, so search runs over a
//! plain slice at memory speed. A piece table gives cheaper undo and huge-file
//! edits, but every read (render, search, UTF-8 decoding) has to walk pieces,
//! and undo is already O(edit size) here because each [`undo::Edit`] stores the
//! exact removed and inserted bytes. The line index costs O(lines after the
//! edit) per keystroke (an integer add per line: ~50 µs for 50 000 lines), which
//! is the one trade-off made for O(log n) line lookup.
//!
//! # Text model
//!
//! * Bytes are kept verbatim. Invalid UTF-8 bytes are never dropped or
//!   rewritten: each is one character of width 1 shown as U+FFFD, so saving is
//!   byte-exact.
//! * A line ends at `\n`; a `\r` right before it is part of the terminator, not
//!   of the content. Mixed `\n`/`\r\n` files therefore round-trip exactly, and
//!   new lines typed with Enter use the file's first terminator style ([`Eol`]).
//! * Cursor and columns count characters (code points); every code point has
//!   display width 1, except Tab which expands to the next multiple of the tab
//!   width.
//! * Invariant: the cursor is always at a character boundary inside the content
//!   of a line (never between `\r` and `\n`), and `0 <= cursor <= len`.

mod buffer;
mod dialog;
mod keys;
mod search;
#[cfg(test)]
mod tests;
pub mod ui;
mod undo;
mod view;

pub use buffer::{GapBuffer, REPLACEMENT, TextBuf};
pub use dialog::{
    CloseAsk, CloseChoice, MAX_FIELD, PickEvent, PickMode, PickRow, Picker, StatusBar, status_bar,
    status_line,
};
pub use search::{Notice, PromptKind, PromptView};
pub use undo::{Edit, History, Kind};
pub use view::{Cell, Cells, RowView, VisibleRows};

use crate::format::text::{self, Encoding};
use crate::system::clipboard::{self, Clipboard};
use alloc::string::String;
use alloc::vec::Vec;

mod editing;
mod history;
mod selection;

/// Line terminator style.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Eol {
    Lf,
    Crlf,
}

impl Eol {
    pub const fn bytes(self) -> &'static [u8] {
        match self {
            Eol::Lf => b"\n",
            Eol::Crlf => b"\r\n",
        }
    }
}

/// What the caller should do after a key was handled.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Event {
    /// The key meant nothing to the editor.
    Ignored,
    /// The editor consumed the key (redraw).
    Handled,
    /// Ctrl+S: the caller should write [`Editor::to_bytes`] and then call
    /// [`Editor::mark_saved`].
    SaveRequested,
    /// Ctrl+Q: the caller should close the editor (check
    /// [`Editor::is_modified`] first).
    QuitRequested,
}

/// Editor options.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Config {
    /// Width of a tab stop (`>= 1`).
    pub tab_width: usize,
    /// Tab inserts spaces instead of `\t`.
    pub use_spaces: bool,
    /// Enter copies the indentation of the current line.
    pub auto_indent: bool,
    /// Show a line-number gutter.
    pub line_numbers: bool,
    /// Wrap long lines at the window width instead of scrolling sideways.
    pub soft_wrap: bool,
    /// Case-sensitive search.
    pub case_sensitive: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            tab_width: 4,
            use_spaces: true,
            auto_indent: true,
            line_numbers: false,
            soft_wrap: false,
            case_sensitive: false,
        }
    }
}

/// Snapshot for a status bar.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Status {
    /// 1-based line of the cursor.
    pub line: usize,
    /// 1-based character column of the cursor.
    pub col: usize,
    pub total_lines: usize,
    pub bytes: usize,
    pub modified: bool,
    pub eol: Eol,
    /// How the file spells its text (see [`Editor::load_file`]).
    pub encoding: Encoding,
    /// Characters currently selected.
    pub selected_chars: usize,
}

/// The editor.
pub struct Editor {
    pub(crate) text: TextBuf,
    pub(crate) cursor: usize,
    pub(crate) anchor: Option<usize>,
    /// Display column kept while moving vertically.
    pub(crate) pref_dc: usize,
    pub(crate) cfg: Config,
    pub(crate) view: view::View,
    pub(crate) hist: History,
    pub(crate) eol: Eol,
    pub(crate) encoding: Encoding,
    pub(crate) srch: search::Search,
    pub(crate) prompt: Option<search::Prompt>,
    pub(crate) notice: Notice,
}

impl Default for Editor {
    fn default() -> Self {
        Self::new()
    }
}

/// Character class used by word movement.
fn class(c: char) -> u8 {
    if c.is_alphanumeric() || c == '_' {
        2
    } else if c.is_whitespace() {
        0
    } else {
        1
    }
}

/// Cut `data` to at most `max` bytes without splitting a UTF-8 sequence.
pub fn truncate_utf8(data: &[u8], max: usize) -> &[u8] {
    if data.len() <= max {
        return data;
    }
    let mut end = max;
    while end > 0 && buffer::is_cont(data[end]) {
        end -= 1;
    }
    &data[..end]
}

impl Editor {
    /// An empty, clean editor with a 24x80 window.
    pub fn new() -> Self {
        Self::from_bytes(b"")
    }

    /// An editor holding `data` (not modified).
    pub fn from_bytes(data: &[u8]) -> Self {
        let mut e = Self {
            text: TextBuf::new(),
            cursor: 0,
            anchor: None,
            pref_dc: 0,
            cfg: Config::default(),
            view: view::View::new(24, 80),
            hist: History::new(),
            eol: Eol::Lf,
            encoding: Encoding::Utf8,
            srch: search::Search::default(),
            prompt: None,
            notice: Notice::None,
        };
        e.set_text(data);
        e
    }

    /// Replace the whole buffer (loading a file). Clears history, selection and
    /// scroll, and marks the buffer clean.
    pub fn set_text(&mut self, data: &[u8]) {
        self.text.set(data);
        self.eol = detect_eol(data);
        self.encoding = Encoding::Utf8;
        self.cursor = 0;
        self.anchor = None;
        self.pref_dc = 0;
        self.hist.clear();
        self.prompt = None;
        self.notice = Notice::None;
        self.view.reset_scroll();
    }

    /// Load the bytes of a file: UTF-16 and Windows-1252 text are converted to UTF-8 for editing
    /// (see [`crate::format::text`]) and written back the way they came by
    /// [`Editor::file_bytes`]. Anything else is loaded as it is.
    pub fn load_file(&mut self, raw: &[u8]) {
        let d = text::decode(raw);
        self.set_text(&d.text);
        self.encoding = d.encoding;
    }

    /// The bytes to save: the text in the encoding the file had, or in UTF-8 when it holds a
    /// character that encoding cannot write (the document then becomes a UTF-8 one).
    pub fn file_bytes(&mut self) -> Vec<u8> {
        let utf8 = self.text.to_vec();
        match text::encode(&utf8, self.encoding) {
            Some(b) => b,
            None => {
                self.encoding = Encoding::Utf8;
                utf8
            }
        }
    }

    pub fn encoding(&self) -> Encoding {
        self.encoding
    }

    // ---- configuration -------------------------------------------------

    pub fn config(&self) -> &Config {
        &self.cfg
    }

    pub fn set_tab_width(&mut self, w: usize) {
        self.cfg.tab_width = w.clamp(1, 16);
        self.pref_dc = self.text.dc_of(self.cursor, self.cfg.tab_width);
        self.ensure_visible();
    }

    pub fn set_use_spaces(&mut self, on: bool) {
        self.cfg.use_spaces = on;
    }

    pub fn set_auto_indent(&mut self, on: bool) {
        self.cfg.auto_indent = on;
    }

    pub fn set_line_numbers(&mut self, on: bool) {
        self.cfg.line_numbers = on;
        self.ensure_visible();
    }

    pub fn set_soft_wrap(&mut self, on: bool) {
        self.cfg.soft_wrap = on;
        self.view.reset_scroll_x();
        self.ensure_visible();
    }

    pub fn set_case_sensitive(&mut self, on: bool) {
        self.cfg.case_sensitive = on;
    }

    /// Bound the memory used by undo history (`usize::MAX` = unlimited, the
    /// default). The oldest groups are dropped first.
    pub fn set_undo_limit(&mut self, bytes: usize) {
        self.hist.set_limit(bytes);
    }

    pub fn eol(&self) -> Eol {
        self.eol
    }

    pub fn set_eol(&mut self, eol: Eol) {
        self.eol = eol;
    }

    // ---- reading -------------------------------------------------------

    pub fn len_bytes(&self) -> usize {
        self.text.len()
    }

    pub fn is_empty(&self) -> bool {
        self.text.is_empty()
    }

    pub fn line_count(&self) -> usize {
        self.text.line_count()
    }

    /// The whole buffer, byte-exact (what to write on save).
    pub fn to_bytes(&self) -> Vec<u8> {
        self.text.to_vec()
    }

    /// The buffer as two slices (before/after the gap) without copying; write
    /// both in order to save a large file with no extra allocation.
    pub fn as_slices(&self) -> (&[u8], &[u8]) {
        self.text.as_slices()
    }

    /// Content of line `l` (without its terminator).
    pub fn line_bytes(&self, l: usize) -> Vec<u8> {
        self.text
            .copy_range(self.text.line_start(l), self.text.line_end(l))
    }

    /// Content of line `l` as text (invalid bytes become U+FFFD).
    pub fn line_string(&self, l: usize) -> String {
        String::from_utf8_lossy(&self.line_bytes(l)).into_owned()
    }

    /// Number of characters in line `l`.
    pub fn line_chars(&self, l: usize) -> usize {
        self.text.line_chars(l)
    }

    /// Cursor as `(line, column)`, both 0-based; the column counts characters.
    pub fn cursor(&self) -> (usize, usize) {
        (
            self.text.line_of(self.cursor),
            self.text.col_of(self.cursor),
        )
    }

    /// Cursor as a byte offset.
    pub fn cursor_byte(&self) -> usize {
        self.cursor
    }

    /// Place the cursor at `(line, col)` (clamped); clears the selection.
    pub fn set_cursor(&mut self, line: usize, col: usize) {
        let p = self.text.pos_of(line, col);
        self.move_to(p, false, false);
    }

    pub fn status(&self) -> Status {
        let (line, col) = self.cursor();
        let selected_chars = self.sel_range().map_or(0, |(a, b)| {
            let mut n = 0;
            let mut p = a;
            while p < b {
                p = self.text.next_boundary(p);
                n += 1;
            }
            n
        });
        Status {
            line: line + 1,
            col: col + 1,
            total_lines: self.text.line_count(),
            bytes: self.text.len(),
            modified: self.is_modified(),
            eol: self.eol,
            encoding: self.encoding,
            selected_chars,
        }
    }

    /// Verify the internal invariants (cursor validity, line index, scroll
    /// state). O(text length); for tests and fuzzing.
    pub fn check_invariants(&self) -> Result<(), &'static str> {
        if !self.text.lines_consistent() {
            return Err("line index out of sync with the text");
        }
        if self.cursor > self.text.len() {
            return Err("cursor past the end");
        }
        if self.normalize(self.cursor) != self.cursor {
            return Err("cursor not at a valid position");
        }
        if self.anchor.is_some_and(|a| a > self.text.len()) {
            return Err("selection anchor past the end");
        }
        if self.view.top >= self.text.line_count() {
            return Err("scroll position past the last line");
        }
        if self.view.rows == 0 || self.view.cols == 0 {
            return Err("empty viewport");
        }
        let mut shown = 0;
        for row in self.visible_rows() {
            shown += 1;
            if row.cells().count() > self.text_cols() {
                return Err("row wider than the window");
            }
        }
        if shown > self.view.rows {
            return Err("more rows than the window holds");
        }
        if let Some((r, c)) = self.cursor_screen()
            && (r >= self.view.rows || c >= self.view.cols)
        {
            return Err("cursor drawn outside the window");
        }
        Ok(())
    }

    // ---- modified state ------------------------------------------------

    /// True when the text differs from what was last saved/loaded. Undoing
    /// back to the saved state makes it false again.
    pub fn is_modified(&self) -> bool {
        self.hist.modified()
    }

    /// Call after the file was written successfully.
    pub fn mark_saved(&mut self) {
        self.hist.mark_saved();
    }

    pub fn can_undo(&self) -> bool {
        self.hist.can_undo()
    }

    pub fn can_redo(&self) -> bool {
        self.hist.can_redo()
    }

    // ---- selection -----------------------------------------------------
}

fn detect_eol(data: &[u8]) -> Eol {
    match data.iter().position(|&b| b == b'\n') {
        Some(i) if i > 0 && data[i - 1] == b'\r' => Eol::Crlf,
        _ => Eol::Lf,
    }
}
