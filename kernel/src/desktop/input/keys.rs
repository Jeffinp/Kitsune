//! Keyboard routing: raw scancodes to keys, global shortcuts (Ctrl+C/V/S/N, Alt+Tab) and the hand-off to the focused window's app.

use crate::desktop::shell::Cmd;
use crate::desktop::*;

/// Keys the [`Keymap`] has no `Key` for; read from the raw scancode.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Special {
    F2,
    F3,
    F5,
    PageUp,
    PageDown,
    KpPlus,
    KpMinus,
}

/// The special key of a scancode, if it is one.
fn special_of(scan: u8, extended: bool) -> Option<Special> {
    match (scan, extended) {
        (0x3C, false) => Some(Special::F2),
        (0x3D, false) => Some(Special::F3),
        (0x3F, false) => Some(Special::F5),
        (0x49, true) => Some(Special::PageUp),
        (0x51, true) => Some(Special::PageDown),
        (0x4E, false) => Some(Special::KpPlus),
        (0x4A, false) => Some(Special::KpMinus),
        _ => None,
    }
}

impl Desktop {
    /// A special key for the focused file manager or viewer. `true` when consumed.
    fn handle_special(&mut self, sp: Special) -> bool {
        let Some(top) = self.focused() else {
            return false;
        };
        match self.kind_of(top) {
            Some(Kind::Files) => self.files_special(top, sp),
            Some(Kind::Viewer) => self.viewer_special(top, sp),
            Some(k @ (Kind::Terminal | Kind::Editor)) => self.text_special(top, k, sp),
            _ => false,
        }
    }

    /// PageUp, PageDown and F3 for a terminal (scrollback) or an editor.
    fn text_special(&mut self, id: WindowId, kind: Kind, sp: Special) -> bool {
        let code = match sp {
            Special::PageUp => kitsune_core::input::KeyCode::PageUp,
            Special::PageDown => kitsune_core::input::KeyCode::PageDown,
            Special::F3 => kitsune_core::input::KeyCode::F(3),
            _ => return false,
        };
        let ev = kitsune_core::input::KeyEvent::new(code, self.mods());
        if kind == Kind::Terminal {
            self.term_event(id, ev);
        } else {
            self.editor_event(id, ev);
        }
        true
    }

