//! Window geometry: minimize, maximize, snap, restore, move and resize.

use super::*;

impl<A> WindowManager<A> {
    /// Starts minimizing (fade out, then hidden). `false` if the window is not
    /// active.
    pub fn minimize(&mut self, id: WindowId) -> bool {
        let Some(w) = self.get_mut(id) else {
            return false;
        };
        if !w.active() {
            return false;
        }
        w.leaving = Leaving::Minimize;
        w.anim = Some(Anim::minimize());
        true
    }

    /// Maximizes to `work`, remembering the current rect. No-op (`false`) for
    /// a non-resizable or already maximized window.
    pub fn maximize(&mut self, id: WindowId, work: Rect) -> bool {
        self.snap_to(id, SnapZone::Maximize, work)
    }

    /// Puts the window in `zone` of `work` (a half, a quarter or the whole area), remembering the
    /// free rectangle it came from (an already tiled or maximised window keeps the one it had).
    /// The move animates like a maximise. `false` for a non-resizable window or when it is
    /// already there.
    pub fn snap_to(&mut self, id: WindowId, zone: SnapZone, work: Rect) -> bool {
        let Some(w) = self.get_mut(id) else {
            return false;
        };
        if !w.resizable || w.snap_state() == Some(zone) {
            return false;
        }
        let target = snap::zone_rect_min(zone, work, w.min_w, w.min_h);
        if w.maximized || w.snap.is_some() {
            // Keep the original free rectangle.
        } else {
            w.restore = w.rect;
        }
        let from = w.visual_rect();
        w.rect = target;
        w.maximized = zone == SnapZone::Maximize;
        w.snap = (zone != SnapZone::Maximize).then_some(zone);
        w.start_zoom(from);
        true
    }

    /// Leaves the maximized or tiled state, returning to the remembered rect.
    pub fn unmaximize(&mut self, id: WindowId) -> bool {
        let Some(w) = self.get_mut(id) else {
            return false;
        };
        if !w.maximized && w.snap.is_none() {
            return false;
        }
        let from = w.visual_rect();
        w.rect = w.restore;
        w.maximized = false;
        w.snap = None;
        w.start_zoom(from);
        true
    }

    /// Maximize if free, restore if maximized or tiled.
    pub fn toggle_maximize(&mut self, id: WindowId, work: Rect) -> bool {
        match self.get(id).map(|w| w.snap_state().is_some()) {
            Some(true) => self.unmaximize(id),
            Some(false) => self.maximize(id, work),
            None => false,
        }
    }

    /// A drag grabbed the title of a maximised or tiled window at `(px, py)`: restore it
    /// *without animation* to the size it had, put under the pointer so the grab keeps its
    /// proportional place on the bar, and return the new grab offset `(dx, dy)` for the move that
    /// follows. `None` when the window is free (nothing to restore).
    pub fn restore_for_drag(&mut self, id: WindowId, px: i32, py: i32) -> Option<(i32, i32)> {
        let w = self.get_mut(id)?;
        if !w.maximized && w.snap.is_none() {
            return None;
        }
        let cur = w.rect;
        let size = w.restore;
        let frac = ((px - cur.x).clamp(0, cur.w.max(1)) * 256) / cur.w.max(1);
        let dx = (size.w * frac / 256).clamp(0, size.w);
        let dy = (py - cur.y).clamp(0, crate::windowing::window::TITLE_H - 1);
        w.rect = Rect::new(px - dx, py - dy, size.w, size.h);
        w.maximized = false;
        w.snap = None;
        w.zoom = None;
        Some((dx, dy))
    }

    // ------------------------------------------------------ move / resize

    /// Moves a window's top-left to `(x, y)`, keeping its title bar on the
    /// `sw x sh` screen. Maximized windows do not move.
    pub fn move_to(&mut self, id: WindowId, x: i32, y: i32, sw: i32, sh: i32) -> bool {
        let Some(w) = self.get_mut(id) else {
            return false;
        };
        if w.maximized {
            return false;
        }
        let (cx, cy) = Rect::new(x, y, w.rect.w, w.rect.h).clamped_pos(sw, sh);
        w.snap = None;
        w.rect.x = cx;
        w.rect.y = cy;
        true
    }

    /// Resizes a window by dragging `edge` of `start` by `(dx, dy)` (use the
    /// rect captured when the drag began, so the result does not drift).
    pub fn resize(
        &mut self,
        id: WindowId,
        edge: ResizeEdge,
        start: Rect,
        delta: (i32, i32),
        screen: (i32, i32),
    ) -> bool {
        let Some(w) = self.get_mut(id) else {
            return false;
        };
        if !w.resizable || w.maximized {
            return false;
        }
        w.rect = start.resized(edge, delta, (w.min_w, w.min_h), screen);
        w.snap = None;
        true
    }
}
