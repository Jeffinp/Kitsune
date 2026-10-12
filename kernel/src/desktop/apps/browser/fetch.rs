//! The browser window's side of the fetcher hand-off, titles, loading and failures.

use super::layout::clamp_scroll;
use super::layout::jump_scroll;
use super::layout::layout_browser;
use crate::desktop::*;
use kitsune_core::anim::curves;
use kitsune_core::t;

impl Desktop {
    /// The (single) browser window, if open.
    pub(crate) fn browser_id(&self) -> Option<WindowId> {
        self.wm
            .windows()
            .iter()
            .find(|w| w.app.kind() == Kind::Browser && !w.is_closing())
            .map(|w| w.id)
    }

    pub(super) fn browser_win_rect(&self, id: WindowId) -> Option<Rect> {
        self.wm.get(id).map(|w| w.rect)
    }

    /// Per-frame stepping of the browser's own motion; returns whether anything moves.
    pub(crate) fn step_browser(&mut self, dt: f32) -> bool {
        let mut busy = false;
        let Some(id) = self.browser_id() else {
            return false;
        };
        if self.browser_win_rect(id).is_none() {
            return false;
        }
        let now = crate::desktop::shell::toasts::now_ms();
        let Some(b) = self.browser_state_mut(id) else {
            return false;
        };
        for t in b.tabs.iter_mut() {
            busy |= t.load.step(dt);
            busy |= t.star.step(dt);
            if t.scroll_spring.step(dt) || (t.scroll as f32 - t.scroll_spring.value()).abs() >= 0.5
            {
                let v = (t.scroll_spring.value().max(0.0) + 0.5) as i32;
                if v != t.scroll {
                    t.scroll = v;
                    busy = true;
                }
            }
            busy |= t.scroll_bar.active(now);
        }
        busy |= b.strip_h.step(dt);
        for e in b.strip.iter_mut() {
            busy |= e.weight.step(dt);
        }
        // Ghosts whose animation ended are gone.
        b.strip.retain(|e| e.id.is_some() || !e.weight.finished());
        busy |= b.notice_flash.step(dt);
        busy |= b.zoom_flash.step(dt);
        if !b.notice_flash.active() {
            b.notice = None;
        }
        busy
    }

    /// Does the browser window need a frame every tick right now?
    pub(crate) fn browser_busy(&self, w: &Win) -> bool {
        match &w.app.app {
            App::Browser(b) => b.busy(),
            _ => false,
        }
    }

    /// Set the window title; the title bar is outside the client area, so the whole window
    /// repaints.
    pub(super) fn browser_set_title(&mut self, id: WindowId, title: String) {
        if let Some(w) = self.wm.get_mut(id)
            && w.app.title != title
        {
            w.app.title = title;
            // The title bar is outside the client area.
            self.client_dirty = None;
            self.force_full = true;
        }
    }

    /// The client area of `w` (below the title bar).
    pub(crate) fn client_rect(w_box: Rect) -> Rect {
        Rect::new(
            w_box.x,
            w_box.y + TITLE_H,
            w_box.w,
            (w_box.h - TITLE_H).max(0),
        )
    }

    // ---- network hand-off (driven by the kernel main loop) ----

    /// If a tab has a pending navigation, copy its target URL into `out`, return the length
    /// and remember the tab as the one the fetcher now works for. The active tab goes first,
    /// then the others in order. The kernel fetches it and reports back with
    /// [`browser_load`](Self::browser_load) / [`browser_fail`](Self::browser_fail).
    pub fn browser_take_request(&mut self, out: &mut [u8]) -> Option<usize> {
        let id = self.browser_id()?;
        let b = self.browser_state_mut(id)?;
        let n = b.tabs.len();
        let act = b.tabs.active_index();
        let order = core::iter::once(act).chain((0..n).filter(|&i| i != act));
        for i in order {
            let Some(t) = b.tabs.get_mut(i) else { continue };
            if let Some(url) = t.browser.take_request() {
                let len = url.len().min(out.len());
                out[..len].copy_from_slice(&url[..len]);
                t.cancelled = false;
                t.load.start();
                let tid = t.id;
                b.req_tab = Some(tid);
                b.touch();
                return Some(len);
            }
        }
        None
    }

