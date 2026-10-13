//! The messages between the browser and the script process.
//!
//! **Browser to process**: one JSON object per message (the process parses it with the engine's own
//! `JSON.parse`).
//!
//! | `t` | fields | meaning |
//! |---|---|---|
//! | `init` | `url`, `ua`, `lang`, `ref`, `cookie`, `w`, `h`, `quirks`, `dom`, `wait` | the document, once; `wait` scripts follow |
//! | `script` | `name`, `el`, then a newline and the source ([`script_message`]) | one script of the document, in order |
//! | `click` | `uid`, `x`, `y`, `cx`, `cy`, `seq`, `n` | a click on element `uid` at page `(x, y)` (device px) and viewport `(cx, cy)` (CSS px); `seq` > 0 asks for a verdict on the default action |
//! | `key`, `focus`, `field`, `mouse`, `view`, `pop`, `hist`, `vis`, `store` | see [`super::input`] | |
//!
//! `dom` is the tree as nested arrays: an element is `[tag, uid, {attributes}, [children]]`, text is a
//! string, and whitespace the parser folded between two elements is a `" "` before the second one.
//! The scripts come after the `init`, one message each, in document order: a source goes as it is
//! (not as a JSON string), so a bundle of megabytes is neither escaped here nor parsed as JSON in the
//! process. (`jsrun` still takes the older `scripts: [[name, source], ...]` inside the `init`.)
//!
//! **Process to browser**: the first byte says what the rest is.
//!
//! | byte | rest | meaning |
//! |---|---|---|
//! | `D` | HTML | the whole document after a change (ids in `data-kjs`) |
//! | `A` | `SEQ 0` or `SEQ 1` | the verdict on click `SEQ`: `1` lets the default action happen |
//! | `N` | URL | go to this address (`location.href = ...`, `location.reload()`) |
//! | `F`, `X`, `C` | see [`super::net`] | network requests and `document.cookie` |
//! | `Q`, `G`, `W`, `S`, `H`, `B`, `O`, `L` | see [`super::input`] | inputs handled, focus, the events wanted, scroll, history, new tab, `localStorage` |
//! | `K` (page), `M`, `E`, `Z` (worker) | see [`super::worker`] | Web Workers |

use super::*;

/// What the page is shown in, for `init`.
pub struct InitInfo<'a> {
    pub url: &'a str,
    pub lang: &'a str,
    /// Viewport in CSS pixels.
    pub width: i32,
    pub height: i32,
    /// `document.cookie` when the page starts.
    pub cookie: &'a str,
    /// `document.referrer`: the `Referer` the navigation to this page sent (empty: none).
    pub referrer: &'a str,
}

/// Append `s` as a JSON string.
pub fn json_str(out: &mut String, s: &str) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 || c == '\u{2028}' || c == '\u{2029}' => {
                out.push_str(&format!("\\u{:04x}", c as u32));
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

/// The tree below `nodes` as nested arrays (see the module documentation).
fn tree_json(out: &mut String, nodes: &[Node], depth: usize) {
    out.push('[');
    let mut first = true;
    let mut sep = |out: &mut String| {
        if !core::mem::take(&mut first) {
            out.push(',');
        }
    };
    for n in nodes {
        match n {
            Node::Text(t) => {
                sep(out);
                json_str(out, t);
            }
            Node::Element(e) => {
                if e.ws_before {
                    sep(out);
                    out.push_str("\" \"");
                }
                sep(out);
                out.push('[');
                json_str(out, &e.tag);
                out.push_str(&format!(",{},{{", e.uid));
                for (i, (k, v)) in e.attrs.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    json_str(out, k);
                    out.push(':');
                    json_str(out, &decode_attr(v));
                }
                out.push_str("},");
                if depth < crate::browsing::web::MAX_DEPTH + 8 {
                    tree_json(out, &e.children, depth + 1);
                } else {
                    out.push_str("[]");
                }
                out.push(']');
            }
        }
    }
    out.push(']');
}

