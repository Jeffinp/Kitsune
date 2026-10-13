//! What the user does, for the script process: keys, focus, the controls' changes, the mouse, the
//! viewport and the history (browser to process), and what the page asks of the browser in return
//! (process to browser). The process's side is `user-apps/jsrun/src/bin/jsrun/events.js` (input),
//! `layoutapi.js` (viewport, scroll), `history.js` and `storage.js`.
//!
//! **Browser to process** (JSON, see the table in [`super::wire`]): every input carries `n`, a number
//! that grows with each input sent; the process answers `Q<n>` before the document that reflects the
//! inputs up to `n`, so the browser knows whether a document it receives already saw what was typed
//! (see [`Inputs`]).
//!
//! | `t` | fields | meaning |
//! |---|---|---|
//! | `key` | `k`, `c`, `kc`, `m`, `uid`, `v`?, `seq`, `n` | a key (`k` the DOM `key`, `c` the `code`, `kc` the `keyCode`, `m` the modifiers: 1 Shift, 2 Ctrl, 4 Alt, 8 Meta) for control `uid` (0: the page); `v` is the control's text after the key when the key changed it; `seq` > 0 asks for a verdict (Enter in a form: may it be sent?) |
//! | `focus` | `uid`, `n` | the browser moved the focus to control `uid` (0: none) |
//! | `field` | `uid`, `v` / `ck` / `si`, `n` | control `uid` changed without a key reaching the page: text `v`, check box `ck` (0/1), `<select>` option `si` |
//! | `mouse` | `e`, `uid`, `x`, `y`, `b`, `dy`, `m`, `n` | `move` (only when the page listens: [`Subs::MOVE`]), `dbl`, `ctx`, `wheel` (`dy` pixels) over element `uid` at viewport `(x, y)` |
//! | `view` | `w`, `h`, `sx`, `sy`, `pw`, `ph`, `z` | viewport size, scroll and page size in CSS px, zoom in percent |
//! | `pop` | `url`, `i`, `len` | back/forward reached entry `i` of this document (`popstate`) |
//! | `hist` | `i`, `len` | where the page is in the tab's history |
//! | `vis` | `v` | the tab is shown (`true`) or hidden |
//! | `store` | `items` | the origin's `localStorage`, once after `init` |
//!
//! **Process to browser** (first byte): `Q<n>` (inputs handled), `G<uid>` (a script focused a control;
//! 0 = none), `W<kinds>` ([`Subs`]), `S<x> <y>` (scroll the page to), `H<push|replace> <url>`
//! (`pushState`/`replaceState`), `B<n>` (`history.go(n)`), `O<url>` (`window.open`), `L<json>` (a
//! `localStorage` change, [`super::StorageOp`]).

use super::*;

/// Modifier bits of a key or mouse message.
pub const MOD_SHIFT: u8 = 1;
pub const MOD_CTRL: u8 = 2;
pub const MOD_ALT: u8 = 4;
pub const MOD_META: u8 = 8;

/// The kinds of events the browser sends only when the page listens to them (`W` message).
#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
pub struct Subs(pub u8);

impl Subs {
    /// `mousemove`, `mouseover`/`out`/`enter`/`leave` and the pointer ones.
    pub const MOVE: u8 = 1;
    /// Keys when no control has the focus.
    pub const KEY: u8 = 2;
    pub const WHEEL: u8 = 4;
    /// `scroll` (and the observers that follow the scroll).
    pub const SCROLL: u8 = 8;
    /// `contextmenu`.
    pub const CTX: u8 = 16;
    pub const DBL: u8 = 32;

    /// Read the list of a `W` message (`move,key,...`; unknown names are ignored).
    pub fn parse(list: &str) -> Subs {
        let mut bits = 0;
        for k in list.split(',') {
            bits |= match k.trim() {
                "move" => Self::MOVE,
                "key" => Self::KEY,
                "wheel" => Self::WHEEL,
                "scroll" => Self::SCROLL,
                "ctx" => Self::CTX,
                "dbl" => Self::DBL,
                _ => 0,
            };
        }
        Subs(bits)
    }

