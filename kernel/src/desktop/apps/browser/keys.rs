//! Keyboard handling of the browser window and the actions its chords start. Keys go, in this order, to: an open context menu or popover (Esc), Ctrl chords (tabs, favourites, find, zoom, address), the find bar, a focused form control, then the page (scrolling) or the omnibox.

use super::layout::layout_browser;
use crate::desktop::*;
use kitsune_core::t;
use kitsune_core::web::form::FormOutcome;

/// A page step with the keyboard (Page Down, Space): a view less a little overlap.
fn page_step(view_h: i32) -> i32 {
    (view_h - 48).max(48)
}

impl Desktop {
    /// Keys for the browser window `id`.
    pub(crate) fn browser_key(&mut self, id: WindowId, key: Key) {
        let ctrl = self.keymap.ctrl();
        let shift = self.keymap.shift();
        let Some(rect) = self.wm.get(id).map(|w| w.rect) else {
            return;
        };
        // Whatever a key does here stays inside the client area, unless a handler below says
        // otherwise (the title changes, the window closes).
        self.client_dirty = Some(id);
        let Some(b) = self.browser_state_mut(id) else {
            return;
        };
        let view_h = b.chrome(rect).content.h;
        b.notice = None;
        b.notice_flash.hide();
        b.touch();
        // A menu or the popover: Esc closes it, every other key is for nobody.
        if b.ctx.is_some() || b.popover || b.shield_pop {
            if key == Key::Esc {
                b.ctx = None;
                b.popover = false;
                b.shield_pop = false;
                b.glass[1].clear();
                b.glass[3].clear();
            }
            return;
        }
        if ctrl {
            self.browser_ctrl_key(id, key, shift);
            return;
        }
        // The find bar takes the keys while it is open.
        if b.find.is_open() {
            use kitsune_core::web::find::FindOutcome;
            let t = b.tabs.active_mut();
            let out = t.find.on_key(
                key,
                shift,
                t.page.as_ref(),
                &apps::browser::paint::KernelMetrics,
            );
            let y = t.find.current_y();
            if out == FindOutcome::Closed {
                b.glass[2].clear();
            }
            if out == FindOutcome::Changed
                && let Some(y) = y
            {
                // Bring the match into the middle of the view.
                self.scroll_page_to(id, (y - view_h / 2).max(0));
            }
            return;
        }
        // A focused form control gets the key first.
        let has_focus = b.forms.focus().is_some() && b.page.is_some();
        if has_focus {
            let t = b.tabs.active_mut();
            let Some(page) = &t.page else { return };
            if key == Key::Tab {
                if !t.forms.tab(&page.forms, shift) {
                    t.browser.set_bar_focus(true);
                }
                self.browser_scroll_to_focus(id);
                return;
            }
            match t.forms.on_key(&page.forms, key) {
                FormOutcome::Submit { form, submitter } => {
                    self.browser_submit_form(id, form, submitter);
                }
                FormOutcome::Blur => {}
                FormOutcome::Changed => self.browser_scroll_to_focus(id),
                FormOutcome::Ignored => match key {
                    Key::PageUp => {
                        self.scroll_page(id, -page_step(view_h));
                    }
                    Key::PageDown => {
                        self.scroll_page(id, page_step(view_h));
                    }
                    Key::Up => {
                        self.scroll_page(id, -48);
                    }
                    Key::Down => {
                        self.scroll_page(id, 48);
                    }
                    _ => {}
                },
            }
            return;
        }
        // Esc: close the suggestion list, drop the selection, stop a load. It never closes the window.
        if key == Key::Esc {
            let t = b.tabs.active_mut();
            if !t.browser.suggestions().is_empty() || t.sel.is_some() {
                t.browser.on_key(Key::Esc);
                t.sel = None;
            } else if t.browser.is_loading() {
                self.browser_stop(id);
            } else if t.browser.bar_focus() && !t.browser.is_home() {
                t.browser.set_bar_focus(false);
            }
            return;
        }
        let t = b.tabs.active_mut();
        let page_focus = !t.browser.bar_focus();
        match key {
            Key::PageUp => {
                self.scroll_page(id, -page_step(view_h));
            }
            Key::PageDown => {
                self.scroll_page(id, page_step(view_h));
            }
            Key::Home if page_focus => {
                self.scroll_page(id, i32::MIN / 2);
            }
            Key::End if page_focus => {
                self.scroll_page(id, i32::MAX / 2);
            }
            Key::Char(b' ') if page_focus => {
                self.scroll_page(
                    id,
                    if shift {
                        -page_step(view_h)
                    } else {
                        page_step(view_h)
                    },
                );
            }
            Key::Tab => {
                // Into the page's controls (if it has any).
                let has = t.page.as_ref().is_some_and(|p| !p.forms.is_empty());
                if has && let Some(page) = &t.page {
                    t.browser.set_bar_focus(false);
                    if !t.forms.tab(&page.forms, shift) {
                        t.browser.set_bar_focus(true);
                    }
                    self.browser_scroll_to_focus(id);
                }
            }
            Key::Up | Key::Down => {
                // The suggestion list uses them; on a page they scroll.
                if !t.browser.on_key(key) {
                    self.scroll_page(id, if key == Key::Up { -64 } else { 64 });
                }
            }
            Key::Left | Key::Right if page_focus => {
                self.scroll_page(id, if key == Key::Left { -64 } else { 64 });
            }
            _ => {
                t.browser.set_bar_focus(true);
                t.forms.blur();
                t.browser.on_key(key);
                // Enter hands the keyboard to the page; typing or Ctrl+L brings the bar back.
                if key == Key::Enter && (t.browser.is_loading() || !t.browser.is_home()) {
                    t.browser.set_bar_focus(false);
                }
            }
        }
    }

