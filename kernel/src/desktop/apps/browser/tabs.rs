//! Opening, closing and switching tabs.

use super::layout::jump_scroll;
use super::layout::layout_browser;
use crate::desktop::*;
use kitsune_core::anim::{Tween, curves};
use kitsune_core::browser::tabs;
use kitsune_core::t;

impl Desktop {
    // ---- tabs ----

    /// Open a new tab (the start page) and show it. Returns whether there was room.
    pub(crate) fn browser_new_tab(&mut self, id: WindowId) -> bool {
        let Some(b) = self.browser_state_mut(id) else {
            return false;
        };
        if !b.tabs.can_open() {
            b.say(&t!("web.tab.limit", n = tabs::MAX_TABS));
            return false;
        }
        let tid = b.next_id;
        b.next_id += 1;
        let browser = b.tabs.active().browser.sibling();
        let old_active = b.tabs.active_index();
        let Some(_) = b.tabs.open(TabData::new(tid, browser)) else {
            return false;
        };
        // The strip entry goes right after the active tab's.
        let slots = b.strip_tab_slots();
        let entry_at = slots
            .iter()
            .find(|(_, ti)| *ti == old_active)
            .map_or(b.strip.len(), |(ei, _)| ei + 1);
        let mut weight = Tween::at(0.0);
        weight.retarget(1.0, 0.24, curves::ENTER);
        b.strip.insert(
            entry_at,
            StripEntry {
                id: Some(tid),
                weight,
                title: String::new(),
                badge: ' ',
            },
        );
        self.browser_after_switch(id, Some(old_active));
        true
    }

    /// Close tab `i`; the last one closes the window.
    pub(crate) fn browser_close_tab(&mut self, id: WindowId, i: usize) {
        let Some(b) = self.browser_state_mut(id) else {
            return;
        };
        if b.tabs.len() <= 1 {
            self.client_dirty = None;
            self.request_close(id);
            return;
        }
        let old_active = b.tabs.active_index();
        let Some(t) = b.tabs.get(i) else {
            return;
        };
        let (tid, title, badge) = (
            t.id,
            tabs::tab_title(
                t.browser.page_title(),
                &String::from_utf8_lossy(t.browser.nav_url()),
            ),
            tabs::tab_badge(
                t.browser.page_title(),
                &String::from_utf8_lossy(t.browser.nav_url()),
            ),
        );
        // A fetch for it keeps running; its answer is dropped.
        if let Some(t) = b.tabs.get_mut(i).filter(|_| b.req_tab == Some(tid)) {
            t.cancelled = true;
        }
        if b.img_inflight.as_ref().is_some_and(|(x, _)| *x == tid) {
            b.img_inflight = None;
        }
        if b.sheet_inflight.as_ref().is_some_and(|(x, _)| *x == tid) {
            b.sheet_inflight = None;
        }
        if b.script_inflight.as_ref().is_some_and(|(x, _)| *x == tid) {
            b.script_inflight = None;
        }
        let Some(_closed) = b.tabs.close(i) else {
            return;
        };
        // Its strip entry becomes a ghost that shrinks away.
        if let Some(e) = b.strip.iter_mut().find(|e| e.id == Some(tid)) {
            e.id = None;
            e.title = title;
            e.badge = badge;
            e.weight.retarget(0.0, 0.18, curves::EXIT);
        }
        // If the closed tab was the shown one, the one now active has to be laid out.
        self.browser_after_switch(
            id,
            if i == old_active {
                None
            } else {
                Some(old_active)
            },
        );
    }

    /// Show tab `i`.
    pub(crate) fn browser_select_tab(&mut self, id: WindowId, i: usize) {
        let Some(b) = self.browser_state_mut(id) else {
            return;
        };
        let old = b.tabs.active_index();
        if i == old || !b.tabs.select(i) {
            return;
        }
        // The tab we leave frees its display list: only its document stays.
        if let Some(t) = b.tabs.get_mut(old) {
            t.page = None;
            t.hover_link = None;
            t.sel_anchor = None;
        }
        self.browser_after_switch(id, None);
    }

    /// The active tab changed (or was just opened): lay its page out, fix the window title,
    /// strip and focus state.
    fn browser_after_switch(&mut self, id: WindowId, leaving: Option<usize>) {
        let Some(rect) = self.browser_win_rect(id) else {
            return;
        };
        let Some(b) = self.browser_state_mut(id) else {
            return;
        };
        if let Some(old) = leaving
            && let Some(t) = b.tabs.get_mut(old)
        {
            t.page = None;
            t.hover_link = None;
            t.sel_anchor = None;
        }
        b.popover = false;
        b.ctx = None;
        b.hover = BrowserHover::None;
        for s in b.glass.iter() {
            s.clear();
        }
        let strip_target = if b.tabs.len() >= 2 { 36.0 } else { 0.0 };
        b.strip_h.retarget(strip_target, 0.22, curves::ENTER);
        let content = b.chrome(rect).content;
        let BrowserState { tabs, images, .. } = &mut *b;
        let t = tabs.active_mut();
        let was = t.scroll;
        if t.doc.is_some() {
            images.begin_page();
            t.layout_w = content.w;
            layout_browser(t, images, content.w, true);
            let max = t.page.as_ref().map_or(0, |p| (p.height - content.h).max(0));
            jump_scroll(t, was.clamp(0, max));
        }
        // A tab that is the start page has the omnibox ready for typing.
        let fresh = t.browser.is_home();
        let fav = t.browser.is_bookmarked();
        t.star.retarget(f32::from(fav as u8), 0.01, curves::ENTER);
        if fresh {
            t.browser.set_bar_focus(true);
        }
        let title = {
            let tt = t.browser.page_title();
            if tt.is_empty() {
                String::from(t!("app.browser"))
            } else {
                String::from(tt)
            }
        };
        b.touch();
        b.cache.borrow_mut().invalidate();
        self.browser_set_title(id, title);
    }

    /// Stop the load of the active tab.
    pub(crate) fn browser_stop(&mut self, id: WindowId) {
        let Some(b) = self.browser_state_mut(id) else {
            return;
        };
        let req = b.req_tab;
        let t = b.tabs.active_mut();
        if !t.browser.is_loading() {
            return;
        }
        if req == Some(t.id) {
            t.cancelled = true;
        }
        t.browser.stop();
        if t.doc.is_none() {
            t.browser.go_home();
        }
        t.load.finish();
        b.touch();
    }
}