    /// The host the user allowed past a certificate error for the navigation just taken by
    /// [`browser_take_request`](Self::browser_take_request) (empty when none).
    pub fn browser_insecure_host(&mut self, out: &mut [u8]) -> usize {
        let Some(id) = self.browser_id() else {
            return 0;
        };
        let Some(b) = self.browser_state_mut(id) else {
            return 0;
        };
        let Some(tid) = b.req_tab else {
            return 0;
        };
        let Some((_, t)) = b.tab_by_id(tid) else {
            return 0;
        };
        match t.browser.insecure_host() {
            Some(h) => {
                let n = h.len().min(out.len());
                out[..n].copy_from_slice(&h[..n]);
                n
            }
            None => 0,
        }
    }

    /// The host the user allowed past a certificate error for the page on screen (empty when none):
    /// its pictures and style sheets come over the same connection rules.
    pub fn browser_insecure_host_active(&mut self, out: &mut [u8]) -> usize {
        let Some(id) = self.browser_id() else {
            return 0;
        };
        let Some(b) = self.browser_state_mut(id) else {
            return 0;
        };
        match b.tabs.active().browser.insecure_host() {
            Some(h) => {
                let n = h.len().min(out.len());
                out[..n].copy_from_slice(&h[..n]);
                n
            }
            None => 0,
        }
    }

    /// The body of the POST the tab just handed over by
    /// [`browser_take_request`](Self::browser_take_request) (`None` for a GET).
    pub fn browser_post_body(&mut self) -> Option<alloc::vec::Vec<u8>> {
        let id = self.browser_id()?;
        let b = self.browser_state_mut(id)?;
        let tid = b.req_tab?;
        let (_, t) = b.tab_by_id(tid)?;
        t.browser.post_body().map(<[u8]>::to_vec)
    }

    /// Show a fetched raw HTTP response in the tab that asked for it. `conn` says how the
    /// final connection was authenticated, `truncated` that the response hit the size cap and
    /// `cert` summarises the server certificate (https only). Dropped when the tab was closed
    /// or the load was stopped meanwhile.
    pub fn browser_load(
        &mut self,
        resp: &[u8],
        conn: kitsune_core::browser::Conn,
        truncated: bool,
        cert: Option<kitsune_core::browser::CertInfo>,
    ) {
        let Some(id) = self.browser_id() else {
            return;
        };
        let Some(rect) = self.browser_win_rect(id) else {
            return;
        };
        let page = kitsune_core::browser::page_body_partial(resp, truncated);
        let body = page.body;
        let Some(b) = self.browser_state_mut(id) else {
            return;
        };
        let Some(tid) = b.req_tab.take() else {
            return;
        };
        let content_w = b.chrome(rect).content.w;
        let active_id = b.tabs.active().id;
        let BrowserState { tabs, images, .. } = &mut *b;
        let Some(t) = tabs.iter_mut().find(|t| t.id == tid) else {
            return;
        };
        if core::mem::take(&mut t.cancelled) {
            return;
        }
        let t0 = crate::trace::t();
        let doc = kitsune_core::web::Doc::parse(&body);
        t.sheets.begin(&doc, t.browser.nav_url());
        t.doc = Some(doc);
        crate::trace::note("parse", t0, body.len() as u64);
        t.trace_t0.set(t0);
        t.page = None;
        t.cert = cert;
        t.sel = None;
        t.sel_anchor = None;
        t.forms = kitsune_core::web::form::FormState::default();
        t.find.close();
        t.browser.loaded_with_note(conn, page.note);
        t.load.finish();
        jump_scroll(t, 0);
        if t.id == active_id {
            images.begin_page();
            t.layout_w = content_w;
            layout_browser(t, images, content_w, true);
        } else {
            t.layout_w = 0;
        }
        let tid = t.id;
        self.browser_page_changed(id, tid);
    }

    /// Lay out and show the HTML of a page the browser generated itself (`kitsune://...`), if
    /// one was just opened in any tab. Called every frame by the main loop, right next to the
    /// network hand-off.
    pub fn browser_poll_internal(&mut self) -> bool {
        let Some(id) = self.browser_id() else {
            return false;
        };
        let Some(rect) = self.browser_win_rect(id) else {
            return false;
        };
        let Some(b) = self.browser_state_mut(id) else {
            return false;
        };
        let content_w = b.chrome(rect).content.w;
        let active_id = b.tabs.active().id;
        let BrowserState { tabs, images, .. } = &mut *b;
        let mut changed: Option<u32> = None;
        for t in tabs.iter_mut() {
            let Some(html) = t.browser.take_internal() else {
                continue;
            };
            t.doc = Some(kitsune_core::web::Doc::parse(&html));
            t.sheets = kitsune_core::web::sheets::SheetSet::default();
            t.page = None;
            t.cert = None;
            t.sel = None;
            t.sel_anchor = None;
            t.forms = kitsune_core::web::form::FormState::default();
            t.find.close();
            t.load.finish();
            jump_scroll(t, 0);
            if t.id == active_id {
                images.begin_page();
                t.layout_w = content_w;
                layout_browser(t, images, content_w, true);
            } else {
                t.layout_w = 0;
            }
            changed = Some(t.id);
        }
        match changed {
            Some(tid) => {
                self.browser_page_changed(id, tid);
                true
            }
            None => false,
        }
    }