    /// A Ctrl chord from the panel for browser window `id`.
    pub(crate) fn browser_ctrl_chord(&mut self, id: WindowId, c: char) {
        if c.is_ascii() {
            self.browser_ctrl_key(id, Key::Char(c as u8), false);
        }
    }

    /// Ctrl+T/W new and close tab, Ctrl+Tab and Ctrl+1..9 switch, Ctrl+D favourite, Ctrl+F
    /// find, Ctrl+plus/minus/0 zoom, Ctrl+L and Ctrl+R.
    fn browser_ctrl_key(&mut self, id: WindowId, key: Key, shift: bool) {
        let Some(rect) = self.wm.get(id).map(|w| w.rect) else {
            return;
        };
        let Some(b) = self.browser_state_mut(id) else {
            return;
        };
        let content = b.chrome(rect).content;
        match key {
            Key::Char(b't' | b'T') => {
                self.browser_new_tab(id);
                if let Some(b) = self.browser_state_mut(id) {
                    b.tabs.active_mut().browser.select_bar();
                }
            }
            Key::Char(b'w' | b'W') => {
                let i = b.tabs.active_index();
                self.browser_close_tab(id, i);
            }
            Key::Tab | Key::PageDown | Key::PageUp => {
                let back = key == Key::PageUp || (key == Key::Tab && shift);
                let n = b.tabs.len();
                let cur = b.tabs.active_index();
                let to = if back {
                    (cur + n - 1) % n
                } else {
                    (cur + 1) % n
                };
                self.browser_select_tab(id, to);
            }
            Key::Char(d @ b'1'..=b'9') => {
                let n = usize::from(d - b'0');
                let cur = b.tabs.active_index();
                let to = if n == 9 { b.tabs.len() - 1 } else { n - 1 };
                if to < b.tabs.len() && to != cur {
                    self.browser_select_tab(id, to);
                }
            }
            Key::Char(b'd' | b'D') => self.browser_toggle_bookmark(id),
            Key::Char(b'f' | b'F') => {
                let t = b.tabs.active_mut();
                if t.page.is_some() {
                    t.find
                        .open(t.page.as_ref(), &apps::browser::paint::KernelMetrics);
                    t.browser.set_bar_focus(false);
                    b.glass[2].clear();
                }
            }
            Key::Char(b'l' | b'L') => {
                let t = b.tabs.active_mut();
                t.find.close();
                t.forms.blur();
                t.browser.select_bar();
            }
            Key::Char(b'r' | b'R') => self.browser_reload(id),
            Key::Char(b'=' | b'+' | b'-' | b'_' | b'0') => {
                use kitsune_core::web::{zoom_in, zoom_out};
                let BrowserState { tabs, images, .. } = &mut *b;
                let t = tabs.active_mut();
                t.zoom = match key {
                    Key::Char(b'=' | b'+') => zoom_in(t.zoom),
                    Key::Char(b'-' | b'_') => zoom_out(t.zoom),
                    _ => 100,
                };
                let old = t.scroll;
                t.layout_w = content.w;
                layout_browser(t, images, content.w, false);
                let max = t.page.as_ref().map_or(0, |p| (p.height - content.h).max(0));
                let y = old.clamp(0, max);
                t.scroll = y;
                t.scroll_spring.jump(y as f32);
                b.zoom_flash.show(1.2);
            }
            _ => {}
        }
        if let Some(b) = self.browser_state_mut(id) {
            b.touch();
        }
    }

