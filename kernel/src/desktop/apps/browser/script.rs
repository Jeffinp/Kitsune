//! The page's JavaScript: fetching the scripts, starting the page's script process (`jsrun --page`,
//! ring 3), sending it clicks and taking back the document it changed.
//!
//! Each tab whose page has scripts gets its own process once the scripts are in (`JsProc`); it lives
//! until the tab shows another page or closes (dropping the `JsProc` kills it). The process holds its
//! own copy of the DOM: it receives the document once and sends the whole document back after a task
//! changed it, which is laid out again here. A click on the page goes to the process with the element
//! under the pointer; a click with a default action (a link, a submit button) waits for the page's
//! verdict (`preventDefault`) for at most `ANSWER_TIMEOUT_MS`. Keys, focus, the mouse and the
//! layout snapshot are `jsinput.rs`; `localStorage` is `jsstore.rs`. The messages and the bookkeeping
//! are `kitsune_core::web::script`; the kernel side of the link is `userproc/page.rs`.

use super::layout::{clamp_scroll, layout_browser};
use crate::desktop::*;
use kitsune_core::web::form::{FieldKind, FormState, field_by_uid};
use kitsune_core::web::script::{
    ClickAt, FromScript, InitInfo, Inputs, Session, Subs, View, init_message, parse_message,
    view_message,
};

/// What a click or a key on the page does unless its scripts prevent it, and what the page asks for.
pub(crate) enum JsAction {
    Link(Vec<u8>),
    /// Submit form `.0` with the button `.1` (Enter in a field: none).
    Submit(usize, Option<usize>),
    /// Scroll the page to this y (device px).
    Scroll(i32),
    /// Open this address in a new tab (`window.open`).
    Open(Vec<u8>),
}

