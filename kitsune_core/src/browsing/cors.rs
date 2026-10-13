//! The same-origin rule for what a page's scripts ask for (`fetch`, `XMLHttpRequest`): CORS, as the
//! Fetch standard describes it, in the parts the browser applies.
//!
//! * A request to the page's own origin is always readable.
//! * A request to another origin in `cors` mode carries `Origin`; the answer is readable only when
//!   `Access-Control-Allow-Origin` is `*` (not with credentials) or the page's origin, and, when the
//!   script sent credentials (`credentials: 'include'`, `withCredentials`), only with
//!   `Access-Control-Allow-Credentials: true` and an exact origin.
//! * A method other than `GET`/`HEAD`/`POST` or a header outside the safelist ([`needs_preflight`])
//!   first asks the server with an `OPTIONS` pre-flight ([`check_preflight`]).
//! * `no-cors` gives an opaque answer (status 0, no headers, no body) and only allows simple requests;
//!   `same-origin` refuses another origin outright.
//! * What a script may read of the headers of a cross-origin answer ([`exposed_headers`]) is the
//!   safelist plus `Access-Control-Expose-Headers`; `Set-Cookie` is never readable.
//!
//! Simplified (see `docs/design/tls-browser.md` §13): no pre-flight cache, a redirect to another
//! origin keeps the original `Origin` (the standard turns it into `null`), and `Referer` follows the
//! default `strict-origin-when-cross-origin` policy only ([`referrer`]).

use crate::browsing::http1::{Target, has_token, header_lines};
use alloc::string::String;
use alloc::vec::Vec;

/// A request's mode (`fetch(url, { mode })`).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Mode {
    Cors,
    NoCors,
    SameOrigin,
    /// A `<script src>` a page's script inserted, loaded by the page process itself (`fetch` never
    /// makes one): like `no-cors` (no CORS check, simple requests only), but the body of a
    /// JavaScript answer ([`is_javascript_type`]) is handed over so the process can run it, never to
    /// the page's code. Any other answer from another origin is refused (what browsers do for
    /// scripts with Opaque Response Blocking: HTML, JSON or images are not readable this way).
    Script,
}

/// Is `content_type` (a `Content-Type` value) a JavaScript MIME type? (The essence before any `;`,
/// case-insensitive, from the HTML standard's list.)
pub fn is_javascript_type(content_type: &[u8]) -> bool {
    let essence = content_type
        .split(|&b| b == b';')
        .next()
        .unwrap_or(b"")
        .trim_ascii();
    const TYPES: [&[u8]; 16] = [
        b"application/ecmascript",
        b"application/javascript",
        b"application/x-ecmascript",
        b"application/x-javascript",
        b"text/ecmascript",
        b"text/javascript",
        b"text/javascript1.0",
        b"text/javascript1.1",
        b"text/javascript1.2",
        b"text/javascript1.3",
        b"text/javascript1.4",
        b"text/javascript1.5",
        b"text/jscript",
        b"text/livescript",
        b"text/x-ecmascript",
        b"text/x-javascript",
    ];
    TYPES.iter().any(|t| essence.eq_ignore_ascii_case(t))
}

/// A request's credentials mode (`fetch(url, { credentials })`).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Credentials {
    Omit,
    SameOrigin,
    Include,
}

impl Credentials {
    /// May cookies go with a request that is (or is not) to the page's origin?
    pub fn send(self, same_origin: bool) -> bool {
        match self {
            Credentials::Omit => false,
            Credentials::SameOrigin => same_origin,
            Credentials::Include => true,
        }
    }
}

/// Why a script may not see an answer.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum CorsError {
    /// `mode: 'same-origin'` and the address is elsewhere.
    SameOriginOnly,
    /// `no-cors` with a method or header that is not simple.
    NotSimple,
    /// No `Access-Control-Allow-Origin`, or another origin.
    AllowOrigin,
    /// Credentials sent, and `*` or no `Access-Control-Allow-Credentials: true`.
    Credentials,
    /// The pre-flight failed (status, method or header not allowed).
    Preflight,
}

