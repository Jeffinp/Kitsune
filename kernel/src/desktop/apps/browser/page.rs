//! Scrolling, re-layout and the pictures of the active page.

use super::layout::clamp_scroll;
use super::layout::layout_browser;
use super::model::WHEEL_PX;
use crate::desktop::*;

impl Desktop {
    // ---- scrolling ----

    /// Largest scroll of the page on screen in window `id`.
    fn browser_max_scroll(&mut self, id: WindowId) -> i32 {
        let Some(rect) = self.browser_win_rect(id) else {
            return 0;
        };
        let Some(b) = self.browser_state_mut(id) else {
            return 0;
        };
        let view_h = b.chrome(rect).content.h;
        b.page.as_ref().map_or(0, |p| (p.height - view_h).max(0))
    }

    /// Scroll the page of window `id` by `dy` pixels (eased, clamped to the page).
    pub(crate) fn scroll_page(&mut self, id: WindowId, dy: i32) -> bool {
        let max = self.browser_max_scroll(id);
        let Some(b) = self.browser_state_mut(id) else {
            return false;
        };
        let now = crate::desktop::shell::toasts::now_ms();
        let t = b.tabs.active_mut();
        let target = (t.scroll_spring.target() as i32)
            .saturating_add(dy)
            .clamp(0, max);
        let moved = target != t.scroll_spring.target() as i32;
        t.scroll_spring.set_target(target as f32);
        t.scroll_bar.touch(now);
        moved
    }

    /// Put the page at scroll `y` at once (find, focus: the jump is part of another change).
    pub(crate) fn scroll_page_to(&mut self, id: WindowId, y: i32) {
        let max = self.browser_max_scroll(id);
        if let Some(b) = self.browser_state_mut(id) {
            let now = crate::desktop::shell::toasts::now_ms();
            let t = b.tabs.active_mut();
            let y = y.clamp(0, max);
            t.scroll_spring.set_target(y as f32);
            t.scroll_bar.touch(now);
        }
    }

    /// Wheel over browser window `id`: three text lines per notch.
    pub(crate) fn browser_wheel(&mut self, id: WindowId, notches: i32) -> bool {
        // Over an open menu or popover the wheel does nothing to the page.
        if let Some(b) = self.browser_state_mut(id)
            && (b.ctx.is_some() || b.popover)
        {
            return false;
        }
        self.scroll_page(id, notches * WHEEL_PX)
    }

    /// Lay the page of the active tab out again for the window's current width (after a
    /// resize or maximize). Cheap no-op when the width did not change.
    pub(crate) fn relayout_browser(&mut self, id: WindowId) {
        let Some(rect) = self.browser_win_rect(id) else {
            return;
        };
        let Some(b) = self.browser_state_mut(id) else {
            return;
        };
        let content = b.chrome(rect).content;
        let BrowserState { tabs, images, .. } = &mut *b;
        let t = tabs.active_mut();
        if t.doc.is_some() && (t.layout_w != content.w || t.page.is_none()) {
            t.layout_w = content.w;
            let register = t.page.is_none();
            layout_browser(t, images, content.w, register);
            let max = t.page.as_ref().map_or(0, |p| (p.height - content.h).max(0));
            clamp_scroll(t, max);
        }
        b.touch();
    }

    // ---- pictures ----

    /// The next picture of the page that has to be downloaded, if the fetcher is free: copies its
    /// URL into `out` and returns the length and the column width to scale it to. Inline `data:`
    /// pictures are decoded right here (they are small) and never reach the fetcher.
    pub fn browser_next_image(&mut self, out: &mut [u8]) -> Option<(usize, usize)> {
        let id = self.browser_id()?;
        let rect = self.browser_win_rect(id)?;
        let b = self.browser_state_mut(id)?;
        let content = b.chrome(rect).content;
        if b.img_inflight.is_some() {
            return None;
        }
        let fit_w = (content.w - 16).max(16) as usize;
        let BrowserState {
            tabs,
            images,
            img_inflight,
            ..
        } = &mut *b;
        let t = tabs.active_mut();
        t.page.as_ref()?;
        let mut inline_done = false;
        let mut result = None;
        while let Some((key, data)) = images.next_pending() {
            if let Some(uri) = data {
                let r = kitsune_core::web::imgcache::decode_data_uri(&uri, fit_w);
                images.finish(&key, r);
                inline_done = true;
                continue;
            }
            let n = key.len().min(out.len());
            out[..n].copy_from_slice(&key.as_bytes()[..n]);
            *img_inflight = Some((t.id, key));
            result = Some((n, fit_w));
            break;
        }
        if inline_done {
            layout_browser(t, images, content.w, false);
            b.touch();
        }
        result
    }

