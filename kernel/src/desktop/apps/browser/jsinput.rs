//! What the user does on a page with scripts, for its script process: keys (with the control's text
//! after the key), the focus, controls changed by a click, the mouse (moves only while the page
//! listens), and, every frame, what the page should know: the layout snapshot after a layout, the
//! viewport and the scroll, back/forward inside the document, whether the tab is shown.
//!
//! The browser keeps drawing and editing the controls itself (no wait for the process); the page gets
//! the result and the document it sends back carries the controls' state, which the browser takes
//! (`script::relayout_js`). The messages are `kitsune_core::web::script::input`.

use super::script::{JsAction, JsProc};
use crate::desktop::*;
use kitsune_core::web::form::{FieldKind, FormInfo, FormState};
use kitsune_core::web::script::{
    FieldChange, KeyInfo, MOD_SHIFT, MouseKind, Subs, View, boxes_message, dom_key, field_message,
    focus_message, hist_message, key_message, mouse_message, pop_message, view_message,
    vis_message,
};

impl JsProc {
    /// Give the process `msg` (dropped when it is gone or flooded).
    pub(super) fn post(&self, msg: Vec<u8>) -> bool {
        crate::userproc::page_post(self.pid, msg)
    }
}

/// The element of the control the browser's focus is on (0: none).
pub(super) fn focused_uid(forms: &FormState, page: Option<&kitsune_core::web::Page>) -> u32 {
    let (Some((f, i)), Some(p)) = (forms.focus(), page) else {
        return 0;
    };
    p.forms
        .get(f)
        .and_then(|x| x.fields.get(i))
        .map_or(0, |x| x.uid)
}

/// Tell the page where the browser's focus is, if it moved since it was last told.
pub(super) fn sync_focus(t: &mut TabData) {
    let uid = focused_uid(&t.forms, t.page.as_ref());
    let Some(js) = t.js.as_mut() else { return };
    if uid != js.focus_uid {
        js.focus_uid = uid;
        let n = js.inputs.next_number();
        js.post(focus_message(uid, n));
    }
}

/// The focused control before a key: which it is and its state.
pub(super) struct Before {
    uid: u32,
    at: Option<(usize, usize)>,
    kind: Option<FieldKind>,
    text: String,
    checked: bool,
    select: usize,
}

impl Before {
    pub(super) fn take(t: &TabData) -> Before {
        let at = t.forms.focus();
        let fi = at.and_then(|(f, i)| t.page.as_ref()?.forms.get(f)?.fields.get(i));
        let (f, i) = at.unwrap_or((0, 0));
        Before {
            uid: fi.map_or(0, |x| x.uid),
            at,
            kind: fi.map(|x| x.kind),
            text: String::from(t.forms.value(f, i)),
            checked: t.forms.is_checked(f, i),
            select: t.forms.select_index(f, i),
        }
    }
}

/// A key the browser handled for the focused control (`before` was taken before it): the page gets
/// it with the control's new text, and a check box or a `<select>` it changed. `action` (Enter in a
/// form) waits for the page's verdict.
pub(super) fn after_key(
    t: &mut TabData,
    key: Key,
    shift: bool,
    before: &Before,
    action: Option<JsAction>,
) {
    let Some(js) = t.js.as_mut() else { return };
    let (k, code, kc) = dom_key(key);
    let (f, i) = before.at.unwrap_or((0, 0));
    let now_text = t.forms.value(f, i);
    let value = (before.kind.is_some_and(FieldKind::is_text) && now_text != before.text)
        .then_some(now_text);
    let seq = js.session.wait(action, now_ms());
    let n = js.inputs.next_number();
    if value.is_some() {
        js.inputs.edited(before.uid, n);
    }
    let info = KeyInfo {
        key: &k,
        code,
        key_code: kc,
        mods: if shift { MOD_SHIFT } else { 0 },
        uid: before.uid,
        value,
        seq,
    };
    js.post(key_message(&info, n));
    match before.kind {
        Some(FieldKind::Checkbox | FieldKind::Radio)
            if t.forms.is_checked(f, i) != before.checked =>
        {
            let n = js.inputs.next_number();
            js.post(field_message(
                before.uid,
                FieldChange::Checked(t.forms.is_checked(f, i)),
                n,
            ));
        }
        Some(FieldKind::Select) if t.forms.select_index(f, i) != before.select => {
            let n = js.inputs.next_number();
            js.post(field_message(
                before.uid,
                FieldChange::Selected(t.forms.select_index(f, i)),
                n,
            ));
        }
        _ => {}
    }
    sync_focus(t);
}

/// A key on the page when no control has the focus, for a page that listens to keys.
pub(super) fn page_key(t: &mut TabData, key: Key, shift: bool) -> bool {
    let Some(js) = t.js.as_mut() else {
        return false;
    };
    if !js.subs.has(Subs::KEY) {
        return false;
    }
    let (k, code, kc) = dom_key(key);
    let n = js.inputs.next_number();
    let info = KeyInfo {
        key: &k,
        code,
        key_code: kc,
        mods: if shift { MOD_SHIFT } else { 0 },
        ..KeyInfo::default()
    };
    js.post(key_message(&info, n));
    true
}

