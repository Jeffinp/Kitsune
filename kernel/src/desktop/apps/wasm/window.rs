//! WASM app windows: opening, launching, titles, and syncing the guest's frame and clipboard.

use crate::desktop::*;
use crate::serial_println;
use crate::wasm::{self, appfs_backend};
use alloc::boxed::Box;
use kitsune_core::appinstall;
use kitsune_core::appmanifest::{Abi, Manifest};
use kitsune_core::t;

/// Pixels the window frame adds around a WASM app's content area.
pub(crate) const FRAME_W: i32 = 28;
pub(crate) const FRAME_H: i32 = TITLE_H + 26;

/// Content area of a WASM window of rect `r` (origin, size).
pub(crate) fn wasm_content(r: Rect) -> Rect {
    Rect::new(
        r.x + 14,
        r.y + TITLE_H + 12,
        (r.w - FRAME_W).max(1),
        (r.h - FRAME_H).max(1),
    )
}

pub(super) fn argb_to_rgba(px: &[u32]) -> Vec<u8> {
    let mut out = Vec::with_capacity(px.len() * 4);
    for &p in px {
        let [a, r, g, b] = p.to_be_bytes();
        out.extend_from_slice(&[r, g, b, a]);
    }
    out
}

impl Desktop {
    /// The language changed: a window of an installed app shows the name its manifest gives
    /// in the new language (call after [`Self::refresh_catalog`]).
    pub(crate) fn retitle_wasm_windows(&mut self) {
        let mut renamed: Vec<(WindowId, String)> = Vec::new();
        for w in self.wm.windows() {
            if let App::Wasm(ww) = &w.app.app
                && let Some(e) = self.apps.iter().find(|e| e.id == ww.app_id)
            {
                let mut t = String::from(e.manifest.display_name());
                if w.app.index > 1 {
                    let mut tmp = [0u8; 16];
                    let k = numbered_name("", w.app.index, &mut tmp);
                    t.push_str(core::str::from_utf8(&tmp[..k]).unwrap_or(""));
                }
                if t != w.app.title {
                    renamed.push((w.id, t));
                }
            }
        }
        for (id, t) in renamed {
            if let Some(w) = self.wm.get_mut(id) {
                w.app.title = t;
            }
        }
    }

    // ---- windows ----

    fn free_index_wasm(&self, app_id: &str) -> u8 {
        let mut idx = 1u8;
        while self
            .wm
            .windows()
            .iter()
            .any(|w| matches!(&w.app.app, App::Wasm(x) if x.app_id == app_id) && w.app.index == idx)
        {
            idx += 1;
        }
        idx
    }

    /// The most recently used live window of installed app `app_id`.
    fn mru_of_app(&self, app_id: &str) -> Option<WindowId> {
        self.wm.switch_list().into_iter().find(|&id| {
            self.wm
                .get(id)
                .is_some_and(|w| matches!(&w.app.app, App::Wasm(x) if x.app_id == app_id))
        })
    }

    /// Focus the app's window, opening one when there is none.
    pub(crate) fn launch_wasm_app(&mut self, app_id: &str) -> Option<WindowId> {
        if let Some(id) = self.mru_of_app(app_id) {
            self.wm.activate(id);
            return Some(id);
        }
        self.open_wasm_app(app_id)
    }

    /// Open installed app `app_id` in a new window.
    pub(crate) fn open_wasm_app(&mut self, app_id: &str) -> Option<WindowId> {
        let manifest = self.apps.iter().find(|e| e.id == app_id)?.manifest.clone();
        let bytes = match appfs_backend::try_with(|fs| appinstall::read_package(fs, app_id)) {
            Ok(b) => b,
            Err(e) => {
                serial_println!("apps: cannot read `{}`: {}", app_id, e);
                return None;
            }
        };
        self.open_wasm(manifest, bytes)
    }

    /// The dock's WASM icon: the legacy embedded app (DOOM / C demo) when the image
    /// has one, else the default packaged app.
    pub(crate) fn launch_default_wasm(&mut self) -> Option<WindowId> {
        if wasm::LEGACY_APP.is_empty() {
            return self.launch_wasm_app(wasm::DEFAULT_APP);
        }
        if let Some(id) = self.mru_of_app("app") {
            self.wm.activate(id);
            return Some(id);
        }
        let manifest = Manifest::legacy("app", t!("app.wasm_title"));
        self.open_wasm(manifest, wasm::LEGACY_APP.to_vec())
    }

    /// A new window for the default app (generic `open_new(Kind::WasmApp)`).
    pub(crate) fn open_default_wasm(&mut self) -> Option<WindowId> {
        if wasm::LEGACY_APP.is_empty() {
            return self.open_wasm_app(wasm::DEFAULT_APP);
        }
        let manifest = Manifest::legacy("app", t!("app.wasm_title"));
        self.open_wasm(manifest, wasm::LEGACY_APP.to_vec())
    }

