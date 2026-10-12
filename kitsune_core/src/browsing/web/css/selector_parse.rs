//! Reading selector text: lists, complex selectors, compounds, pseudo-classes and attribute tests.

use super::*;

/// Split a selector list at the commas outside parentheses (`a:not(.b, .c), d`).
pub(super) fn split_commas(s: &str) -> Vec<&str> {
    let (mut depth, mut start) = (0i32, 0usize);
    let mut out = Vec::new();
    for (i, ch) in s.char_indices() {
        match ch {
            '(' => depth += 1,
            ')' => depth = (depth - 1).max(0),
            ',' if depth == 0 => {
                out.push(&s[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }
    out.push(&s[start..]);
    out
}

pub(super) fn parse_selectors(text: &str) -> Vec<Selector> {
    let mut out = Vec::new();
    for part in split_commas(text) {
        let part = part.trim();
        if part.is_empty() || part.len() > MAX_SELECTOR_LEN {
            continue;
        }
        if let Some(sel) = parse_complex_selector(part) {
            out.push(sel);
        }
    }
    out
}

/// Split a complex selector into compounds and combinators and build the
/// [`Selector`]. `None` for anything unsupported.
pub(super) fn parse_complex_selector(s: &str) -> Option<Selector> {
    let b = s.as_bytes();
    let mut parts: Vec<(Comb, Compound)> = Vec::new(); // (relation to the NEXT compound, compound)
    let mut i = 0;
    let mut pending = Comb::Descendant;
    while i < b.len() {
        match b[i] {
            c if c.is_ascii_whitespace() => {
                i += 1;
            }
            b'>' => {
                pending = Comb::Child;
                i += 1;
            }
            b'+' => {
                pending = Comb::Adjacent;
                i += 1;
            }
            b'~' => {
                pending = Comb::Sibling;
                i += 1;
            }
            _ => {
                let (comp, n) = parse_compound(&s[i..])?;
                i += n;
                if parts.len() >= MAX_COMPOUNDS {
                    return None;
                }
                parts.push((pending, comp));
                pending = Comb::Descendant;
            }
        }
    }
    // `pending` after the loop would be a dangling combinator ("a >"): invalid.
    if parts.is_empty() || pending != Comb::Descendant {
        return None;
    }
    // A generated box can only end a selector.
    if parts[..parts.len() - 1]
        .iter()
        .any(|(_, c)| c.pseudo_el != 0)
    {
        return None;
    }
    // `parts[k].0` is the combinator between compound k-1 and compound k.
    let n = parts.len();
    let mut ancestors: Vec<(Comb, Compound)> = Vec::new();
    for k in (0..n - 1).rev() {
        ancestors.push((parts[k + 1].0, parts[k].1.clone()));
    }
    let last = parts.pop()?.1;
    Some(Selector {
        tag: last.tag,
        id: last.id,
        classes: last.classes,
        attrs: last.attrs,
        pseudos: last.pseudos,
        pseudo_el: last.pseudo_el,
        ancestors,
    })
}

/// Parse one compound at the start of `s`; returns it and the bytes used.
pub(super) fn parse_compound(s: &str) -> Option<(Compound, usize)> {
    let b = s.as_bytes();
    let mut c = Compound::default();
    let mut i = 0;
    if i < b.len() && (b[i].is_ascii_alphabetic() || b[i] == b'*') {
        let start = i;
        while i < b.len() && (b[i].is_ascii_alphanumeric() || b[i] == b'-' || b[i] == b'*') {
            i += 1;
        }
        c.tag = Some(s[start..i].to_ascii_lowercase());
    }
    while i < b.len() {
        match b[i] {
            b'.' | b'#' => {
                let kind = b[i];
                i += 1;
                let start = i;
                while i < b.len() && (b[i].is_ascii_alphanumeric() || b[i] == b'-' || b[i] == b'_')
                {
                    i += 1;
                }
                let name = s[start..i].to_string();
                if name.is_empty() {
                    return None;
                }
                if kind == b'.' {
                    c.classes.push(name);
                } else {
                    c.id = Some(name);
                }
            }
            b'[' => {
                let close = s[i..].find(']')? + i;
                c.attrs.push(parse_attr(&s[i + 1..close])?);
                i = close + 1;
            }
            b':' => {
                i += parse_pseudo(&s[i..], &mut c)?;
            }
            c if c.is_ascii_whitespace() || c == b'>' => break,
            b'+' | b'~' | b',' => return None,
            _ => return None,
        }
    }
    if c.tag.is_none()
        && c.id.is_none()
        && c.classes.is_empty()
        && c.attrs.is_empty()
        && c.pseudos.is_empty()
        && c.pseudo_el == 0
    {
        return None;
    }
    Some((c, i))
}

/// A pseudo-class at the start of `s` (which begins with `:`), added to `c`; returns the bytes
/// used. `None` for pseudo-elements and for what a static page can never match (`:hover`...), so
/// the whole selector is dropped.
pub(super) fn parse_pseudo(s: &str, c: &mut Compound) -> Option<usize> {
    let b = s.as_bytes();
    // `::before` / `::after` (and the one-colon spelling) style a generated box; the other
    // pseudo-elements are not drawn.
    for (name, kind) in [
        ("::before", 1u8),
        (":before", 1),
        ("::after", 2),
        (":after", 2),
    ] {
        if s.len() >= name.len()
            && s.as_bytes()[..name.len()].eq_ignore_ascii_case(name.as_bytes())
            && !s
                .as_bytes()
                .get(name.len())
                .is_some_and(|c| c.is_ascii_alphanumeric() || *c == b'-')
        {
            c.pseudo_el = kind;
            return Some(name.len());
        }
    }
    if b.get(1) == Some(&b':') {
        return None; // another pseudo-element
    }
    let mut n = 1;
    while n < b.len() && (b[n].is_ascii_alphanumeric() || b[n] == b'-') {
        n += 1;
    }
    let name = s[1..n].to_ascii_lowercase();
    // Arguments in parentheses.
    let mut arg: Option<&str> = None;
    if b.get(n) == Some(&b'(') {
        let mut depth = 0;
        let mut end = None;
        for (k, ch) in s[n..].char_indices() {
            match ch {
                '(' => depth += 1,
                ')' => {
                    depth -= 1;
                    if depth == 0 {
                        end = Some(n + k);
                        break;
                    }
                }
                _ => {}
            }
        }
        let end = end?;
        arg = Some(&s[n + 1..end]);
        n = end + 1;
    }
    let nth = |a: i32, b: i32, last: bool, of_type: bool| Pseudo::Nth {
        a,
        b,
        last,
        of_type,
    };
    let p = match (name.as_str(), arg) {
        ("root", None) if c.tag.is_none() => {
            c.tag = Some("html".to_string());
            return Some(n);
        }
        ("first-child", None) => nth(0, 1, false, false),
        ("last-child", None) => nth(0, 1, true, false),
        ("first-of-type", None) => nth(0, 1, false, true),
        ("last-of-type", None) => nth(0, 1, true, true),
        ("only-child", None) => Pseudo::OnlyChild,
        ("empty", None) => Pseudo::Empty,
        ("link" | "any-link", None) => Pseudo::Link,
        ("checked", None) => Pseudo::Checked,
        ("disabled", None) => Pseudo::Disabled,
        ("enabled", None) => Pseudo::Enabled,
        ("nth-child" | "nth-last-child" | "nth-of-type" | "nth-last-of-type", Some(a)) => {
            let (a, b) = parse_nth(a)?;
            nth(a, b, name.contains("last"), name.ends_with("of-type"))
        }
        ("not" | "is" | "where" | "matches", Some(list)) => {
            let mut comps = Vec::new();
            for part in split_commas(list) {
                let part = part.trim();
                let (comp, used) = parse_compound(part)?;
                if used != part.len() || comps.len() >= 8 {
                    return None;
                }
                comps.push(comp);
            }
            if name == "not" {
                Pseudo::Not(comps)
            } else {
                Pseudo::Any(comps)
            }
        }
        _ => return None,
    };
    c.pseudos.push(p);
    Some(n)
}

/// `an+b`, `odd`, `even`, `3`, `-n+2`.
pub(super) fn parse_nth(s: &str) -> Option<(i32, i32)> {
    let t: String = s
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect::<String>()
        .to_ascii_lowercase();
    match t.as_str() {
        "odd" => return Some((2, 1)),
        "even" => return Some((2, 0)),
        _ => {}
    }
    if let Some(npos) = t.find('n') {
        let a = match &t[..npos] {
            "" | "+" => 1,
            "-" => -1,
            v => v.parse().ok()?,
        };
        let rest = &t[npos + 1..];
        let b = if rest.is_empty() {
            0
        } else {
            rest.parse().ok()?
        };
        Some((a, b))
    } else {
        Some((0, t.parse().ok()?))
    }
}

pub(super) fn parse_attr(inner: &str) -> Option<AttrSel> {
    let inner = inner.trim();
    let ops = [
        ("~=", AttrOp::Word),
        ("^=", AttrOp::Prefix),
        ("$=", AttrOp::Suffix),
        ("*=", AttrOp::Contains),
        ("=", AttrOp::Equals),
    ];
    for (tok, op) in ops {
        if let Some((n, v)) = inner.split_once(tok) {
            let name = n.trim().to_ascii_lowercase();
            if name.is_empty() {
                return None;
            }
            // `[a="b" i]` flags are ignored (matching is case-insensitive anyway).
            let v = v.trim();
            let v = v.split_once(" i").map_or(v, |(a, _)| a).trim();
            let v = v.trim_matches(|c| c == '"' || c == '\'');
            return Some(AttrSel {
                name,
                op,
                value: v.to_string(),
            });
        }
    }
    let name = inner.to_ascii_lowercase();
    if name.is_empty()
        || !name
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_')
    {
        return None;
    }
    Some(AttrSel {
        name,
        op: AttrOp::Exists,
        value: String::new(),
    })
}