/// After a click changed a `<select>` (the browser steps it: there is no drop-down list).
pub(super) fn select_changed(t: &mut TabData, forms: &[FormInfo], form: usize, field: usize) {
    let Some(js) = t.js.as_mut() else { return };
    let uid = forms
        .get(form)
        .and_then(|f| f.fields.get(field))
        .map_or(0, |f| f.uid);
    if uid != 0 {
        let n = js.inputs.next_number();
        js.post(field_message(
            uid,
            FieldChange::Selected(t.forms.select_index(form, field)),
            n,
        ));
    }
}

fn now_ms() -> u64 {
    u64::from(crate::desktop::shell::toasts::now_ms())
}

/// CSS px from device px at `zoom` percent.
pub(super) fn css(v: i32, zoom: u16) -> i32 {
    (i64::from(v) * 100 / i64::from(zoom.max(1))) as i32
}

/// Every frame, for a tab with a script process: the layout snapshot after a layout, the viewport
/// and the scroll when they changed, the pointer's last move, back/forward inside the document, and
/// whether the tab is shown.
pub(super) fn frame(t: &mut TabData, content: Rect, active: bool) {
    let pop = t.browser.take_pop();
    let hist_len = t.browser.history_len();
    let Some(js) = t.js.as_mut() else { return };
    if let Some((url, i)) = pop {
        js.post(pop_message(&String::from_utf8_lossy(&url), i, hist_len));
    }
    if js.visible != active {
        js.visible = active;
        js.post(vis_message(active));
    }
    let (Some(page), true) = (t.page.as_ref(), active) else {
        return;
    };
    let view = View::from_device(content.w, content.h, t.scroll, page.height, t.zoom);
    if js.boxes_due {
        js.boxes_due = false;
        js.view = view;
        let t0 = crate::trace::t();
        let m = boxes_message(page, &view);
        crate::trace::note("js-boxes", t0, m.len() as u64);
        js.post(m);
    } else if view != js.view {
        // While the page scrolls: every frame for a page that follows the scroll, else once it rests.
        let resting = t.scroll == t.scroll_spring.target() as i32;
        let resized = (view.w, view.h, view.zoom) != (js.view.w, js.view.h, js.view.zoom);
        if resized || resting || js.subs.has(Subs::SCROLL) {
            js.view = view;
            js.post(view_message(&view));
        }
    }
    if let Some((qx, qy)) = js.pending_move.take() {
        let uid = page.node_at(qx, qy).unwrap_or(0);
        let n = js.inputs.next_number();
        let (x, y) = (css(qx, t.zoom), css(qy - t.scroll, t.zoom));
        js.post(mouse_message(MouseKind::Move, uid, x, y, 0, n));
    }
}

/// Right after `init`: where the page is in the history.
pub(super) fn after_init(t: &TabData) {
    if let Some(js) = &t.js {
        js.post(hist_message(
            t.browser.history_index(),
            t.browser.history_len(),
        ));
    }
}

impl Desktop {
    /// The pointer moved to `(cx, cy)`: a page that listens to the mouse gets the move (one per
    /// frame at most, see [`frame`]).
    pub(crate) fn browser_js_pointer(&mut self, cx: i32, cy: i32) {
        let Some(id) = self.browser_id() else { return };
        if self.drag.is_some() || self.topmost_at(cx, cy) != Some(id) {
            return;
        }
        let Some(rect) = self.browser_win_rect(id) else {
            return;
        };
        let Some(b) = self.browser_state_mut(id) else {
            return;
        };
        let content = b.chrome(rect).content;
        let t = b.tabs.active_mut();
        let scroll = t.scroll;
        if let Some(js) = t.js.as_mut()
            && js.subs.has(Subs::MOVE)
            && content.contains(cx, cy)
        {
            js.pending_move = Some((cx - content.x, cy - content.y + scroll));
        }
    }

    /// A double click, a right click or the wheel over the page of window `id` at `(px, py)`, for a
    /// page that listens to it.
    pub(crate) fn browser_js_mouse(&mut self, id: WindowId, px: i32, py: i32, kind: MouseKind) {
        let Some(rect) = self.browser_win_rect(id) else {
            return;
        };
        let Some(b) = self.browser_state_mut(id) else {
            return;
        };
        let content = b.chrome(rect).content;
        if !content.contains(px, py) {
            return;
        }
        let t = b.tabs.active_mut();
        let (qx, qy) = (px - content.x, py - content.y + t.scroll);
        let uid = t.page.as_ref().and_then(|p| p.node_at(qx, qy)).unwrap_or(0);
        let (x, y) = (css(qx, t.zoom), css(qy - t.scroll, t.zoom));
        let Some(js) = t.js.as_mut() else { return };
        let bit = match kind {
            MouseKind::Double => Subs::DBL,
            MouseKind::Context => Subs::CTX,
            MouseKind::Wheel(_) => Subs::WHEEL,
            MouseKind::Move => Subs::MOVE,
        };
        if js.subs.has(bit) {
            let n = js.inputs.next_number();
            js.post(mouse_message(kind, uid, x, y, 0, n));
        }
    }
}

/// What the page asked the browser through its scroll (`S` is in CSS px).
pub(super) fn scroll_target(zoom: u16, y_css: i32) -> i32 {
    (i64::from(y_css) * i64::from(zoom.max(1)) / 100).clamp(0, i64::from(i32::MAX / 2)) as i32
}
