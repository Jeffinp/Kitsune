//! Host checks over the catalogs and the sources (`cargo test -p kitsune_core i18n`).
//!
//! * the two catalogs define exactly the same keys with the same placeholders, and plural
//!   families are complete;
//! * every key used in the sources exists, and every key in the catalogs is used;
//! * Portuguese text has its accents (word list in `tools/i18n/accents.txt`), both in the
//!   catalog and in the string literals of the files already migrated;
//! * no text names the implementation language or toolchain;
//! * both fonts have a glyph for every character of both catalogs.
//!
//! `cargo test -p kitsune_core i18n_report -- --ignored --nocapture` prints, for the whole tree,
//! the literals that look like Portuguese without accents (the migration backlog).

use super::{Lang, placeholders};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::string::String;
use std::vec::Vec;

mod accents;
mod scan;
use accents::*;
use scan::*;

#[test]
fn catalogs_define_the_same_keys() {
    let (pt, en) = (entries(Lang::Pt), entries(Lang::En));
    let only_pt: Vec<_> = pt.keys().filter(|k| !en.contains_key(*k)).collect();
    let only_en: Vec<_> = en.keys().filter(|k| !pt.contains_key(*k)).collect();
    assert!(
        only_pt.is_empty() && only_en.is_empty(),
        "keys only in pt: {only_pt:?}\nkeys only in en: {only_en:?}"
    );
}
#[test]
fn catalogs_use_the_same_placeholders() {
    let (pt, en) = (entries(Lang::Pt), entries(Lang::En));
    let mut bad = Vec::new();
    for (k, pv) in &pt {
        let Some(ev) = en.get(k) else { continue };
        // `{Month_long}` and `{month_long}` name the same argument.
        fn set(v: &str) -> BTreeSet<String> {
            placeholders(v)
                .into_iter()
                .map(|p| {
                    let mut c = p.chars();
                    c.next()
                        .map(|f| f.to_lowercase().chain(c).collect())
                        .unwrap_or_default()
                })
                .collect()
        }
        if set(pv) != set(ev) {
            bad.push(std::format!("{k}: pt {:?} vs en {:?}", set(pv), set(ev)));
        }
    }
    assert!(bad.is_empty(), "placeholder mismatch:\n{}", bad.join("\n"));
}
#[test]
fn keys_and_values_are_well_formed() {
    let mut bad = Vec::new();
    for l in Lang::ALL {
        for (k, v) in l.catalog().entries() {
            if !is_valid_key(k) {
                bad.push(std::format!("{}: bad key syntax {k:?}", l.code()));
            }
            if v.chars().any(|c| c.is_control()) {
                bad.push(std::format!("{}: {k}: control character", l.code()));
            }
            // Braces must be escaped or form a complete {name}.
            let mut depth = 0i32;
            let mut it = v.chars().peekable();
            while let Some(c) = it.next() {
                match c {
                    '{' if it.peek() == Some(&'{') => {
                        it.next();
                    }
                    '}' if it.peek() == Some(&'}') => {
                        it.next();
                    }
                    '{' => depth += 1,
                    '}' => depth -= 1,
                    _ => {}
                }
                if !(0..=1).contains(&depth) {
                    break;
                }
            }
            if depth != 0 {
                bad.push(std::format!(
                    "{}: {k}: unbalanced braces in {v:?}",
                    l.code()
                ));
            }
            if v.trim() != *v {
                bad.push(std::format!("{}: {k}: surrounding blanks", l.code()));
            }
            if v.is_empty() && !k.starts_with("meta.") {
                bad.push(std::format!("{}: {k}: empty value", l.code()));
            }
        }
    }
    assert!(bad.is_empty(), "{}", bad.join("\n"));
}
#[test]
fn plural_families_are_complete() {
    for l in Lang::ALL {
        let e = entries(l);
        for k in e.keys() {
            if let Some(base) = k.strip_suffix(".one") {
                assert!(
                    e.contains_key(std::format!("{base}.other").as_str()),
                    "{}: {k} has no .other",
                    l.code()
                );
            }
        }
    }
}
#[test]
fn every_language_defines_the_format_keys() {
    for l in Lang::ALL {
        let e = entries(l);
        for k in [
            "meta.code",
            "meta.name",
            "fmt.decimal",
            "fmt.group",
            "fmt.plural",
            "fmt.am",
            "fmt.pm",
            "fmt.clock24_default",
        ] {
            assert!(e.contains_key(k), "{}: missing {k}", l.code());
        }
        assert_eq!(e["fmt.decimal"].chars().count(), 1);
        assert_eq!(e["fmt.group"].chars().count(), 1);
        assert_ne!(e["fmt.decimal"], e["fmt.group"]);
        assert!(matches!(e["fmt.clock24_default"], "0" | "1"));
        assert!(
            matches!(e["fmt.plural"], "one" | "zero-one" | "invariant"),
            "{}: unknown plural rule",
            l.code()
        );
    }
}