    pub fn handle_key(&mut self, scan: u8, extended: bool, pressed: bool, _time: Time) -> bool {
        self.last_input_ms = crate::netd::now_ms();
        let alt_before = self.keymap.alt();
        let login = self.login_active();
        if pressed
            && !alt_before
            && !login
            && let Some(sp) = special_of(scan, extended)
            && self.handle_special(sp)
        {
            return true;
        }
        let key = self.keymap.process(scan, extended, pressed);
        let alt_now = self.keymap.alt();

        // Alt released: commit the Alt+Tab selection (restoring a minimized
        // window), or just drop a stale switcher.
        if alt_before
            && !alt_now
            && let Some(sw) = self.switcher.take()
        {
            self.shell.switcher_glass.clear();
            self.wm.activate(sw.selected());
            self.force_full = true;
            return true;
        }
        let Some(key) = key else {
            return false;
        };
        // The sign-in / lock screen takes every key: no shortcut reaches what is behind it.
        if login {
            self.login_key(key);
            return true;
        }
        if !alt_now && self.switcher.take().is_some() {
            self.shell.switcher_glass.clear();
            self.force_full = true;
        }

        // System shortcuts that work everywhere.
        if self.keymap.ctrl() {
            match key {
                // Ctrl+Alt+H: the performance HUD. Ctrl+Alt+G: the component gallery.
                Key::Char(b'h' | b'H') if alt_now => {
                    self.shell.hud = !self.shell.hud;
                    self.force_full = true;
                    return true;
                }
                Key::Char(b'g' | b'G') if alt_now => {
                    self.execute(Cmd::Gallery);
                    return true;
                }
                // Ctrl+Alt+R: toggle the reference mode (every frame recomposed from scratch),
                // a debugging aid for tools/perf/scen/w27-oracle.sh.
                // Ctrl+Alt+V: verify mode (every frame is compared with a full redraw).
                Key::Char(b'v' | b'V') if alt_now => {
                    self.verify = !self.verify;
                    return true;
                }
                Key::Char(b'r' | b'R') if alt_now => {
                    self.reference = !self.reference;
                    self.force_full = true;
                    return true;
                }
                // Ctrl+Alt+Left / Right: the previous / next workspace; with Shift they carry the
                // focused window along.
                Key::Left | Key::Right if alt_now => {
                    let cur = self.wm.workspace();
                    let to = if key == Key::Left {
                        cur.saturating_sub(1)
                    } else {
                        (cur + 1).min(self.wm.visible_workspaces() - 1)
                    };
                    if self.keymap.shift() {
                        if let Some(id) = self.focused() {
                            self.move_window_to_workspace(id, to);
                        }
                    } else {
                        self.go_workspace(to);
                    }
                    return true;
                }
                // Ctrl+Alt+D: show the desktop (or bring the windows back).
                Key::Char(b'd' | b'D') if alt_now => {
                    self.execute(Cmd::ShowDesktop);
                    return true;
                }
                // Ctrl+Alt+L: lock the screen.
                Key::Char(b'l' | b'L') if alt_now => {
                    self.execute(Cmd::Lock);
                    return true;
                }
                // Ctrl+Space: Busca.
                Key::Char(b' ') if !alt_now => {
                    self.open_search();
                    return true;
                }
                _ => {}
            }
        }

        if alt_now {
            // Alt+Tab / Alt+Shift+Tab opens the switcher, repeats advance it.
            match key {
                Key::Tab => {
                    let back = self.keymap.shift();
                    match self.switcher.as_mut() {
                        Some(s) => s.advance(back),
                        None => {
                            self.close_transients();
                            self.shell.switcher_glass.clear();
                            self.switcher = Switcher::start(self.wm.switch_list(), back);
                        }
                    }
                    self.force_full = true;
                    return true;
                }
                Key::Esc if self.switcher.take().is_some() => {
                    self.shell.switcher_glass.clear();
                    self.force_full = true;
                    return true;
                }
                // Alt+arrows tile, maximise, restore or minimise the focused window (Alt+Shift+
                // arrows always do; plain Alt+Left / Alt+Right go back / forward in the browser).
                Key::Left | Key::Right | Key::Up | Key::Down => {
                    if self.switcher.is_some() {
                        return true;
                    }
                    use kitsune_core::snap::Arrow;
                    let shift = self.keymap.shift();
                    if let Some(f) = self.focused()
                        && !shift
                        && matches!(key, Key::Left | Key::Right)
                        && self.kind_of(f) == Some(Kind::Browser)
                        && let Some(b) = self.browser_state_mut(f)
                    {
                        if key == Key::Left {
                            b.browser.back();
                        } else {
                            b.browser.forward();
                        }
                        return true;
                    }
                    let arrow = match key {
                        Key::Left => Arrow::Left,
                        Key::Right => Arrow::Right,
                        Key::Up => Arrow::Up,
                        _ => Arrow::Down,
                    };
                    return self.snap_key(arrow);
                }
                // The editor's find bar uses Alt+A (replace all), Alt+R and Alt+C (case).
                Key::Char(_) if self.switcher.is_none() => {
                    if let Some(f) = self.focused()
                        && self.kind_of(f) == Some(Kind::Editor)
                    {
                        return self.editor_key(f, key);
                    }
                    return false;
                }
                // Other Alt+key chords belong to no app: do not type them.
                _ => return self.switcher.is_some(),
            }
        }

        // Menus, popovers, sheets, Apps and Busca own the keyboard while they are up.
        if self.shell_key(key) {
            return true;
        }
        // An ABNT2 accent followed by a letter it cannot combine with types both.
        let mut changed = self.dispatch_key(key);
        while let Some(k) = self.keymap.take_pending() {
            changed |= self.dispatch_key(k);
        }
        changed
    }

