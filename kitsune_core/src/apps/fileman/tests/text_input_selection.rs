use super::*;

#[test]
fn renaming_selects_the_stem() {
    let mut t = TextInput::new(b"relatorio.final.pdf", 255);
    assert_eq!(t.selection(), None);
    t.select_stem();
    assert_eq!(t.selection(), Some((0, 15)));
    t.insert(b'X');
    assert_eq!(t.text(), b"X.pdf");
    assert_eq!(t.caret(), 1);
    assert_eq!(t.selection(), None);
    // No extension, or a leading dot: everything.
    let mut t = TextInput::new(b"Nova pasta", 255);
    t.select_stem();
    assert_eq!(t.selection(), Some((0, 10)));
    let mut t = TextInput::new(b".config", 255);
    t.select_stem();
    assert_eq!(t.selection(), Some((0, 7)));
}

#[test]
fn selected_text_is_replaced_deleted_or_collapsed() {
    let mut t = TextInput::new(b"abcdef", 255);
    t.select_all();
    assert_eq!(t.selection(), Some((0, 6)));
    t.backspace();
    assert_eq!(t.text(), b"");
    let mut t = TextInput::new(b"abcdef", 255);
    t.select_all();
    t.delete();
    assert_eq!(t.text(), b"");
    let mut t = TextInput::new(b"abcdef", 255);
    t.select_all();
    t.left();
    assert_eq!((t.caret(), t.selection()), (0, None));
    let mut t = TextInput::new(b"abcdef", 255);
    t.select_all();
    t.right();
    assert_eq!((t.caret(), t.selection()), (6, None));
    let mut t = TextInput::new(b"abcdef", 255);
    t.select_all();
    t.home();
    assert_eq!(t.selection(), None);
    // Replacing does not overflow the limit even when the selection is bigger than the key.
    let mut t = TextInput::new(b"abcd", 4);
    t.select_all();
    t.insert(b'z');
    assert_eq!(t.text(), b"z");
    t.insert(b'a');
    t.insert(b'b');
    t.insert(b'c');
    t.insert(b'd');
    assert_eq!(t.text(), b"zabc");
}

#[test]
fn latin1_keys_are_stored_as_utf8() {
    let mut t = TextInput::new(b"", 255);
    for b in [b'a', 0xE7, 0xE3, b'o'] {
        t.insert(b);
    }
    assert_eq!(t.text(), "açãо".replace('о', "o").as_bytes());
    assert_eq!(t.to_string_lossy(), "ação");
    t.backspace();
    t.backspace();
    assert_eq!(t.to_string_lossy(), "aç");
    // The limit counts bytes, and a character never splits.
    let mut t = TextInput::new(b"abc", 4);
    t.insert(0xE7); // needs two bytes: no room
    assert_eq!(t.text(), b"abc");
    let t = TextInput::new("aç".as_bytes(), 2);
    assert_eq!(t.text(), b"a");
}

#[test]
fn counts_and_selection_text_in_both_languages() {
    use crate::i18n::{Lang, testlang::LangGuard};
    let mut v = searchable();
    let n = v.rows.len();
    assert!(n > 1);
    for (l, none, all_sel) in [
        (
            Lang::Pt,
            std::format!("{n} itens"),
            std::format!("{n} selecionados"),
        ),
        (
            Lang::En,
            std::format!("{n} items"),
            std::format!("{n} selected"),
        ),
    ] {
        let _g = LangGuard::new(l);
        v.sel.clear();
        assert_eq!(v.summary(), none, "{l:?}");
        v.sel.select_all();
        assert!(v.summary().starts_with(&all_sel), "{l:?}: {}", v.summary());
    }
    let _g = LangGuard::new(Lang::En);
    assert_eq!(crate::tp!("files.count", 1u32), "1 item");
    assert_eq!(crate::tp!("files.count", 0u32), "0 items");
    drop(_g);
    // pt-BR: zero is singular too.
    let _g = LangGuard::new(Lang::Pt);
    assert_eq!(crate::tp!("files.count", 0u32), "0 item");
    assert_eq!(crate::tp!("files.count", 1u32), "1 item");
    assert_eq!(crate::tp!("files.count", 2u32), "2 itens");
    assert_eq!(
        crate::tp!("files.selected", 1u32, size = crate::i18n::bytes(1536)),
        "1 selecionado (1,5 KiB)"
    );
}

#[test]
fn context_menu_labels_follow_the_language() {
    use crate::i18n::{Lang, testlang::LangGuard};
    let blank = MenuCtx {
        zip: false,
        in_trash: false,
        in_apps: false,
        app_installed: false,
        selected: 0,
        image: false,
        clip_has_items: true,
    };
    let labels = || {
        context_menu(blank)
            .into_iter()
            .map(|(_, l)| l)
            .collect::<Vec<_>>()
    };
    {
        let _g = LangGuard::new(Lang::Pt);
        assert_eq!(
            labels(),
            [
                "Novo arquivo",
                "Nova pasta",
                "Colar",
                "Selecionar tudo",
                "Atualizar",
                "Informações"
            ]
        );
        assert_eq!(Cmd::TogglePreview.shortcut(), "Espaço");
    }
    let _g = LangGuard::new(Lang::En);
    assert_eq!(
        labels(),
        [
            "New file",
            "New folder",
            "Paste",
            "Select all",
            "Refresh",
            "Properties"
        ]
    );
    assert_eq!(Cmd::TogglePreview.shortcut(), "Space");
    let trash = MenuCtx {
        zip: false,
        in_trash: true,
        selected: 1,
        ..blank
    };
    let l: Vec<_> = context_menu(trash).into_iter().map(|(_, l)| l).collect();
    assert_eq!(
        l,
        [
            "Restore",
            "Delete permanently",
            "Empty trash",
            "Properties",
            "Select all"
        ]
    );
}

#[test]
fn breadcrumbs_special_labels_follow_the_language() {
    use crate::i18n::{Lang, testlang::LangGuard};
    let _g = LangGuard::new(Lang::En);
    assert_eq!(breadcrumbs(b"/")[0].label, b"Disk");
    assert_eq!(breadcrumbs(TRASH_PATH)[1].label, b"Trash");
    assert_eq!(breadcrumbs(b"/Projetos/x")[1].label, b"Projetos");
}
