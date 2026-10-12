//! Text handling: decoding references in text and attributes, folding whitespace, which characters are kept.

use super::*;

/// HTML's whitespace: space, tab, line feed, form feed, carriage return.
pub(crate) fn is_html_space(b: u8) -> bool {
    matches!(b, b' ' | b'\t' | b'\n' | 0x0c | b'\r')
}

/// May `c` appear in page text? Controls, bidirectional overrides (they can make
/// text lie about its order), zero-width joiners, combining marks (the face has no
/// composition) and non-characters are dropped. A zero-width space stays: it is
/// where a long word may break.
pub(crate) fn keep_char(c: char) -> bool {
    !matches!(
        c,
        '\u{0}'..='\u{8}'
            | '\u{b}'
            | '\u{e}'..='\u{1f}'
            | '\u{7f}'..='\u{9f}'
            | '\u{200c}'..='\u{200f}'
            | '\u{202a}'..='\u{202e}'
            | '\u{2060}'..='\u{2064}'
            | '\u{2066}'..='\u{206f}'
            | '\u{061c}'
            | '\u{feff}'
            | '\u{fe00}'..='\u{fe0f}'
            | '\u{0300}'..='\u{036f}'
            | '\u{fffe}'
            | '\u{ffff}'
    )
}

/// Decode HTML entities and sanitise `text`; whitespace is kept as written
/// (line ends normalised to `\n`) because collapsing depends on the style of the
/// element the text ends up in (`white-space: pre`), which layout knows.
pub(crate) fn decode_text(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while i < bytes.len() {
        let c = bytes[i];
        if c == b'&' {
            let mut j = i + 1;
            while j < bytes.len() && j < i + 12 && bytes[j] != b';' {
                j += 1;
            }
            if j < bytes.len()
                && bytes[j] == b';'
                && let Some(ch) = text.get(i + 1..j).and_then(entity_char)
            {
                if keep_char(ch) {
                    out.push(ch);
                }
                i = j + 1;
                continue;
            }
            out.push('&');
            i += 1;
        } else if c == b'\r' {
            // CRLF and lone CR are line ends.
            out.push('\n');
            i += 1;
            if i < bytes.len() && bytes[i] == b'\n' {
                i += 1;
            }
        } else if c == 0x0c {
            out.push(' ');
            i += 1;
        } else if c < 0x80 {
            if keep_char(c as char) {
                out.push(c as char);
            }
            i += 1;
        } else {
            // `i` is on a char boundary: only whole characters are skipped.
            let ch = text[i..].chars().next().unwrap_or('\u{fffd}');
            if keep_char(ch) {
                out.push(ch);
            }
            i += ch.len_utf8();
        }
    }
    out
}

/// Decode the entities of an attribute value (`&amp;` in a URL, `&eacute;` in a
/// field value) to real characters, keeping every other character (UTF-8
/// included) as it is.
pub(crate) fn decode_attr(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let b = s.as_bytes();
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'&' {
            let mut j = i + 1;
            while j < b.len() && j < i + 12 && b[j] != b';' {
                j += 1;
            }
            if j < b.len()
                && b[j] == b';'
                && let Some(ch) = s.get(i + 1..j).and_then(entity_char)
            {
                out.push(ch);
                i = j + 1;
                continue;
            }
        }
        // `i` always sits on a char boundary: it only advances by whole characters.
        let ch = s[i..].chars().next().unwrap_or('\u{fffd}');
        out.push(ch);
        i += ch.len_utf8();
    }
    out
}

/// `s` made fit for one line of display: runs of whitespace and controls collapse to
/// one space and the characters [`keep_char`] refuses are dropped. Accents and every
/// other character are kept.
pub(crate) fn fold_display(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut last_space = false;
    for ch in s.chars() {
        let cp = ch as u32;
        if (ch.is_whitespace() && ch != '\u{a0}') || cp < 0x20 || cp == 0x7f {
            if !last_space {
                out.push(' ');
            }
            last_space = true;
        } else if keep_char(ch) {
            out.push(ch);
            last_space = false;
        }
    }
    out
}

/// The text of `nodes` (depth-limited), without markup.
pub(crate) fn text_content(nodes: &[Node]) -> String {
    pub(crate) fn walk(nodes: &[Node], depth: usize, out: &mut String) {
        if depth > 12 || out.len() > 4096 {
            return;
        }
        for n in nodes {
            match n {
                Node::Text(t) => out.push_str(t),
                Node::Element(e) => walk(&e.children, depth + 1, out),
            }
        }
    }
    let mut s = String::new();
    walk(nodes, 0, &mut s);
    s
}

/// Decode Windows-1252 bytes (Latin-1 plus the printable 0x80-0x9F block).
pub(crate) fn latin1_to_string(b: &[u8]) -> String {
    crate::format::text::windows1252_to_string(b)
}