    /// Deliver one logical key to the focused app (after the Alt+Tab handling):
    /// Ctrl shortcuts first, then the app's own key handler.
    fn dispatch_key(&mut self, key: Key) -> bool {
        let (Some(top), Some(kind)) =
            (self.focused(), self.focused().and_then(|f| self.kind_of(f)))
        else {
            return false;
        };
        // Ctrl+C / Ctrl+V / Ctrl+S / Ctrl+N are intercepted before the app sees the key
        // (a WASM app gets every chord itself).
        if self.keymap.ctrl() && kind != Kind::WasmApp {
            // The file manager owns Ctrl+A/C/X/V/R (files, not text).
            if kind == Kind::Files
                && let Key::Char(ch) = key
                && ch != b'n'
                && ch != b'N'
                && ch != b's'
                && ch != b'S'
                && self.files_ctrl(top, ch)
            {
                return true;
            }
            // The editor does its own copy, paste and save (the text engine owns Ctrl+C/X/V/S);
            // in the terminal Ctrl+C interrupts, Ctrl+Shift+C copies the typed line.
            match key {
                Key::Char(b'c' | b'C')
                    if kind != Kind::Editor && (kind != Kind::Terminal || self.keymap.shift()) =>
                {
                    self.copy_from_focused();
                    return true;
                }
                Key::Char(b'v' | b'V') if kind != Kind::Editor => {
                    self.paste_into_focused();
                    return true;
                }
                Key::Char(b's' | b'S') if kind == Kind::Viewer => {
                    self.viewer_save_prompt(top);
                    return true;
                }
                // Ctrl+N: another window of the focused app (single-instance
                // apps only re-focus; the WASM guest and the task manager keep
                // the key).
                Key::Char(b'n') | Key::Char(b'N')
                    if !matches!(kind, Kind::WasmApp | Kind::TaskMgr | Kind::Gallery) =>
                {
                    if kind == Kind::Browser {
                        // One browser window: Ctrl+N opens a tab.
                        self.browser_key(top, Key::Char(b't'));
                    } else {
                        self.new_window(kind);
                    }
                    return true;
                }
                // Ctrl+W closes the window, Ctrl+M minimises it.
                Key::Char(b'w' | b'W') => {
                    if kind == Kind::Browser {
                        self.browser_close_active(top);
                    } else {
                        self.request_close(top);
                    }
                    return true;
                }
                Key::Char(b'm' | b'M') => {
                    self.wm.minimize(top);
                    return true;
                }
                _ => {}
            }
        }
        match kind {
            Kind::Terminal => {
                self.term_key(top, key);
            }
            Kind::Editor => {
                self.editor_key(top, key);
            }
            Kind::TaskMgr => self.tarefas_key(top, key),
            Kind::Calculator => match key {
                Key::Char(b) => self.calc_input(top, b),
                Key::Enter => self.calc_input(top, b'='),
                Key::Backspace => self.calc_input(top, 0x08),
                Key::Delete => self.calc_input(top, b'C'),
                Key::Esc => self.request_close(top),
                _ => {}
            },
            Kind::Browser => {
                self.browser_key(top, key);
                // A key that only changed the page area or the bar needs no new scene.
                if self.client_dirty == Some(top) && !self.force_full {
                    return false;
                }
                self.client_dirty = None;
            }
            // Forward keystrokes to the guest (printable bytes as-is, Enter as
            // LF, Esc as 0x1B so apps like DOOM get their menu key). A WASM app is
            // closed with the title-bar button, not Esc, so the guest keeps Esc.
            Kind::WasmApp => {
                if let Some(h) = self.wasm_handle(top) {
                    // Hand the app the current clipboard (it may read it with `clip_get`).
                    crate::wasm::clip_load(self.clipboard.get());
                    let mods = self.keymap.shift() as i32
                        | ((self.keymap.ctrl() as i32) << 1)
                        | ((self.keymap.alt() as i32) << 2);
                    crate::wasm::key(h, wasm_key_code(key), mods);
                }
            }
            Kind::Files => self.files_key(top, key),
            Kind::Settings => self.settings_key(top, key),
            Kind::LogViewer => self.log_key(top, key),
            Kind::Viewer => self.viewer_key(top, key),
            Kind::Gallery => self.gallery_key(top, key),
        }
        true
    }
}
