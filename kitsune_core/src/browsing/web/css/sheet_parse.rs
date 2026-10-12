//! Reading a stylesheet: rules, `@media`/`@supports`/`@layer` blocks and `@import`.

use super::*;

/// Drop `/* ... */` comments.
pub(super) fn strip_comments(input: &str) -> String {
    if !input.contains("/*") {
        return String::from(input);
    }
    let mut out = String::with_capacity(input.len());
    let mut rest = input;
    while let Some(i) = rest.find("/*") {
        out.push_str(&rest[..i]);
        match rest[i + 2..].find("*/") {
            Some(j) => rest = &rest[i + 2 + j + 2..],
            None => return out,
        }
    }
    out.push_str(rest);
    out
}

/// Parse a stylesheet. Tolerant: malformed rules and unsupported `@` rules are
/// skipped without aborting the parse. `@media` blocks keep their rules with the condition
/// ([`Media`]), checked against the viewport at layout time; `@supports` and `@layer` blocks are
/// read as if their content stood alone.
pub fn parse_css(input: &str) -> Stylesheet {
    let mut end = input.len().min(MAX_CSS_BYTES);
    while !input.is_char_boundary(end) {
        end -= 1;
    }
    let text = strip_comments(&input[..end]);
    let mut st = ParseState::default();
    parse_into(text.as_bytes(), 0, 0, &mut st);
    let has_pseudo = st.rules.iter().any(|r| r.pseudo != 0);
    Stylesheet {
        rules: st.rules,
        has_pseudo,
        medias: st.medias,
        imports: st.imports,
        ..Default::default()
    }
}

#[derive(Default)]
pub(super) struct ParseState {
    pub(super) rules: Vec<Rule>,
    pub(super) selector_total: usize,
    pub(super) medias: Vec<Media>,
    pub(super) imports: Vec<String>,
}

/// Deepest nesting of `@media` / `@supports` / `@layer` honoured.
pub(super) const MAX_AT_DEPTH: u8 = 3;

