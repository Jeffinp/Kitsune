//! View state and the commands (new folder, rename, delete, paste, ...).

use crate::desktop::*;
use kitsune_core::fileman::apps::AppKey;
use kitsune_core::fileman::ui::ViewMode;
use kitsune_core::fileman::{self, Cmd};
use kitsune_core::{t, tp};

impl Desktop {
    // ---- view state ----

    /// Switch between the list and the icon grid.
    pub(super) fn files_set_view(&mut self, id: WindowId, m: ViewMode) {
        let Some(f) = self.files_mut(id) else {
            return;
        };
        if f.mode == m {
            return;
        }
        f.mode = m;
        f.enter_t = kitsune_core::anim::Tween::at(0.0);
        f.enter_t
            .retarget(1.0, 0.18, kitsune_core::anim::curves::ENTER);
        f.scroller.jump(0);
        f.hover = None;
        self.files_reveal(id);
    }

    /// Replace the search text (and filter the rows).
    pub(super) fn files_set_search(&mut self, id: WindowId, text: &[u8]) {
        let Some(f) = self.files_mut(id) else {
            return;
        };
        f.search.input.set(text);
        f.view.set_filter(text);
        f.scroller.jump(0);
        f.view.select_first();
    }

    // ---- commands ----

    /// Run a file-manager command on window `id`'s selection.
    pub(crate) fn files_cmd(&mut self, id: WindowId, cmd: Cmd) {
        let Some(f) = self.files_mut(id) else {
            return;
        };
        if let Cmd::SetView(m) = cmd {
            return self.files_set_view(id, m);
        }
        match cmd {
            Cmd::TogglePreview => {
                f.preview_open = !f.preview_open;
                f.preview = None;
                self.files_sync_preview(id);
                return;
            }
            Cmd::SortBy(k) => {
                if f.view.sort.key == k {
                    f.view.click_header(k);
                } else {
                    f.view.set_sort(k);
                }
                return;
            }
            Cmd::SortDir(asc) => {
                if f.view.sort.asc != asc {
                    let k = f.view.sort.key;
                    f.view.click_header(k);
                }
                return;
            }
            _ => {}
        }
        if f.view.in_apps() {
            let c = f.view.sel.cursor();
            return match cmd {
                Cmd::Open => self.files_app_key(id, c, AppKey::Enter),
                Cmd::InstallApp => self.files_app_key(id, c, AppKey::Install),
                // Delete and Shift+Delete both mean "remove the app".
                Cmd::RemoveApp | Cmd::Delete | Cmd::DeletePermanent => {
                    self.files_app_key(id, c, AppKey::Remove)
                }
                Cmd::Properties => self.files_app_properties(id),
                Cmd::SelectAll => f.view.sel.select_all(),
                Cmd::Refresh => {
                    self.files_refresh(id);
                    self.files_note(id, t!("files.msg.refreshed"), false);
                }
                _ => {}
            };
        }
        let in_trash = f.view.in_trash();
        let cwd = f.view.cwd.clone();
        let paths = f.view.selected_paths();
        let ids: Vec<Vec<u8>> = f
            .view
            .selected_rows()
            .iter()
            .map(|r| r.id.clone())
            .collect();
        let first_name = f.view.selected_rows().first().map(|r| r.name.clone());
        match cmd {
            Cmd::Open => {
                let c = f.view.sel.cursor();
                self.files_activate(id, c);
            }
            Cmd::SelectAll => f.view.sel.select_all(),
            Cmd::Refresh => {
                self.files_refresh(id);
                self.files_note(id, t!("files.msg.refreshed"), false);
            }
            Cmd::NewFile | Cmd::NewFolder => {
                if in_trash {
                    return;
                }
                let base: &[u8] = if cmd == Cmd::NewFile {
                    t!("files.new_file_name").as_bytes()
                } else {
                    t!("files.new_folder_name").as_bytes()
                };
                let name = vfs::unique_name_in(&cwd, base);
                let made = if cmd == Cmd::NewFile {
                    vfs::new_file(&cwd, &name)
                } else {
                    vfs::new_folder(&cwd, &name)
                };
                match made {
                    Ok(path) => {
                        // The new item exists at once; its name is being edited.
                        self.fs_changed();
                        if let Some(f) = self.files_mut(id) {
                            // A new item must show: drop a search that would hide it.
                            if !f.view.filter().is_empty() {
                                f.search.input.clear();
                                f.view.clear_filter();
                            }
                            f.view.select_name(vfs::base_name(&path));
                        }
                        self.files_reveal(id);
                        self.files_begin_rename(id, path, true);
                    }
                    Err(e) => self.files_note(id, e.message(), true),
                }
            }
            Cmd::Rename => {
                if in_trash || paths.len() != 1 {
                    return;
                }
                let _ = first_name;
                self.files_begin_rename(id, paths[0].clone(), false);
            }
            Cmd::Copy | Cmd::Cut => {
                if paths.is_empty() {
                    return;
                }
                let n = paths.len();
                let cut = cmd == Cmd::Cut;
                f.say(
                    &if cut {
                        tp!("files.msg.cut", n)
                    } else {
                        tp!("files.msg.copied", n)
                    },
                    false,
                );
                self.pathclip.set(paths, cut);
            }
            Cmd::Paste => self.files_paste(id),
            Cmd::Delete => {
                if in_trash {
                    return self.files_cmd(id, Cmd::DeletePermanent);
                }
                if paths.is_empty() {
                    return;
                }
                let mut done = 0;
                let mut err = None;
                for p in &paths {
                    match vfs::remove(p) {
                        Ok(()) => done += 1,
                        Err(e) => {
                            err = Some(e);
                            break;
                        }
                    }
                }
                self.fs_changed();
                match err {
                    None => self.files_note(id, &tp!("files.msg.trashed", done), false),
                    Some(e) => self.files_note(id, e.message(), true),
                }
            }
            Cmd::DeletePermanent => {
                if in_trash {
                    if !ids.is_empty() {
                        f.confirm = Some(Confirm::PurgeTrash(ids));
                        f.open_sheet();
                    }
                } else if !paths.is_empty() {
                    f.confirm = Some(Confirm::Purge(paths));
                    f.open_sheet();
                }
            }
            Cmd::EmptyTrash => {
                f.confirm = Some(Confirm::EmptyTrash);
                f.open_sheet();
            }
            Cmd::Restore => {
                if !in_trash || ids.is_empty() {
                    return;
                }
                let mut done = 0;
                let mut err = None;
                for t in &ids {
                    match vfs::restore(t) {
                        Ok(_) => done += 1,
                        Err(e) => err = Some(e),
                    }
                }
                self.fs_changed();
                match err {
                    None => self.files_note(id, &tp!("files.msg.restored", done), false),
                    Some(e) => self.files_note(id, e.message(), true),
                }
            }
            Cmd::Properties => self.files_properties(id, in_trash, &cwd, &paths),
            Cmd::ExtractHere | Cmd::Compress => {
                if in_trash || paths.is_empty() {
                    return;
                }
                self.files_archive(id, cmd == Cmd::Compress, paths);
            }
            // The Apps place handles these above; they do not exist on a volume path.
            Cmd::InstallApp | Cmd::RemoveApp => {}
            Cmd::SetWallpaper => {
                if let Some(p) = paths.first() {
                    match self.set_wallpaper_path(p) {
                        Some(m) => self.files_note(id, &m, true),
                        None => self.files_note(id, t!("files.msg.wallpaper"), false),
                    }
                }
            }
            Cmd::SortBy(_) | Cmd::SortDir(_) | Cmd::SetView(_) | Cmd::TogglePreview => {}
        }
        self.files_sync_preview(id);
    }

