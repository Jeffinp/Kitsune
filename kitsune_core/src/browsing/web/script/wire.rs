//! The messages between the browser and the script process.
//!
//! **Browser to process**: one JSON object per message (the process parses it with the engine's own
//! `JSON.parse`).
//!
//! | `t` | fields | meaning |
//! |---|---|---|
//! | `init` | `url`, `ua`, `lang`, `cookie`, `w`, `h`, `quirks`, `dom`, `scripts` | the document and its scripts, once |
//! | `click` | `uid`, `x`, `y`, `seq` | a click on element `uid`; `seq` > 0 asks for a verdict on the default action |
//!
//! `dom` is the tree as nested arrays: an element is `[tag, uid, {attributes}, [children]]`, text is a
//! string, and whitespace the parser folded between two elements is a `" "` before the second one.
//! `scripts` is `[[name, source], ...]` in document order.
//!
//! **Process to browser**: the first byte says what the rest is.
//!
//! | byte | rest | meaning |
//! |---|---|---|
//! | `D` | HTML | the whole document after a change (ids in `data-kjs`) |
//! | `A` | `SEQ 0` or `SEQ 1` | the verdict on click `SEQ`: `1` lets the default action happen |
//! | `N` | URL | go to this address (`location.href = ...`, `location.reload()`) |
//! | `F`, `X`, `C` | see [`super::net`] | network requests and `document.cookie` |

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

/// The `init` message: the document `doc` shown at `info.url` and its scripts.
pub fn init_message(doc: &Doc, info: &InitInfo, scripts: &[(String, String)]) -> Vec<u8> {
    let mut out = String::with_capacity(4096 + scripts.iter().map(|s| s.1.len()).sum::<usize>());
    out.push_str("{\"t\":\"init\",\"url\":");
    json_str(&mut out, info.url);
    out.push_str(",\"ua\":");
    json_str(&mut out, USER_AGENT);
    out.push_str(",\"lang\":");
    json_str(&mut out, info.lang);
    out.push_str(",\"cookie\":");
    json_str(&mut out, info.cookie);
    out.push_str(&format!(
        ",\"w\":{},\"h\":{},\"quirks\":{},\"dom\":",
        info.width,
        info.height,
        doc.quirks()
    ));
    tree_json(&mut out, &doc.dom, 0);
    out.push_str(",\"scripts\":[");
    for (i, (name, code)) in scripts.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        out.push('[');
        json_str(&mut out, name);
        out.push(',');
        json_str(&mut out, code);
        out.push(']');
    }
    out.push_str("]}");
    out.into_bytes()
}

/// A click on element `uid` at page `(x, y)` (CSS pixels); `seq` > 0 asks for a verdict.
pub fn click_message(uid: u32, x: i32, y: i32, seq: u32) -> Vec<u8> {
    format!("{{\"t\":\"click\",\"uid\":{uid},\"x\":{x},\"y\":{y},\"seq\":{seq}}}").into_bytes()
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
        b'M' => crate::browsing::media::wire::parse_control(rest)
            .map_or(FromScript::Unknown, FromScript::Media),
        b'm' => match crate::browsing::media::wire::parse_data(rest) {
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
