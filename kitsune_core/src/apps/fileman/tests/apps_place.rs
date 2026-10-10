use super::*;

#[test]
fn apps_rows_carry_id_state_and_size() {
    let _g = crate::i18n::testlang::LangGuard::new(crate::i18n::Lang::Pt);
    let rows = apps::rows(&items());
    assert_eq!(rows.len(), 4);
    let r = &rows[2];
    assert_eq!(r.name, b"Pintura");
    assert_eq!(r.id, b"paint");
    assert!(!r.installed && r.mtime == 0 && r.size == 9000 && !r.is_dir());
    assert!(rows[0].installed && rows[0].mtime == 1);
    assert_eq!(apps::status_label(true), "instalado");
    assert_eq!(apps::status_label(false), "não instalado");
}

#[test]
fn apps_place_is_a_pseudo_path_that_never_touches_the_volume() {
    let mut fs = fresh();
    fs.write_file("/a.txt", b"x", NOW).unwrap();
    let mut v = FileView::new();
    v.refresh(&mut fs).unwrap();
    assert!(!v.in_apps());
    assert!(!v.rows.is_empty(), "the root has rows");
    v.navigate(&mut fs, APPS_PATH).unwrap();
    assert!(v.in_apps() && !v.in_trash());
    assert_eq!(v.cwd, APPS_PATH);
    // The volume has no such folder: no row of the volume leaks in.
    assert!(v.rows.is_empty());
    let crumbs = breadcrumbs(APPS_PATH);
    assert_eq!(crumbs.len(), 2);
    assert_eq!(
        (&crumbs[1].label[..], &crumbs[1].path[..]),
        (&b"Apps"[..], APPS_PATH)
    );
    v.set_apps(&items());
    // Name order (the default sort): Notas, Pintura, Relógio, Snake.
    assert_eq!(names(&v.rows), ["Notas", "Pintura", "Relógio", "Snake"]);
    assert_eq!(v.sel.cursor(), 0);
    assert!(v.sel.is_selected(0), "the first app is selected");
    v.set_apps(&items());
    assert!(v.sel.is_selected(0), "and stays selected on a reload");
    // A reload (the volume changed) keeps the app rows.
    v.refresh(&mut fs).unwrap();
    assert_eq!(v.rows.len(), 4);
    // No file paths in this place.
    assert_eq!(v.path_of(0), None);
    assert!(v.selected_paths().is_empty());
    // Up goes to the root; history remembers the Apps place.
    v.go_up(&mut fs).unwrap();
    assert_eq!(v.cwd, b"/");
    v.go_back(&mut fs).unwrap();
    assert!(v.in_apps());
}

#[test]
fn apps_selection_follows_the_app_across_catalog_changes() {
    let _g = crate::i18n::testlang::LangGuard::new(crate::i18n::Lang::Pt);
    let mut fs = fresh();
    let mut v = FileView::new();
    v.navigate(&mut fs, APPS_PATH).unwrap();
    v.set_apps(&items());
    v.sel.only(2); // Relógio
    assert_eq!(v.rows[2].id, b"clock");
    // Pintura gets installed and Notas removed: the cursor stays on the clock.
    let mut now = items();
    now[2].installed = true;
    now.retain(|a| a.id != "notes");
    v.set_apps(&now);
    assert_eq!(v.rows[v.sel.cursor()].id, b"clock");
    // The selected app disappears: the cursor falls back to the first row.
    now.retain(|a| a.id != "clock");
    v.set_apps(&now);
    assert_eq!(v.sel.cursor(), 0);
    v.set_apps(&[]);
    assert!(v.rows.is_empty());
    assert_eq!(v.summary(), "0 item");
}

#[test]
fn activating_an_app_row_asks_for_the_app_not_a_file() {
    let mut fs = fresh();
    let mut v = FileView::new();
    v.navigate(&mut fs, APPS_PATH).unwrap();
    v.set_apps(&items());
    match v.activate(&mut fs, 1) {
        Activation::App { id, installed } => {
            assert_eq!(id, b"paint");
            assert!(!installed);
        }
        a => panic!("{a:?}"),
    }
    assert_eq!(v.activate(&mut fs, 99), Activation::None);
}

#[test]
fn app_keys_install_remove_and_run() {
    let _g = crate::i18n::testlang::LangGuard::new(crate::i18n::Lang::Pt);
    let rows = apps::rows(&items());
    let installed = rows.iter().find(|r| r.id == b"notes").unwrap();
    let missing = rows.iter().find(|r| r.id == b"paint").unwrap();
    use apps::{AppAction, AppKey, app_action};
    assert_eq!(
        app_action(installed, AppKey::Enter),
        Ok(AppAction::Launch("notes".into()))
    );
    assert_eq!(
        app_action(missing, AppKey::Enter),
        Ok(AppAction::InstallAndLaunch("paint".into()))
    );
    assert_eq!(
        app_action(missing, AppKey::Install),
        Ok(AppAction::Install("paint".into()))
    );
    assert_eq!(app_action(installed, AppKey::Install), Err("Já instalado"));
    {
        let _en = crate::i18n::testlang::LangGuard::new(crate::i18n::Lang::En);
        assert_eq!(
            app_action(installed, AppKey::Install),
            Err("Already installed")
        );
        assert_eq!(app_action(missing, AppKey::Remove), Err("Not installed"));
    }
    assert_eq!(
        app_action(installed, AppKey::Remove),
        Ok(AppAction::Remove("notes".into()))
    );
    assert_eq!(app_action(missing, AppKey::Remove), Err("Não instalado"));
}

