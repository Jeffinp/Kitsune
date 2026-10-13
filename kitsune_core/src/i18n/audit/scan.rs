//! Reading the sources: a tokenizer for Rust string literals and the finder of catalog keys.

use super::*;

#[derive(Debug, Clone, PartialEq)]
pub(super) enum Tok {
    Ident(String),
    Punct(char),
    Str(String),
}

/// Rust tokens good enough to find calls and string literals: comments are skipped, char
/// literals and lifetimes are told apart, raw strings are handled. Returns `(token, line)`.
pub(super) fn tokenize(src: &str) -> Vec<(Tok, usize)> {
    let b: Vec<char> = src.chars().collect();
    let mut out = Vec::new();
    let (mut i, mut line) = (0, 1);
    while i < b.len() {
        let c = b[i];
        if c == '\n' {
            line += 1;
            i += 1;
        } else if c.is_whitespace() {
            i += 1;
        } else if c == '/' && b.get(i + 1) == Some(&'/') {
            while i < b.len() && b[i] != '\n' {
                i += 1;
            }
        } else if c == '/' && b.get(i + 1) == Some(&'*') {
            let mut depth = 1;
            i += 2;
            while i < b.len() && depth > 0 {
                if b[i] == '\n' {
                    line += 1;
                }
                if b[i] == '/' && b.get(i + 1) == Some(&'*') {
                    depth += 1;
                    i += 2;
                } else if b[i] == '*' && b.get(i + 1) == Some(&'/') {
                    depth -= 1;
                    i += 2;
                } else {
                    i += 1;
                }
            }
        } else if c == '"' || (c == 'b' && b.get(i + 1) == Some(&'"')) {
            let start_line = line;
            // `b"..."` byte strings are data, not text: skip them without a token.
            let is_bytes = c == 'b';
            i += if is_bytes { 2 } else { 1 };
            let mut s = String::new();
            while i < b.len() && b[i] != '"' {
                if b[i] == '\n' {
                    line += 1;
                }
                if b[i] == '\\' && i + 1 < b.len() {
                    i += 1;
                    match b[i] {
                        'n' => s.push('\n'),
                        't' => s.push('\t'),
                        'r' => s.push('\r'),
                        '0' => s.push('\0'),
                        '\\' => s.push('\\'),
                        '"' => s.push('"'),
                        '\'' => s.push('\''),
                        'u' => {
                            // \u{XXXX}
                            let mut hex = String::new();
                            i += 1;
                            if b.get(i) == Some(&'{') {
                                i += 1;
                                while i < b.len() && b[i] != '}' {
                                    hex.push(b[i]);
                                    i += 1;
                                }
                            }
                            if let Some(ch) =
                                u32::from_str_radix(&hex, 16).ok().and_then(char::from_u32)
                            {
                                s.push(ch);
                            }
                        }
                        '\n' => {
                            // line continuation: skip the newline and the next indentation
                            line += 1;
                            while i + 1 < b.len() && b[i + 1].is_whitespace() {
                                if b[i + 1] == '\n' {
                                    line += 1;
                                }
                                i += 1;
                            }
                        }
                        'x' => {
                            i += 2;
                        }
                        other => s.push(other),
                    }
                    i += 1;
                } else {
                    s.push(b[i]);
                    i += 1;
                }
            }
            i += 1;
            if !is_bytes {
                out.push((Tok::Str(s), start_line));
            }
        } else if c == 'r' && matches!(b.get(i + 1), Some('"') | Some('#')) && raw_start(&b, i) {
            let start_line = line;
            let mut j = i + 1;
            let mut hashes = 0;
            while b.get(j) == Some(&'#') {
                hashes += 1;
                j += 1;
            }
            j += 1; // the quote
            let mut s = String::new();
            'raw: while j < b.len() {
                if b[j] == '"' {
                    let mut k = 0;
                    while k < hashes && b.get(j + 1 + k) == Some(&'#') {
                        k += 1;
                    }
                    if k == hashes {
                        j += 1 + hashes;
                        break 'raw;
                    }
                }
                if b[j] == '\n' {
                    line += 1;
                }
                s.push(b[j]);
                j += 1;
            }
            i = j;
            out.push((Tok::Str(s), start_line));
        } else if c == '\'' {
            // char literal or lifetime
            if b.get(i + 1) == Some(&'\\') {
                i += 2;
                while i < b.len() && b[i] != '\'' {
                    i += 1;
                }
                i += 1;
            } else if b.get(i + 2) == Some(&'\'') {
                i += 3;
            } else {
                i += 1; // a lifetime tick
            }
        } else if c.is_alphabetic() || c == '_' {
            let mut s = String::new();
            while i < b.len() && (b[i].is_alphanumeric() || b[i] == '_') {
                s.push(b[i]);
                i += 1;
            }
            out.push((Tok::Ident(s), line));
        } else if c.is_ascii_digit() {
            while i < b.len() && (b[i].is_alphanumeric() || b[i] == '_' || b[i] == '.') {
                // stop before `..` ranges
                if b[i] == '.' && b.get(i + 1) == Some(&'.') {
                    break;
                }
                i += 1;
            }
        } else {
            out.push((Tok::Punct(c), line));
            i += 1;
        }
    }
    out
}

