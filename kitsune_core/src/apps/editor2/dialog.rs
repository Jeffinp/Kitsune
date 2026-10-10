//! The questions the editor window asks outside the text itself: the Open /
//! Save-as file picker, the "unsaved changes" choice and the status line.
//!
//! Pure state machines: the kernel feeds [`KeyEvent`]s (and mouse rows), lists
//! folders through the VFS when asked to, and draws what the structs expose.
//! Nothing here touches a filesystem, so every rule is unit-tested.

use super::{Eol, Status};
use crate::apps::fileman::natural_cmp;
use crate::apps::shell::fs::normalize;
use crate::system::input::{KeyCode, KeyEvent};
use alloc::string::String;
use alloc::vec::Vec;

/// Longest path or name the picker's field accepts, in characters.
pub const MAX_FIELD: usize = 255;

/// What the picker is for.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PickMode {
    Open,
    SaveAs,
}

/// One line of the folder listing.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct PickRow {
    pub name: String,
    pub dir: bool,
    pub size: u64,
}

/// What the kernel must do after a picker key or click.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum PickEvent {
    /// Nothing changed.
    None,
    /// Redraw the dialog.
    Redraw,
    /// List this folder (absolute path) and call [`Picker::set_entries`] (or
    /// [`Picker::set_error`] when it cannot be listed).
    Navigate(String),
    /// The user chose this absolute path: open it, or save to it. For a save
    /// the kernel checks whether it exists and, if so, calls
    /// [`Picker::confirm_overwrite`].
    Choose(String),
    /// The user agreed to replace the file asked about.
    Overwrite(String),
    /// The dialog was cancelled (Esc).
    Cancel,
}

/// The Open / Save-as dialog's state.
#[derive(Clone, Debug)]
pub struct Picker {
    pub mode: PickMode,
    dir: String,
    rows: Vec<PickRow>,
    sel: usize,
    scroll: usize,
    visible: usize,
    field: Vec<char>,
    caret: usize,
    error: Option<String>,
    ask: Option<String>,
    /// The field still holds the name it started with: the first typed character replaces it
    /// (as a selected text would), any other key keeps it.
    fresh: bool,
}

impl Picker {
    /// A picker showing `dir` (not listed yet: call [`Picker::set_entries`]),
    /// the name field starting as `name`.
    pub fn new(mode: PickMode, dir: &str, name: &str) -> Self {
        let field: Vec<char> = name.chars().take(MAX_FIELD).collect();
        Self {
            mode,
            dir: normalize("/", dir),
            rows: Vec::new(),
            sel: 0,
            scroll: 0,
            visible: 8,
            caret: field.len(),
            fresh: !field.is_empty(),
            field,
            error: None,
            ask: None,
        }
    }

    /// The folder being shown (absolute).
    pub fn dir(&self) -> &str {
        &self.dir
    }

    pub fn rows(&self) -> &[PickRow] {
        &self.rows
    }

    /// Index of the highlighted row.
    pub fn selected(&self) -> usize {
        self.sel
    }

    /// First visible row.
    pub fn scroll(&self) -> usize {
        self.scroll
    }

    /// The name field's text and caret (in characters).
    pub fn field(&self) -> (String, usize) {
        (self.field.iter().collect(), self.caret)
    }

    /// The name field still holds the name it started with: it is drawn selected, and the
    /// first typed character replaces it.
    pub fn field_fresh(&self) -> bool {
        self.fresh
    }

    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    /// The path the overwrite question is about, while it is open.
    pub fn asking(&self) -> Option<&str> {
        self.ask.as_deref()
    }

    /// How many rows the list shows at once (set from the window height).
    pub fn set_visible(&mut self, rows: usize) {
        self.visible = rows.max(1);
        self.keep_selection_visible();
    }

    /// Show `entries` of folder `dir`: folders first, then files, each sorted
    /// naturally, with `..` on top below the root.
    pub fn set_entries(&mut self, dir: &str, entries: Vec<PickRow>) {
        self.dir = normalize("/", dir);
        let mut rows = entries;
        rows.sort_by(|a, b| {
            b.dir
                .cmp(&a.dir)
                .then_with(|| natural_cmp(a.name.as_bytes(), b.name.as_bytes()))
        });
        if self.dir != "/" {
            rows.insert(
                0,
                PickRow {
                    name: String::from(".."),
                    dir: true,
                    size: 0,
                },
            );
        }
        self.rows = rows;
        self.error = None;
        self.sel = 0;
        self.scroll = 0;
        // Land on the file named in the field, if it is here.
        let name: String = self.field.iter().collect();
        if let Some(i) = self.rows.iter().position(|r| !r.dir && r.name == name) {
            self.sel = i;
            self.keep_selection_visible();
        }
    }