impl CorsError {
    /// The message the script sees in the `TypeError` (and the log).
    pub fn message(self) -> &'static str {
        match self {
            CorsError::SameOriginOnly => "request to another origin with mode same-origin",
            CorsError::NotSimple => "no-cors request with a method or header that is not simple",
            CorsError::AllowOrigin => {
                "CORS: the answer has no matching Access-Control-Allow-Origin"
            }
            CorsError::Credentials => "CORS: credentials are not allowed by the answer",
            CorsError::Preflight => "CORS: the pre-flight request was refused",
        }
    }
}

/// `GET`, `HEAD` or `POST`.
pub fn is_simple_method(m: &str) -> bool {
    matches!(m, "GET" | "HEAD" | "POST")
}

/// Headers a script may never set (the browser writes them, or they would let a page lie about
/// itself); dropped from a script's request.
pub fn is_forbidden_header(name: &str) -> bool {
    const LIST: [&str; 21] = [
        "accept-charset",
        "accept-encoding",
        "access-control-request-headers",
        "access-control-request-method",
        "connection",
        "content-length",
        "cookie",
        "cookie2",
        "date",
        "dnt",
        "expect",
        "host",
        "keep-alive",
        "origin",
        "referer",
        "set-cookie",
        "te",
        "trailer",
        "transfer-encoding",
        "upgrade",
        "via",
    ];
    let n = name.to_ascii_lowercase();
    LIST.contains(&n.as_str()) || n.starts_with("proxy-") || n.starts_with("sec-")
}

/// Is `bytes=N-` or `bytes=N-M` (one range, from a first byte)?
pub fn is_simple_range(v: &str) -> bool {
    let Some(r) = v.trim().strip_prefix("bytes=") else {
        return false;
    };
    let Some((a, b)) = r.split_once('-') else {
        return false;
    };
    let num = |s: &str| !s.is_empty() && s.len() <= 19 && s.bytes().all(|c| c.is_ascii_digit());
    num(a) && (b.is_empty() || (num(b) && a.parse::<u64>().ok() <= b.parse::<u64>().ok()))
}

/// A header a cross-origin request may carry without a pre-flight (CORS-safelisted).
pub fn is_safelisted_header(name: &str, value: &str) -> bool {
    if value.len() > 128 {
        return false;
    }
    let unsafe_byte = |b: u8| {
        (b < 0x20 && b != b'\t')
            || matches!(
                b,
                b'"' | b'('
                    | b')'
                    | b':'
                    | b'<'
                    | b'>'
                    | b'?'
                    | b'@'
                    | b'['
                    | b'\\'
                    | b']'
                    | b'{'
                    | b'}'
                    | 0x7f
            )
    };
    match name.to_ascii_lowercase().as_str() {
        "accept" => !value.bytes().any(unsafe_byte),
        "accept-language" | "content-language" => value.bytes().all(|b| {
            b.is_ascii_alphanumeric() || matches!(b, b' ' | b'*' | b',' | b'-' | b'.' | b';' | b'=')
        }),
        "content-type" => {
            let essence = value
                .split(';')
                .next()
                .unwrap_or("")
                .trim()
                .to_ascii_lowercase();
            !value.bytes().any(unsafe_byte)
                && matches!(
                    essence.as_str(),
                    "application/x-www-form-urlencoded" | "multipart/form-data" | "text/plain"
                )
        }
        "range" => is_simple_range(value),
        _ => false,
    }
}

/// Does a cross-origin request need an `OPTIONS` pre-flight?
pub fn needs_preflight(method: &str, headers: &[(String, String)]) -> bool {
    !is_simple_method(method)
        || headers.iter().any(|(n, v)| !is_safelisted_header(n, v))
        || headers
            .iter()
            .map(|(n, v)| n.len() + v.len())
            .sum::<usize>()
            > 1024
}

/// The names (lowercase, sorted, once each) a pre-flight announces in
/// `Access-Control-Request-Headers`: the headers that are not safelisted.
pub fn preflight_names(headers: &[(String, String)]) -> Vec<String> {
    let mut names: Vec<String> = headers
        .iter()
        .filter(|(n, v)| !is_safelisted_header(n, v))
        .map(|(n, _)| n.to_ascii_lowercase())
        .collect();
    names.sort();
    names.dedup();
    names
}

