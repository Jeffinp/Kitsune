//! The page's JavaScript: fetching the scripts, starting the page's script process (`jsrun --page`,
//! ring 3), sending it clicks and taking back the document it changed.
//!
//! Each tab whose page has scripts gets its own process once the scripts are in (`JsProc`); it lives
//! until the tab shows another page or closes (dropping the `JsProc` kills it). The process holds its
//! own copy of the DOM: it receives the document once and sends the whole document back after a task
//! changed it, which is laid out again here. A click on the page goes to the process with the element
//! under the pointer; a click with a default action (a link, a submit button) waits for the page's
//! verdict (`preventDefault`) for at most `ANSWER_TIMEOUT_MS`. The messages and the bookkeeping are
//! `kitsune_core::web::script`; the kernel side of the link is `userproc/page.rs`.

use super::layout::{clamp_scroll, layout_browser};
use crate::desktop::*;
use kitsune_core::web::form::FieldKind;
use kitsune_core::web::script::{FromScript, InitInfo, Session, init_message, parse_message};

/// What a click on the page does unless its scripts prevent it.
pub(crate) enum JsAction {
    Link(Vec<u8>),
    Submit(usize, usize),
}

/// The script process of a tab, and the players of its media elements.
pub(crate) struct JsProc {
    pub pid: u32,
    pub session: Session<JsAction>,
    pub media: super::media::MediaSet,
}

impl Drop for JsProc {
    fn drop(&mut self) {
        crate::fetch::forget_script_fetches(self.pid);
        crate::userproc::kill(self.pid);
    }
}

/// Most messages taken from one process per frame.
const MAX_PER_FRAME: usize = 32;

/// The tab shows another page (or none): stop its scripts.
pub(crate) fn stop_js(t: &mut TabData) {
    if let Some(js) = t.js.take() {
        crate::klog!(Info, "browser: page scripts stopped (pid {})", js.pid);
    }
    t.scripts.clear();
}

fn now_ms() -> u64 {
    u64::from(crate::desktop::shell::toasts::now_ms())
}

impl Desktop {
    /// The next script of the page on screen to download, if the fetcher is free: copies its URL into
    /// `out` and returns the length.
    pub fn browser_next_script(&mut self, out: &mut [u8]) -> Option<usize> {
        let id = self.browser_id()?;
        let b = self.browser_state_mut(id)?;
        if b.script_inflight.is_some() {
            return None;
        }
        let BrowserState {
            tabs,
            script_inflight,
            ..
        } = &mut *b;
        let t = tabs.active_mut();
        t.page.as_ref()?;
        // The ad blocker answers for every script before it is asked for: a blocked one is never
        // requested and counts as failed (the page goes on without it).
        let key = loop {
            let key = t.scripts.next_pending()?;
            let n0 = t.shield.blocked().len();
            if t.shield.check(key.as_bytes()).is_none() {
                break key;
            }
            super::shield::log_new(&t.shield, n0, "script");
            t.scripts.finish(&key, None);
        };
        let n = key.len().min(out.len());
        out[..n].copy_from_slice(&key.as_bytes()[..n]);
        *script_inflight = Some((t.id, key));
        Some(n)
    }

    /// A script download finished (`None`: it failed).
    pub fn browser_script_done(&mut self, body: Option<Vec<u8>>) {
        let Some(id) = self.browser_id() else {
            return;
        };
        let Some(b) = self.browser_state_mut(id) else {
            return;
        };
        let Some((tid, key)) = b.script_inflight.take() else {
            return;
        };
        if let Some((_, t)) = b.tab_by_id(tid) {
            t.scripts.finish(&key, body.as_deref());
        }
    }