    pub fn has(self, bit: u8) -> bool {
        self.0 & bit != 0
    }
}

/// The inputs sent to a page and how far its documents have caught up with them.
#[derive(Clone, Default, Debug)]
pub struct Inputs {
    sent: u32,
    acked: u32,
    /// `(control uid, input number)` of the keys that changed a control's text, until the page says
    /// it saw them.
    edits: Vec<(u32, u32)>,
}

impl Inputs {
    /// The number of the next input.
    pub fn next_number(&mut self) -> u32 {
        self.sent = self.sent.wrapping_add(1).max(1);
        self.sent
    }

    /// The process said it handled the inputs up to `n` (`Q`).
    pub fn ack(&mut self, n: u32) {
        if n <= self.sent && n > self.acked {
            self.acked = n;
            let acked = self.acked;
            self.edits.retain(|e| e.1 > acked);
        }
    }

    /// Input `n` changed the text of control `uid` (a key typed into it).
    pub fn edited(&mut self, uid: u32, n: u32) {
        if uid != 0 && self.edits.len() < 4096 {
            self.edits.push((uid, n));
        }
    }

    /// The controls whose text the person changed and the page has not seen yet: a document from the
    /// page lacks those changes, so the browser keeps its own text for them (the page's next document
    /// carries it).
    pub fn unseen_edits(&self) -> Vec<u32> {
        let mut v: Vec<u32> = self.edits.iter().map(|e| e.0).collect();
        v.sort_unstable();
        v.dedup();
        v
    }

    /// Has the page seen every input sent? When not, a document it sends now may lack what the user
    /// just typed: the browser keeps its own copy of the focused control.
    pub fn settled(&self) -> bool {
        self.acked >= self.sent
    }
}

/// A key for the page.
#[derive(Clone, Debug, Default)]
pub struct KeyInfo<'a> {
    /// The DOM `key` (`a`, `Enter`, `ArrowLeft`...), `code` and `keyCode`.
    pub key: &'a str,
    pub code: &'a str,
    pub key_code: u32,
    pub mods: u8,
    /// The focused control (0: none).
    pub uid: u32,
    /// Its text after the key, when the key changed it.
    pub value: Option<&'a str>,
    /// > 0: the page decides whether the key's default action happens (Enter submitting a form).
    pub seq: u32,
}

/// The DOM `key`, `code` and `keyCode` of a key of the keyboard model (`shift` for the letters' code).
pub fn dom_key(k: crate::Key) -> (String, &'static str, u32) {
    use crate::Key as K;
    let named = |key: &str, code: &'static str, kc: u32| (String::from(key), code, kc);
    match k {
        K::Char(b) => {
            let c = b as char;
            let code: &'static str = match b.to_ascii_lowercase() {
                b'a'..=b'z' => LETTER_CODES[usize::from(b.to_ascii_lowercase() - b'a')],
                b'0'..=b'9' => DIGIT_CODES[usize::from(b - b'0')],
                b' ' => "Space",
                b'-' | b'_' => "Minus",
                b'=' | b'+' => "Equal",
                b'[' | b'{' => "BracketLeft",
                b']' | b'}' => "BracketRight",
                b';' | b':' => "Semicolon",
                b'\'' | b'"' => "Quote",
                b',' | b'<' => "Comma",
                b'.' | b'>' => "Period",
                b'/' | b'?' => "Slash",
                b'\\' | b'|' => "Backslash",
                b'`' | b'~' => "Backquote",
                b'!' => "Digit1",
                b'@' => "Digit2",
                b'#' => "Digit3",
                b'$' => "Digit4",
                b'%' => "Digit5",
                b'^' => "Digit6",
                b'&' => "Digit7",
                b'*' => "Digit8",
                b'(' => "Digit9",
                b')' => "Digit0",
                _ => "",
            };
            let kc = if b.is_ascii_alphanumeric() {
                u32::from(b.to_ascii_uppercase())
            } else if b == b' ' {
                32
            } else {
                0
            };
            (String::from(c), code, kc)
        }
        K::Enter => named("Enter", "Enter", 13),
        K::Backspace => named("Backspace", "Backspace", 8),
        K::Tab => named("Tab", "Tab", 9),
        K::Esc => named("Escape", "Escape", 27),
        K::Delete => named("Delete", "Delete", 46),
        K::Left => named("ArrowLeft", "ArrowLeft", 37),
        K::Right => named("ArrowRight", "ArrowRight", 39),
        K::Up => named("ArrowUp", "ArrowUp", 38),
        K::Down => named("ArrowDown", "ArrowDown", 40),
        K::Home => named("Home", "Home", 36),
        K::End => named("End", "End", 35),
        K::PageUp => named("PageUp", "PageUp", 33),
        K::PageDown => named("PageDown", "PageDown", 34),
    }
}