    /// A folder could not be listed: show why, keep the old listing.
    pub fn set_error(&mut self, msg: &str) {
        self.error = Some(String::from(msg));
    }

    /// Forget the error shown (its text was written in the language of the time).
    pub fn clear_error(&mut self) {
        self.error = None;
    }

    /// Ask "replace it?" about `path` (after [`PickEvent::Choose`]).
    pub fn confirm_overwrite(&mut self, path: &str) {
        self.ask = Some(String::from(path));
    }

    fn keep_selection_visible(&mut self) {
        if self.sel < self.scroll {
            self.scroll = self.sel;
        } else if self.sel >= self.scroll + self.visible {
            self.scroll = self.sel + 1 - self.visible;
        }
        let max = self.rows.len().saturating_sub(self.visible);
        self.scroll = self.scroll.min(max);
    }

    fn move_sel(&mut self, delta: isize) {
        if self.rows.is_empty() {
            return;
        }
        let last = self.rows.len() - 1;
        self.sel = if delta >= 0 {
            self.sel.saturating_add(delta as usize).min(last)
        } else {
            self.sel.saturating_sub(delta.unsigned_abs())
        };
        self.keep_selection_visible();
        // Moving onto a file puts its name in the field.
        if let Some(r) = self.rows.get(self.sel)
            && !r.dir
        {
            self.field = r.name.chars().take(MAX_FIELD).collect();
            self.caret = self.field.len();
        }
    }

    /// Scroll the list by `rows` (the mouse wheel) without moving the selection.
    pub fn scroll_by(&mut self, rows: isize) {
        let max = self.rows.len().saturating_sub(self.visible);
        self.scroll = if rows >= 0 {
            self.scroll.saturating_add(rows as usize).min(max)
        } else {
            self.scroll.saturating_sub(rows.unsigned_abs())
        };
    }

    fn row_path(&self, i: usize) -> Option<(String, bool)> {
        let r = self.rows.get(i)?;
        let p = if r.name == ".." {
            normalize(&self.dir, "..")
        } else {
            normalize(&self.dir, &r.name)
        };
        Some((p, r.dir))
    }

    fn activate_row(&mut self, i: usize) -> PickEvent {
        match self.row_path(i) {
            Some((p, true)) => PickEvent::Navigate(p),
            Some((p, false)) => {
                let name = self.rows[i].name.clone();
                self.field = name.chars().take(MAX_FIELD).collect();
                self.caret = self.field.len();
                PickEvent::Choose(p)
            }
            None => PickEvent::None,
        }
    }

    /// A click on list row `i` (index into [`Picker::rows`]); a double click
    /// opens the row.
    pub fn click(&mut self, i: usize, double: bool) -> PickEvent {
        self.fresh = false;
        if i >= self.rows.len() || self.ask.is_some() {
            return PickEvent::None;
        }
        self.sel = i;
        self.keep_selection_visible();
        if double {
            return self.activate_row(i);
        }
        if !self.rows[i].dir {
            self.field = self.rows[i].name.chars().take(MAX_FIELD).collect();
            self.caret = self.field.len();
        }
        PickEvent::Redraw
    }

    fn enter(&mut self) -> PickEvent {
        let typed: String = self.field.iter().collect();
        let typed = typed.trim();
        if typed.is_empty() {
            return self.activate_row(self.sel);
        }
        // A typed name that is a folder here (or ends with `/`) opens it.
        let p = normalize(&self.dir, typed);
        if typed.ends_with('/') || typed == "." || typed == ".." {
            self.field.clear();
            self.caret = 0;
            return PickEvent::Navigate(p);
        }
        if !typed.contains('/')
            && let Some(r) = self.rows.iter().find(|r| r.dir && r.name == typed)
        {
            let p = normalize(&self.dir, &r.name);
            self.field.clear();
            self.caret = 0;
            return PickEvent::Navigate(p);
        }
        PickEvent::Choose(p)
    }