    /// Start the script process of the active tab when its scripts are in, and take what every
    /// process sent. Called every frame by the main loop; returns whether the window must repaint.
    pub fn browser_pump_js(&mut self) -> bool {
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
        let active_id = b.tabs.active().id;
        super::jsnet::save_cookies();
        let mut redraw = false;
        let mut media_relayout = false;
        let mut media_redraw = false;
        let mut actions: Vec<(u32, JsAction)> = Vec::new();
        let mut title: Option<String> = None;
        let BrowserState { tabs, images, .. } = &mut *b;
        for t in tabs.iter_mut() {
            if t.id == active_id && t.js.is_none() && t.scripts.ready() && t.page.is_some() {
                start_js(t, images, content);
                redraw = true;
            }
            let Some(js) = t.js.as_mut() else { continue };
            // The last document of the batch is the one that counts.
            let mut doc_msg: Option<Vec<u8>> = None;
            for _ in 0..MAX_PER_FRAME {
                let Some(m) = crate::userproc::page_take(js.pid) else {
                    break;
                };
                match parse_message(&m) {
                    FromScript::Document(_) => doc_msg = Some(m),
                    FromScript::Answer { seq, allow } => {
                        if let Some(a) = js.session.answer(seq, allow) {
                            actions.push((t.id, a));
                        }
                    }
                    FromScript::Navigate(url) => {
                        actions.push((t.id, JsAction::Link(url.as_bytes().to_vec())));
                    }
                    FromScript::Fetch(req) => {
                        super::jsnet::on_fetch(&mut t.shield, &t.browser, js.pid, req);
                    }
                    FromScript::Abort(rid) => crate::fetch::abort_script_fetch(js.pid, rid),
                    FromScript::SetCookie(text) => {
                        super::jsnet::on_set_cookie(&t.browser, js.pid, text);
                    }
                    FromScript::Media(c) => js.media.command(js.pid, c),
                    FromScript::MediaData {
                        id,
                        src,
                        seq,
                        bytes,
                    } => js.media.data(js.pid, id, src, seq, bytes),
                    FromScript::Unknown => {}
                }
            }
            // The players: a tab in the background pauses them.
            if t.id != active_id {
                js.media.pause_all();
            }
            if !js.media.players.is_empty() {
                let mut net = super::media::NetCtx {
                    page_url: String::from_utf8_lossy(t.browser.nav_url()).into_owned(),
                    shield: &mut t.shield,
                    insecure_host: t.browser.insecure_host().unwrap_or_default(),
                };
                let m = js
                    .media
                    .pump(js.pid, &mut net, t.page.as_ref(), t.doc.as_mut());
                if t.id == active_id {
                    media_redraw |= m.redraw;
                    media_relayout |= m.relayout;
                }
            }
            if let Some(a) = js.session.expire(now_ms()) {
                crate::klog!(Info, "browser: the page did not answer a click in time");
                actions.push((t.id, a));
            }
            let alive = crate::userproc::end(js.pid) == crate::userproc::End::Running;
            if let (Some(m), Some(doc)) = (doc_msg, t.doc.as_mut()) {
                let t0 = crate::trace::t();
                doc.replace_from_script(&m[1..]);
                crate::trace::note("js-dom", t0, m.len() as u64);
                let tt = String::from(doc.title());
                if tt != t.browser.page_title() {
                    t.browser.set_page_title(&tt);
                    if t.id == active_id && !tt.is_empty() {
                        title = Some(tt);
                    }
                }
                if t.id == active_id {
                    relayout_js(t, images, content);
                    media_relayout = false;
                } else {
                    t.layout_w = 0;
                    t.page = None;
                }
                redraw = true;
            } else if media_relayout && t.id == active_id {
                // A video learnt its size.
                media_relayout = false;
                relayout_js(t, images, content);
                redraw = true;
            }
            if !alive && let Some(mut js) = t.js.take() {
                crate::klog!(
                    Info,
                    "browser: the page's script process ended (pid {})",
                    js.pid
                );
                if let Some(a) = js.session.abandon() {
                    actions.push((t.id, a));
                }
            }
        }
        // A new picture only: just the video boxes are painted again (and uploaded).
        let mut video_rects: Vec<Rect> = Vec::new();
        if media_redraw && !redraw {
            let t = b.tabs.active();
            if let Some(p) = &t.page {
                video_rects = p
                    .videos
                    .iter()
                    .filter_map(|v| {
                        Rect::new(content.x + v.x, content.y - t.scroll + v.y, v.w, v.h)
                            .intersection(&content)
                    })
                    .collect();
            }
        }
        if redraw {
            b.touch();
        }
        for r in video_rects {
            self.mark_dirty(r);
        }
        if let Some(tt) = title {
            self.browser_set_title(id, tt);
        }
        for (tid, a) in actions {
            self.browser_js_action(id, tid, a);
            redraw = true;
        }
        redraw
    }

    /// Do what a click (or the page) asked for, in tab `tid`.
    fn browser_js_action(&mut self, id: WindowId, tid: u32, a: JsAction) {
        let Some(b) = self.browser_state_mut(id) else {
            return;
        };
        let active = b.tabs.active().id == tid;
        match a {
            JsAction::Link(href) => {
                if let Some((_, t)) = b.tab_by_id(tid)
                    && !t.browser.open_link(&href)
                {
                    b.say(kitsune_core::t!("web.notice.link_bad"));
                }
            }
            JsAction::Submit(form, field) if active => {
                self.browser_submit_form(id, form, Some(field));
            }
            JsAction::Submit(..) => {}
        }
    }