const LETTER_CODES: [&str; 26] = [
    "KeyA", "KeyB", "KeyC", "KeyD", "KeyE", "KeyF", "KeyG", "KeyH", "KeyI", "KeyJ", "KeyK", "KeyL",
    "KeyM", "KeyN", "KeyO", "KeyP", "KeyQ", "KeyR", "KeyS", "KeyT", "KeyU", "KeyV", "KeyW", "KeyX",
    "KeyY", "KeyZ",
];
const DIGIT_CODES: [&str; 10] = [
    "Digit0", "Digit1", "Digit2", "Digit3", "Digit4", "Digit5", "Digit6", "Digit7", "Digit8",
    "Digit9",
];

/// The `key` message (input number `n`).
pub fn key_message(k: &KeyInfo, n: u32) -> Vec<u8> {
    let mut out = String::from("{\"t\":\"key\",\"k\":");
    json_str(&mut out, k.key);
    out.push_str(",\"c\":");
    json_str(&mut out, k.code);
    out.push_str(&format!(
        ",\"kc\":{},\"m\":{},\"uid\":{},\"seq\":{},\"n\":{n}",
        k.key_code, k.mods, k.uid, k.seq
    ));
    if let Some(v) = k.value {
        out.push_str(",\"v\":");
        json_str(&mut out, v);
    }
    out.push('}');
    out.into_bytes()
}

/// The `focus` message: the browser's focus is on control `uid` (0: none).
pub fn focus_message(uid: u32, n: u32) -> Vec<u8> {
    format!("{{\"t\":\"focus\",\"uid\":{uid},\"n\":{n}}}").into_bytes()
}

/// How a control changed without a key reaching the page.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FieldChange<'a> {
    Text(&'a str),
    Checked(bool),
    Selected(usize),
}

/// The `field` message for control `uid`.
pub fn field_message(uid: u32, change: FieldChange, n: u32) -> Vec<u8> {
    let mut out = format!("{{\"t\":\"field\",\"uid\":{uid},\"n\":{n},");
    match change {
        FieldChange::Text(v) => {
            out.push_str("\"v\":");
            json_str(&mut out, v);
        }
        FieldChange::Checked(on) => out.push_str(&format!("\"ck\":{}", u8::from(on))),
        FieldChange::Selected(i) => out.push_str(&format!("\"si\":{i}")),
    }
    out.push('}');
    out.into_bytes()
}

/// What the mouse did, for a `mouse` message.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MouseKind {
    Move,
    Double,
    Context,
    /// The wheel, by this many pixels (positive: down).
    Wheel(i32),
}

