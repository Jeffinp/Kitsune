//! Opening, saving and the unsaved-changes guard of the editor.

use super::geometry::picker_go;
use super::geometry::read_for_editor;
use crate::desktop::apps::editor::state::EdModal;
use crate::desktop::services::vfs;
use crate::desktop::*;
use kitsune_core::editor2::{CloseAsk, PickMode, Picker};
use kitsune_core::t;
use kitsune_core::vfs::VfsError;

impl Desktop {
    // ---- open ----

    /// Ctrl+O.
    pub(super) fn editor_open_dialog(&mut self, id: WindowId) {
        let Some(e) = self.editor_mut(id) else { return };
        let dir = e.path.as_deref().map_or_else(
            || String::from("/"),
            |p| String::from_utf8_lossy(&vfs::parent(p)).into_owned(),
        );
        let mut p = Picker::new(PickMode::Open, &dir, "");
        picker_go(&mut p, &dir);
        e.raise(EdModal::Open(p));
    }

    /// The user picked `path` in the Open dialog `p`.
    pub(super) fn editor_open_chosen(&mut self, id: WindowId, mut p: Picker, path: String) {
        let bytes = path.into_bytes();
        let here = self.editor_mut(id).is_some_and(|e| e.pristine());
        if here {
            match read_for_editor(&bytes) {
                Ok(data) => {
                    if let Some(e) = self.editor_mut(id) {
                        e.load(bytes, &data);
                    }
                }
                Err(err) => {
                    p.set_error(err.message());
                    self.set_modal(id, Some(EdModal::Open(p)));
                }
            }
            return;
        }
        // The window holds a document: the chosen file gets its own window (or the
        // one already showing it), so nothing is replaced.
        if let Err(err) = self.fs_load_path(bytes) {
            p.set_error(err.message());
            self.set_modal(id, Some(EdModal::Open(p)));
        }
    }