    /// The next style sheet of the page that has to be downloaded, if the fetcher is free: copies its
    /// URL into `out` and returns the length. Sheets go before pictures: they change the layout.
    pub fn browser_next_sheet(&mut self, out: &mut [u8]) -> Option<usize> {
        let id = self.browser_id()?;
        let b = self.browser_state_mut(id)?;
        if b.sheet_inflight.is_some() {
            return None;
        }
        let BrowserState {
            tabs,
            sheet_inflight,
            ..
        } = &mut *b;
        let t = tabs.active_mut();
        t.page.as_ref()?;
        let key = t.sheets.next_pending()?;
        let n = key.len().min(out.len());
        out[..n].copy_from_slice(&key.as_bytes()[..n]);
        *sheet_inflight = Some((t.id, key));
        Some(n)
    }

    /// A style sheet request finished (`None`: it failed): add its rules to the page; the layout
    /// follows ([`Desktop::browser_flush_layout`]).
    pub fn browser_sheet_done(&mut self, css: Option<alloc::vec::Vec<u8>>) {
        let Some(id) = self.browser_id() else {
            return;
        };
        let Some(b) = self.browser_state_mut(id) else {
            return;
        };
        let Some((tid, key)) = b.sheet_inflight.take() else {
            return;
        };
        let active_id = b.tabs.active().id;
        let BrowserState { tabs, .. } = &mut *b;
        let Some(t) = tabs.iter_mut().find(|t| t.id == tid) else {
            return;
        };
        let text = css.map(|c| alloc::string::String::from_utf8_lossy(&c).into_owned());
        let Some(doc) = t.doc.as_mut() else {
            return;
        };
        if t.sheets.finish(doc, &key, text.as_deref()) && tid == active_id {
            // The styles changed what is shown (and which pictures): lay out as for a new page, but
            // not yet: more sheets and pictures are probably on their way (see `Relayout`).
            t.relayout.request(true);
        }
    }

    /// A picture request finished: store it; the layout follows ([`Desktop::browser_flush_layout`]).
    pub fn browser_image_done(
        &mut self,
        res: Result<kitsune_core::web::imgcache::Loaded, kitsune_core::web::imgcache::ImgFail>,
    ) {
        let Some(id) = self.browser_id() else {
            return;
        };
        let Some(b) = self.browser_state_mut(id) else {
            return;
        };
        let Some((tid, key)) = b.img_inflight.take() else {
            return;
        };
        let active_id = b.tabs.active().id;
        let BrowserState { tabs, images, .. } = &mut *b;
        images.finish(&key, res);
        if tid == active_id {
            tabs.active_mut().relayout.request(false);
        }
    }

    /// Lay the active page out if a sheet or a picture arrived and the layout is due (see
    /// [`Relayout`]). Returns whether it did.
    pub fn browser_flush_layout(&mut self) -> bool {
        let Some(id) = self.browser_id() else {
            return false;
        };
        let Some(rect) = self.browser_win_rect(id) else {
            return false;
        };
        let Some(b) = self.browser_state_mut(id) else {
            return false;
        };
        let content = b.chrome(rect).content;
        let BrowserState { tabs, images, .. } = &mut *b;
        let t = tabs.active_mut();
        if !t.relayout.pending {
            return false;
        }
        if !t
            .relayout
            .due(crate::interrupts::ticks(), t.sheets.busy(), images.busy())
        {
            return false;
        }
        let register = t.relayout.register;
        layout_browser(t, images, content.w, register);
        let max = t.page.as_ref().map_or(0, |p| (p.height - content.h).max(0));
        clamp_scroll(t, max);
        b.touch();
        true
    }
}