/// The `init` message: the document `doc` shown at `info.url`, and how many scripts follow it
/// ([`script_message`], one each).
pub fn init_message(doc: &Doc, info: &InitInfo, scripts: usize) -> Vec<u8> {
    let mut out = String::with_capacity(4096);
    out.push_str("{\"t\":\"init\",\"url\":");
    json_str(&mut out, info.url);
    out.push_str(",\"ua\":");
    json_str(&mut out, USER_AGENT);
    out.push_str(",\"lang\":");
    json_str(&mut out, info.lang);
    out.push_str(",\"ref\":");
    json_str(&mut out, info.referrer);
    out.push_str(",\"cookie\":");
    json_str(&mut out, info.cookie);
    out.push_str(&format!(
        ",\"w\":{},\"h\":{},\"quirks\":{},\"dom\":",
        info.width,
        info.height,
        doc.quirks()
    ));
    tree_json(&mut out, &doc.dom, 0);
    out.push_str(&format!(",\"wait\":{scripts}}}"));
    out.into_bytes()
}

/// Most bytes of one message to the script process (`kitsune_userabi::MAX_PAGE_MSG`).
pub const MAX_MESSAGE: usize = 4 * 1024 * 1024;

/// One script of the page, after the `init`: `{"t":"script","name":...,"el":N}`, a newline and the
/// source as it is. `None` when it would not fit in one message ([`MAX_MESSAGE`]).
pub fn script_message(s: &PageScript) -> Option<Vec<u8>> {
    let mut head = String::from("{\"t\":\"script\",\"name\":");
    json_str(&mut head, &s.name);
    head.push_str(&format!(",\"el\":{}}}\n", s.elem));
    let len = head.len() + s.code.len();
    if len > MAX_MESSAGE {
        return None;
    }
    let mut out = Vec::with_capacity(len);
    out.extend_from_slice(head.as_bytes());
    out.extend_from_slice(s.code.as_bytes());
    Some(out)
}

/// Where a click happened: page coordinates in device pixels, viewport coordinates in CSS pixels.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ClickAt {
    pub x: i32,
    pub y: i32,
    pub cx: i32,
    pub cy: i32,
}

/// A click on element `uid` (input number `n`); `seq` > 0 asks for a verdict.
pub fn click_message(uid: u32, at: ClickAt, seq: u32, n: u32) -> Vec<u8> {
    format!(
        "{{\"t\":\"click\",\"uid\":{uid},\"x\":{},\"y\":{},\"cx\":{},\"cy\":{},\"seq\":{seq},\"n\":{n}}}",
        at.x, at.y, at.cx, at.cy
    )
    .into_bytes()
}

