//! Per-frame work of the desktop: stepping every animation, the scheduler quantum, the clock
//! and date, and the dirty-region bookkeeping the compositor consumes.

use crate::desktop::*;

impl Desktop {
    /// Advance all running animations by `dt`. Returns `true` while any window
    /// is still animating (the caller keeps rendering).
    pub fn animate(&mut self, dt: f32) -> bool {
        self.auto_lock_tick();
        crate::userproc::reap();
        self.step_file_jobs();
        let files_busy = self.step_files(dt) | self.step_viewers(dt) | self.step_editors(dt);
        self.step_shell_jobs();
        self.sync_text_windows();
        self.live_step(dt);
        let (active, gone) = self.wm.step(dt);
        let focus_busy = self.step_focus(dt);
        let shell_busy = self.step_shell(dt);
        let browser_busy = self.step_browser(dt);
        let active = active || focus_busy || shell_busy || files_busy || browser_busy;
        if !active {
            // Nothing animates any more: the next animation re-captures its window.
            self.tex_key.set(None);
        }
        for mut w in gone {
            if let App::Files(f) = &mut w.app.app
                && let Some(mut job) = f.job.take()
            {
                vfs::copy_abort(&mut job.copy);
            }
            // Closing an app terminates its process (removed from the table),
            // matching how a desktop app behaves; dropping `w` frees its state.
            self.procs.kill(w.app.pid);
            // The window is gone: the app's runtime (memory, descriptors) goes with it.
            if let App::Wasm(ww) = &w.app.app {
                crate::wasm::close(ww.id);
                if self.wasm_grab == Some(w.id) {
                    self.wasm_grab = None;
                }
            }
            if self.hover == Some(w.id) {
                self.hover = None;
            }
            if self.title_hover.is_some_and(|(id, _)| id == w.id) {
                self.title_hover = None;
            }
            if self.drag.as_ref().is_some_and(|d| d.win == w.id) {
                self.drag = None;
            }
            if w.minimized {
                // A hidden window vanished: its dock indicator must go.
                self.force_full = true;
            }
        }
        active
    }

    /// One scheduler quantum: advance CPU-time of running processes.
    pub fn tick_processes(&mut self) {
        self.procs.tick();
        self.refresh_logs();
        self.refresh_date();
        self.refresh_notifs();
    }

    /// Read the local date once a second (the panel clock and the calendar).
    fn refresh_date(&mut self) {
        let hour = crate::rtc::now().h;
        if hour != self.shell.last_hour {
            self.poll_appearance(hour);
        }
        let local =
            kitsune_core::hw::rtc::utc_to_local(crate::rtc::read_utc(), crate::rtc::tz_minutes());
        if local.is_valid() {
            self.today
                .set((local.date.y as i32, local.date.m, local.date.d));
            self.weekday.set(local.weekday());
        }
    }

    /// On-screen rect of window `w` right now: its resting rectangle, the in-flight
    /// rectangle of a zoom, or the scaled/moved one of an open / close / minimise.
    pub(crate) fn window_box(&self, w: &Win) -> Rect {
        if let Some(a) = w.anim {
            return a.frame(w.rect, self.dock_target(w)).rect;
        }
        w.visual_rect()
    }

    /// While any window is opening, closing, being dragged, or is a live WASM app (and more: see
    /// below) the compositor runs its per-frame damage path so the app gets continuous frames, rather
    /// than the steady (repaint-on-change) one.
    ///
    /// What kind of frames the screen needs now: [`Animation::Motion`] when something moves, glides,
    /// fades or runs live (frames at the full animation rate), [`Animation::Ambient`] when the only
    /// thing changing is a caret's blink (a few frames a second look the same and cost a quarter).
    pub fn animation(&self) -> Animation {
        let focused = self.focused();
        let motion = self.drag.is_some()
            || self.shell_animating()
            || self.toasts_sliding()
            || self.live_busy() || self.wm.windows().iter().any(|w| {
            w.shown()
                && (w.anim.is_some()
                    || w.zoom.is_some()
                    || self.focus_busy(w.id)
                    || w.app.kind() == Kind::WasmApp
                    || matches!(&w.app.app, App::Files(f) if f.animating())
                    || matches!(&w.app.app, App::Viewer(v) if v.animating())
                    || matches!(&w.app.app, App::Editor(e) if e.moving())
                    || matches!(&w.app.app, App::Terminal(t) if t.term.is_running() || t.moving())
                    || self.browser_busy(w))
        });
        if motion {
            return Animation::Motion;
        }
        let blinking = self.wm.windows().iter().any(|w| {
            w.shown()
                && match &w.app.app {
                    App::Editor(e) => e.blinking(focused == Some(w.id)),
                    App::Terminal(t) => t.blinking(focused == Some(w.id)),
                    _ => false,
                }
        });
        if blinking {
            Animation::Ambient
        } else {
            Animation::None
        }
    }

    /// Is the verify mode (Ctrl+Alt+V, see the `verify` field) on?
    pub fn verify_mode(&self) -> bool {
        self.verify
    }

    /// Is the reference mode (Ctrl+Alt+R, see the `reference` field) on?
    pub fn reference_mode(&self) -> bool {
        self.reference
    }

    /// Consume the "only this window's client area changed" note.
    pub fn take_client_dirty(&mut self) -> Option<WindowId> {
        self.client_dirty.take()
    }

    /// Consume the "repaint everything" request (maximize, restore, ...).
    pub fn take_full_repaint(&mut self) -> bool {
        core::mem::take(&mut self.force_full)
    }

    /// Consume the extra region the next steady frame must upload.
    pub fn take_extra_dirty(&mut self) -> Option<Rect> {
        let r = core::mem::replace(&mut self.extra_dirty, Rect::new(0, 0, 0, 0));
        (!r.is_empty()).then_some(r)
    }

    /// Add `r` to the extra upload region.
    pub(crate) fn mark_dirty(&mut self, r: Rect) {
        self.extra_dirty = self.extra_dirty.union(&r);
    }
}

/// How much the screen is changing on its own (see [`Desktop::animation`]).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Animation {
    None,
    /// Only a caret blinks.
    Ambient,
    /// Something moves, fades, glides or runs live.
    Motion,
}