// ------------------------------------------------------------------------ source checks
/// Every key literal in the sources exists in both catalogs, every catalog key is used.
#[test]
fn source_keys_match_the_catalogs() {
    let files = source_files();
    if files.iter().all(|(f, _)| !f.starts_with("kernel/")) {
        return; // built from a package without the kernel: nothing to scan
    }
    let (pt, en) = (entries(Lang::Pt), entries(Lang::En));
    let mut used: BTreeSet<String> = BTreeSet::new();
    let mut missing = Vec::new();
    for (file, src) in &files {
        for u in key_uses(src, file) {
            if u.plural {
                for suffix in ["other"] {
                    let full = std::format!("{}.{suffix}", u.key);
                    used.insert(std::format!("{}.one", u.key));
                    used.insert(full.clone());
                    for (name, cat) in [("pt", &pt), ("en", &en)] {
                        if !cat.contains_key(full.as_str()) {
                            missing.push(std::format!(
                                "{}:{}: {name} lacks {full}",
                                u.file,
                                u.line
                            ));
                        }
                    }
                }
            } else {
                used.insert(u.key.clone());
                for (name, cat) in [("pt", &pt), ("en", &en)] {
                    if !cat.contains_key(u.key.as_str()) {
                        missing.push(std::format!(
                            "{}:{}: {name} lacks {:?}",
                            u.file,
                            u.line,
                            u.key
                        ));
                    }
                }
            }
        }
    }
    assert!(missing.is_empty(), "missing keys:\n{}", missing.join("\n"));
    let unused: Vec<_> = pt
        .keys()
        .filter(|k| !k.starts_with("meta.") && !used.contains(**k))
        .collect();
    assert!(
        unused.is_empty(),
        "keys in the catalogs that no source uses (remove them or use them):\n{unused:#?}"
    );
}
#[test]
fn key_scanner_finds_the_api_forms() {
    let src = r##"
        // t!("in.a.comment")
        let a = t!("a.b");
        let b = crate::i18n::tr("c.d");
        let c = tr_in(Lang::Pt, "e.f");
        let d = tp!("g.h", n, name = x);
        let e = [tk!("i.j"), tk!("k.l")];
        let f = plural_fmt_in(l, "m.n", 3, &[]);
        let g = x.tr("not.a.key");
        let h = "t!(\"in.a.string\")";
        let i = r#"tr("in.a.raw")"#;
        fn tr(k: &str) {}
        #[cfg(test)]
        mod tests {
            fn t() { let z = t!("fixture.key"); }
        }
        let after = tk!("after.tests");
        let j = tr_fmt("o.p", &[("q", Arg::Str("r.s"))]);
    "##;
    let keys: Vec<_> = key_uses(src, "x.rs")
        .into_iter()
        .map(|u| (u.key, u.plural))
        .collect();
    let want: Vec<(String, bool)> = [
        ("a.b", false),
        ("c.d", false),
        ("e.f", false),
        ("g.h", true),
        ("i.j", false),
        ("k.l", false),
        ("m.n", true),
        ("after.tests", false),
        ("o.p", false),
    ]
    .iter()
    .map(|(k, p)| (k.to_string(), *p))
    .collect();
    assert_eq!(keys, want);
}
#[test]
fn tokenizer_handles_chars_lifetimes_and_raw_strings() {
    let src = "fn f<'a>(x: &'a str) { let c = '\"'; let d = '\\''; let s = \"q\\\"uote\"; let r = r#\"a\"b\"#; }";
    let strs: Vec<_> = tokenize(src)
        .into_iter()
        .filter_map(|(t, _)| if let Tok::Str(s) = t { Some(s) } else { None })
        .collect();
    assert_eq!(strs, ["q\"uote", "a\"b"]);
}

// ------------------------------------------------------------------------- accents
#[test]
fn fonts_cover_both_catalogs_and_the_portuguese_alphabet() {
    static FONTS: [(&str, &[u8]); 4] = [
        (
            "Inter Regular",
            include_bytes!("../../../assets/fonts/Inter-Regular.subset.ttf"),
        ),
        (
            "Inter Medium",
            include_bytes!("../../../assets/fonts/Inter-Medium.subset.ttf"),
        ),
        (
            "Inter SemiBold",
            include_bytes!("../../../assets/fonts/Inter-SemiBold.subset.ttf"),
        ),
        (
            "JetBrains Mono",
            include_bytes!("../../../assets/fonts/JetBrainsMono-Regular.subset.ttf"),
        ),
    ];
    // Everything a Portuguese or English text can need, whatever the catalogs hold today.
    let required: String =
        "áàâãäéèêëíìîïóòôõöúùûüçñÁÀÂÃÄÉÈÊËÍÌÎÏÓÒÔÕÖÚÙÛÜÇÑ«»‘’“”„…–—−°ªº€£¥¿¡·•×÷±§¶©®™ß←→↑↓".into();
    let mut from_catalogs: BTreeSet<char> = BTreeSet::new();
    for l in Lang::ALL {
        for (k, v) in l.catalog().entries() {
            // Template syntax and keys are not drawn; the values are.
            let _ = k;
            from_catalogs.extend(v.chars());
        }
    }
    let mut missing = Vec::new();
    for (name, data) in FONTS {
        let font = crate::ui::ttf::Font::parse(data).unwrap_or_else(|| panic!("{name}: no parse"));
        for c in required.chars() {
            // The mono face is the terminal and the editor; it only needs letters and
            // punctuation, not the arrows.
            if name == "JetBrains Mono" && "←→↑↓".contains(c) {
                continue;
            }
            if !font_has(&font, c) {
                missing.push(std::format!("{name}: U+{:04X} {c}", c as u32));
            }
        }
        if name != "JetBrains Mono" {
            for &c in &from_catalogs {
                if c != '\n' && !font_has(&font, c) {
                    missing.push(std::format!("{name}: catalog char U+{:04X} {c}", c as u32));
                }
            }
        }
    }
    assert!(
        missing.is_empty(),
        "glyphs missing:\n{}",
        missing.join("\n")
    );
    // The check itself can fail: a snowman is in none of the subsets.
    let inter = crate::ui::ttf::Font::parse(FONTS[0].1).unwrap();
    assert!(!font_has(&inter, '\u{2603}'));
}
