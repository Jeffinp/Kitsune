//! Portuguese text keeps its accents, English text has no Portuguese in it, fonts cover the catalogs.

use super::*;

pub(super) struct Accents {
    pub(super) exact: BTreeMap<String, String>,
    pub(super) suffix: Vec<String>,
    pub(super) allow: BTreeSet<String>,
}

pub(super) fn accents() -> Accents {
    let mut a = Accents {
        exact: BTreeMap::new(),
        suffix: Vec::new(),
        allow: BTreeSet::new(),
    };
    for line in include_str!("../../../../tools/i18n/accents.txt").lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some(s) = line.strip_prefix('~') {
            a.suffix.push(s.into());
        } else if let Some(w) = line.strip_prefix('!') {
            a.allow.insert(w.into());
        } else if let Some((w, r)) = line.split_once('=') {
            a.exact.insert(w.into(), r.into());
        }
    }
    a
}

/// Words of `text` (runs of letters) that Portuguese writes with an accent but that appear
/// without: `(word, suggestion)`.
pub(super) fn unaccented_words(a: &Accents, text: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for w in text.split(|c: char| !c.is_alphabetic()) {
        if w.len() < 3 || !w.is_ascii() {
            continue;
        }
        let lw = w.to_ascii_lowercase();
        if a.allow.contains(&lw) {
            continue;
        }
        if let Some(r) = a.exact.get(&lw) {
            out.push((w.into(), r.clone()));
        } else if a
            .suffix
            .iter()
            .any(|s| lw.ends_with(s.as_str()) && lw.len() > s.len())
        {
            out.push((w.into(), "(accent)".into()));
        }
    }
    out
}

#[test]
pub(super) fn the_word_list_is_big_and_sane() {
    let a = accents();
    assert!(a.exact.len() >= 150, "only {} stems", a.exact.len());
    for (w, r) in &a.exact {
        assert!(
            w.is_ascii() && !r.is_ascii(),
            "{w}={r}: the right form must have an accent"
        );
    }
}

#[test]
pub(super) fn the_checker_catches_the_classics() {
    let a = accents();
    let bad = |t: &str| {
        unaccented_words(&a, t)
            .into_iter()
            .map(|p| p.0)
            .collect::<Vec<_>>()
    };
    assert_eq!(bad("Configuracoes"), ["Configuracoes"]);
    assert_eq!(bad("Nao tem pagina na area"), ["Nao", "pagina", "area"]);
    assert_eq!(bad("Ultima modificacao"), ["Ultima", "modificacao"]);
    assert_eq!(bad("nenhum arquivo, nenhuma pasta"), Vec::<String>::new());
    assert_eq!(bad("Configurações não há"), Vec::<String>::new());
    assert_eq!(bad("Mario"), Vec::<String>::new());
    assert_eq!(bad("selecionar versao"), ["versao"]);
}

#[test]
pub(super) fn portuguese_catalog_text_has_its_accents() {
    let a = accents();
    let mut bad = Vec::new();
    for (k, v) in Lang::Pt.catalog().entries() {
        for (w, r) in unaccented_words(&a, v) {
            bad.push(std::format!("{k}: {w:?} should be {r}"));
        }
    }
    assert!(bad.is_empty(), "{}", bad.join("\n"));
}

#[test]
pub(super) fn english_text_has_no_portuguese_leftovers() {
    // The accented Portuguese forms never occur in the English catalog.
    let a = accents();
    let right: BTreeSet<&str> = a.exact.values().map(String::as_str).collect();
    let mut bad = Vec::new();
    for (k, v) in Lang::En.catalog().entries() {
        for w in v.split(|c: char| !c.is_alphabetic()) {
            if right.contains(w.to_lowercase().as_str()) {
                bad.push(std::format!("{k}: {w:?}"));
            }
        }
    }
    assert!(bad.is_empty(), "{}", bad.join("\n"));
}

#[test]
pub(super) fn text_never_names_the_toolchain() {
    for l in Lang::ALL {
        for (k, v) in l.catalog().entries() {
            let lv = v.to_lowercase();
            for w in ["rust", "cargo", "rustc", "llvm", "no_std", "crate"] {
                assert!(
                    !lv.split(|c: char| !c.is_alphanumeric() && c != '_')
                        .any(|x| x == w),
                    "{}: {k} mentions {w}",
                    l.code()
                );
            }
        }
    }
}