    /// Tab: complete the field from the names in the list.
    fn complete(&mut self) {
        let typed: String = self.field.iter().collect();
        let names: Vec<&str> = self
            .rows
            .iter()
            .filter(|r| r.name != ".." && r.name.starts_with(typed.as_str()))
            .map(|r| r.name.as_str())
            .collect();
        let Some(first) = names.first() else {
            return;
        };
        let mut common: &str = first;
        for n in &names[1..] {
            let k = common
                .chars()
                .zip(n.chars())
                .take_while(|(a, b)| a == b)
                .map(|(a, _)| a.len_utf8())
                .sum::<usize>();
            common = &common[..k];
        }
        if common.chars().count() > self.field.len() {
            self.field = common.chars().take(MAX_FIELD).collect();
            self.caret = self.field.len();
        }
    }

    /// Handle a key.
    pub fn key(&mut self, ev: KeyEvent) -> PickEvent {
        if let Some(path) = self.ask.clone() {
            return match ev.code {
                KeyCode::Enter => {
                    self.ask = None;
                    PickEvent::Overwrite(path)
                }
                KeyCode::Char(c) if !ev.mods.ctrl && matches!(c, 's' | 'S' | 'y' | 'Y') => {
                    self.ask = None;
                    PickEvent::Overwrite(path)
                }
                KeyCode::Esc => {
                    self.ask = None;
                    PickEvent::Redraw
                }
                KeyCode::Char(c) if !ev.mods.ctrl && matches!(c, 'n' | 'N') => {
                    self.ask = None;
                    PickEvent::Redraw
                }
                _ => PickEvent::None,
            };
        }
        self.error = None;
        let (ctrl, alt) = (ev.mods.ctrl, ev.mods.alt);
        if core::mem::take(&mut self.fresh) {
            match ev.code {
                KeyCode::Char(_) if !ctrl && !alt => {
                    self.field.clear();
                    self.caret = 0;
                }
                KeyCode::Backspace | KeyCode::Delete => {
                    self.field.clear();
                    self.caret = 0;
                    return PickEvent::Redraw;
                }
                _ => {}
            }
        }
        match ev.code {
            KeyCode::Esc => PickEvent::Cancel,
            KeyCode::Enter => self.enter(),
            KeyCode::Up => {
                self.move_sel(-1);
                PickEvent::Redraw
            }
            KeyCode::Down => {
                self.move_sel(1);
                PickEvent::Redraw
            }
            KeyCode::PageUp => {
                self.move_sel(-(self.visible as isize));
                PickEvent::Redraw
            }
            KeyCode::PageDown => {
                self.move_sel(self.visible as isize);
                PickEvent::Redraw
            }
            KeyCode::Tab => {
                self.complete();
                PickEvent::Redraw
            }
            KeyCode::Left => {
                self.caret = self.caret.saturating_sub(1);
                PickEvent::Redraw
            }
            KeyCode::Right => {
                self.caret = (self.caret + 1).min(self.field.len());
                PickEvent::Redraw
            }
            KeyCode::Home => {
                self.caret = 0;
                PickEvent::Redraw
            }
            KeyCode::End => {
                self.caret = self.field.len();
                PickEvent::Redraw
            }
            KeyCode::Backspace => {
                if self.caret > 0 {
                    self.caret -= 1;
                    self.field.remove(self.caret);
                } else if self.field.is_empty() && self.dir != "/" {
                    // Backspace on an empty field goes up a folder.
                    return PickEvent::Navigate(normalize(&self.dir, ".."));
                }
                PickEvent::Redraw
            }
            KeyCode::Delete => {
                if self.caret < self.field.len() {
                    self.field.remove(self.caret);
                }
                PickEvent::Redraw
            }
            KeyCode::Char(c) if ctrl && !alt => {
                if c.eq_ignore_ascii_case(&'u') {
                    self.field.clear();
                    self.caret = 0;
                    PickEvent::Redraw
                } else {
                    PickEvent::None
                }
            }
            KeyCode::Char(c) if !alt => {
                if c.is_control() || self.field.len() >= MAX_FIELD {
                    return PickEvent::None;
                }
                self.field.insert(self.caret, c);
                self.caret += 1;
                PickEvent::Redraw
            }
            _ => PickEvent::None,
        }
    }
}

/// The answer to "save changes before closing?".
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum CloseChoice {
    Save,
    Discard,
    Cancel,
}

/// The three-button question shown when a window with unsaved changes closes.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct CloseAsk {
    sel: CloseChoice,
}

impl Default for CloseAsk {
    fn default() -> Self {
        Self::new()
    }
}

impl CloseAsk {
    /// Salvar is highlighted first: the safe answer is one Enter away.
    pub const fn new() -> Self {
        Self {
            sel: CloseChoice::Save,
        }
    }

    /// The highlighted button.
    pub const fn selected(&self) -> CloseChoice {
        self.sel
    }