#[test]
fn apps_context_menu_offers_what_applies() {
    let ctx = |selected, installed| MenuCtx {
        zip: false,
        in_trash: false,
        in_apps: true,
        app_installed: installed,
        selected,
        image: false,
        clip_has_items: true,
    };
    let cmds = |c| {
        context_menu(c)
            .into_iter()
            .map(|(c, _)| c)
            .collect::<Vec<_>>()
    };
    let on_installed = cmds(ctx(1, true));
    assert_eq!(on_installed[0], Cmd::Open);
    assert!(on_installed.contains(&Cmd::RemoveApp) && !on_installed.contains(&Cmd::InstallApp));
    let on_missing = cmds(ctx(1, false));
    assert!(on_missing.contains(&Cmd::InstallApp) && !on_missing.contains(&Cmd::RemoveApp));
    // None of the file commands (they would act on paths that do not exist here).
    for c in [
        &on_installed,
        &on_missing,
        &cmds(ctx(0, false)),
        &cmds(ctx(3, false)),
    ] {
        for bad in [
            Cmd::NewFile,
            Cmd::NewFolder,
            Cmd::Cut,
            Cmd::Copy,
            Cmd::Paste,
            Cmd::Delete,
            Cmd::DeletePermanent,
            Cmd::Rename,
            Cmd::SetWallpaper,
        ] {
            assert!(!c.contains(&bad), "{bad:?}");
        }
    }
    assert!(
        context_menu(ctx(1, false))
            .iter()
            .all(|(_, l)| !l.is_empty())
    );
}

#[test]
fn manifest_lines_show_every_permission() {
    let _g = crate::i18n::testlang::LangGuard::new(crate::i18n::Lang::Pt);
    use crate::platform::appmanifest::{ClipPerm, FsPerm, NetPerm};
    let mut m = crate::platform::appmanifest::Manifest::legacy("demo", "Demo");
    m.fs = FsPerm::Own;
    m.net = NetPerm::Http;
    m.clipboard = ClipPerm::Rw;
    let lines = apps::manifest_lines(&m);
    let all = lines.join("\n");
    assert!(all.contains("Demo (demo)"), "{all}");
    assert!(all.contains("/data/demo"), "{all}");
    assert!(all.contains("HTTP"), "{all}");
    assert!(all.contains("ler e escrever"), "{all}");
    assert!(all.contains("Memória:") && all.contains("Janela "), "{all}");
    {
        let _en = crate::i18n::testlang::LangGuard::new(crate::i18n::Lang::En);
        let en = apps::manifest_lines(&m).join("\n");
        assert!(en.contains("Files: only /data/demo"), "{en}");
        assert!(
            en.contains("read and write") && en.contains("Window "),
            "{en}"
        );
        assert!(en.contains("Clipboard:"), "{en}");
    }
    m.fs = FsPerm::None;
    m.net = NetPerm::None;
    m.clipboard = ClipPerm::None;
    let none = apps::manifest_lines(&m).join("\n");
    assert!(
        none.contains("Arquivos: nenhum") && none.contains("Rede: nenhuma"),
        "{none}"
    );
    // Each line is a short "label: value" the information sheet can split at the colon.
    assert!(
        lines
            .iter()
            .all(|l| l.contains(": ") && l.chars().count() <= 60),
        "{lines:?}"
    );
    m.fs = FsPerm::Home;
    assert!(apps::manifest_lines(&m).join("\n").contains("/home"));
}

#[test]
fn manifest_lines_name_the_app_in_the_language_in_effect() {
    let mut m = crate::platform::appmanifest::Manifest::legacy("clock", "Clock");
    m.names = alloc::vec![
        (
            alloc::string::String::from("pt"),
            alloc::string::String::from("Relógio")
        ),
        (
            alloc::string::String::from("en"),
            alloc::string::String::from("Clock")
        ),
    ];
    {
        let _pt = crate::i18n::testlang::LangGuard::new(crate::i18n::Lang::Pt);
        assert!(
            apps::manifest_lines(&m)
                .join("\n")
                .contains("Relógio (clock)")
        );
    }
    let _en = crate::i18n::testlang::LangGuard::new(crate::i18n::Lang::En);
    assert!(
        apps::manifest_lines(&m)
            .join("\n")
            .contains("Clock (clock)")
    );
}
