//! Opening, closing and stepping windows: the open animation, deferred removal of closing windows.

use super::*;

impl<A> WindowManager<A> {
    /// Opens a window on top (opening animation, focused). Gives the app back
    /// when the table is full.
    pub fn open(&mut self, spec: WindowSpec, app: A) -> Result<WindowId, A> {
        if self.is_full() {
            return Err(app);
        }
        let id = WindowId::from_raw(self.next_id);
        self.next_id += 1;
        self.wins.push(Window {
            id,
            rect: spec.rect,
            restore: spec.rect,
            minimized: false,
            maximized: false,
            snap: None,
            anim: Some(Anim::open()),
            zoom: None,
            leaving: Leaving::Destroy,
            min_w: spec.min_w,
            min_h: spec.min_h,
            resizable: spec.resizable,
            ws: self.cur_ws,
            off_ws: false,
            app,
        });
        self.mru.insert(0, id);
        Ok(id)
    }

    /// Starts the closing animation. `false` if the window is unknown or is
    /// already being destroyed. Closing a minimizing window turns the
    /// minimize into a close.
    pub fn request_close(&mut self, id: WindowId) -> bool {
        let Some(w) = self.get_mut(id) else {
            return false;
        };
        if w.is_closing() {
            return false;
        }
        // A minimized window stays hidden: its closing animation runs unseen
        // and the next `step` destroys it.
        w.leaving = Leaving::Destroy;
        w.anim = Some(Anim::close());
        true
    }

    /// Removes a window immediately (no animation) and returns it.
    pub fn remove(&mut self, id: WindowId) -> Option<Window<A>> {
        let i = self.z_index(id)?;
        self.mru.retain(|&m| m != id);
        Some(self.wins.remove(i))
    }

    /// Advances every animation by `dt`. Returns whether any is still running
    /// and the windows whose closing animation finished (already removed from
    /// the table: the caller releases their resources). A finished minimize
    /// hides the window instead.
    pub fn step(&mut self, dt: f32) -> (bool, Vec<Window<A>>) {
        let mut active = false;
        let mut gone: Vec<WindowId> = Vec::new();
        for w in self.wins.iter_mut() {
            if let Some(z) = w.zoom.as_mut() {
                if z.step(dt) {
                    active = true;
                } else {
                    w.zoom = None;
                }
            }
            let Some(a) = w.anim.as_mut() else { continue };
            a.step(dt);
            if !a.finished() {
                active = true;
                continue;
            }
            let closing = a.is_closing();
            w.anim = None;
            if closing {
                match w.leaving {
                    Leaving::Destroy => gone.push(w.id),
                    Leaving::Minimize => {
                        w.minimized = true;
                        w.leaving = Leaving::Destroy;
                    }
                    Leaving::Workspace => {
                        w.off_ws = true;
                        w.leaving = Leaving::Destroy;
                    }
                }
            }
        }
        let mut removed = Vec::new();
        for id in gone {
            if let Some(w) = self.remove(id) {
                removed.push(w);
            }
        }
        (active, removed)
    }
}
