//! Clicks, drags and pointer shapes in the browser window. Clicks are resolved against the same geometry the window is drawn with (`kitsune_core::layout`).

use super::hover::page_menu_row_at;
use super::hover::start_items;
use crate::desktop::BrowserHover as H;
use crate::desktop::*;
use kitsune_core::browser::Status;
use kitsune_core::layout as geo;
use kitsune_core::t;
use kitsune_core::web::form::FieldKind;
use kitsune_core::web::textops::Selection;

impl Desktop {
    // ---- mouse ----

    /// Resolve a click inside browser window `id`. Returns `true` when it starts selecting text
    /// (the caller then tracks the drag).
    pub(crate) fn browser_click(&mut self, id: WindowId, rect: Rect, px: i32, py: i32) -> bool {
        let Some(b) = self.browser_state_mut(id) else {
            return false;
        };
        b.touch();
        b.notice = None;
        b.notice_flash.hide();
        let ch = b.chrome(rect);
        // An open context menu takes the click; outside it, it only closes.
        if let Some(m) = &b.ctx {
            let pick = page_menu_row_at(m, rect, px, py).and_then(|i| {
                m.items
                    .get(i)
                    .copied()
                    .filter(|(_, _, ok)| *ok)
                    .map(|(cmd, _, _)| cmd)
            });
            let link = m.link.clone();
            b.ctx = None;
            b.glass[3].clear();
            if let Some(cmd) = pick {
                self.browser_menu_command(id, cmd, link);
            }
            return false;
        }
        // The popover: a click in it does nothing, elsewhere it closes first.
        if b.popover {
            let h = 200;
            let pop = geo::browser_popover(ch.bar, rect, h);
            if ch.shield.contains(px, py) {
                b.popover = false;
                b.glass[1].clear();
                return false;
            }
            if pop.contains(px, py) {
                return false;
            }
            b.popover = false;
            b.glass[1].clear();
        }
        // The ad blocker's popover: the switch row toggles the site, the button closes it, a click
        // elsewhere in it does nothing, outside it closes it first.
        if b.shield_pop {
            let g = super::shield::popover_geom(b, &ch, rect);
            let toggle = g.row.contains(px, py);
            if toggle || g.pop.contains(px, py) || ch.blocker.contains(px, py) {
                if toggle {
                    self.browser_toggle_site_shield(id);
                } else if ch.blocker.contains(px, py) {
                    b.shield_pop = false;
                    b.glass[1].clear();
                }
                return false;
            }
            b.shield_pop = false;
            b.glass[1].clear();
        }
        // The suggestion list floats over everything under the omnibox.
        let t = b.tabs.active_mut();
        let n = t.browser.suggestions().len();
        if let Some(i) = geo::browser_suggestion_at(ch.bar, n, px, py) {
            t.browser.pick_suggestion(i);
            t.browser.set_bar_focus(false);
            return false;
        }
        if ch.back.contains(px, py) {
            t.browser.back();
        } else if ch.forward.contains(px, py) {
            t.browser.forward();
        } else if ch.reload.contains(px, py) {
            self.browser_reload(id);
        } else if ch.newtab.contains(px, py) {
            self.browser_new_tab(id);
            if let Some(b) = self.browser_state_mut(id) {
                b.tabs.active_mut().browser.select_bar();
            }
        } else if ch.star.contains(px, py) && !t.browser.is_home() {
            self.browser_toggle_bookmark(id);
        } else if ch.blocker.w > 0 && ch.blocker.contains(px, py) {
            b.popover = false;
            b.shield_pop = true;
            b.glass[1].clear();
        } else if ch.shield.w > 0 && ch.shield.contains(px, py) {
            b.shield_pop = false;
            b.popover = true;
            b.glass[1].clear();
        } else if ch.bar.contains(px, py) {
            t.browser.select_bar();
            t.forms.blur();
        } else if ch.strip.h > 0 && ch.strip.contains(px, py) {
            let rects = geo::browser_tab_rects(ch.strip, &b.strip_weights());
            if let Some(ei) = geo::browser_tab_at(&rects, px, py)
                && let Some(&(_, ti)) = b.strip_tab_slots().iter().find(|(e, _)| *e == ei)
            {
                if geo::browser_tab_close(rects[ei]).contains(px, py) {
                    self.browser_close_tab(id, ti);
                } else {
                    self.browser_select_tab(id, ti);
                }
            }
        } else if t.find.is_open() && {
            let f = geo::browser_find_layout(ch.content);
            f.bar.contains(px, py)
        } {
            let f = geo::browser_find_layout(ch.content);
            if f.close.contains(px, py) {
                t.find.close();
                b.glass[2].clear();
            } else if f.next.contains(px, py) || f.prev.contains(px, py) {
                if f.next.contains(px, py) {
                    t.find.next();
                } else {
                    t.find.prev();
                }
                if let Some(y) = t.find.current_y() {
                    self.scroll_page_to(id, (y - ch.content.h / 2).max(0));
                }
            }
        } else if ch.content.contains(px, py) {
            return self.browser_content_click(id, ch.content, px, py);
        }
        false
    }