fn allow_origin_ok(
    origin: &str,
    include: bool,
    acao: Option<&[u8]>,
    acac: Option<&[u8]>,
) -> Result<(), CorsError> {
    let acao = acao.map(<[u8]>::trim_ascii).ok_or(CorsError::AllowOrigin)?;
    if acao == b"*" {
        return if include {
            Err(CorsError::Credentials)
        } else {
            Ok(())
        };
    }
    if acao != origin.as_bytes() {
        return Err(CorsError::AllowOrigin);
    }
    if include && acac.map(<[u8]>::trim_ascii) != Some(b"true".as_slice()) {
        return Err(CorsError::Credentials);
    }
    Ok(())
}

/// May the page at `origin` read this cross-origin answer (header block `head`)? `include`: the
/// request carried credentials.
pub fn check_response(origin: &str, include: bool, head: &[u8]) -> Result<(), CorsError> {
    allow_origin_ok(
        origin,
        include,
        crate::browsing::http1::header(head, b"access-control-allow-origin"),
        crate::browsing::http1::header(head, b"access-control-allow-credentials"),
    )
}

/// Does the pre-flight answer (`status`, header block `head`) allow `method` with the header names
/// `names` (from [`preflight_names`])?
pub fn check_preflight(
    origin: &str,
    include: bool,
    method: &str,
    names: &[String],
    status: u16,
    head: &[u8],
) -> Result<(), CorsError> {
    if !(200..300).contains(&status) {
        return Err(CorsError::Preflight);
    }
    check_response(origin, include, head).map_err(|_| CorsError::Preflight)?;
    let list = |name: &[u8]| crate::browsing::http1::header_joined(head, name).unwrap_or_default();
    let methods = list(b"access-control-allow-methods");
    let method_ok = is_simple_method(method)
        || methods.split(',').any(|m| m.trim() == method)
        || (!include && has_token(methods.as_bytes(), b"*"));
    if !method_ok {
        return Err(CorsError::Preflight);
    }
    let allowed = list(b"access-control-allow-headers");
    for n in names {
        let ok = has_token(allowed.as_bytes(), n.as_bytes())
            || (!include && n != "authorization" && has_token(allowed.as_bytes(), b"*"));
        if !ok {
            return Err(CorsError::Preflight);
        }
    }
    Ok(())
}

/// The response headers a script may read: all but `Set-Cookie` for its own origin; for another
/// origin (`cross`) the safelist plus `Access-Control-Expose-Headers` (`*` = all, without
/// credentials). Names come out lowercase.
pub fn exposed_headers(head: &[u8], cross: bool, include: bool) -> Vec<(String, String)> {
    const SAFE: [&str; 7] = [
        "cache-control",
        "content-language",
        "content-length",
        "content-type",
        "expires",
        "last-modified",
        "pragma",
    ];
    let expose = crate::browsing::http1::header_joined(head, b"access-control-expose-headers")
        .unwrap_or_default()
        .to_ascii_lowercase();
    let all = !cross || (!include && has_token(expose.as_bytes(), b"*"));
    let mut out = Vec::new();
    for (n, v) in header_lines(head) {
        let name = String::from_utf8_lossy(n).to_ascii_lowercase();
        if name == "set-cookie" || name == "set-cookie2" {
            continue;
        }
        if all || SAFE.contains(&name.as_str()) || has_token(expose.as_bytes(), name.as_bytes()) {
            out.push((name, String::from_utf8_lossy(v).into_owned()));
        }
    }
    out
}

/// The `Referer` a request from the page at `page` to `to` carries (policy
/// `strict-origin-when-cross-origin`): the whole address for the same origin, only the origin for
/// another one, nothing from `https` to `http`.
pub fn referrer(page: &Target, to: &Target) -> Option<String> {
    if page.https && !to.https {
        return None;
    }
    if page.origin() == to.origin() {
        Some(page.to_url())
    } else {
        let mut o = page.origin();
        o.push('/');
        Some(o)
    }
}

#[cfg(test)]
mod tests;