    /// Button label keys, in order; look them up with [`CloseAsk::label`].
    pub const LABELS: [(CloseChoice, &'static str); 3] = [
        (CloseChoice::Save, crate::tk!("edit.save")),
        (CloseChoice::Discard, crate::tk!("edit.close.discard")),
        (CloseChoice::Cancel, crate::tk!("common.cancel")),
    ];

    /// The label of a button in the language in effect.
    pub fn label(choice: CloseChoice) -> &'static str {
        match choice {
            CloseChoice::Save => crate::t!("edit.save"),
            CloseChoice::Discard => crate::t!("edit.close.discard"),
            CloseChoice::Cancel => crate::t!("common.cancel"),
        }
    }

    fn index(&self) -> usize {
        match self.sel {
            CloseChoice::Save => 0,
            CloseChoice::Discard => 1,
            CloseChoice::Cancel => 2,
        }
    }

    fn set_index(&mut self, i: usize) {
        self.sel = Self::LABELS[i % 3].0;
    }

    /// Highlight a button (a mouse hover).
    pub fn select(&mut self, c: CloseChoice) {
        self.sel = c;
    }

    /// Handle a key: `Some(choice)` when the question is answered. S, D and C
    /// (or Esc for cancel) answer at once; arrows and Tab move; Enter takes
    /// the highlighted button.
    pub fn key(&mut self, ev: KeyEvent) -> Option<CloseChoice> {
        match ev.code {
            KeyCode::Esc => Some(CloseChoice::Cancel),
            KeyCode::Enter => Some(self.sel),
            KeyCode::Left => {
                self.set_index(self.index() + 2);
                None
            }
            KeyCode::Right => {
                self.set_index(self.index() + 1);
                None
            }
            KeyCode::Tab => {
                let back = ev.mods.shift;
                self.set_index(self.index() + if back { 2 } else { 1 });
                None
            }
            KeyCode::Char(c) if !ev.mods.ctrl && !ev.mods.alt => match c.to_ascii_lowercase() {
                's' => Some(CloseChoice::Save),
                'd' | 'n' => Some(CloseChoice::Discard),
                'c' => Some(CloseChoice::Cancel),
                _ => None,
            },
            _ => None,
        }
    }
}

/// The status bar text: position, size, encoding, line ending, state (in the language in
/// effect).
pub fn status_line(st: &Status, read_only: bool) -> String {
    let mut s = crate::t!(
        "edit.status.line",
        line = st.line,
        col = st.col,
        lines = &lines_text(st.total_lines),
        size = &crate::apps::fileman::format_size(st.bytes as u64),
        eol = eol_text(st.eol)
    );
    if st.selected_chars > 0 {
        s.push_str("   ");
        s.push_str(&crate::t!("edit.status.sel", n = st.selected_chars));
    }
    if read_only {
        s.push_str("   ");
        s.push_str(crate::t!("edit.status.read_only"));
    } else if st.modified {
        s.push_str("   ");
        s.push_str(crate::t!("edit.status.modified"));
    }
    s
}

fn eol_text(eol: Eol) -> &'static str {
    match eol {
        Eol::Lf => "LF",
        Eol::Crlf => "CRLF",
    }
}

/// `1 linha` / `3 linhas` (`1 line` / `3 lines`).
fn lines_text(n: usize) -> String {
    crate::tp!("edit.status.lines", n)
}

/// The pieces of the editor's status bar, ready to lay out: the cursor position on the left, the
/// document facts on the right.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct StatusBar {
    /// `Ln 12, Col 5`, with the selection size when there is one (`Ln 12, Col 5 (14 selecionados)`).
    pub position: String,
    /// `UTF-8`, `LF` or `CRLF`, the size, the line count, in the order they are drawn.
    pub facts: [String; 4],
    /// The document has unsaved changes.
    pub modified: bool,
}

/// Build the status bar pieces of `st` in the language in effect.
pub fn status_bar(st: &Status) -> StatusBar {
    let mut position = crate::t!("edit.status.pos", line = st.line, col = st.col);
    if st.selected_chars > 0 {
        position.push(' ');
        position.push_str(&crate::tp!("edit.status.selected", st.selected_chars));
    }
    StatusBar {
        position,
        facts: [
            String::from(st.encoding.label()),
            String::from(eol_text(st.eol)),
            crate::apps::fileman::format_size(st.bytes as u64),
            lines_text(st.total_lines),
        ],
        modified: st.modified,
    }
}

#[cfg(test)]
mod tests;
