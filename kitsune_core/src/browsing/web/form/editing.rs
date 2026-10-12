//! Focus and keyboard editing of the focused control: caret, Tab, typing, Enter.

use super::*;

impl FormState {
    /// The focused control, if any.
    pub fn focus(&self) -> Option<(usize, usize)> {
        self.focus
    }

    /// The caret (byte offset) in the focused control.
    pub fn caret(&self) -> usize {
        self.caret
    }

    /// Focus a control (text controls take the caret at their end; a button
    /// takes the focus for Enter). Returns whether the control exists.
    pub fn set_focus(&mut self, forms: &[FormInfo], form: usize, field: usize) -> bool {
        let Some(fi) = forms.get(form).and_then(|f| f.fields.get(field)) else {
            return false;
        };
        if fi.kind == FieldKind::Hidden {
            return false;
        }
        self.compose = Compose::new();
        self.focus = Some((form, field));
        self.caret = self.value(form, field).len();
        true
    }

    /// Drop the focus.
    pub fn blur(&mut self) {
        self.compose = Compose::new();
        self.focus = None;
        self.caret = 0;
    }

    /// Move the focus to the next (or previous) visible control in document
    /// order, across forms. From the last one it returns `false` and clears
    /// the focus (Tab leaves the page's controls).
    pub fn tab(&mut self, forms: &[FormInfo], back: bool) -> bool {
        let order: Vec<(usize, usize)> = forms
            .iter()
            .enumerate()
            .flat_map(|(i, f)| {
                f.fields
                    .iter()
                    .enumerate()
                    .filter(|(_, fi)| fi.kind != FieldKind::Hidden)
                    .map(move |(j, _)| (i, j))
            })
            .collect();
        if order.is_empty() {
            return false;
        }
        let pos = self.focus.and_then(|f| order.iter().position(|&o| o == f));
        let next = match (pos, back) {
            (None, false) => Some(0),
            (None, true) => Some(order.len() - 1),
            (Some(p), false) if p + 1 < order.len() => Some(p + 1),
            (Some(p), true) if p > 0 => Some(p - 1),
            _ => None,
        };
        match next {
            Some(n) => self.set_focus(forms, order[n].0, order[n].1),
            None => {
                self.blur();
                false
            }
        }
    }