/// The script process of a tab.
pub(crate) struct JsProc {
    pub pid: u32,
    pub session: Session<JsAction>,
    /// The inputs sent and how far the page's documents caught up with them.
    pub inputs: Inputs,
    /// The events the page listens to that are only sent on request.
    pub subs: Subs,
    /// The control the page was last told has the focus (element uid, 0: none).
    pub focus_uid: u32,
    /// A layout ran: the page gets the new snapshot this frame.
    pub boxes_due: bool,
    /// The viewport and scroll the page last heard of.
    pub view: View,
    /// The last pointer position over the page (page px) not yet sent.
    pub pending_move: Option<(i32, i32)>,
    /// The page was last told its tab is shown.
    pub visible: bool,
    /// The page's Web Workers (`jsworker.rs`); dropped (killed) with the page.
    pub workers: Vec<super::jsworker::WorkerProc>,
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
        b.webstore.save_due();
        let mut redraw = false;
        let mut actions: Vec<(u32, JsAction)> = Vec::new();
        let mut title: Option<String> = None;
        let BrowserState {
            tabs,
            images,
            webstore,
            ..
        } = &mut *b;
        for t in tabs.iter_mut() {
            if t.id == active_id && t.js.is_none() && t.scripts.ready() && t.page.is_some() {
                start_js(t, images, content, webstore);
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
                    FromScript::Ack(n) => js.inputs.ack(n),
                    FromScript::Want(s) => js.subs = s,
                    FromScript::Focus(uid) => {
                        // A script focused a control: the caret goes there (or nowhere).
                        js.focus_uid = uid;
                        if let Some(p) = &t.page {
                            match field_by_uid(&p.forms, uid) {
                                Some((f, i)) => {
                                    t.forms.set_focus(&p.forms, f, i);
                                    if t.id == active_id {
                                        t.browser.set_bar_focus(false);
                                    }
                                }
                                None => t.forms.blur(),
                            }
                            redraw = true;
                        }
                    }
                    FromScript::Scroll(_, y) if t.id == active_id => {
                        let y = super::jsinput::scroll_target(t.zoom, y);
                        actions.push((t.id, JsAction::Scroll(y)));
                    }
                    FromScript::Scroll(..) => {}
                    FromScript::History { replace, url } => {
                        if t.browser.push_state(url.as_bytes(), replace) {
                            crate::klog!(
                                Info,
                                "browser: the page's address is now {} ({})",
                                url,
                                if replace { "replaceState" } else { "pushState" }
                            );
                            redraw = true;
                        }
                    }
                    FromScript::Go(n) => t.browser.go(n),
                    FromScript::Open(url) => {
                        actions.push((t.id, JsAction::Open(url.as_bytes().to_vec())));
                    }
                    FromScript::Storage(op) => {
                        let url = String::from_utf8_lossy(t.browser.nav_url()).into_owned();
                        webstore.apply(&url, op);
                    }
                    FromScript::Worker(op) => super::jsworker::on_op(js, op),
                    // What only a worker sends.
                    FromScript::ToPage(_)
                    | FromScript::WorkerFailed(_)
                    | FromScript::WorkerClose => {}
                    FromScript::Unknown => {}
                }
            }
            super::jsworker::pump(t);
            let Some(js) = t.js.as_mut() else { continue };
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
                } else {
                    t.layout_w = 0;
                    t.page = None;
                }
                redraw = true;
            }
            super::jsinput::frame(t, content, t.id == active_id);
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
        if redraw {
            b.touch();
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
            JsAction::Submit(form, submitter) if active => {
                self.browser_submit_form(id, form, submitter);
            }
            JsAction::Submit(..) => {}
            JsAction::Scroll(y) if active => self.scroll_page_to(id, y),
            JsAction::Scroll(_) => {}
            JsAction::Open(url) => {
                self.browser_new_tab(id);
                if let Some(b) = self.browser_state_mut(id) {
                    b.tabs.active_mut().browser.open(&url);
                }
            }
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
        let uid = page.node_at(qx, qy).unwrap_or(0);
        let mut action = None;
        let mut select = false;
        let mut stepped = None;
        if let Some(f) = page.field_at(qx, qy).copied() {
            t.forms.set_focus(&page.forms, f.form, f.field);
            match f.kind {
                FieldKind::Submit => action = Some(JsAction::Submit(f.form, Some(f.field))),
                FieldKind::Checkbox | FieldKind::Radio => {
                    t.forms.toggle(&page.forms, f.form, f.field);
                }
                FieldKind::Select => {
                    t.forms.step_select(&page.forms, f.form, f.field, 1);
                    stepped = Some((f.form, f.field));
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
        // The page hears of the focus first, then the click, then a <select> the click stepped.
        super::jsinput::sync_focus(t);
        let at = ClickAt {
            x: qx,
            y: qy,
            cx: super::jsinput::css(qx, t.zoom),
            cy: super::jsinput::css(qy - t.scroll, t.zoom),
        };
        let forms = t.page.as_ref().map(|p| p.forms.clone()).unwrap_or_default();
        let Some(js) = t.js.as_mut() else {
            return select;
        };
        let n = js.inputs.next_number();
        let msg = js.session.click(uid, at, n, action, now_ms());
        let posted = crate::userproc::page_post(js.pid, msg);
        if let Some((f, i)) = stepped {
            super::jsinput::select_changed(t, &forms, f, i);
        }
        let Some(js) = t.js.as_mut() else {
            return select;
        };
        if !posted {
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
/// now on, and the process gets the document and the scripts, then the viewport, its origin's
/// `localStorage` and where it is in the history.
fn start_js(
    t: &mut TabData,
    images: &mut kitsune_core::web::imgcache::ImageCache,
    content: Rect,
    webstore: &mut super::jsstore::Store,
) {
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
    // Where the page is on screen (the QEMU scenarios turn the page's coordinates into clicks).
    crate::klog!(
        Info,
        "browser: {} script(s) handed to pid {} (page at {},{} zoom {})",
        sources.len(),
        pid,
        content.x,
        content.y,
        t.zoom
    );
    doc.set_track(true);
    let view = View::from_device(content.w, content.h, t.scroll, 0, t.zoom);
    crate::userproc::page_post(pid, view_message(&view));
    crate::userproc::page_post(pid, webstore.message_for(&url));
    t.js = Some(JsProc {
        pid,
        session: Session::default(),
        inputs: Inputs::default(),
        subs: Subs::default(),
        focus_uid: 0,
        boxes_due: true,
        view,
        pending_move: None,
        visible: true,
        workers: Vec::new(),
    });
    super::jsinput::after_init(t);
    // The element boxes (and the controls outside forms) appear: lay out as for a new page.
    layout_browser(t, images, content.w, true);
    let max = t.page.as_ref().map_or(0, |p| (p.height - content.h).max(0));
    clamp_scroll(t, max);
}

/// The page's document changed: lay it out again and ask for any new pictures. The controls' state
/// comes from the document (the page owns it: what a script set shows); the focus and the caret stay on
/// the same control, and a control the person typed into keeps the browser's text while the page has
/// not seen those keys yet (its next document will carry them).
fn relayout_js(
    t: &mut TabData,
    images: &mut kitsune_core::web::imgcache::ImageCache,
    content: Rect,
) {
    let old = core::mem::take(&mut t.forms);
    let focus_uid = super::jsinput::focused_uid(&old, t.page.as_ref());
    let (typed, caret) = match old.focus() {
        Some((f, i)) => (String::from(old.value(f, i)), old.caret()),
        None => (String::new(), 0),
    };
    // The text of the controls with keys the page has not seen.
    let keep: Vec<(u32, String)> = match (&t.js, &t.page) {
        (Some(js), Some(p)) => js
            .inputs
            .unseen_edits()
            .into_iter()
            .filter_map(|uid| {
                field_by_uid(&p.forms, uid).map(|(f, i)| (uid, String::from(old.value(f, i))))
            })
            .collect(),
        _ => Vec::new(),
    };
    let srcs: Vec<String> = t.page.as_ref().map_or(Vec::new(), |p| {
        p.images.iter().map(|i| i.src.clone()).collect()
    });
    layout_browser(t, images, content.w, false);
    let same_pics = t.page.as_ref().is_none_or(|p| {
        p.images.len() == srcs.len() && p.images.iter().zip(&srcs).all(|(a, b)| a.src == *b)
    });
    if !same_pics {
        layout_browser(t, images, content.w, true);
    }
    if let Some(p) = &t.page {
        let mut forms = FormState::new(&p.forms);
        for (uid, text) in &keep {
            if let Some((f, i)) = field_by_uid(&p.forms, *uid) {
                forms.set_text(&p.forms, f, i, text);
            }
        }
        if let Some((f, i)) = field_by_uid(&p.forms, focus_uid) {
            forms.set_focus(&p.forms, f, i);
            if forms.value(f, i) == typed {
                forms.set_caret(caret);
            }
        }
        t.forms = forms;
    }
    let max = t.page.as_ref().map_or(0, |p| (p.height - content.h).max(0));
    clamp_scroll(t, max);
}