    /// A new page is on screen in tab `tid`: remember its title, reset selection and find, set
    /// the window title when the tab is the active one.
    fn browser_page_changed(&mut self, id: WindowId, tid: u32) {
        let mut title = String::from(t!("app.browser"));
        let mut active = false;
        if let Some(b) = self.browser_state_mut(id) {
            b.touch();
            let active_id = b.tabs.active().id;
            if let Some((_, t)) = b.tab_by_id(tid) {
                let tt = t
                    .doc
                    .as_ref()
                    .map(|d| String::from(d.title()))
                    .unwrap_or_default();
                t.browser.set_page_title(&tt);
                t.sel = None;
                t.sel_anchor = None;
                let (page, find) = (&t.page, &mut t.find);
                find.refresh(page.as_ref(), &apps::browser::paint::KernelMetrics);
                // The star fills when the page is a favourite.
                let fav = t.browser.is_bookmarked();
                t.star.retarget(f32::from(fav as u8), 0.3, curves::ENTER);
                if !tt.is_empty() {
                    title = tt;
                }
                active = t.id == active_id;
            }
            b.popover = false;
            b.ctx = None;
            b.notice = None;
        }
        if active {
            self.browser_set_title(id, title);
        }
    }

    /// Mark the in-flight browser fetch as failed.
    pub fn browser_fail(&mut self, reason: kitsune_core::browser::FailReason) {
        let Some(id) = self.browser_id() else {
            return;
        };
        let mut set_title = false;
        if let Some(b) = self.browser_state_mut(id) {
            b.touch();
            let active_id = b.tabs.active().id;
            let Some(tid) = b.req_tab.take() else {
                return;
            };
            if let Some((_, t)) = b.tab_by_id(tid) {
                if core::mem::take(&mut t.cancelled) {
                    return;
                }
                t.page = None;
                t.doc = None;
                t.sel = None;
                t.cert = None;
                t.load.finish();
                t.browser.fail_with(reason);
                set_title = t.id == active_id;
            }
        }
        if set_title {
            self.browser_set_title(id, String::from(t!("app.browser")));
        }
    }

    /// The language changed: the browser's own pages are built again, the active page is
    /// laid out again (a few words are baked into a layout, like the line under a picture
    /// that failed to load) and an open context menu closes. Error pages and the chrome
    /// ask the catalog at every frame and need nothing.
    pub(crate) fn browser_language_changed(&mut self) {
        let Some(id) = self.browser_id() else {
            return;
        };
        let Some(rect) = self.browser_win_rect(id) else {
            return;
        };
        let Some(b) = self.browser_state_mut(id) else {
            return;
        };
        b.ctx = None;
        b.glass[3].clear();
        let content = b.chrome(rect).content;
        let active_id = b.tabs.active().id;
        let BrowserState { tabs, images, .. } = &mut *b;
        let mut rebuilt: Vec<u32> = Vec::new();
        for t in tabs.iter_mut() {
            if let Some(html) = t.browser.internal_html() {
                t.doc = Some(kitsune_core::web::Doc::parse(&html));
                t.page = None;
                t.find.close();
                if t.id == active_id {
                    layout_browser(t, images, content.w, true);
                    let max = t.page.as_ref().map_or(0, |p| (p.height - content.h).max(0));
                    clamp_scroll(t, max);
                } else {
                    t.layout_w = 0;
                }
                rebuilt.push(t.id);
            } else if t.id == active_id && t.page.is_some() {
                layout_browser(t, images, content.w, false);
            }
        }
        b.touch();
        for tid in rebuilt {
            self.browser_page_changed(id, tid);
        }
        self.client_dirty = Some(id);
    }
}