    /// A click at page `(qx, qy)` on a page with a script process: the controls react as always, the
    /// click goes to the page with the element under the pointer, and a link or a submit button waits
    /// for the page's verdict. Returns whether the click starts a text selection.
    pub(super) fn browser_js_click(&mut self, id: WindowId, qx: i32, qy: i32) -> bool {
        let Some(b) = self.browser_state_mut(id) else {
            return false;
        };
        let t = b.tabs.active_mut();
        let Some(page) = &t.page else {
            return false;
        };
        if let (Some(v), Some(js)) = (page.video_at(qx, qy).copied(), t.js.as_mut())
            && js.media.click(js.pid, &v, qx, qy, i32::from(t.zoom.max(1)))
        {
            b.touch();
            return false;
        }
        let t = b.tabs.active_mut();
        let Some(page) = &t.page else {
            return false;
        };
        let uid = page.node_at(qx, qy).unwrap_or(0);
        let mut action = None;
        let mut select = false;
        if let Some(f) = page.field_at(qx, qy).copied() {
            t.forms.set_focus(&page.forms, f.form, f.field);
            match f.kind {
                FieldKind::Submit => action = Some(JsAction::Submit(f.form, f.field)),
                FieldKind::Checkbox | FieldKind::Radio => {
                    t.forms.toggle(&page.forms, f.form, f.field);
                }
                FieldKind::Select => {
                    t.forms.step_select(&page.forms, f.form, f.field, 1);
                }
                _ => {}
            }
        } else {
            t.forms.blur();
            match page.link_at(qx, qy) {
                Some(href) => action = Some(JsAction::Link(href.as_bytes().to_vec())),
                None => {
                    t.sel_anchor = Some((qx, qy));
                    select = true;
                }
            }
        }
        let Some(js) = t.js.as_mut() else {
            return select;
        };
        let msg = js.session.click(uid, qx, qy, action, now_ms());
        if !crate::userproc::page_post(js.pid, msg) {
            // The process is gone or flooded: the browser does what the click would have done.
            if let Some(a) = js.session.abandon() {
                let tid = t.id;
                self.browser_js_action(id, tid, a);
            }
        }
        select
    }
}

/// Start the script process of tab `t` (its scripts are in): the page records its element boxes from
/// now on, and the process gets the document and the scripts.
fn start_js(t: &mut TabData, images: &mut kitsune_core::web::imgcache::ImageCache, content: Rect) {
    let sources = t.scripts.take();
    let cookie = super::jsnet::init_cookie(t);
    let Some(doc) = t.doc.as_mut() else {
        return;
    };
    let url = String::from_utf8_lossy(t.browser.nav_url()).into_owned();
    let lang = if kitsune_core::i18n::lang() == kitsune_core::i18n::Lang::En {
        "en"
    } else {
        "pt-BR"
    };
    let zoom = i32::from(t.zoom.max(1));
    let msg = init_message(
        doc,
        &InitInfo {
            url: &url,
            lang,
            width: content.w * 100 / zoom,
            height: content.h * 100 / zoom,
            cookie: &cookie,
        },
        &sources,
    );
    let pid = match crate::userproc::spawn_page(crate::desktop::services::accounts::cred()) {
        Ok(pid) => pid,
        Err(e) => {
            crate::klog!(Warn, "browser: no script process for the page ({:?})", e);
            return;
        }
    };
    if !crate::userproc::page_post(pid, msg) {
        crate::klog!(
            Warn,
            "browser: the page's scripts are too large to hand over"
        );
        crate::userproc::kill(pid);
        return;
    }
    crate::klog!(
        Info,
        "browser: {} script(s) handed to pid {}",
        sources.len(),
        pid
    );
    doc.set_track(true);
    t.js = Some(JsProc {
        pid,
        session: Session::default(),
        media: Default::default(),
    });
    // The element boxes (and the controls outside forms) appear: lay out as for a new page.
    layout_browser(t, images, content.w, true);
    let max = t.page.as_ref().map_or(0, |p| (p.height - content.h).max(0));
    clamp_scroll(t, max);
}

/// The page's document changed: lay it out again, keeping what was typed into its controls while the
/// forms are the same, and asking for any new pictures.
fn relayout_js(
    t: &mut TabData,
    images: &mut kitsune_core::web::imgcache::ImageCache,
    content: Rect,
) {
    let forms = core::mem::take(&mut t.forms);
    let srcs: Vec<String> = t.page.as_ref().map_or(Vec::new(), |p| {
        p.images.iter().map(|i| i.src.clone()).collect()
    });
    let shape: Vec<usize> = t.page.as_ref().map_or(Vec::new(), |p| {
        p.forms.iter().map(|f| f.fields.len()).collect()
    });
    layout_browser(t, images, content.w, false);
    let (same_pics, same_forms) = t.page.as_ref().map_or((true, false), |p| {
        (
            p.images.len() == srcs.len() && p.images.iter().zip(&srcs).all(|(a, b)| a.src == *b),
            p.forms
                .iter()
                .map(|f| f.fields.len())
                .eq(shape.iter().copied()),
        )
    });
    if !same_pics {
        layout_browser(t, images, content.w, true);
    }
    if same_forms {
        t.forms = forms;
    } else if same_pics && let Some(p) = &t.page {
        t.forms = kitsune_core::web::form::FormState::new(&p.forms);
    }
    let max = t.page.as_ref().map_or(0, |p| (p.height - content.h).max(0));
    clamp_scroll(t, max);
}