    /// A click in the page area: the start page, an error page or the page itself.
    fn browser_content_click(&mut self, id: WindowId, content: Rect, px: i32, py: i32) -> bool {
        let Some(b) = self.browser_state_mut(id) else {
            return false;
        };
        if b.tabs.active().browser.is_home() {
            let (tiles, recents) = start_items(b);
            let l = geo::browser_start_layout(content, tiles.len(), recents.len());
            let t = b.tabs.active_mut();
            if l.search.contains(px, py) {
                t.browser.select_bar();
            } else if let Some(i) = l.tiles.iter().position(|r| r.contains(px, py)) {
                t.browser.open(tiles[i].1.as_bytes());
            } else if let Some(i) = l.recents.iter().position(|r| r.contains(px, py)) {
                t.browser.open(recents[i].as_bytes());
            }
            return false;
        }
        let t = b.tabs.active_mut();
        if t.page.is_none() {
            if t.browser.status() == Status::Error {
                let cert = t.browser.can_continue_insecure();
                let e = geo::browser_error_layout(content, cert);
                if e.retry.contains(px, py) {
                    t.browser.retry();
                } else if cert && e.proceed.contains(px, py) {
                    // The explicit, per-site, per-session "continue anyway".
                    t.browser.continue_insecure();
                }
            }
            return false;
        }
        let (qx, qy) = (px - content.x, py - content.y + t.scroll);
        t.browser.set_bar_focus(false);
        t.sel = None;
        if t.js.is_some() {
            // The page's scripts see the click first (see `script.rs`).
            return self.browser_js_click(id, qx, qy);
        }
        let page = t.page.as_ref();
        if let Some(f) = page.and_then(|p| p.field_at(qx, qy)).copied()
            && let Some(page) = &t.page
        {
            // A control: focus a text box, toggle a box, press a button.
            t.forms.set_focus(&page.forms, f.form, f.field);
            match f.kind {
                FieldKind::Submit => self.browser_submit_form(id, f.form, Some(f.field)),
                FieldKind::Checkbox | FieldKind::Radio => {
                    t.forms.toggle(&page.forms, f.form, f.field);
                }
                // No drop-down list: a click chooses the next option (wrapping round).
                FieldKind::Select => {
                    t.forms.step_select(&page.forms, f.form, f.field, 1);
                }
                _ => {}
            }
            return false;
        }
        t.forms.blur();
        if let Some(href) = page.and_then(|p| p.link_at(qx, qy)) {
            // A click on link text: resolve it against the page and navigate.
            let href = href.as_bytes().to_vec();
            if !t.browser.open_link(&href) {
                b.say(t!("web.notice.link_bad"));
            }
            false
        } else {
            // Empty page area: start selecting text.
            t.sel_anchor = Some((qx, qy));
            true
        }
    }

    /// A double click on page text selects the word.
    pub(crate) fn browser_double_click(&mut self, id: WindowId, content: Rect, px: i32, py: i32) {
        let Some(b) = self.browser_state_mut(id) else {
            return;
        };
        let t = b.tabs.active_mut();
        let Some(page) = &t.page else { return };
        if !content.contains(px, py) {
            return;
        }
        let (qx, qy) = (px - content.x, py - content.y + t.scroll);
        if page.field_at(qx, qy).is_some() || page.link_at(qx, qy).is_some() {
            return;
        }
        t.sel = page.select_word(qx, qy, &apps::browser::paint::KernelMetrics);
        t.sel_anchor = None;
        b.touch();
    }

    /// Right click in the page area: the context menu.
    pub(crate) fn browser_rclick(&mut self, id: WindowId, px: i32, py: i32) {
        let Some(rect) = self.wm.get(id).map(|w| w.rect) else {
            return;
        };
        let Some(b) = self.browser_state_mut(id) else {
            return;
        };
        let ch = b.chrome(rect);
        b.touch();
        b.popover = false;
        b.shield_pop = false;
        if !ch.content.contains(px, py) {
            b.ctx = None;
            return;
        }
        // A page that listens gets `contextmenu` (the browser's menu still opens).
        self.browser_js_mouse(id, px, py, kitsune_core::web::script::MouseKind::Context);
        let Some(b) = self.browser_state_mut(id) else {
            return;
        };
        let t = b.tabs.active();
        let mut link = None;
        if let Some(page) = &t.page {
            let (qx, qy) = (px - ch.content.x, py - ch.content.y + t.scroll);
            link = page.link_at(qx, qy).map(String::from);
        }
        let has_sel = t.sel.is_some();
        let fav = t.browser.is_bookmarked();
        let can_mark = !t.browser.is_home();
        let items = alloc::vec![
            (PageCmd::Copy, t!("menu.edit.copy"), has_sel),
            (PageCmd::OpenLink, t!("web.menu.open_link"), link.is_some()),
            (PageCmd::CopyLink, t!("web.menu.copy_link"), link.is_some()),
            (PageCmd::Back, t!("web.menu.back"), t.browser.can_back()),
            (PageCmd::Reload, t!("web.menu.reload"), !t.browser.is_home()),
            (
                PageCmd::Bookmark,
                if fav {
                    t!("web.menu.bookmark_remove")
                } else {
                    t!("web.menu.bookmark_add")
                },
                can_mark,
            ),
        ];
        b.glass[3].clear();
        b.ctx = Some(PageMenu {
            x: px,
            y: py,
            link,
            items,
        });
        b.hover = H::None;
    }

