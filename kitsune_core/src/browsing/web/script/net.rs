//! The network messages between the script process and the browser (`fetch`, `XMLHttpRequest`,
//! `document.cookie`). See `docs/design/tls-browser.md` §13.
//!
//! **Process to browser** (after the kind byte):
//!
//! | byte | rest | meaning |
//! |---|---|---|
//! | `F` | lines `id`, `METHOD`, `url`, `mode`, `credentials`, `redirect`, then `Name: value` header lines, an empty line and the body | a request ([`parse_fetch`]) |
//! | `X` | `id` | abort request `id` |
//! | `C` | text | `document.cookie = text` |
//!
//! **Browser to process**: JSON for the head and the end of an answer ([`head_message`],
//! [`end_message`], [`error_message`], [`cookie_message`]) and a binary message for body bytes
//! ([`data_message`]: byte `0x01`, the id as 4 bytes little endian, the bytes), so a 10 MiB video
//! segment is not turned into text.

use super::*;
use crate::browsing::cors::{Credentials, Mode, is_forbidden_header};
use crate::browsing::http1::{is_header_value, is_token};

/// The first byte of a binary message (body bytes of an answer).
pub const DATA_TAG: u8 = 1;
/// Most headers a script's request keeps.
pub const MAX_FETCH_HEADERS: usize = 64;

/// What `redirect:` asks for.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Redirect {
    Follow,
    Error,
    Manual,
}

/// A request from a page's script.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct FetchRequest {
    /// The script's own number for it (answers carry it back).
    pub id: u32,
    /// Upper case for the standard methods.
    pub method: String,
    pub url: String,
    pub mode: Mode,
    pub credentials: Credentials,
    pub redirect: Redirect,
    /// Valid names and values; headers a script may not set are dropped.
    pub headers: Vec<(String, String)>,
    pub body: Option<Vec<u8>>,
}

/// Read an `F` message (`m` without the kind byte). `None` when it is malformed, the method is not a
/// token or is `CONNECT`/`TRACE`/`TRACK`, or the address is longer than 8 KiB.
pub fn parse_fetch(m: &[u8]) -> Option<FetchRequest> {
    let mut at = 0;
    let mut line = || -> Option<&[u8]> {
        let rest = m.get(at..)?;
        let n = rest.iter().position(|&b| b == b'\n')?;
        at += n + 1;
        Some(&rest[..n])
    };
    let text = |b: &[u8]| core::str::from_utf8(b).ok().map(String::from);
    let id: u32 = core::str::from_utf8(line()?).ok()?.parse().ok()?;
    let mut method = text(line()?)?;
    let url = text(line()?)?;
    let mode = match line()? {
        b"cors" | b"" => Mode::Cors,
        b"no-cors" => Mode::NoCors,
        b"same-origin" | b"navigate" => Mode::SameOrigin,
        _ => return None,
    };
    let credentials = match line()? {
        b"omit" => Credentials::Omit,
        b"same-origin" | b"" => Credentials::SameOrigin,
        b"include" => Credentials::Include,
        _ => return None,
    };
    let redirect = match line()? {
        b"follow" | b"" => Redirect::Follow,
        b"error" => Redirect::Error,
        b"manual" => Redirect::Manual,
        _ => return None,
    };
    let has_body = line()? == b"1";
    let mut headers = Vec::new();
    loop {
        let l = line()?;
        if l.is_empty() {
            break;
        }
        let l = core::str::from_utf8(l).ok()?;
        let Some((n, v)) = l.split_once(':') else {
            continue;
        };
        let (n, v) = (n.trim(), v.trim());
        if is_token(n)
            && is_header_value(v)
            && !is_forbidden_header(n)
            && headers.len() < MAX_FETCH_HEADERS
        {
            headers.push((String::from(n), String::from(v)));
        }
    }
    const STANDARD: [&str; 6] = ["DELETE", "GET", "HEAD", "OPTIONS", "POST", "PUT"];
    let up = method.to_ascii_uppercase();
    if STANDARD.contains(&up.as_str()) {
        method = up.clone();
    }
    if !is_token(&method)
        || matches!(up.as_str(), "CONNECT" | "TRACE" | "TRACK")
        || url.len() > 8192
    {
        return None;
    }
    let body = (has_body && !matches!(method.as_str(), "GET" | "HEAD")).then(|| m[at..].to_vec());
    Some(FetchRequest {
        id,
        method,
        url,
        mode,
        credentials,
        redirect,
        headers,
        body,
    })
}

/// The answer's kind as `Response.type` says it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ResponseType {
    Basic,
    Cors,
    Opaque,
    OpaqueRedirect,
}

impl ResponseType {
    fn name(self) -> &'static str {
        match self {
            ResponseType::Basic => "basic",
            ResponseType::Cors => "cors",
            ResponseType::Opaque => "opaque",
            ResponseType::OpaqueRedirect => "opaqueredirect",
        }
    }
}

/// The head of an answer to request `id`.
pub struct FetchHead<'a> {
    pub id: u32,
    pub status: u16,
    pub status_text: &'a str,
    /// The final address (after redirects).
    pub url: &'a str,
    pub redirected: bool,
    pub kind: ResponseType,
    /// What the script may read.
    pub headers: &'a [(String, String)],
    /// The page's `document.cookie` now (a `Set-Cookie` may have changed it).
    pub cookie: &'a str,
}

/// `{"t":"fetch","id":..,"ev":"head",...}`.
pub fn head_message(h: &FetchHead) -> Vec<u8> {
    let mut out = String::with_capacity(256);
    out.push_str(&format!(
        "{{\"t\":\"fetch\",\"id\":{},\"ev\":\"head\",\"status\":{},\"statusText\":",
        h.id, h.status
    ));
    json_str(&mut out, h.status_text);
    out.push_str(",\"url\":");
    json_str(&mut out, h.url);
    out.push_str(&format!(
        ",\"redirected\":{},\"type\":\"{}\",\"cookie\":",
        h.redirected,
        h.kind.name()
    ));
    json_str(&mut out, h.cookie);
    out.push_str(",\"headers\":[");
    for (i, (n, v)) in h.headers.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        out.push('[');
        json_str(&mut out, n);
        out.push(',');
        json_str(&mut out, v);
        out.push(']');
    }
    out.push_str("]}");
    out.into_bytes()
}

/// Body bytes of the answer to request `id` (binary: [`DATA_TAG`], the id, the bytes).
pub fn data_message(id: u32, bytes: &[u8]) -> Vec<u8> {
    let mut v = Vec::with_capacity(5 + bytes.len());
    v.push(DATA_TAG);
    v.extend_from_slice(&id.to_le_bytes());
    v.extend_from_slice(bytes);
    v
}

/// The answer to request `id` is complete.
pub fn end_message(id: u32) -> Vec<u8> {
    format!("{{\"t\":\"fetch\",\"id\":{id},\"ev\":\"end\"}}").into_bytes()
}

/// Request `id` failed (a network error for the script: `TypeError` in `fetch`).
pub fn error_message(id: u32, why: &str) -> Vec<u8> {
    let mut out = format!("{{\"t\":\"fetch\",\"id\":{id},\"ev\":\"error\",\"message\":");
    json_str(&mut out, why);
    out.push('}');
    out.into_bytes()
}

/// The page's `document.cookie` after a write.
pub fn cookie_message(value: &str) -> Vec<u8> {
    let mut out = String::from("{\"t\":\"cookie\",\"v\":");
    json_str(&mut out, value);
    out.push('}');
    out.into_bytes()
}

#[cfg(test)]
mod tests;
