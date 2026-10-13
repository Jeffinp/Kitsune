//! HTTP/1.1 on a connection that stays open: where one response ends, how the next request is
//! written, and the owned URL the page's scripts ask for.
//!
//! The fetcher keeps connections open between requests (keep-alive, `docs/design/tls-browser.md`
//! §13), so it can no longer read "until the server closes". [`ResponseReader`] is fed the bytes as
//! they arrive and says where the response ends: `Content-Length`, `Transfer-Encoding: chunked` (the
//! framing is removed from what it hands on), or the close of the connection (then the connection
//! is not reused). Interim answers (`100 Continue`, `103 Early Hints`) are skipped, and a `HEAD`,
//! `204` or `304` has no body whatever its headers say. It never allocates per byte: the body comes
//! out as slices of the input.
//!
//! [`write_request`] writes a request with any method, header list and body; [`Target`] is an
//! absolute `http(s)` address without the size caps of `browser::Url` (a video segment address is
//! often longer than 480 bytes), and [`resolve`] turns a `Location` into the next [`Target`].
//!
//! Pure and host-tested; the kernel's `netstack` drives the I/O.

use alloc::string::String;
use alloc::vec::Vec;

mod reader;
mod request;
pub use reader::*;
pub use request::*;

/// The `User-Agent` the fetcher sends (the same as `browser::build_request`).
pub const HTTP_USER_AGENT: &str = concat!("Kitsune/", env!("CARGO_PKG_VERSION"));

/// The largest header block accepted (status line and headers), in bytes.
pub const MAX_HEAD: usize = 64 * 1024;

/// Why a response could not be read.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum HttpError {
    /// The status line is not `HTTP/1.x NNN ...`.
    BadStatus,
    /// The header block is larger than [`MAX_HEAD`].
    HeadTooLarge,
    /// A chunk size line is not hexadecimal, or the framing after a chunk is wrong.
    BadChunk,
    /// Two different `Content-Length` values, or one that is not a number.
    BadLength,
    /// The connection closed before the response was complete.
    Incomplete,
}

/// The header lines of a header block (`name`, `value`), the status line skipped. Values are trimmed;
/// a line without a colon is skipped.
pub fn header_lines(head: &[u8]) -> impl Iterator<Item = (&[u8], &[u8])> {
    head.split(|&b| b == b'\n').skip(1).filter_map(|line| {
        let line = line.strip_suffix(b"\r").unwrap_or(line);
        let colon = line.iter().position(|&b| b == b':')?;
        let name = line[..colon].trim_ascii();
        if name.is_empty() {
            return None;
        }
        Some((name, line[colon + 1..].trim_ascii()))
    })
}

/// The first value of header `name` (any case) in `head`.
pub fn header<'a>(head: &'a [u8], name: &[u8]) -> Option<&'a [u8]> {
    header_lines(head)
        .find(|(n, _)| n.eq_ignore_ascii_case(name))
        .map(|(_, v)| v)
}

/// Every value of header `name` (any case) in `head`, joined with `", "` as RFC 9110 allows for lists.
pub fn header_joined(head: &[u8], name: &[u8]) -> Option<String> {
    let mut out: Option<String> = None;
    for (n, v) in header_lines(head) {
        if n.eq_ignore_ascii_case(name) {
            let s = out.get_or_insert_with(String::new);
            if !s.is_empty() {
                s.push_str(", ");
            }
            s.push_str(&String::from_utf8_lossy(v));
        }
    }
    out
}

/// The status code and reason phrase of a header block.
pub fn status_line(head: &[u8]) -> Option<(u16, &[u8])> {
    let line = head.split(|&b| b == b'\n').next()?;
    let line = line.strip_suffix(b"\r").unwrap_or(line);
    let rest = line.strip_prefix(b"HTTP/1.")?;
    let (_, rest) = rest.split_first()?;
    let rest = rest.strip_prefix(b" ")?;
    let code = rest.get(..3)?;
    if !code.iter().all(u8::is_ascii_digit) {
        return None;
    }
    let n = u16::from(code[0] - b'0') * 100
        + u16::from(code[1] - b'0') * 10
        + u16::from(code[2] - b'0');
    let reason = rest.get(3..).unwrap_or(&[]).trim_ascii();
    Some((n, reason))
}

/// Does the comma-separated header value `list` contain `token` (any case)?
pub fn has_token(list: &[u8], token: &[u8]) -> bool {
    list.split(|&b| b == b',')
        .any(|t| t.trim_ascii().eq_ignore_ascii_case(token))
}

/// Parse a decimal number (no sign, no spaces), refusing overflow.
pub(crate) fn parse_dec(s: &[u8]) -> Option<u64> {
    if s.is_empty() || s.len() > 19 {
        return None;
    }
    let mut n = 0u64;
    for &c in s {
        if !c.is_ascii_digit() {
            return None;
        }
        n = n.checked_mul(10)?.checked_add(u64::from(c - b'0'))?;
    }
    Some(n)
}

/// `Content-Range: bytes FIRST-LAST/TOTAL` (`TOTAL` may be `*`): `(first, last, total)`.
pub fn parse_content_range(v: &[u8]) -> Option<(u64, u64, Option<u64>)> {
    let v = v.trim_ascii().strip_prefix(b"bytes")?.trim_ascii_start();
    let slash = v.iter().position(|&b| b == b'/')?;
    let (range, total) = (&v[..slash], &v[slash + 1..]);
    let dash = range.iter().position(|&b| b == b'-')?;
    let first = parse_dec(range[..dash].trim_ascii())?;
    let last = parse_dec(range[dash + 1..].trim_ascii())?;
    let total = match total.trim_ascii() {
        b"*" => None,
        t => Some(parse_dec(t)?),
    };
    if last < first || total.is_some_and(|t| last >= t) {
        return None;
    }
    Some((first, last, total))
}

/// Append the decimal form of `n`.
pub(crate) fn push_dec(out: &mut Vec<u8>, n: u64) {
    let mut digits = [0u8; 20];
    let mut i = digits.len();
    let mut n = n;
    loop {
        i -= 1;
        digits[i] = b'0' + (n % 10) as u8;
        n /= 10;
        if n == 0 {
            break;
        }
    }
    out.extend_from_slice(&digits[i..]);
}

#[cfg(test)]
mod tests;
