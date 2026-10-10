//! The file manager's context and sort menus.

use crate::desktop::shell::{Cmd as ShellCmd, Entry, MenuOrigin};
use crate::desktop::*;
use kitsune_core::fileman::{self, Cmd, MenuCtx, SortKey};
use kitsune_core::t;

impl Desktop {
    /// Context needed to build the context menu for a click on row `row`.
    fn files_menu_ctx(&self, id: WindowId) -> Option<MenuCtx> {
        let Some(App::Files(f)) = self.wm.get(id).map(|w| &w.app.app) else {
            return None;
        };
        let rows = f.view.selected_rows();
        Some(MenuCtx {
            in_trash: f.view.in_trash(),
            in_apps: f.view.in_apps(),
            app_installed: rows.len() == 1 && rows[0].installed,
            selected: rows.len(),
            image: rows.len() == 1 && fileman::is_image(&rows[0].name) && !rows[0].is_dir(),
            zip: rows.len() == 1 && !rows[0].is_dir() && fileman::archive::is_zip(&rows[0].name),
            clip_has_items: !self.pathclip.is_empty(),
        })
    }

    /// Open the context menu of window `id` at `(px, py)`.
    pub(super) fn files_context_menu(&mut self, id: WindowId, px: i32, py: i32) {
        let Some(ctx) = self.files_menu_ctx(id) else {
            return;
        };
        let mut entries: Vec<Entry> = Vec::new();
        let mut last_group = None;
        for (cmd, label) in fileman::context_menu(ctx) {
            let g = cmd.group();
            if last_group.is_some_and(|l| l != g) {
                entries.push(Entry::sep());
            }
            last_group = Some(g);
            entries.push(Entry::item(label, cmd.shortcut(), ShellCmd::Files(cmd)));
        }
        self.open_menu(MenuOrigin::Context, entries, (px, py));
    }

    /// The sort menu, under the toolbar button.
    pub(super) fn files_sort_menu(&mut self, id: WindowId, at: (i32, i32)) {
        let Some(App::Files(f)) = self.wm.get(id).map(|w| &w.app.app) else {
            return;
        };
        let sort = f.view.sort;
        let apps = f.view.in_apps();
        let trash = f.view.in_trash();
        let mut entries = Vec::new();
        for (key, label) in [
            (SortKey::Name, t!("files.col.name")),
            (SortKey::Size, t!("files.col.size")),
            (
                SortKey::Modified,
                if trash {
                    t!("files.col.deleted")
                } else if apps {
                    t!("files.col.state")
                } else {
                    t!("files.col.modified")
                },
            ),
        ] {
            let mut e = Entry::item(label, "", ShellCmd::Files(Cmd::SortBy(key)));
            e.checked = sort.key == key;
            entries.push(e);
        }
        entries.push(Entry::sep());
        for (asc, label) in [(true, t!("files.sort.asc")), (false, t!("files.sort.desc"))] {
            let mut e = Entry::item(label, "", ShellCmd::Files(Cmd::SortDir(asc)));
            e.checked = sort.asc == asc;
            entries.push(e);
        }
        self.open_menu(MenuOrigin::Context, entries, at);
    }

    /// A menu entry of the file manager chosen: run it on the focused window.
    pub(crate) fn files_run_focused(&mut self, cmd: Cmd) {
        if let Some(id) = self.focused()
            && self.kind_of(id) == Some(Kind::Files)
        {
            self.files_cmd(id, cmd);
        }
    }
}
