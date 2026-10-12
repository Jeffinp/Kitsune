//! Declaration lists (`name: value; ...`), split outside parentheses and quotes.

use super::*;

/// The declarations of a block: split at the `;` that sit outside parentheses and quotes.
pub(super) fn split_decls(text: &str) -> Vec<&str> {
    let b = text.as_bytes();
    let (mut start, mut depth, mut quote) = (0usize, 0i32, 0u8);
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        let c = b[i];
        if quote != 0 {
            if c == b'\\' {
                i += 1;
            } else if c == quote {
                quote = 0;
            }
        } else {
            match c {
                b'"' | b'\'' => quote = c,
                b'(' => depth += 1,
                b')' => depth = (depth - 1).max(0),
                b';' if depth == 0 => {
                    out.push(&text[start..i]);
                    start = i + 1;
                }
                _ => {}
            }
        }
        i += 1;
    }
    if start < text.len() {
        out.push(&text[start..]);
    }
    out
}

pub(crate) fn parse_decls(text: &str) -> Vec<Decl> {
    let text = if text.contains("/*") {
        strip_comments(text)
    } else {
        String::from(text)
    };
    let mut out = Vec::new();
    for chunk in split_decls(&text) {
        let chunk = chunk.trim();
        if chunk.is_empty() {
            continue;
        }
        if let Some((name, value)) = chunk.split_once(':') {
            let name = name.trim().to_ascii_lowercase();
            let mut value = value.trim();
            let mut important = false;
            if let Some(p) = value.rfind('!')
                && value[p + 1..].trim().eq_ignore_ascii_case("important")
            {
                important = true;
                value = value[..p].trim();
            }
            if !name.is_empty() && !value.is_empty() {
                out.push(Decl {
                    name,
                    value: value.to_string(),
                    important,
                });
            }
        }
    }
    out
}