    /// Run a context-menu command.
    fn browser_menu_command(&mut self, id: WindowId, cmd: PageCmd, link: Option<String>) {
        match cmd {
            PageCmd::Copy => self.copy_from_focused(),
            PageCmd::OpenLink => {
                if let (Some(href), Some(b)) = (link, self.browser_state_mut(id)) {
                    b.tabs.active_mut().browser.open_link(href.as_bytes());
                }
            }
            PageCmd::CopyLink => {
                if let Some(href) = link {
                    let n = href.len().min(clipboard::CAP);
                    self.clipboard.set(&href.as_bytes()[..n]);
                    if let Some(b) = self.browser_state_mut(id) {
                        b.say(t!("web.notice.copied"));
                    }
                }
            }
            PageCmd::Back => {
                if let Some(b) = self.browser_state_mut(id) {
                    b.tabs.active_mut().browser.back();
                }
            }
            PageCmd::Reload => self.browser_reload(id),
            PageCmd::Bookmark => self.browser_toggle_bookmark(id),
        }
    }

    /// While the left button drags on a browser page: extend the selection to the pointer.
    pub(crate) fn browser_select_drag(&mut self, id: WindowId) {
        let Some(rect) = self.wm.get(id).map(|w| w.rect) else {
            return;
        };
        let (cx, cy) = (self.cursor_x, self.cursor_y);
        let Some(b) = self.browser_state_mut(id) else {
            return;
        };
        let content = b.chrome(rect).content;
        let t = b.tabs.active_mut();
        let (Some(a), Some(page)) = (t.sel_anchor, &t.page) else {
            return;
        };
        let cur = (cx - content.x, cy - content.y + t.scroll);
        let sel: Option<Selection> = page.select(a, cur, &apps::browser::paint::KernelMetrics);
        if sel != t.sel {
            t.sel = sel;
            b.touch();
        }
        // Dragging past the top or bottom edge scrolls.
        if cy < content.y + 4 {
            self.scroll_page(id, -24);
        } else if cy > content.bottom() - 4 {
            self.scroll_page(id, 24);
        }
        self.mark_dirty(rect);
    }

    // ---- the pointer ----

    /// Is the pointer over something clickable in the browser window? Then it is a hand.
    pub(crate) fn browser_cursor_hand(&self, win: &Win, cx: i32, cy: i32) -> bool {
        let App::Browser(b) = &win.app.app else {
            return false;
        };
        let (h, link) = super::hover::browser_hover_at(b, win.rect, cx, cy);
        let t = b.tabs.active();
        match h {
            H::Back => t.browser.can_back(),
            H::Forward => t.browser.can_forward(),
            H::Reload
            | H::NewTab
            | H::Shield
            | H::Blocker
            | H::BlockerSwitch
            | H::Tab(_)
            | H::TabClose(_) => true,
            H::Star => !t.browser.is_home(),
            H::Suggestion(_)
            | H::MenuRow(_)
            | H::Tile(_)
            | H::Recent(_)
            | H::FindPrev
            | H::FindNext
            | H::FindClose
            | H::Retry
            | H::Proceed => true,
            H::Bar | H::None => {
                link.is_some()
                    || t.page.as_ref().is_some_and(|p| {
                        let ch = b.chrome(win.rect);
                        ch.content.contains(cx, cy)
                            && p.field_at(cx - ch.content.x, cy - ch.content.y + t.scroll)
                                .is_some_and(|f| {
                                    matches!(
                                        f.kind,
                                        FieldKind::Submit
                                            | FieldKind::PushButton
                                            | FieldKind::Checkbox
                                            | FieldKind::Radio
                                            | FieldKind::Select
                                    )
                                })
                    })
            }
        }
    }

    /// Is the pointer over text the user can edit (the omnibox, a text field of the page)?
    pub(crate) fn browser_cursor_text(&self, win: &Win, cx: i32, cy: i32) -> bool {
        let App::Browser(b) = &win.app.app else {
            return false;
        };
        let ch = b.chrome(win.rect);
        if b.ctx.is_some() {
            return false;
        }
        let t = b.tabs.active();
        if ch.bar.contains(cx, cy) && !ch.star.contains(cx, cy) && !ch.shield.contains(cx, cy) {
            return true;
        }
        if t.browser.is_home() {
            let (tiles, recents) = start_items(b);
            let l = geo::browser_start_layout(ch.content, tiles.len(), recents.len());
            return l.search.contains(cx, cy);
        }
        t.page.as_ref().is_some_and(|p| {
            ch.content.contains(cx, cy)
                && p.field_at(cx - ch.content.x, cy - ch.content.y + t.scroll)
                    .is_some_and(|f| f.kind.is_text())
        })
    }
}