    fn open_wasm(&mut self, manifest: Manifest, bytes: Vec<u8>) -> Option<WindowId> {
        if self.wm.is_full() {
            return None;
        }
        let app_id = manifest.id.clone();
        let (cw, ch) = (manifest.win_w as i32, manifest.win_h as i32);
        let v1 = manifest.abi == Abi::V1;
        let handle = match wasm::launch(bytes, manifest.clone(), v1, cw, ch) {
            Ok(h) => h,
            Err(e) => {
                serial_println!("apps: cannot start `{}`: {:?}", app_id, e);
                return None;
            }
        };
        self.adopt_wasm(manifest, handle)
    }

    /// The window of the app `handle` (already in the manager's table): process entry, rectangle,
    /// title and the window itself. Closes the app again if there is no room.
    fn adopt_wasm(&mut self, manifest: Manifest, handle: wasm::AppId) -> Option<WindowId> {
        let app_id = manifest.id.clone();
        let (cw, ch) = (manifest.win_w as i32, manifest.win_h as i32);
        let (min_w, min_h) = (
            manifest.win_min_w as i32 + FRAME_W,
            manifest.win_min_h as i32 + FRAME_H,
        );
        let resizable = manifest.resizable;
        let title = String::from(manifest.display_name());
        let index = self.free_index_wasm(&app_id);
        let mut name = [0u8; 16];
        let n = numbered_name(&app_id, index, &mut name);
        let Some(pid) = self
            .procs
            .spawn(&name[..n], ProcKind::App, ProcState::Running)
        else {
            wasm::close(handle);
            return None;
        };
        let work = self.work_area();
        let (ow, oh) = ((cw + FRAME_W).min(work.w), (ch + FRAME_H).min(work.h));
        let base = Rect::new(240, 130, ow, oh);
        // New WASM windows cascade from the first position, whatever the app.
        let open_wasm = self
            .wm
            .windows()
            .iter()
            .filter(|w| matches!(w.app.app, App::Wasm(_)) && !w.is_closing())
            .count();
        let mut rect = if open_wasm == 0 {
            base
        } else {
            kitsune_core::winman::cascade_rect(base, open_wasm, work)
        };
        rect.x = rect.x.clamp(work.x, (work.right() - rect.w).max(work.x));
        rect.y = rect.y.clamp(work.y, (work.bottom() - rect.h).max(work.y));
        let mut full_title = title;
        if index > 1 {
            let mut tmp = [0u8; 16];
            let k = numbered_name("", index, &mut tmp);
            full_title.push_str(core::str::from_utf8(&tmp[..k]).unwrap_or(""));
        }
        let inst = Inst {
            app: App::Wasm(Box::new(WasmWin { id: handle, app_id })),
            pid,
            index,
            title: full_title,
            cost: core::cell::Cell::new(0),
            cost_pm: core::cell::Cell::new(0),
        };
        let spec = WindowSpec {
            rect,
            min_w: min_w.min(rect.w),
            min_h: min_h.min(rect.h),
            resizable,
        };
        match self.wm.open(spec, inst) {
            Ok(id) => Some(id),
            Err(inst) => {
                self.procs.kill(inst.pid);
                wasm::close(handle);
                None
            }
        }
    }

    /// Open the windows ring 3 processes asked for (`win_open`): each already has its slot in the
    /// manager's table, so it is adopted like a WebAssembly app that has just started.
    fn adopt_native_windows(&mut self) {
        for handle in wasm::take_native_windows() {
            let Some((id, title, w, h)) = wasm::native_info(handle) else {
                continue;
            };
            let mut m = Manifest::legacy(&id, &title);
            m.win_w = w as u32;
            m.win_h = h as u32;
            m.win_min_w = 160;
            m.win_min_h = 120;
            m.resizable = true;
            if self.wm.is_full() || self.adopt_wasm(m, handle).is_none() {
                wasm::close(handle);
            }
        }
    }

    /// The manager handle of a WASM window.
    pub(crate) fn wasm_handle(&self, id: WindowId) -> Option<wasm::AppId> {
        match &self.wm.get(id)?.app.app {
            App::Wasm(w) if w.id != 0 => Some(w.id),
            _ => None,
        }
    }

    /// Once per frame: tell the manager each window's content size and visibility,
    /// adopt guest-set titles, free finished instances and mirror the clipboard.
    pub fn wasm_sync(&mut self) {
        wasm::reap();
        self.adopt_native_windows();
        self.poll_fs_changes();
        let mut titles: Vec<(WindowId, String)> = Vec::new();
        for w in self.wm.windows() {
            if let App::Wasm(ww) = &w.app.app {
                let c = wasm_content(w.rect);
                if let Some(t) = wasm::sync_window(ww.id, c.w, c.h, w.shown()) {
                    let mut t = t;
                    if w.app.index > 1 {
                        let mut tmp = [0u8; 16];
                        let k = numbered_name("", w.app.index, &mut tmp);
                        t.push_str(core::str::from_utf8(&tmp[..k]).unwrap_or(""));
                    }
                    if t != w.app.title {
                        titles.push((w.id, t));
                    }
                }
            }
        }
        for (id, t) in titles {
            if let Some(w) = self.wm.get_mut(id) {
                w.app.title = t;
            }
        }
        let g = wasm::clip_generation();
        if g != self.clip_gen {
            self.clip_gen = g;
            let mut b = [0u8; clipboard::CAP];
            let n = wasm::clip_get(&mut b);
            self.clipboard.set(&b[..n]);
        }
    }
}