/// The tokens without the bodies of `#[cfg(test)] mod name { ... }` (their keys are fixtures).
pub(super) fn without_test_modules(toks: Vec<(Tok, usize)>) -> Vec<(Tok, usize)> {
    let is = |i: usize, t: &Tok| toks.get(i).is_some_and(|x| &x.0 == t);
    let id = |n: &str| Tok::Ident(n.into());
    let mut out = Vec::new();
    let mut i = 0;
    while i < toks.len() {
        // # [ cfg ( test ) ] [pub] mod name {
        if is(i, &Tok::Punct('#'))
            && is(i + 1, &Tok::Punct('['))
            && is(i + 2, &id("cfg"))
            && is(i + 3, &Tok::Punct('('))
            && is(i + 4, &id("test"))
            && is(i + 5, &Tok::Punct(')'))
            && is(i + 6, &Tok::Punct(']'))
        {
            let mut j = i + 7;
            if is(j, &id("pub")) {
                j += 1;
            }
            if is(j, &id("mod")) && matches!(toks.get(j + 2).map(|t| &t.0), Some(Tok::Punct('{'))) {
                let mut depth = 0;
                j += 2;
                while j < toks.len() {
                    match toks[j].0 {
                        Tok::Punct('{') => depth += 1,
                        Tok::Punct('}') => {
                            depth -= 1;
                            if depth == 0 {
                                break;
                            }
                        }
                        _ => {}
                    }
                    j += 1;
                }
                i = j + 1;
                continue;
            }
        }
        out.push(toks[i].clone());
        i += 1;
    }
    out
}

/// `r"`, `r#"`, `r##"`... at `i` (and not the tail of an identifier).
pub(super) fn raw_start(b: &[char], i: usize) -> bool {
    if i > 0 && (b[i - 1].is_alphanumeric() || b[i - 1] == '_') {
        return false;
    }
    let mut j = i + 1;
    while b.get(j) == Some(&'#') {
        j += 1;
    }
    b.get(j) == Some(&'"')
}

/// Names that look a key up, and whether their key is a plural family (`.one`/`.other`).
pub(super) const CALLS: &[(&str, bool, bool)] = &[
    // (name, is_macro, plural)
    ("t", true, false),
    ("tk", true, false),
    ("tp", true, true),
    ("tr", false, false),
    ("tr_in", false, false),
    ("tr_fmt", false, false),
    ("tr_fmt_in", false, false),
    ("plural", false, true),
    ("plural_in", false, true),
    ("plural_fmt", false, true),
    ("plural_fmt_in", false, true),
];

#[derive(Debug)]
pub(super) struct KeyUse {
    pub(super) key: String,
    pub(super) plural: bool,
    pub(super) file: String,
    pub(super) line: usize,
}

