//! Values that are rewritten before they are read: `var()`, `light-dark()`, `color-mix()`, `env()` and `content` strings.

use super::*;

/// `value` with every `var(--name, fallback)` replaced; `None` when one has neither a value nor a
/// fallback (the declaration is then invalid and ignored).
pub(super) fn subst_vars(value: &str, vars: &Vars) -> Option<String> {
    pub(super) fn go(v: &str, vars: &Vars, depth: u8, out: &mut String) -> Option<()> {
        let mut rest = v;
        while let Some(p) = rest.find("var(") {
            out.push_str(&rest[..p]);
            let args = &rest[p + 4..];
            let mut depth_p = 1i32;
            let mut end = None;
            let mut comma = None;
            for (i, ch) in args.char_indices() {
                match ch {
                    '(' => depth_p += 1,
                    ')' => {
                        depth_p -= 1;
                        if depth_p == 0 {
                            end = Some(i);
                            break;
                        }
                    }
                    ',' if depth_p == 1 && comma.is_none() => comma = Some(i),
                    _ => {}
                }
            }
            let end = end?;
            let (name, fallback) = match comma {
                Some(c) => (args[..c].trim(), Some(args[c + 1..end].trim())),
                None => (args[..end].trim(), None),
            };
            if depth >= 6 || out.len() > 4096 {
                return None;
            }
            match vars.get(&name.to_ascii_lowercase()) {
                Some(val) => go(val, vars, depth + 1, out)?,
                None => go(fallback?, vars, depth + 1, out)?,
            }
            rest = &args[end + 1..];
        }
        out.push_str(rest);
        Some(())
    }
    let mut out = String::new();
    go(value, vars, 0, &mut out)?;
    Some(out)
}

/// Does `v` use a function [`pick_args`] rewrites?
pub(super) fn needs_pick(v: &str) -> bool {
    v.contains("light-dark(") || v.contains("color-mix(") || v.contains("env(")
}

/// Rewrite `light-dark(a, b)` as `a` (the page is always shown light), `color-mix(in srgb, a ..., b ...)` as
/// its first colour and `env(name, fallback)` as the fallback (or 0), at any depth.
#[cfg(test)]
pub(super) fn pick_args_for_test(v: &str) -> String {
    pick_args(v)
}

pub(super) fn pick_args(v: &str) -> String {
    let mut out = String::with_capacity(v.len());
    let mut rest = v;
    'outer: while !rest.is_empty() {
        for name in ["light-dark(", "color-mix(", "env("] {
            if let Some(p) = rest.find(name) {
                // Only the earliest function is rewritten in this round.
                let earliest = ["light-dark(", "color-mix(", "env("]
                    .iter()
                    .filter_map(|n| rest.find(n))
                    .min()
                    .unwrap_or(p);
                if p != earliest {
                    continue;
                }
                out.push_str(&rest[..p]);
                let args = &rest[p + name.len()..];
                let mut depth = 1i32;
                let mut end = args.len();
                let mut commas: Vec<usize> = Vec::new();
                for (i, ch) in args.char_indices() {
                    match ch {
                        '(' => depth += 1,
                        ')' => {
                            depth -= 1;
                            if depth == 0 {
                                end = i;
                                break;
                            }
                        }
                        ',' if depth == 1 => commas.push(i),
                        _ => {}
                    }
                }
                let parts: Vec<&str> = {
                    let mut v = Vec::new();
                    let mut start = 0;
                    for &c in &commas {
                        v.push(&args[start..c]);
                        start = c + 1;
                    }
                    v.push(&args[start..end]);
                    v
                };
                let chosen = match name {
                    "light-dark(" => parts.first().copied().unwrap_or(""),
                    "color-mix(" => parts
                        .get(1)
                        .map(|c| c.split_whitespace().next().unwrap_or(""))
                        .unwrap_or(""),
                    _ => parts.get(1).copied().unwrap_or("0"),
                };
                out.push_str(&pick_args(chosen.trim()));
                rest = args.get(end + 1..).unwrap_or("");
                continue 'outer;
            }
        }
        out.push_str(rest);
        break;
    }
    out
}

/// The text of a `content` value: its quoted strings (CSS escapes decoded), `open-quote` and
/// `close-quote`; `attr()`, counters and `url()` give nothing. `None` for `none` / `normal`. Private-use
/// characters (icon fonts) are dropped: there is no font for them.
pub(super) fn parse_content(v: &str) -> Option<String> {
    let l = v.trim();
    if l.eq_ignore_ascii_case("none") || l.eq_ignore_ascii_case("normal") {
        return None;
    }
    let mut out = String::new();
    let b = l.as_bytes();
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            q @ (b'"' | b'\'') => {
                i += 1;
                while i < b.len() && b[i] != q {
                    if b[i] == b'\\' && i + 1 < b.len() {
                        i += 1;
                        let hex: String = l[i..]
                            .chars()
                            .take_while(char::is_ascii_hexdigit)
                            .take(6)
                            .collect();
                        if hex.is_empty() {
                            // An escaped character stands for itself (a newline escape is nothing).
                            let ch = l[i..].chars().next().unwrap_or(' ');
                            if ch != '\n' {
                                out.push(ch);
                            }
                            i += ch.len_utf8();
                        } else {
                            let cp = u32::from_str_radix(&hex, 16).unwrap_or(0xfffd);
                            if let Some(ch) = char::from_u32(cp) {
                                out.push(ch);
                            }
                            i += hex.len();
                            // One space after a hex escape belongs to it.
                            if b.get(i) == Some(&b' ') {
                                i += 1;
                            }
                        }
                    } else {
                        let ch = l[i..].chars().next().unwrap_or(' ');
                        out.push(ch);
                        i += ch.len_utf8();
                    }
                }
                i += 1;
            }
            _ => {
                let rest = &l[i..];
                if rest.starts_with("open-quote") {
                    out.push('\u{201c}');
                    i += 10;
                } else if rest.starts_with("close-quote") {
                    out.push('\u{201d}');
                    i += 11;
                } else {
                    i += rest.chars().next().map_or(1, char::len_utf8);
                }
            }
        }
    }
    out.retain(|c| !('\u{e000}'..='\u{f8ff}').contains(&c));
    Some(out)
}
