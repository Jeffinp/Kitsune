use super::*;

#[test]
fn context_menu_adapts_to_the_selection() {
    let ctx = |in_trash, selected, image, clip| MenuCtx {
        zip: false,
        in_trash,
        in_apps: false,
        app_installed: false,
        selected,
        image,
        clip_has_items: clip,
    };
    let cmds = |m: Vec<(Cmd, &str)>| m.into_iter().map(|(c, _)| c).collect::<Vec<_>>();
    let blank = cmds(context_menu(ctx(false, 0, false, false)));
    assert!(blank.contains(&Cmd::NewFolder) && !blank.contains(&Cmd::Paste));
    let blank = cmds(context_menu(ctx(false, 0, false, true)));
    assert!(blank.contains(&Cmd::Paste));
    let one = cmds(context_menu(ctx(false, 1, true, false)));
    assert!(one.contains(&Cmd::Rename) && one.contains(&Cmd::SetWallpaper));
    let two = cmds(context_menu(ctx(false, 2, false, false)));
    assert!(!two.contains(&Cmd::Rename) && !two.contains(&Cmd::Open));
    let trash = cmds(context_menu(ctx(true, 1, false, false)));
    assert!(trash.contains(&Cmd::Restore) && trash.contains(&Cmd::EmptyTrash));
    assert!(!trash.contains(&Cmd::NewFile));
    for m in [
        context_menu(ctx(false, 0, false, true)),
        context_menu(ctx(false, 1, true, false)),
        context_menu(ctx(true, 1, false, false)),
    ] {
        assert!(m.iter().all(|(_, l)| !l.is_empty()));
        // Entries come grouped: the group number never goes back.
        let groups: Vec<u8> = m.iter().map(|(c, _)| c.group()).collect();
        let mut sorted = groups.clone();
        sorted.sort();
        assert_eq!(groups, sorted, "{m:?}");
    }
}