    /// "Extract here" (one ZIP) or "Compress" (the selection) on `paths`, answered with a note.
    fn files_archive(&mut self, id: WindowId, compress: bool, paths: Vec<Vec<u8>>) {
        use kitsune_core::fileman::archive::{self, ArchiveError};
        use kitsune_core::zip::ZipError;
        let now = crate::storage::now();
        let shown = |p: &[u8]| String::from_utf8_lossy(vfs::base_name(p)).into_owned();
        let done: Result<(Vec<u8>, String), ArchiveError> = if compress {
            vfs::with_backend(|b| archive::compress(b, &paths, t!("files.archive_name"), now))
                .map_err(ArchiveError::Fs)
                .and_then(|r| r)
                .map(|out| {
                    let note = t!("files.msg.compressed", name = shown(&out).as_str());
                    (out, note)
                })
        } else {
            vfs::with_backend(|b| archive::extract_zip(b, &paths[0], now))
                .map_err(ArchiveError::Fs)
                .and_then(|r| r)
                .map(|r| {
                    let name = shown(&r.folder);
                    let note = if r.skipped == 0 {
                        t!("files.msg.extracted", n = r.files, name = name.as_str())
                    } else {
                        t!(
                            "files.msg.extracted_skipped",
                            n = r.files,
                            name = name.as_str(),
                            skipped = r.skipped
                        )
                    };
                    (r.folder, note)
                })
        };
        self.fs_changed();
        match done {
            Ok((out, note)) => {
                self.files_note(id, &note, false);
                self.files_select_paths(id, &[out]);
            }
            Err(ArchiveError::Zip(ZipError::TooLarge)) => {
                self.files_note(id, t!("files.msg.archive_too_big"), true);
            }
            Err(ArchiveError::Zip(_)) => self.files_note(id, t!("files.msg.archive_bad"), true),
            Err(ArchiveError::Fs(e)) => self.files_note(id, e.message(), true),
        }
    }