    /// Reload the page of the active tab (the stop button while loading).
    pub(crate) fn browser_reload(&mut self, id: WindowId) {
        let Some(b) = self.browser_state_mut(id) else {
            return;
        };
        let t = b.tabs.active_mut();
        if t.browser.is_loading() {
            self.browser_stop(id);
            return;
        }
        t.browser.reload();
    }

    /// Ctrl+D, the star: add the page to the favourites or remove it.
    pub(crate) fn browser_toggle_bookmark(&mut self, id: WindowId) {
        let Some(b) = self.browser_state_mut(id) else {
            return;
        };
        let t = b.tabs.active_mut();
        let state = t.browser.toggle_bookmark();
        let msg = match state {
            Some(true) => t!("web.notice.bookmark_added"),
            Some(false) => t!("web.notice.bookmark_removed"),
            None => t!("web.notice.bookmark_none"),
        };
        t.star.retarget(
            f32::from(state == Some(true)),
            if state == Some(true) { 0.38 } else { 0.18 },
            kitsune_core::anim::curves::ENTER,
        );
        b.say(msg);
    }

    /// Close the active tab (Ctrl+W, the menu): the last tab closes the window.
    pub(crate) fn browser_close_active(&mut self, id: WindowId) {
        let i = self
            .browser_state_mut(id)
            .map_or(0, |b| b.tabs.active_index());
        self.browser_close_tab(id, i);
    }

    /// Submit form `form` (Enter in a field, or a button): navigate (GET) or send the body (POST),
    /// or say why not.
    pub(crate) fn browser_submit_form(
        &mut self,
        id: WindowId,
        form: usize,
        submitter: Option<usize>,
    ) {
        let Some(b) = self.browser_state_mut(id) else {
            return;
        };
        let t = b.tabs.active_mut();
        let Some(page) = &t.page else {
            return;
        };
        match t.forms.submission(&page.forms, form, submitter) {
            Ok(sub) => {
                t.forms.blur();
                t.browser.set_bar_focus(false);
                let ok = match sub.body {
                    Some(body) => t.browser.open_post(sub.href.as_bytes(), body.into_bytes()),
                    None => t.browser.open_link(sub.href.as_bytes()),
                };
                if !ok {
                    b.say(t!("web.form.bad_action"));
                }
            }
            Err(e) => b.say(e.message()),
        }
    }

    /// Scroll the page so the focused form control is visible.
    fn browser_scroll_to_focus(&mut self, id: WindowId) {
        let Some(rect) = self.wm.get(id).map(|w| w.rect) else {
            return;
        };
        let Some(b) = self.browser_state_mut(id) else {
            return;
        };
        let view_h = b.chrome(rect).content.h;
        let t = b.tabs.active();
        let (Some(page), Some((f, i))) = (&t.page, t.forms.focus()) else {
            return;
        };
        let Some(fb) = page.fields.iter().find(|x| x.form == f && x.field == i) else {
            return;
        };
        let (top, bottom) = (fb.y, fb.y + fb.h);
        let scroll = t.scroll_spring.target() as i32;
        if top < scroll + 16 {
            self.scroll_page_to(id, top - 16);
        } else if bottom > scroll + view_h - 48 {
            self.scroll_page_to(id, bottom - (view_h - 48));
        }
    }
}
