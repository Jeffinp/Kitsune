//! The desktop described as a compositor scene: which layers exist, where they can draw, what is
//! opaque and what their look is. Nothing here paints and nothing here says what to repaint: the
//! engine (`kitsune_core::compositor`) diffs this description with the previous frame's.
//!
//! Bottom to top: wallpaper (implicit), windows in stacking order, the app bar, the panel, the
//! snap preview, then the shell layers (Apps, Busca, menu, popover, Alt+Tab, sheet). The cursor,
//! the toasts and the performance HUD are not layers: they live only in the framebuffer (see
//! `present.rs`).
//!
//! # Adding a window type or an overlay
//! A new kind of window needs nothing here: it is a window. A new overlay is a [`LayerId`]
//! constant, one entry in [`Slot`], a footprint (everything it can draw, shadow included) and a
//! look that changes whenever its pixels do; then a branch in `paint.rs`.

use super::super::*;
use kitsune_core::compositor::{Layer, LayerId, Look, Scene};
use kitsune_core::style::R_WINDOW;

pub(super) const PANEL: LayerId = LayerId(0xF000_0001);
pub(super) const DOCK: LayerId = LayerId(0xF000_0002);
pub(super) const SNAP: LayerId = LayerId(0xF000_0003);
pub(super) const APPS: LayerId = LayerId(0xF000_0004);
pub(super) const SEARCH: LayerId = LayerId(0xF000_0005);
pub(super) const MENU: LayerId = LayerId(0xF000_0006);
pub(super) const POPOVER: LayerId = LayerId(0xF000_0007);
pub(super) const SWITCHER: LayerId = LayerId(0xF000_0008);
pub(super) const DIALOG: LayerId = LayerId(0xF000_0009);
pub(super) const LOGIN: LayerId = LayerId(0xF000_000A);

/// What a [`LayerId`] stands for.
pub(super) enum Slot {
    Wallpaper,
    Window(WindowId),
    Panel,
    Dock,
    Snap,
    Apps,
    Search,
    Menu,
    Popover,
    Switcher,
    Dialog,
    Login,
}

pub(super) fn slot_of(id: LayerId) -> Slot {
    match id {
        LayerId::WALLPAPER => Slot::Wallpaper,
        PANEL => Slot::Panel,
        DOCK => Slot::Dock,
        SNAP => Slot::Snap,
        APPS => Slot::Apps,
        SEARCH => Slot::Search,
        MENU => Slot::Menu,
        POPOVER => Slot::Popover,
        SWITCHER => Slot::Switcher,
        DIALOG => Slot::Dialog,
        LOGIN => Slot::Login,
        other => Slot::Window(WindowId::from_raw(other.0)),
    }
}

/// Version counters of the chrome: they advance when something that is not a window may have
/// changed (an input event, an overlay animating). Windows do not use them.
#[derive(Clone, Copy, Default)]
pub(super) struct Epochs {
    /// The panel and the app bar change on input and when an animation settles.
    pub chrome: u64,
    /// The shell layers also follow the pointer (hover highlights) and their own animations.
    pub overlay: u64,
}

/// The scene and what the compositor needs to know about how it was built.
pub(super) struct Built {
    pub scene: Scene,
    /// Windows that are repainted every frame right now (animating, running, live).
    pub dynamic: Vec<WindowId>,
    /// The only thing changing is the inside of live windows (Tarefas gliding): such a glide
    /// needs a frame every 20 ms, not every timer tick.
    pub live_only: bool,
}

const NOTHING: Rect = Rect::new(0, 0, 0, 0);

impl Desktop {
    /// Footprint of a window: its rectangle and, unless it is maximised and still, its shadow.
    fn window_footprint(&self, w: &Win) -> (Rect, Rect) {
        let rect = self.window_box(w);
        let still_max = w.maximized && w.zoom.is_none() && w.anim.is_none();
        (rect, if still_max { rect } else { shadow_box(rect) })
    }