    pub(super) fn focused_text<'a>(&'a mut self, forms: &[FormInfo]) -> Option<&'a mut String> {
        let (f, i) = self.focus?;
        if !forms.get(f)?.fields.get(i)?.kind.is_text() {
            return None;
        }
        self.values.get_mut(f)?.get_mut(i)
    }

    /// Insert `s` at the caret of the focused text control (a paste, or the
    /// result of composition). Returns whether anything was inserted.
    pub fn insert_str(&mut self, forms: &[FormInfo], s: &str) -> bool {
        let caret = self.caret;
        let multi = self
            .focus
            .and_then(|(f, i)| forms.get(f)?.fields.get(i))
            .is_some_and(|fi| fi.kind == FieldKind::TextArea);
        let max = if multi { MAX_TEXTAREA } else { MAX_VALUE };
        let Some(v) = self.focused_text(forms) else {
            return false;
        };
        let mut at = caret.min(v.len());
        while !v.is_char_boundary(at) {
            at -= 1;
        }
        let mut added = 0;
        for ch in s.chars() {
            if (ch < ' ' && !(multi && ch == '\n')) || ch == '\u{7f}' {
                continue; // no control characters in a field
            }
            if v.len() + ch.len_utf8() > max {
                break;
            }
            v.insert(at + added, ch);
            added += ch.len_utf8();
        }
        self.caret = at + added;
        added > 0
    }

    /// Handle a key for the focused control.
    pub fn on_key(&mut self, forms: &[FormInfo], key: Key) -> FormOutcome {
        let Some((form, field)) = self.focus else {
            return FormOutcome::Ignored;
        };
        let kind = match forms.get(form).and_then(|f| f.fields.get(field)) {
            Some(fi) => fi.kind,
            None => return FormOutcome::Ignored,
        };
        // Any key but a printable one lets a waiting dead key out as itself.
        if !matches!(key, Key::Char(_)) {
            let pending: Vec<char> = self.compose.flush().iter().collect();
            if !pending.is_empty() {
                let s: String = pending.into_iter().collect();
                self.insert_str(forms, &s);
            }
        }
        if kind == FieldKind::Select {
            return match key {
                Key::Up | Key::Left => {
                    self.step_select(forms, form, field, -1);
                    FormOutcome::Changed
                }
                Key::Down | Key::Right | Key::Char(b' ') => {
                    self.step_select(forms, form, field, 1);
                    FormOutcome::Changed
                }
                Key::Char(b) if b.is_ascii_graphic() => {
                    self.select_by_letter(forms, form, field, b as char);
                    FormOutcome::Changed
                }
                Key::Enter => FormOutcome::Submit {
                    form,
                    submitter: None,
                },
                Key::Esc => {
                    self.blur();
                    FormOutcome::Blur
                }
                _ => FormOutcome::Ignored,
            };
        }
        if matches!(kind, FieldKind::Checkbox | FieldKind::Radio) {
            return match key {
                Key::Char(b' ') => {
                    self.toggle(forms, form, field);
                    FormOutcome::Changed
                }
                Key::Enter => FormOutcome::Submit {
                    form,
                    submitter: None,
                },
                Key::Esc => {
                    self.blur();
                    FormOutcome::Blur
                }
                _ => FormOutcome::Ignored,
            };
        }
        if kind == FieldKind::PushButton {
            return match key {
                Key::Esc => {
                    self.blur();
                    FormOutcome::Blur
                }
                _ => FormOutcome::Ignored,
            };
        }
        if kind == FieldKind::Submit {
            return match key {
                Key::Enter | Key::Char(b' ') => FormOutcome::Submit {
                    form,
                    submitter: Some(field),
                },
                Key::Esc => {
                    self.blur();
                    FormOutcome::Blur
                }
                _ => FormOutcome::Ignored,
            };
        }
        match key {
            Key::Char(b) => {
                let c = b as char;
                if !b.is_ascii() || b < b' ' || b == 0x7f {
                    return FormOutcome::Ignored;
                }
                let out: Vec<char> = self.compose.feed(c).iter().collect();
                let s: String = out.into_iter().collect();
                if s.is_empty() {
                    return FormOutcome::Changed; // dead key waiting
                }
                self.insert_str(forms, &s);
                FormOutcome::Changed
            }
            Key::Backspace => {
                let caret = self.caret;
                if let Some(v) = self.focused_text(forms)
                    && caret > 0
                {
                    let p = prev_boundary(v, caret);
                    v.replace_range(p..caret, "");
                    self.caret = p;
                }
                FormOutcome::Changed
            }
            Key::Delete => {
                let caret = self.caret;
                if let Some(v) = self.focused_text(forms)
                    && caret < v.len()
                {
                    let n = next_boundary(v, caret);
                    v.replace_range(caret..n, "");
                }
                FormOutcome::Changed
            }
            Key::Left => {
                let caret = self.caret;
                if let Some(v) = self.focused_text(forms) {
                    self.caret = prev_boundary(v, caret);
                }
                FormOutcome::Changed
            }
            Key::Right => {
                let caret = self.caret;
                if let Some(v) = self.focused_text(forms) {
                    self.caret = next_boundary(v, caret);
                }
                FormOutcome::Changed
            }
            Key::Home => {
                let v = self.value(form, field);
                self.caret = if kind == FieldKind::TextArea {
                    line_start(v, self.caret)
                } else {
                    0
                };
                FormOutcome::Changed
            }
            Key::End => {
                let v = self.value(form, field);
                self.caret = if kind == FieldKind::TextArea {
                    line_end(v, self.caret)
                } else {
                    v.len()
                };
                FormOutcome::Changed
            }
            Key::Up | Key::Down if kind == FieldKind::TextArea => {
                let v = self.value(form, field);
                self.caret = vertical(v, self.caret, key == Key::Up);
                FormOutcome::Changed
            }
            Key::Enter if kind == FieldKind::TextArea => {
                self.insert_str(forms, "\n");
                FormOutcome::Changed
            }
            Key::Enter => FormOutcome::Submit {
                form,
                submitter: None,
            },
            Key::Esc => {
                self.blur();
                FormOutcome::Blur
            }
            // Tab belongs to the caller (`tab`).
            _ => FormOutcome::Ignored,
        }
    }
}
