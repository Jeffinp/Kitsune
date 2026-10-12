//! Workspaces: the current one, how many are shown, switching and moving windows between them.

use super::*;

impl<A> WindowManager<A> {
    /// The workspace on screen (0-based).
    pub fn workspace(&self) -> u8 {
        self.cur_ws
    }

    /// How many workspaces the switcher shows: at least [`MIN_WORKSPACES`], one more than the
    /// last one that holds a window (an empty one to move to), the current one included, at
    /// most [`MAX_WORKSPACES`].
    pub fn visible_workspaces(&self) -> u8 {
        let last_used = self
            .wins
            .iter()
            .filter(|w| !w.is_closing())
            .map(|w| w.ws)
            .max()
            .map_or(0, |m| m + 1);
        (last_used + 1)
            .max(self.cur_ws + 1)
            .clamp(MIN_WORKSPACES, MAX_WORKSPACES)
    }

    /// How many live windows are on workspace `ws`.
    pub fn windows_on(&self, ws: u8) -> usize {
        self.wins
            .iter()
            .filter(|w| w.ws == ws && !w.is_closing())
            .count()
    }

    /// Shows workspace `to`: the windows of the one on screen slide away (toward the side `to`
    /// is on, in the opposite direction) and those of `to` slide in. `false` when `to` is out of
    /// range or already current.
    pub fn switch_workspace(&mut self, to: u8) -> bool {
        if to >= MAX_WORKSPACES || to == self.cur_ws {
            return false;
        }
        let dir: i8 = if to > self.cur_ws { 1 } else { -1 };
        for w in self.wins.iter_mut() {
            if w.is_closing() {
                continue;
            }
            if w.ws == to {
                let was_hidden = w.off_ws || w.leaving == Leaving::Workspace;
                w.off_ws = false;
                if w.leaving == Leaving::Workspace {
                    w.leaving = Leaving::Destroy;
                }
                if !w.minimized && was_hidden {
                    w.anim = Some(Anim::slide_in(dir));
                }
            } else if w.ws == self.cur_ws && !w.minimized && !w.off_ws {
                w.leaving = Leaving::Workspace;
                w.anim = Some(Anim::slide_out(dir));
            } else if w.ws != to {
                w.off_ws = true;
            }
        }
        self.cur_ws = to;
        true
    }

    /// Moves window `id` to workspace `to`. When that is not the one on screen the window slides
    /// away (toward the side `to` is on). `false` for an unknown window or an out-of-range `to`.
    pub fn move_to_workspace(&mut self, id: WindowId, to: u8) -> bool {
        if to >= MAX_WORKSPACES {
            return false;
        }
        let cur = self.cur_ws;
        let Some(w) = self.get_mut(id) else {
            return false;
        };
        if w.ws == to {
            return true;
        }
        w.ws = to;
        if to != cur && !w.minimized && !w.off_ws && !w.is_closing() {
            let dir: i8 = if to > cur { 1 } else { -1 };
            w.leaving = Leaving::Workspace;
            w.anim = Some(Anim::slide_out(-dir));
        } else if to != cur {
            w.off_ws = true;
        }
        true
    }
}