    /// Open `path` in an editor window: the one that already shows it, else a new
    /// one. Used by the file manager and by `edit`. The `Option` is a warning for
    /// the caller to show (none today: files are opened whole).
    pub(crate) fn fs_load_path(&mut self, path: Vec<u8>) -> Result<Option<&'static str>, VfsError> {
        let open = self
            .wm
            .windows()
            .iter()
            .filter(|w| !w.is_closing())
            .find_map(|w| match &w.app.app {
                App::Editor(e) if e.path.as_deref() == Some(path.as_slice()) => Some(w.id),
                _ => None,
            });
        if let Some(id) = open {
            self.wm.activate(id);
            return Ok(None);
        }
        let data = read_for_editor(&path)?;
        let id = self.open_new(Kind::Editor).ok_or(VfsError::Busy)?;
        if let Some(e) = self.editor_mut(id) {
            e.load(path, &data);
        }
        self.sync_editor(id);
        self.refresh_editor_title(id);
        Ok(None)
    }

    /// `edit [FILE]` from the terminal: open `path` (an empty document that
    /// will be created on save when it does not exist yet), or a blank editor.
    pub(crate) fn open_editor_for(&mut self, path: Option<String>) {
        let Some(path) = path else {
            self.open_new(Kind::Editor);
            return;
        };
        let bytes = path.into_bytes();
        if !vfs::exists(&bytes) {
            let Some(id) = self.open_new(Kind::Editor) else {
                return;
            };
            if let Some(e) = self.editor_mut(id) {
                e.path = Some(bytes);
                e.msg = Some((String::from(t!("edit.new_file")), false));
            }
            self.refresh_editor_title(id);
            return;
        }
        if let Err(e) = self.fs_load_path(bytes) {
            crate::notify!(Warn, "{}", t!("edit.notify", msg = e.message()));
        }
    }

    // ---- save ----

    /// Ctrl+S (and "Salvar" in the close question).
    pub(crate) fn editor_save(&mut self, id: WindowId, then_close: bool) {
        let Some(path) = self.editor_mut(id).map(|e| e.path.clone()) else {
            return;
        };
        match path {
            None => self.editor_save_as_dialog(id, then_close),
            Some(p) => {
                if let Err(err) = self.editor_write(id, &p) {
                    if let Some(e) = self.editor_mut(id) {
                        e.msg = Some((String::from(err.message()), true));
                    }
                } else if then_close {
                    self.finish_close(id);
                }
            }
        }
        self.refresh_editor_title(id);
    }

    /// Write the buffer to `path`; on success it becomes the document's file.
    fn editor_write(&mut self, id: WindowId, path: &[u8]) -> Result<(), VfsError> {
        let Some(e) = self.editor_mut(id) else {
            return Err(VfsError::NotFound);
        };
        let data = e.ed.file_bytes();
        vfs::write_file(path, &data)?;
        e.ed.mark_saved();
        e.path = Some(path.to_vec());
        e.msg = Some((
            t!(
                "edit.saved",
                name = &String::from_utf8_lossy(vfs::base_name(path)).into_owned()
            ),
            false,
        ));
        self.fs_changed();
        Ok(())
    }

    fn finish_close(&mut self, id: WindowId) {
        if let Some(e) = self.editor_mut(id) {
            e.force_close = true;
        }
        self.request_close(id);
    }

    /// Ctrl+Shift+S, and Ctrl+S on a document without a file.
    pub(super) fn editor_save_as_dialog(&mut self, id: WindowId, then_close: bool) {
        let Some(e) = self.editor_mut(id) else { return };
        let (dir, name) = match &e.path {
            Some(p) => (
                String::from_utf8_lossy(&vfs::parent(p)).into_owned(),
                String::from_utf8_lossy(vfs::base_name(p)).into_owned(),
            ),
            None => (String::from("/"), String::from(t!("edit.untitled_file"))),
        };
        let mut picker = Picker::new(PickMode::SaveAs, &dir, &name);
        picker_go(&mut picker, &dir);
        e.raise(EdModal::SaveAs { picker, then_close });
    }

    /// The user chose `path` in the Save-as dialog. An existing file other than the
    /// document's own is replaced only after "Substituir?" is confirmed.
    pub(super) fn editor_save_chosen(
        &mut self,
        id: WindowId,
        mut picker: Picker,
        path: String,
        then_close: bool,
        confirmed: bool,
    ) {
        let bytes = path.clone().into_bytes();
        let own = self
            .editor_mut(id)
            .is_some_and(|e| e.path.as_deref() == Some(bytes.as_slice()));
        if !confirmed && !own && vfs::exists(&bytes) {
            picker.confirm_overwrite(&path);
            self.set_modal(id, Some(EdModal::SaveAs { picker, then_close }));
            return;
        }
        match self.editor_write(id, &bytes) {
            Ok(()) => {
                if then_close {
                    self.finish_close(id);
                }
            }
            Err(err) => {
                picker.set_error(err.message());
                self.set_modal(id, Some(EdModal::SaveAs { picker, then_close }));
            }
        }
    }

    // ---- closing ----

    /// An editor with unsaved changes is asked about first. `true` when the close
    /// was held back (the question is now on screen).
    pub(crate) fn editor_holds_close(&mut self, id: WindowId) -> bool {
        let Some(w) = self.wm.get_mut(id) else {
            return false;
        };
        if w.is_closing() {
            return false;
        }
        let App::Editor(e) = &mut w.app.app else {
            return false;
        };
        if !e.ed.is_modified() || e.force_close {
            return false;
        }
        if !matches!(e.modal, Some(EdModal::Close(_))) {
            e.raise(EdModal::Close(CloseAsk::new()));
        }
        self.wm.activate(id);
        true
    }

    /// Before a reboot or shutdown: if some editor holds unsaved changes, ask
    /// about it instead and cancel the power action. `true` = blocked.
    pub(crate) fn guard_unsaved(&mut self) -> bool {
        let dirty = self
            .wm
            .windows()
            .iter()
            .find(|w| !w.is_closing() && matches!(&w.app.app, App::Editor(e) if e.ed.is_modified()))
            .map(|w| w.id);
        match dirty {
            Some(id) => self.editor_holds_close(id),
            None => false,
        }
    }
}
