//! commands (split out of `fileman.rs`).

use super::*;

/// An action of the file manager (context menu entries and key shortcuts).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Cmd {
    Open,
    NewFile,
    NewFolder,
    Rename,
    Copy,
    Cut,
    Paste,
    Delete,
    DeletePermanent,
    Restore,
    EmptyTrash,
    Properties,
    SelectAll,
    Refresh,
    SetWallpaper,
    /// Extract the selected ZIP archive into a folder beside it.
    ExtractHere,
    /// Compress the selection into a ZIP archive beside it.
    Compress,
    /// Apps place: install the selected bundled package.
    InstallApp,
    /// Apps place: remove the selected installed app.
    RemoveApp,
    /// Sort by a column (the sort menu): the same column again flips the direction.
    SortBy(SortKey),
    /// Set the sort direction (`true` = ascending).
    SortDir(bool),
    /// Switch between the list and the icon grid.
    SetView(ui::ViewMode),
    /// Show or hide the preview pane (Space).
    TogglePreview,
}

impl Cmd {
    /// Menu group: entries of different groups are separated by a line.
    pub fn group(self) -> u8 {
        match self {
            Cmd::Open | Cmd::Restore | Cmd::InstallApp | Cmd::TogglePreview => 0,
            Cmd::SetWallpaper | Cmd::ExtractHere | Cmd::Compress => 1,
            Cmd::NewFile | Cmd::NewFolder => 2,
            Cmd::Cut | Cmd::Copy | Cmd::Paste | Cmd::Rename => 3,
            Cmd::Delete | Cmd::DeletePermanent | Cmd::EmptyTrash | Cmd::RemoveApp => 4,
            Cmd::SelectAll
            | Cmd::Refresh
            | Cmd::Properties
            | Cmd::SortBy(_)
            | Cmd::SortDir(_)
            | Cmd::SetView(_) => 5,
        }
    }

    /// The keyboard shortcut shown next to the entry.
    pub fn shortcut(self) -> &'static str {
        match self {
            Cmd::Open => "Enter",
            Cmd::Copy => "Ctrl+C",
            Cmd::Cut => "Ctrl+X",
            Cmd::Paste => "Ctrl+V",
            Cmd::Rename => "F2",
            Cmd::Delete => "Del",
            Cmd::SelectAll => "Ctrl+A",
            Cmd::Refresh => "F5",
            Cmd::TogglePreview => crate::t!("files.key.space"),
            Cmd::NewFolder => "N",
            _ => "",
        }
    }
}

/// What the context menu is about to be shown for.
#[derive(Clone, Copy, Debug)]
pub struct MenuCtx {
    pub in_trash: bool,
    /// The Apps place (the menu then offers run / install / remove).
    pub in_apps: bool,
    /// Apps place: the single selected app is installed.
    pub app_installed: bool,
    /// Number of selected rows under the cursor click (0 = empty space).
    pub selected: usize,
    /// The single selected row is an image.
    pub image: bool,
    /// The single selected row is a ZIP archive.
    pub zip: bool,
    pub clip_has_items: bool,
}

/// The entries of the context menu, in order, with their labels in the language in effect.
pub fn context_menu(ctx: MenuCtx) -> Vec<(Cmd, &'static str)> {
    let mut m = Vec::new();
    if ctx.in_apps {
        if ctx.selected == 1 {
            if ctx.app_installed {
                m.push((Cmd::Open, crate::t!("files.menu.open")));
                m.push((Cmd::RemoveApp, crate::t!("files.menu.remove")));
            } else {
                m.push((Cmd::Open, crate::t!("files.menu.install_open")));
                m.push((Cmd::InstallApp, crate::t!("files.menu.install")));
            }
            m.push((Cmd::Properties, crate::t!("files.menu.properties")));
        }
        m.push((Cmd::Refresh, crate::t!("files.menu.refresh")));
        return m;
    }
    if ctx.in_trash {
        if ctx.selected > 0 {
            m.push((Cmd::Restore, crate::t!("files.menu.restore")));
            m.push((
                Cmd::DeletePermanent,
                crate::t!("files.menu.delete_permanently"),
            ));
        }
        m.push((Cmd::EmptyTrash, crate::t!("files.menu.empty_trash")));
        if ctx.selected > 0 {
            m.push((Cmd::Properties, crate::t!("files.menu.properties")));
        }
        m.push((Cmd::SelectAll, crate::t!("files.menu.select_all")));
        return m;
    }
    if ctx.selected > 0 {
        if ctx.selected == 1 {
            m.push((Cmd::Open, crate::t!("files.menu.open")));
            m.push((Cmd::TogglePreview, crate::t!("files.menu.preview")));
        }
        if ctx.selected == 1 && ctx.image {
            m.push((Cmd::SetWallpaper, crate::t!("files.menu.set_wallpaper")));
        }
        if ctx.selected == 1 && ctx.zip {
            m.push((Cmd::ExtractHere, crate::t!("files.menu.extract")));
        }
        m.push((Cmd::Compress, crate::t!("files.menu.compress")));
        m.push((Cmd::Cut, crate::t!("files.menu.cut")));
        m.push((Cmd::Copy, crate::t!("files.menu.copy")));
        if ctx.selected == 1 {
            m.push((Cmd::Rename, crate::t!("files.menu.rename")));
        }
        m.push((Cmd::Delete, crate::t!("files.menu.delete")));
        m.push((
            Cmd::DeletePermanent,
            crate::t!("files.menu.delete_permanently"),
        ));
        m.push((Cmd::Properties, crate::t!("files.menu.properties")));
    } else {
        m.push((Cmd::NewFile, crate::t!("files.menu.new_file")));
        m.push((Cmd::NewFolder, crate::t!("files.menu.new_folder")));
        if ctx.clip_has_items {
            m.push((Cmd::Paste, crate::t!("files.menu.paste")));
        }
        m.push((Cmd::SelectAll, crate::t!("files.menu.select_all")));
        m.push((Cmd::Refresh, crate::t!("files.menu.refresh")));
        m.push((Cmd::Properties, crate::t!("files.menu.properties")));
    }
    m
}
