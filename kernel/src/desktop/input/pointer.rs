//! The pointer: every mouse packet and the cursor shape.

use crate::desktop::*;

impl Desktop {
    /// Is the pointer over something clickable in a browser window (a link, a button, a tab, a
    /// suggestion)? Then the cursor is drawn as a hand.
    pub(crate) fn cursor_is_hand(&self) -> bool {
        if self.drag.is_some() || self.overlay_open() {
            return false;
        }
        let (cx, cy) = (self.cursor_x, self.cursor_y);
        let Some(w) = self.topmost_at(cx, cy) else {
            return false;
        };
        let Some(win) = self.wm.get(w) else {
            return false;
        };
        self.browser_cursor_hand(win, cx, cy)
    }

    pub fn handle_mouse(&mut self, dx: i32, dy: i32, left: bool, right: bool) -> MouseResult {
        if dx != 0 || dy != 0 || left != self.prev_left || right != self.prev_right {
            self.last_input_ms = crate::netd::now_ms();
        }
        self.cursor_x = (self.cursor_x + dx).clamp(0, self.sw - 1);
        self.cursor_y = (self.cursor_y - dy).clamp(0, self.sh - 1);
        let (cx, cy) = (self.cursor_x, self.cursor_y);
        let cursor_moved = dx != 0 || dy != 0;
        let mut scene = false;

        let left_pressed = left && !self.prev_left;
        let right_pressed = right && !self.prev_right;
        let released = !left && self.prev_left;

        // The sign-in / lock screen takes the pointer: nothing behind it is reachable.
        if self.login_active() {
            if left_pressed {
                self.login_click(cx, cy);
            }
            self.prev_left = left;
            self.prev_right = right;
            return MouseResult {
                scene_dirty: left_pressed,
                cursor_moved,
            };
        }

        // Hover of the shell layers (panel, menus, Apps, Busca) and the app bar.
        if cursor_moved {
            if self.shell_pointer(cx, cy) {
                scene = true;
            }
            self.dock_pointer(cx, cy);
        }

        if right_pressed {
            scene |= self.mouse_right_press(cx, cy);
        }

        if left_pressed {
            scene |= self.mouse_left_press(cx, cy);
        }

        if released {
            scene |= self.mouse_release(cx, cy);
        }

        self.mouse_drag(left, cx, cy);

        // The system apps repaint when what the pointer is over (or the button state) changes.
        if self.drag.is_none()
            && (cursor_moved || left != self.prev_left)
            && self.live_hover(cx, cy, left)
        {
            scene = true;
        }

        scene |= self.mouse_hover(cx, cy, cursor_moved);

        // A link under the pointer is underlined.
        if cursor_moved
            && self.browser_hover_update(cx, cy)
            && let Some(id) = self.browser_id()
        {
            self.client_dirty = Some(id);
        }
        // A page that listens to the mouse gets the move.
        if cursor_moved {
            self.browser_js_pointer(cx, cy);
        }

        // Moving over an open menu / Apps launcher updates the hover highlight,
        // but it is NOT a full-scene change: the compositor repaints only the
        // overlay's rectangle on `cursor_moved` (see the overlay path in the
        // main loop), so we deliberately do not set `scene` here.

        self.wasm_pointer(left, right, cursor_moved);
        self.prev_left = left;
        self.prev_right = right;
        MouseResult {
            scene_dirty: scene,
            cursor_moved,
        }
    }
}