    /// Start editing the name of the item at `path` (the stem selected, or the whole name for
    /// a new item).
    fn files_begin_rename(&mut self, id: WindowId, path: Vec<u8>, whole: bool) {
        let name = vfs::base_name(&path).to_vec();
        if let Some(f) = self.files_mut(id) {
            let mut input = fileman::TextInput::new(&name, vfs::MAX_NAME);
            if whole {
                input.select_all();
            } else {
                input.select_stem();
            }
            f.input = Some(NameEdit {
                input,
                purpose: EditPurpose::Rename(path),
                last_input: appui::ticks(),
            });
        }
    }

    /// Ctrl+V: a cut moves (instant); a copy becomes a job that runs in steps.
    fn files_paste(&mut self, id: WindowId) {
        let Some(f) = self.files_mut(id) else {
            return;
        };
        if f.view.in_trash() {
            f.say(t!("files.msg.paste_in_trash"), true);
            return;
        }
        if f.job.is_some() {
            return;
        }
        let dest = f.view.cwd.clone();
        if self.pathclip.is_empty() {
            self.files_note(id, t!("files.msg.nothing_to_paste"), true);
            return;
        }
        let sources: Vec<Vec<u8>> = self.pathclip.paths().to_vec();
        if self.pathclip.is_cut() {
            let rep = vfs::move_to(&sources, &dest);
            let n = rep.moved.len();
            match rep.error {
                None => {
                    self.pathclip.after_paste();
                    self.files_note(id, &tp!("files.msg.moved", n), false);
                }
                Some(e) => self.files_note(id, e.message(), true),
            }
            self.fs_changed();
            self.files_select_paths(id, &rep.moved);
            return;
        }
        self.files_start_copy(id, &sources, &dest);
    }

    /// Select, in window `id`, the items of `paths` that live in its current folder.
    pub(super) fn files_select_paths(&mut self, id: WindowId, paths: &[Vec<u8>]) {
        if let Some(f) = self.files_mut(id) {
            let names: Vec<Vec<u8>> = paths
                .iter()
                .filter(|p| vfs::parent(p) == f.view.cwd)
                .map(|p| vfs::base_name(p).to_vec())
                .collect();
            f.view.select_names(&names);
        }
        self.files_reveal(id);
    }
}