/// The key literals passed to the lookup API in `src`.
pub(super) fn key_uses(src: &str, file: &str) -> Vec<KeyUse> {
    let toks = without_test_modules(tokenize(src));
    let mut out = Vec::new();
    for (i, (t, line)) in toks.iter().enumerate() {
        let Tok::Ident(name) = t else { continue };
        let Some(&(_, is_macro, plural)) = CALLS.iter().find(|c| c.0 == name) else {
            continue;
        };
        // Not a method or a path tail of something else: `x.tr(` is not ours; `i18n::tr(` is.
        if i > 0 && matches!(toks[i - 1].0, Tok::Punct('.')) {
            continue;
        }
        if i > 0 && matches!(&toks[i - 1].0, Tok::Ident(k) if k == "fn") {
            continue;
        }
        let mut j = i + 1;
        if is_macro {
            if !matches!(toks.get(j).map(|t| &t.0), Some(Tok::Punct('!'))) {
                continue;
            }
            j += 1;
        }
        if !matches!(toks.get(j).map(|t| &t.0), Some(Tok::Punct('(' | '['))) {
            continue;
        }
        j += 1;
        // The key is the first argument, or the second for the `_in(lang, key)` forms.
        if name.ends_with("_in") {
            let mut depth = 0;
            while j < toks.len() {
                match &toks[j].0 {
                    Tok::Punct('(' | '[' | '{') => depth += 1,
                    Tok::Punct(')' | ']' | '}') => {
                        if depth == 0 {
                            break;
                        }
                        depth -= 1;
                    }
                    Tok::Punct(',') if depth == 0 => {
                        j += 1;
                        break;
                    }
                    _ => {}
                }
                j += 1;
            }
        }
        if let Some((Tok::Str(k), _)) = toks.get(j) {
            out.push(KeyUse {
                key: k.clone(),
                plural,
                file: file.into(),
                line: *line,
            });
        }
    }
    out
}

pub(super) fn is_valid_key(k: &str) -> bool {
    let segs: Vec<&str> = k.split('.').collect();
    segs.len() >= 2
        && k.len() <= 80
        && segs.iter().all(|s| {
            !s.is_empty()
                && s.bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
        })
}

/// Rust files under `dir`, recursively (empty when the directory is missing).
pub(super) fn rs_files(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let Ok(rd) = fs::read_dir(dir) else {
        return out;
    };
    let mut items: Vec<_> = rd.flatten().map(|e| e.path()).collect();
    items.sort();
    for p in items {
        if p.is_dir() {
            out.extend(rs_files(&p));
        } else if p.extension().is_some_and(|e| e == "rs") {
            out.push(p);
        }
    }
    out
}

/// The sources the key checks read: the kernel, the core and the ring 3 programs that use the
/// catalogs (`kterm`, `play`), minus this file (its fixtures are made-up keys).
pub(super) fn source_files() -> Vec<(String, String)> {
    let root = repo_root();
    let skip = ["audit.rs"];
    let mut out = Vec::new();
    // Modules declared `#[cfg(test)] mod x;` hold fixtures (made-up keys), like the inline ones.
    let mut skip_tests = std::collections::BTreeSet::new();
    for p in rs_files(&root.join("kitsune_core/src")) {
        if let Ok(s) = fs::read_to_string(&p) {
            crate::testutil::test_modules(&p, &s, &mut skip_tests);
        }
    }
    for dir in ["kernel/src", "kitsune_core/src", "user-apps/kterm/src"] {
        for p in rs_files(&root.join(dir)) {
            if p.ancestors().any(|a| skip_tests.contains(a)) {
                continue;
            }
            let rel = p
                .strip_prefix(&root)
                .unwrap()
                .to_string_lossy()
                .replace('\\', "/");
            if rel.starts_with("kitsune_core/src/i18n/")
                && skip.contains(&p.file_name().unwrap().to_str().unwrap())
            {
                continue;
            }
            if let Ok(s) = fs::read_to_string(&p) {
                out.push((rel, s));
            }
        }
    }
    out
}

// -------------------------------------------------------------------- catalog checks

pub(super) fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("workspace root")
        .to_path_buf()
}

pub(super) fn entries(l: Lang) -> BTreeMap<&'static str, &'static str> {
    l.catalog().entries().iter().copied().collect()
}

// ----------------------------------------------------------------- source tokens
