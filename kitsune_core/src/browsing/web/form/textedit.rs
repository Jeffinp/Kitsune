//! Text helpers for editing: wrapping a `<textarea>` into rows, line and character boundaries, the caret's row.

use super::*;

/// Split a `<textarea>` value into the rows it shows: byte ranges (without the line break) of at
/// most `max_w` pixels, where `width_of` is the width of one character. A row breaks after the
/// last space that fits, or in the middle of a word wider than the box. Every character falls in
/// one row, so a caret can always be placed.
pub fn wrap_rows(value: &str, max_w: i32, width_of: impl Fn(char) -> i32) -> Vec<(usize, usize)> {
    let mut rows = Vec::new();
    let mut line_start = 0;
    loop {
        let line_end = value[line_start..]
            .find('\n')
            .map_or(value.len(), |i| line_start + i);
        let mut a = line_start;
        loop {
            let mut w = 0;
            let mut cut = None; // the last place a row may end: just after a space
            let mut end = line_end;
            for (i, ch) in value[a..line_end].char_indices() {
                let cw = width_of(ch);
                // A space may hang past the edge, so a row ends after it, not before it.
                if w + cw > max_w && i > 0 && ch != ' ' {
                    end = cut.unwrap_or(a + i);
                    break;
                }
                w += cw;
                if ch == ' ' {
                    cut = Some(a + i + 1);
                }
            }
            rows.push((a, end));
            if end >= line_end {
                break;
            }
            a = end;
        }
        if line_end >= value.len() {
            return rows;
        }
        line_start = line_end + 1;
    }
}

/// The row of `rows` (from [`wrap_rows`]) that holds byte offset `caret`, and the offset into it.
pub fn caret_row(rows: &[(usize, usize)], caret: usize) -> (usize, usize) {
    for (k, &(a, b)) in rows.iter().enumerate() {
        let soft_wrap_next = rows.get(k + 1).is_some_and(|n| n.0 == b);
        if caret < b || (caret == b && !soft_wrap_next) {
            return (k, caret.saturating_sub(a));
        }
    }
    rows.last()
        .map_or((0, 0), |&(a, b)| (rows.len() - 1, b - a))
}

/// Longest value of a control, in bytes.
pub(crate) fn max_len(kind: FieldKind) -> usize {
    if kind == FieldKind::TextArea {
        MAX_TEXTAREA
    } else {
        MAX_VALUE
    }
}

/// Byte offset of the start of the line holding `caret`.
pub(crate) fn line_start(s: &str, caret: usize) -> usize {
    let c = caret.min(s.len());
    s[..c].rfind('\n').map_or(0, |i| i + 1)
}

/// Byte offset of the end (before the `\n`) of the line holding `caret`.
pub(crate) fn line_end(s: &str, caret: usize) -> usize {
    let c = caret.min(s.len());
    s[c..].find('\n').map_or(s.len(), |i| c + i)
}

/// The caret one line up or down, in the same column (counted in characters).
pub(crate) fn vertical(s: &str, caret: usize, up: bool) -> usize {
    let c = caret.min(s.len());
    let start = line_start(s, c);
    let col = s[start..c].chars().count();
    let target_start = if up {
        if start == 0 {
            return 0;
        }
        line_start(s, start - 1)
    } else {
        let end = line_end(s, c);
        if end >= s.len() {
            return s.len();
        }
        end + 1
    };
    let target_end = line_end(s, target_start);
    let line = &s[target_start..target_end];
    let off = line.char_indices().nth(col).map_or(line.len(), |(i, _)| i);
    target_start + off
}

pub(crate) fn prev_boundary(s: &str, i: usize) -> usize {
    let mut i = i.min(s.len());
    if i == 0 {
        return 0;
    }
    i -= 1;
    while i > 0 && !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}

pub(crate) fn next_boundary(s: &str, i: usize) -> usize {
    let mut i = i.min(s.len());
    if i >= s.len() {
        return s.len();
    }
    i += 1;
    while i < s.len() && !s.is_char_boundary(i) {
        i += 1;
    }
    i
}

pub(crate) fn truncate_at_boundary(s: &mut String, max: usize) {
    if s.len() <= max {
        return;
    }
    let mut n = max;
    while !s.is_char_boundary(n) {
        n -= 1;
    }
    s.truncate(n);
}