    /// Why a window must be repainted every frame: `Some(rect)` is the part that changes (the
    /// whole footprint, or a live window's chart); `None` when it is still.
    fn window_activity(&self, w: &Win, footprint: Rect) -> Option<(Rect, bool)> {
        // The shape or the shadow changes (it moves, fades, zooms, is dragged, changes focus):
        // everything, shadow included.
        let moving = self.drag.as_ref().is_some_and(|d| {
            d.win == w.id
                && matches!(
                    d.mode,
                    DragMode::Move { .. } | DragMode::Unsnap { .. } | DragMode::Resize { .. }
                )
        });
        let shape_busy = w.anim.is_some() || w.zoom.is_some() || moving || self.focus_busy(w.id);
        if shape_busy {
            return Some((footprint, false));
        }
        // Only the content changes (a game, a running command, a copy): the shadow outside the
        // rectangle stays as it is, so it is not repainted.
        // A game redraws only its content area, well inside the window's opaque body, so nothing
        // under the window is touched (and none of the windows below are painted at all).
        let box_ = self.window_box(w);
        if w.app.kind() == Kind::WasmApp {
            let area = wasm_content(box_).inflated(2);
            return Some((area.intersection(&box_).unwrap_or(box_), false));
        }
        // A browser that scrolls, loads or animates a tab changes only the area below the title
        // bar (the page, the tab strip, the progress bar); the title bar, the shadow and
        // everything around stay as they are.
        if self.browser_busy(w) {
            return Some((Self::client_rect(box_), false));
        }
        // Content that animates by itself (a caret, a copy, a running command, a viewer's
        // transition): the window's rectangle; the shadow outside it is untouched.
        let focused = self.focused() == Some(w.id);
        // A terminal whose only motion is the caret's blink repaints just the caret's cell.
        if let App::Terminal(t) = &w.app.app
            && !t.term.is_running()
            && !t.moving()
            && t.blinking(focused)
            && self.drag.as_ref().is_none_or(|d| d.win != w.id)
            && let Some(caret) = self.terminal_caret_rect(box_, t)
            && let Some(area) = caret.intersection(&box_)
        {
            return Some((area, false));
        }
        // The same for an editor: only the bar of the caret changes.
        if let App::Editor(e) = &w.app.app
            && !e.moving()
            && e.blinking(focused)
            && self.drag.as_ref().is_none_or(|d| d.win != w.id)
            && let Some(caret) = self.editor_caret_rect(box_, e)
            && let Some(area) = caret.intersection(&box_)
        {
            return Some((area, false));
        }
        let content_busy = match &w.app.app {
            App::Files(f) => f.animating(),
            App::Viewer(v) => v.animating(),
            App::Editor(e) => e.animating(focused),
            App::Terminal(t) => t.term.is_running() || t.animating(focused),
            _ => false,
        } || self.drag.as_ref().is_some_and(|d| d.win == w.id);
        if content_busy {
            return Some((box_, false));
        }
        if self.live_dynamic(w) {
            // The chart lies inside the window's opaque body: clipped to it, nothing under the
            // window (not even behind its rounded corners) has to be painted again.
            let b = self.window_box(w);
            let body = Rect::new(b.x, b.y + R_WINDOW, b.w, (b.h - 2 * R_WINDOW).max(0));
            return match self.live_rect(w).and_then(|r| r.intersection(&body)) {
                Some(r) => Some((r, true)),
                None => Some((self.window_box(w), false)),
            };
        }
        None
    }