/// Files whose user-visible literals are fully migrated: Portuguese text in them must carry
/// its accents. (The rest of the tree is the backlog printed by `i18n_report`.)
pub(super) const ACCENT_STRICT: &[&str] = &[
    // An entry ending in `/` is a whole folder: every file under it is strict.
    "kernel/src/desktop/shell/",
    "kernel/src/desktop/windows/chrome.rs",
    "kitsune_core/src/windowing/launcher.rs",
    // w30: Arquivos, Imagens, Editor.
    "kernel/src/desktop/apps/files/",
    "kernel/src/desktop/services/sysstore.rs",
    "kernel/src/desktop/services/vfs.rs",
    "kitsune_core/src/apps/fileman.rs",
    "kitsune_core/src/apps/fileman/apps.rs",
    "kitsune_core/src/apps/fileman/ui.rs",
    "kitsune_core/src/storage/vfs.rs",
    // w31: Terminal, Tarefas, Registro, Calculadora
    "kernel/src/desktop/apps/terminal/",
    "kernel/src/desktop/services/shellhost/",
    "kernel/src/desktop/apps/tarefas/",
    "kernel/src/desktop/apps/registro/",
    "kernel/src/desktop/apps/calculadora/",
    "kitsune_core/src/apps/activity.rs",
    "kitsune_core/src/apps/calc.rs",
    "kitsune_core/src/apps/shell/builtins.rs",
    "kitsune_core/src/apps/shell/exec.rs",
    "kitsune_core/src/apps/shell/fs.rs",
    "kitsune_core/src/apps/shell/glob.rs",
    "kitsune_core/src/apps/shell/line.rs",
    "kitsune_core/src/apps/shell/netcmds.rs",
    "kitsune_core/src/apps/shell/parse.rs",
    "kitsune_core/src/apps/shell/regex.rs",
    "kitsune_core/src/apps/shell/sys.rs",
];

/// Files migrated in W32 (Navegador, Ajustes, apps WASM, Kit de componentes): same rule.
pub(super) const ACCENT_STRICT_W32: &[&str] = &[
    "kernel/src/desktop/apps/browser/",
    "kernel/src/desktop/apps/gallery/",
    "kernel/src/desktop/apps/ajustes/",
    "kernel/src/desktop/apps/wasm/",
    "kernel/src/wasm/abi2.rs",
    "kernel/src/wasm/manager.rs",
    "kernel/src/wasm/manager/runtime.rs",
    "kernel/src/wasm/mod.rs",
    "kitsune_core/src/platform/appfs/mod.rs",
    "kitsune_core/src/platform/appinstall.rs",
    "kitsune_core/src/platform/appmanifest.rs",
    "kitsune_core/src/browsing/browser.rs",
    "kitsune_core/src/browsing/browser/cert.rs",
    "kitsune_core/src/browsing/browser/errors.rs",
    "kitsune_core/src/browsing/browser/pages.rs",
    "kitsune_core/src/browsing/browser/tabs.rs",
    "kitsune_core/src/system/settings.rs",
    "kitsune_core/src/network/tlsverify.rs",
    "kitsune_core/src/ui/wallpaper.rs",
    "kitsune_core/src/platform/wasmsec.rs",
    "kitsune_core/src/browsing/web/form.rs",
    "kitsune_core/src/browsing/web/imgcache.rs",
];

pub(super) fn unaccented_literals(file: &str, src: &str, a: &Accents) -> Vec<String> {
    let mut out = Vec::new();
    for (t, line) in tokenize(src) {
        if let Tok::Str(s) = t {
            // Skip identifiers-like strings (keys, paths, format-only text).
            if !s.contains(' ') && !s.chars().next().is_some_and(char::is_uppercase) {
                continue;
            }
            // Style sheets are code: their selectors (`area`) are tag names, not words.
            if s.contains('{') && s.contains('}') && s.contains(':') && s.contains(';') {
                continue;
            }
            for (w, r) in unaccented_words(a, &s) {
                out.push(std::format!("{file}:{line}: {w:?} should be {r} in {s:?}"));
            }
        }
    }
    out
}

#[test]
pub(super) fn migrated_sources_have_accents() {
    let a = accents();
    let files = source_files();
    let mut bad = Vec::new();
    let matches = |entry: &str, file: &str| {
        if entry.ends_with('/') {
            file.starts_with(entry)
        } else {
            file == entry
        }
    };
    for (file, src) in &files {
        if ACCENT_STRICT
            .iter()
            .chain(ACCENT_STRICT_W32)
            .any(|e| matches(e, file))
        {
            bad.extend(unaccented_literals(file, src, &a));
        }
    }
    if files.iter().any(|(f, _)| f.starts_with("kernel/")) {
        for entry in ACCENT_STRICT.iter().chain(ACCENT_STRICT_W32) {
            assert!(
                files.iter().any(|(f, _)| matches(entry, f)),
                "a strict file is missing: {entry}"
            );
        }
    }
    assert!(bad.is_empty(), "{}", bad.join("\n"));
}

/// The whole tree: literals that look like unaccented Portuguese. Not a pass/fail check.
#[test]
#[ignore = "prints the migration backlog: cargo test -p kitsune_core i18n_report -- --ignored --nocapture"]
pub(super) fn i18n_report() {
    let a = accents();
    let mut n = 0;
    for (file, src) in source_files() {
        for l in unaccented_literals(&file, &src, &a) {
            std::println!("{l}");
            n += 1;
        }
    }
    std::println!("{n} literals without accents");
}

// ---------------------------------------------------------------------------- fonts

pub(super) fn font_has(font: &crate::ui::ttf::Font<'_>, c: char) -> bool {
    font.glyph_index(c) != 0
}