/// A message from the script process.
#[derive(Debug, PartialEq, Eq)]
pub enum FromScript<'a> {
    /// The whole document, as HTML.
    Document(&'a [u8]),
    /// The verdict on click `seq`: may the default action happen?
    Answer { seq: u32, allow: bool },
    /// Go to this address.
    Navigate(&'a str),
    /// A network request (`fetch`, `XMLHttpRequest`, `sendBeacon`).
    Fetch(FetchRequest),
    /// Abort request `id`.
    Abort(u32),
    /// `document.cookie = text`.
    SetCookie(&'a str),
    /// The inputs up to this number are in the documents that follow (`Q`).
    Ack(u32),
    /// A script focused control `uid` (0: no control has the focus) (`G`).
    Focus(u32),
    /// The kinds of events the page listens to (`W`).
    Want(Subs),
    /// Scroll the page to `(x, y)`, CSS px (`S`).
    Scroll(i32, i32),
    /// `history.pushState` (`replace` false) or `replaceState` to this address (`H`).
    History { replace: bool, url: &'a str },
    /// `history.go(n)` (`B`).
    Go(i32),
    /// `window.open(url)`: a new tab (`O`).
    Open(&'a str),
    /// A change to the origin's `localStorage` (`L`).
    Storage(StorageOp),
    /// About the page's workers (`K`, [`super::worker`]).
    Worker(WorkerOp<'a>),
    /// From a worker: `postMessage(data)` for its page (`M`).
    ToPage(&'a str),
    /// From a worker: its script failed (`E`).
    WorkerFailed(&'a str),
    /// From a worker: `close()` (`Z`).
    WorkerClose,
    /// A command for a media element's player (`play()`, `currentTime = t`, ...).
    Media(crate::browsing::media::wire::Command),
    /// WebM bytes for source buffer `src` of player `id` (append number `seq`).
    MediaData {
        id: u32,
        src: u8,
        seq: u32,
        bytes: &'a [u8],
    },
    /// Anything else (ignored).
    Unknown,
}

/// Read a message from the script process (never trusted: anything malformed is `Unknown`).
pub fn parse_message(m: &[u8]) -> FromScript<'_> {
    let Some((&kind, rest)) = m.split_first() else {
        return FromScript::Unknown;
    };
    match kind {
        b'D' => FromScript::Document(rest),
        b'A' => {
            let Ok(text) = core::str::from_utf8(rest) else {
                return FromScript::Unknown;
            };
            let mut it = text.split_ascii_whitespace();
            match (
                it.next().and_then(|s| s.parse::<u32>().ok()),
                it.next(),
                it.next(),
            ) {
                (Some(seq), Some(v @ ("0" | "1")), None) => FromScript::Answer {
                    seq,
                    allow: v == "1",
                },
                _ => FromScript::Unknown,
            }
        }
        b'N' => match core::str::from_utf8(rest) {
            Ok(url) if !url.trim().is_empty() && url.len() <= 2048 => {
                FromScript::Navigate(url.trim())
            }
            _ => FromScript::Unknown,
        },
        b'F' => parse_fetch(rest).map_or(FromScript::Unknown, FromScript::Fetch),
        b'X' => core::str::from_utf8(rest)
            .ok()
            .and_then(|s| s.trim().parse().ok())
            .map_or(FromScript::Unknown, FromScript::Abort),
        b'C' => match core::str::from_utf8(rest) {
            Ok(text) if text.len() <= 8192 => FromScript::SetCookie(text),
            _ => FromScript::Unknown,
        },
        b'Q' | b'G' | b'B' => {
            let n = core::str::from_utf8(rest).ok().map(str::trim);
            match (kind, n) {
                (b'Q', Some(n)) => n.parse().map_or(FromScript::Unknown, FromScript::Ack),
                (b'G', Some(n)) => n.parse().map_or(FromScript::Unknown, FromScript::Focus),
                (_, Some(n)) => n
                    .parse::<i32>()
                    .ok()
                    .filter(|n| n.unsigned_abs() <= 64)
                    .map_or(FromScript::Unknown, FromScript::Go),
                _ => FromScript::Unknown,
            }
        }
        b'W' => core::str::from_utf8(rest)
            .ok()
            .filter(|s| s.len() <= 256)
            .map_or(FromScript::Unknown, |s| FromScript::Want(Subs::parse(s))),
        b'S' => parse_scroll(rest).map_or(FromScript::Unknown, |(x, y)| FromScript::Scroll(x, y)),
        b'H' => parse_history(rest).map_or(FromScript::Unknown, |(replace, url)| {
            FromScript::History { replace, url }
        }),
        b'O' => match core::str::from_utf8(rest) {
            Ok(url) if !url.trim().is_empty() && url.len() <= 2048 => FromScript::Open(url.trim()),
            _ => FromScript::Unknown,
        },
        b'L' if rest.len() <= 2 * ORIGIN_BYTES => {
            parse_storage_op(rest).map_or(FromScript::Unknown, FromScript::Storage)
        }
        b'K' => parse_worker_op(rest).map_or(FromScript::Unknown, FromScript::Worker),
        b'M' => core::str::from_utf8(rest).map_or(FromScript::Unknown, FromScript::ToPage),
        b'E' => match core::str::from_utf8(rest) {
            Ok(text) if text.len() <= 4096 => FromScript::WorkerFailed(text),
            _ => FromScript::Unknown,
        },
        b'Z' if rest.is_empty() => FromScript::WorkerClose,
        b'V' => crate::browsing::media::wire::parse_control(rest)
            .map_or(FromScript::Unknown, FromScript::Media),
        b'v' => match crate::browsing::media::wire::parse_data(rest) {
            Some((id, src, seq, bytes)) => FromScript::MediaData {
                id,
                src,
                seq,
                bytes,
            },
            None => FromScript::Unknown,
        },
        _ => FromScript::Unknown,
    }
}
