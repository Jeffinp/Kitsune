//! Focus and z-order: raising, activating, hit-testing and the Alt+Tab list.

use super::*;

impl<A> WindowManager<A> {
    /// The focused window: the topmost one that takes focus.
    pub fn focused(&self) -> Option<WindowId> {
        self.wins.iter().rev().find(|w| w.active()).map(|w| w.id)
    }

    /// Moves `id` to the front and marks it most recently used.
    pub fn raise(&mut self, id: WindowId) -> bool {
        let Some(i) = self.z_index(id) else {
            return false;
        };
        self.wins[i..].rotate_left(1);
        self.touch(id);
        true
    }

    pub(super) fn touch(&mut self, id: WindowId) {
        self.mru.retain(|&m| m != id);
        self.mru.insert(0, id);
    }

    /// Brings a window to the user: un-minimizes it (with the opening
    /// animation), cancels a pending close or minimize, then raises it.
    pub fn activate(&mut self, id: WindowId) -> bool {
        // A window of another workspace brings its workspace with it.
        if let Some(ws) = self.get(id).map(|w| w.ws).filter(|&ws| ws != self.cur_ws) {
            self.switch_workspace(ws);
        }
        let Some(w) = self.get_mut(id) else {
            return false;
        };
        if w.minimized {
            w.minimized = false;
            w.anim = Some(Anim::restore());
        } else if w.is_leaving() {
            w.anim = Some(Anim::open());
        }
        w.leaving = Leaving::Destroy;
        self.raise(id)
    }

    /// The frontmost active window whose rect contains `(px, py)`.
    pub fn topmost_at(&self, px: i32, py: i32) -> Option<WindowId> {
        self.wins
            .iter()
            .rev()
            .find(|w| w.active() && w.rect.contains(px, py))
            .map(|w| w.id)
    }

    /// Windows in focus-cycle order (Alt+Tab): the focused window first, then
    /// the rest by most recent use. Windows being destroyed are skipped;
    /// minimized ones are included.
    pub fn switch_list(&self) -> Vec<WindowId> {
        let mut out = Vec::with_capacity(self.wins.len());
        let ok = |id: WindowId| self.get(id).is_some_and(|w| !w.is_closing());
        if let Some(f) = self.focused() {
            out.push(f);
        }
        for &id in &self.mru {
            if ok(id) && !out.contains(&id) {
                out.push(id);
            }
        }
        out
    }
}
