//! A left press on a window: title-bar buttons, resize border, title or app content.

use crate::desktop::*;
use kitsune_core::window::TitleBtn;

impl Desktop {
    /// A left press on window `w` at `(cx, cy)`: title-bar buttons, resize
    /// border, title (drag / double-click maximize) or app content.
    pub(super) fn click_window(&mut self, w: WindowId, cx: i32, cy: i32) {
        let Some(win) = self.wm.get(w) else {
            return;
        };
        let (rect, resizable, maximized) = (win.rect, win.resizable, win.maximized);
        let tiled = win.snap_state().is_some();
        let kind = win.app.kind();
        match rect.title_button_at(resizable, true, cx, cy) {
            Some(TitleBtn::Close) => {
                self.request_close(w);
                self.drag = None;
                return;
            }
            Some(TitleBtn::Minimize) => {
                self.wm.minimize(w);
                self.drag = None;
                self.clicks.reset();
                return;
            }
            Some(TitleBtn::Maximize) => {
                self.toggle_maximize(w);
                self.clicks.reset();
                return;
            }
            Some(TitleBtn::Menu) => {
                self.wm.raise(w);
                self.clicks.reset();
                self.open_window_menu(w);
                return;
            }
            None => {}
        }
        self.wm.raise(w);
        if resizable
            && !maximized
            && let Some(edge) = rect.resize_edge_at(cx, cy)
        {
            self.drag = Some(Drag {
                win: w,
                mode: DragMode::Resize {
                    edge,
                    start: rect,
                    ox: cx,
                    oy: cy,
                },
            });
            return;
        }
        if cy >= rect.y && cy < rect.y + TITLE_H {
            let double = self.clicks.press(crate::interrupts::ticks(), cx, cy, w);
            if double && resizable {
                self.toggle_maximize(w);
            } else if maximized || tiled {
                // Dragging a maximised or tiled window frees it once the pointer has moved.
                self.drag = Some(Drag {
                    win: w,
                    mode: DragMode::Unsnap { ox: cx, oy: cy },
                });
            } else {
                self.drag = Some(Drag {
                    win: w,
                    mode: DragMode::Move {
                        grab_dx: cx - rect.x,
                        grab_dy: cy - rect.y,
                    },
                });
            }
            return;
        }
        match kind {
            Kind::Calculator => self.calc_click(w, rect, cx, cy),
            Kind::Browser => {
                let double = self.clicks.press(crate::interrupts::ticks(), cx, cy, w);
                let select = self.browser_click(w, rect, cx, cy);
                if double {
                    self.browser_js_mouse(w, cx, cy, kitsune_core::web::script::MouseKind::Double);
                }
                if select {
                    if double {
                        let content = self
                            .browser_state_mut(w)
                            .map(|b| b.chrome(rect).content)
                            .unwrap_or(rect);
                        self.browser_double_click(w, content, cx, cy);
                    } else {
                        self.drag = Some(Drag {
                            win: w,
                            mode: DragMode::PageSelect,
                        });
                    }
                }
            }
            Kind::WasmApp => {
                // The press is delivered by `wasm_pointer` (content-local coordinates);
                // the window grabs the button so drags and the release reach it.
                let c = wasm_content(rect);
                if cx >= c.x && cy >= c.y && cx < c.x + c.w && cy < c.y + c.h {
                    self.wasm_grab = Some(w);
                }
            }
            Kind::Files => self.files_click(w, rect, cx, cy, false),
            Kind::Viewer => self.viewer_click(w, rect, cx, cy),
            Kind::TaskMgr => self.tarefas_click(w, rect, cx, cy),
            Kind::Settings => self.settings_click(w, rect, cx, cy),
            Kind::Gallery => self.gallery_click(w, rect, cx, cy),
            Kind::LogViewer => self.log_click(w, rect, cx, cy),
            Kind::Editor => self.editor_click(w, rect, cx, cy),
            Kind::Terminal => self.term_click(w, rect, cx, cy),
        }
    }
}