    fn window_layer(&self, w: &Win, dynamic: &mut Vec<WindowId>, live_only: &mut bool) -> Layer {
        let (rect, footprint) = self.window_footprint(w);
        let moving = w.anim.is_some() || w.zoom.is_some();
        let opaque = if moving {
            NOTHING
        } else if w.maximized {
            rect
        } else {
            // Only the rows of the rounded corners are not opaque: the band between them is,
            // all the way to the left and right edges.
            Rect::new(
                rect.x,
                rect.y + R_WINDOW,
                rect.w,
                (rect.h - 2 * R_WINDOW).max(0),
            )
        };
        let focused = self.focused() == Some(w.id);
        let hover_btn = self
            .title_hover
            .filter(|(id, _)| *id == w.id)
            .map_or(0, |(_, b)| b as u64 + 1);
        let look = Look::new()
            .u(self.focus_mix(w.id, focused) as u64)
            .b(focused)
            .u(hover_btn)
            .b(self.hover == Some(w.id))
            .b(self.drag.as_ref().is_some_and(|d| d.win == w.id))
            .b(w.maximized)
            .b(w.snap.is_some())
            .b(w.resizable);
        let dirty = match self.window_activity(w, footprint) {
            Some((r, live)) => {
                dynamic.push(w.id);
                *live_only &= live;
                Some(r)
            }
            None => None,
        };
        Layer::new(LayerId(w.id.raw()), footprint)
            .with_opaque(opaque)
            .with_look(look.get())
            .with_dirty(dirty)
    }

    /// The whole scene for this frame.
    pub(super) fn build_scene(&self, ep: Epochs) -> Built {
        let mut scene = Scene::new();
        let mut dynamic = Vec::new();
        let mut live_only = true;
        for w in self.wm.windows().iter().filter(|w| w.shown()) {
            scene.push(self.window_layer(w, &mut dynamic, &mut live_only));
        }
        let sh = &self.shell;
        let dock_busy = self.dock_animating();
        let zone = self.dock_paint_zone();
        let mut dock = Layer::new(DOCK, zone).with_look(ep.chrome);
        if dock_busy {
            dock = dock.with_dirty(Some(zone));
        }
        scene.push(dock);
        let panel = self.panel_rect();
        scene.push(
            Layer::new(PANEL, panel)
                .with_opaque(panel)
                .with_look(ep.chrome),
        );
        if let Some(r) = self.snap_preview_rect() {
            let t = sh.snap.as_ref().map_or(0, |p| p.t.value().to_bits() as u64);
            scene.push(Layer::new(SNAP, r.inflated(3)).with_look(Look::new().u(t).get()));
        }
        let full = Rect::new(0, 0, self.sw, self.sh);
        if sh.apps.is_some() {
            let hover = sh.dirty.get();
            let mut l = Layer::new(APPS, full).with_look(ep.chrome);
            if !hover.is_empty() {
                l = l.with_dirty(Some(hover));
            }
            scene.push(l);
        }
        if let Some(s) = &sh.search {
            let g = kitsune_core::chrome::spotlight_geom(self.sw, self.sh, s.hits.len());
            scene.push(Layer::new(SEARCH, g.panel.inflated(40)).with_look(ep.overlay));
        }
        if let Some(m) = &sh.menu {
            scene.push(Layer::new(MENU, m.geom.rect.inflated(40)).with_look(ep.overlay));
        }
        if let Some(p) = &sh.pop {
            scene.push(Layer::new(POPOVER, p.rect.inflated(40)).with_look(ep.overlay));
        }
        if let Some(sw) = &self.switcher {
            let r = self.switcher_rect(sw.list().len()).inflated(40);
            scene.push(Layer::new(SWITCHER, r).with_look(ep.overlay));
        }
        if sh.dialog.is_some() {
            scene.push(Layer::new(DIALOG, full).with_look(ep.overlay));
        }
        if let Some(l) = &sh.login {
            // Opaque and over everything: nothing below is painted while it is up.
            let look = Look::new()
                .u(super::super::shell::level(&l.t) as u64)
                .u(l.sel as u64)
                .u(l.pw.len() as u64)
                .u(l.msg.as_ref().map_or(0, |m| m.0.len() as u64 + 1))
                .b(l.msg.as_ref().is_some_and(|m| m.1))
                .u(l.users.len() as u64)
                .u(ep.overlay);
            scene.push(
                Layer::new(LOGIN, full)
                    .with_opaque(full)
                    .with_look(look.get()),
            );
        }
        let live_only = live_only
            && !dynamic.is_empty()
            && !dock_busy
            && !self.overlay_open()
            && !self.shell_animating();
        Built {
            scene,
            dynamic,
            live_only,
        }
    }
}
