//! The values of the controls: reading them, checking boxes and radio buttons, choosing an `<option>`.

use super::*;

impl FormState {
    /// State for `forms`, every control at its initial value, nothing focused.
    pub fn new(forms: &[FormInfo]) -> Self {
        FormState {
            values: forms
                .iter()
                .map(|f| {
                    f.fields
                        .iter()
                        .map(|fi| {
                            if matches!(fi.kind, FieldKind::Checkbox | FieldKind::Radio) {
                                // The state of a toggle: "1" when checked.
                                return String::from(if fi.checked { "1" } else { "" });
                            }
                            if fi.kind == FieldKind::Select {
                                // The index of the chosen option.
                                let i = fi.selected.min(fi.options.len().saturating_sub(1));
                                return alloc::format!("{i}");
                            }
                            let mut v = fi.value.clone();
                            truncate_at_boundary(&mut v, max_len(fi.kind));
                            v
                        })
                        .collect()
                })
                .collect(),
            focus: None,
            caret: 0,
            compose: Compose::new(),
        }
    }

    /// The text of a control (empty for an unknown one).
    pub fn value(&self, form: usize, field: usize) -> &str {
        self.values
            .get(form)
            .and_then(|f| f.get(field))
            .map_or("", String::as_str)
    }

    /// Is the check box or radio button checked?
    pub fn is_checked(&self, form: usize, field: usize) -> bool {
        !self.value(form, field).is_empty()
    }

    /// The index of the option a `<select>` shows.
    pub fn select_index(&self, form: usize, field: usize) -> usize {
        self.value(form, field).parse().unwrap_or(0)
    }

    /// The label of the option a `<select>` shows (empty when it has none).
    pub fn select_label<'a>(&self, forms: &'a [FormInfo], form: usize, field: usize) -> &'a str {
        forms
            .get(form)
            .and_then(|f| f.fields.get(field))
            .and_then(|fi| fi.options.get(self.select_index(form, field)))
            .map_or("", |o| o.label.as_str())
    }

    /// Choose another option of a `<select>`: `delta` steps, wrapping round. Returns whether
    /// the choice changed.
    pub fn step_select(
        &mut self,
        forms: &[FormInfo],
        form: usize,
        field: usize,
        delta: i32,
    ) -> bool {
        let Some(fi) = forms.get(form).and_then(|f| f.fields.get(field)) else {
            return false;
        };
        let n = fi.options.len() as i32;
        if fi.kind != FieldKind::Select || n < 2 {
            return false;
        }
        let now = self.select_index(form, field) as i32;
        let next = (now + delta).rem_euclid(n) as usize;
        if let Some(v) = self.values.get_mut(form).and_then(|r| r.get_mut(field)) {
            *v = alloc::format!("{next}");
        }
        true
    }

    /// Jump to the next option whose label starts with `c` (any case), after the current one.
    pub(super) fn select_by_letter(
        &mut self,
        forms: &[FormInfo],
        form: usize,
        field: usize,
        c: char,
    ) {
        let Some(fi) = forms.get(form).and_then(|f| f.fields.get(field)) else {
            return;
        };
        let n = fi.options.len();
        let now = self.select_index(form, field);
        let c = c.to_ascii_lowercase();
        for k in 1..=n {
            let i = (now + k) % n;
            if fi.options[i]
                .label
                .chars()
                .next()
                .is_some_and(|f| f.to_ascii_lowercase() == c)
            {
                if let Some(v) = self.values.get_mut(form).and_then(|r| r.get_mut(field)) {
                    *v = alloc::format!("{i}");
                }
                return;
            }
        }
    }

    /// Flip a check box, or select a radio button (clearing the others of its group).
    /// Returns whether anything changed.
    pub fn toggle(&mut self, forms: &[FormInfo], form: usize, field: usize) -> bool {
        let Some(f) = forms.get(form) else {
            return false;
        };
        let Some(fi) = f.fields.get(field) else {
            return false;
        };
        let set = |s: &mut Self, i: usize, on: bool| {
            if let Some(v) = s.values.get_mut(form).and_then(|r| r.get_mut(i)) {
                *v = String::from(if on { "1" } else { "" });
            }
        };
        match fi.kind {
            FieldKind::Checkbox => {
                let now = !self.is_checked(form, field);
                set(self, field, now);
                true
            }
            FieldKind::Radio => {
                if self.is_checked(form, field) {
                    return false;
                }
                for (i, other) in f.fields.iter().enumerate() {
                    if other.kind == FieldKind::Radio && other.name == fi.name {
                        set(self, i, i == field);
                    }
                }
                true
            }
            _ => false,
        }
    }
}