/// Rules of `b` (the text of a sheet or of an `@media` block), tagged with `media`.
pub(super) fn parse_into(b: &[u8], media: u16, depth: u8, st: &mut ParseState) {
    let mut i = 0;
    while i < b.len() {
        if st.rules.len() >= MAX_RULES || st.selector_total >= MAX_SELECTORS {
            break;
        }
        i = skip_css_ws(b, i);
        if i >= b.len() {
            break;
        }
        if b[i] == b'@' {
            let start = i;
            let end = skip_at_rule(b, i);
            let at = &b[start..end];
            let name_end = at[1..]
                .iter()
                .position(|c| !c.is_ascii_alphabetic() && *c != b'-')
                .map_or(at.len(), |p| p + 1);
            let name = core::str::from_utf8(&at[1..name_end])
                .unwrap_or("")
                .to_ascii_lowercase();
            let open = at.iter().position(|&c| c == b'{');
            match (name.as_str(), open) {
                ("import", None) => {
                    let prelude = core::str::from_utf8(&at[name_end..]).unwrap_or("");
                    if let Some(url) = import_url(prelude)
                        && st.imports.len() < 16
                    {
                        st.imports.push(url);
                    }
                }
                ("supports", Some(o))
                    if core::str::from_utf8(&at[name_end..o])
                        .unwrap_or("")
                        .trim_start()
                        .starts_with("not") => {}
                ("media" | "supports" | "layer", Some(o)) if depth < MAX_AT_DEPTH => {
                    let prelude = core::str::from_utf8(&at[name_end..o]).unwrap_or("");
                    let inner_end = at.iter().rposition(|&c| c == b'}').unwrap_or(at.len());
                    let inner = &at[(o + 1).min(inner_end)..inner_end];
                    let m = match name.as_str() {
                        "media" => match Media::parse(prelude) {
                            Some(m) if st.medias.len() < usize::from(u16::MAX) - 1 => {
                                st.medias.push(m);
                                Some(st.medias.len() as u16)
                            }
                            _ => None,
                        },
                        // `@supports (...)`: every property we know is supported often enough.
                        // `@layer name { ... }`: the order of layers is not modelled.
                        _ => Some(media),
                    };
                    if let Some(m) = m {
                        // A nested `@media` inside another one needs both: keep the inner
                        // condition only when the outer is none (the common case).
                        let tag = if media != 0 && name == "media" {
                            media
                        } else {
                            m
                        };
                        parse_into(inner, tag, depth + 1, st);
                    }
                }
                _ => {}
            }
            i = end;
            continue;
        }
        // Selector list up to '{'.
        let sel_start = i;
        while i < b.len() && b[i] != b'{' && b[i] != b'}' {
            i += 1;
        }
        if i >= b.len() || b[i] == b'}' {
            // A stray '}' (the end of something unsupported): skip it and go on.
            if i < b.len() {
                i += 1;
                continue;
            }
            break;
        }
        let sel_text = core::str::from_utf8(&b[sel_start..i]).unwrap_or("");
        i += 1; // '{'
        let decl_start = i;
        // The block ends at the '}' outside quotes and parentheses (`url(data:...;base64,...)`).
        let (mut depth_p, mut quote) = (0i32, 0u8);
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
                    b'(' => depth_p += 1,
                    b')' => depth_p = (depth_p - 1).max(0),
                    b'}' if depth_p == 0 => break,
                    _ => {}
                }
            }
            i += 1;
        }
        let decl_text = core::str::from_utf8(&b[decl_start..i.min(b.len())]).unwrap_or("");
        if i < b.len() {
            i += 1; // '}'
        }
        let mut selectors = parse_selectors(sel_text);
        // Keep only as many selectors as the sheet-wide budget has left.
        selectors.truncate(MAX_SELECTORS - st.selector_total);
        let decls = parse_decls(decl_text);
        if !selectors.is_empty() && !decls.is_empty() {
            st.selector_total += selectors.len();
            // A list that mixes elements and generated boxes becomes one rule per kind.
            for kind in 0..3u8 {
                let group: Vec<Selector> = selectors
                    .iter()
                    .filter(|s| s.pseudo_el == kind)
                    .cloned()
                    .collect();
                if !group.is_empty() {
                    st.rules.push(Rule {
                        selectors: group,
                        decls: decls.clone(),
                        author: true,
                        pseudo: kind,
                        media,
                    });
                }
            }
        }
    }
}

/// The address in `@import url(x.css) screen;` or `@import "x.css";` (screen-compatible imports only).
pub(super) fn import_url(prelude: &str) -> Option<String> {
    let p = prelude.trim().trim_end_matches(';').trim();
    let (url, rest) = if let Some(r) = p.strip_prefix("url(") {
        let close = r.find(')')?;
        (r[..close].trim(), r[close + 1..].trim())
    } else {
        let q = p.chars().next().filter(|c| *c == '"' || *c == '\'')?;
        let r = &p[1..];
        let close = r.find(q)?;
        (&r[..close], r[close + 1..].trim())
    };
    let url = url.trim_matches(|c| c == '"' || c == '\'');
    // A condition after the address (`print`, `(min-width: ...)`): only when it can hold on a screen.
    if !rest.is_empty() && Media::parse(rest).is_none_or(|m| !m.matches(DEFAULT_W, DEFAULT_H)) {
        return None;
    }
    (!url.is_empty()).then(|| url.to_string())
}

pub(super) fn skip_css_ws(b: &[u8], mut i: usize) -> usize {
    while i < b.len() && b[i].is_ascii_whitespace() {
        i += 1;
    }
    i
}

pub(super) fn skip_at_rule(b: &[u8], mut i: usize) -> usize {
    while i < b.len() && b[i] != b'{' && b[i] != b';' {
        i += 1;
    }
    if i < b.len() && b[i] == b';' {
        return i + 1;
    }
    // Balanced block.
    let mut depth = 0;
    while i < b.len() {
        match b[i] {
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return i + 1;
                }
            }
            _ => {}
        }
        i += 1;
    }
    i
}