/// The `mouse` message: over element `uid` at viewport `(x, y)` (CSS px).
pub fn mouse_message(kind: MouseKind, uid: u32, x: i32, y: i32, mods: u8, n: u32) -> Vec<u8> {
    let (e, b, dy) = match kind {
        MouseKind::Move => ("move", 0, 0),
        MouseKind::Double => ("dbl", 0, 0),
        MouseKind::Context => ("ctx", 2, 0),
        MouseKind::Wheel(d) => ("wheel", 0, d),
    };
    format!(
        "{{\"t\":\"mouse\",\"e\":\"{e}\",\"uid\":{uid},\"x\":{x},\"y\":{y},\"b\":{b},\"dy\":{dy},\"m\":{mods},\"n\":{n}}}"
    )
    .into_bytes()
}

/// The viewport, the scroll and the page, in CSS px.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct View {
    pub w: i32,
    pub h: i32,
    pub sx: i32,
    pub sy: i32,
    pub page_w: i32,
    pub page_h: i32,
    /// Zoom in percent.
    pub zoom: u16,
}

impl View {
    /// The view of a page laid out at `zoom` percent, from device pixels.
    pub fn from_device(w: i32, h: i32, scroll: i32, page_h: i32, zoom: u16) -> View {
        let z = i32::from(zoom.max(1));
        let css = |v: i32| (i64::from(v) * 100 / i64::from(z)) as i32;
        View {
            w: css(w),
            h: css(h),
            sx: 0,
            sy: css(scroll),
            page_w: css(w),
            page_h: css(page_h),
            zoom: zoom.max(1),
        }
    }
}

/// The `view` message.
pub fn view_message(v: &View) -> Vec<u8> {
    format!(
        "{{\"t\":\"view\",\"w\":{},\"h\":{},\"sx\":{},\"sy\":{},\"pw\":{},\"ph\":{},\"z\":{}}}",
        v.w, v.h, v.sx, v.sy, v.page_w, v.page_h, v.zoom
    )
    .into_bytes()
}

/// The `pop` message: back/forward reached entry `index` (of `len`) of this document, at `url`.
pub fn pop_message(url: &str, index: usize, len: usize) -> Vec<u8> {
    let mut out = String::from("{\"t\":\"pop\",\"url\":");
    json_str(&mut out, url);
    out.push_str(&format!(",\"i\":{index},\"len\":{len}}}"));
    out.into_bytes()
}

/// The `hist` message: entry `index` of `len`.
pub fn hist_message(index: usize, len: usize) -> Vec<u8> {
    format!("{{\"t\":\"hist\",\"i\":{index},\"len\":{len}}}").into_bytes()
}

/// The `vis` message.
pub fn vis_message(visible: bool) -> Vec<u8> {
    format!("{{\"t\":\"vis\",\"v\":{visible}}}").into_bytes()
}

/// The `store` message: the origin's `localStorage` items.
pub fn store_message<'a>(items: impl Iterator<Item = (&'a str, &'a str)>) -> Vec<u8> {
    let mut out = String::from("{\"t\":\"store\",\"items\":{");
    for (i, (k, v)) in items.enumerate() {
        if i > 0 {
            out.push(',');
        }
        json_str(&mut out, k);
        out.push(':');
        json_str(&mut out, v);
    }
    out.push_str("}}");
    out.into_bytes()
}

/// `S<x> <y>`: where the page asks to be scrolled to (CSS px).
pub(super) fn parse_scroll(rest: &[u8]) -> Option<(i32, i32)> {
    let text = core::str::from_utf8(rest).ok()?;
    let mut it = text.split_ascii_whitespace();
    let x = it.next()?.parse::<i32>().ok()?;
    let y = it.next()?.parse::<i32>().ok()?;
    it.next().is_none().then_some((x, y))
}

/// `H<push|replace> <url>`.
pub(super) fn parse_history(rest: &[u8]) -> Option<(bool, &str)> {
    let text = core::str::from_utf8(rest).ok()?;
    let (op, url) = text.split_once(' ')?;
    let url = url.trim();
    if url.is_empty() || url.len() > 2048 {
        return None;
    }
    match op {
        "push" => Some((false, url)),
        "replace" => Some((true, url)),
        _ => None,
    }
}
