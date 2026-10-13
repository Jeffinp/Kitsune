//! The request side: [`Target`] (an owned absolute address), [`resolve`] (a `Location` against it) and
//! [`write_request`].

use super::*;
use alloc::format;

/// The longest address taken (bytes).
pub const MAX_URL: usize = 8192;
/// The longest host name.
pub const MAX_HOST: usize = 253;

/// An absolute `http` or `https` address, split for a request. The fragment is dropped; the host
/// is lowercase; the path always starts with `/` and keeps its query.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Target {
    pub https: bool,
    pub host: String,
    pub port: u16,
    /// Path and query, e.g. `/a/b?x=1`.
    pub path: String,
}

impl Target {
    /// Parse an absolute address. `None` for another scheme, user info, a bad host or port, spaces
    /// or control characters, or more than [`MAX_URL`] bytes.
    pub fn parse(url: &str) -> Option<Target> {
        let url = url.trim();
        if url.len() > MAX_URL || url.bytes().any(|b| b <= 0x20 || b == 0x7f) {
            return None;
        }
        let starts = |p: &str| {
            url.get(..p.len())
                .is_some_and(|s| s.eq_ignore_ascii_case(p))
        };
        let (https, rest) = if starts("https://") {
            (true, &url[8..])
        } else if starts("http://") {
            (false, &url[7..])
        } else {
            return None;
        };
        let rest = rest.split('#').next().unwrap_or("");
        let end = rest.find(['/', '?']).unwrap_or(rest.len());
        let (auth, path) = rest.split_at(end);
        if auth.contains('@') {
            return None;
        }
        let (host, port) = match auth.rfind(':') {
            Some(i) => {
                let p = &auth[i + 1..];
                let port = if p.is_empty() {
                    if https { 443 } else { 80 }
                } else {
                    let n = parse_dec(p.as_bytes())?;
                    if n == 0 || n > 65535 {
                        return None;
                    }
                    n as u16
                };
                (&auth[..i], port)
            }
            None => (auth, if https { 443 } else { 80 }),
        };
        let host = host.trim_end_matches('.');
        if host.is_empty()
            || host.len() > MAX_HOST
            || !host
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'.' || b == b'_')
            || host.starts_with('.')
            || host.contains("..")
        {
            return None;
        }
        let path = if path.is_empty() {
            String::from("/")
        } else if path.starts_with('?') {
            format!("/{path}")
        } else {
            String::from(path)
        };
        Some(Target {
            https,
            host: host.to_ascii_lowercase(),
            port,
            path,
        })
    }

    fn default_port(&self) -> bool {
        self.port == if self.https { 443 } else { 80 }
    }

    /// `scheme://host[:port]`, the port only when it is not the default.
    pub fn origin(&self) -> String {
        let scheme = if self.https { "https" } else { "http" };
        if self.default_port() {
            format!("{scheme}://{}", self.host)
        } else {
            format!("{scheme}://{}:{}", self.host, self.port)
        }
    }

    /// The whole address.
    pub fn to_url(&self) -> String {
        let mut s = self.origin();
        s.push_str(&self.path);
        s
    }

    /// The path without the query (what cookie paths match against).
    pub fn path_only(&self) -> &str {
        self.path.split('?').next().unwrap_or("/")
    }

    /// `host` or `host:port` for the `Host` header.
    pub fn host_header(&self) -> String {
        if self.default_port() {
            self.host.clone()
        } else {
            format!("{}:{}", self.host, self.port)
        }
    }
}

/// The address a `Location` (or any reference) points to, from `base`: absolute (`http(s)://`),
/// scheme-relative (`//host/x`), absolute path (`/x`), query (`?q`) or relative path (`x`, `../x`).
/// `None` for another scheme or something that does not parse.
pub fn resolve(base: &Target, reference: &str) -> Option<Target> {
    let r = reference.trim();
    if r.len() > MAX_URL {
        return None;
    }
    let lower = r.get(..8).unwrap_or(r).to_ascii_lowercase();
    if lower.starts_with("http://") || lower.starts_with("https://") {
        return Target::parse(r);
    }
    if let Some(colon) = r.find(':') {
        // `mailto:x`, `javascript:...`: a scheme before any `/`, `?` or `#`.
        if !r[..colon].contains(['/', '?', '#']) {
            return None;
        }
    }
    if let Some(rest) = r.strip_prefix("//") {
        let scheme = if base.https { "https://" } else { "http://" };
        return Target::parse(&format!("{scheme}{rest}"));
    }
    let r = r.split('#').next().unwrap_or("");
    let path = if r.is_empty() {
        base.path.clone()
    } else if r.starts_with('/') {
        normalize(r)
    } else if r.starts_with('?') {
        format!("{}{}", base.path_only(), r)
    } else {
        let dir = match base.path_only().rfind('/') {
            Some(i) => &base.path_only()[..=i],
            None => "/",
        };
        normalize(&format!("{dir}{r}"))
    };
    let mut t = base.clone();
    t.path = path;
    Target::parse(&t.to_url())
}

/// Remove `.` and `..` segments from an absolute path (its query is kept as it is).
fn normalize(p: &str) -> String {
    let (path, query) = match p.find('?') {
        Some(i) => (&p[..i], &p[i..]),
        None => (p, ""),
    };
    let mut out: Vec<&str> = Vec::new();
    let segs: Vec<&str> = path.split('/').skip(1).collect();
    let last = segs.len().saturating_sub(1);
    let mut trailing = false;
    for (i, s) in segs.iter().enumerate() {
        match *s {
            "." => trailing = i == last,
            ".." => {
                out.pop();
                trailing = i == last;
            }
            s => {
                out.push(s);
                trailing = false;
            }
        }
    }
    let mut s = String::from("/");
    s.push_str(&out.join("/"));
    if trailing && !s.ends_with('/') {
        s.push('/');
    }
    s.push_str(query);
    s
}

/// A request to write with [`write_request`].
pub struct Request<'a> {
    /// `GET`, `POST`... (already checked to be a token).
    pub method: &'a str,
    pub target: &'a Target,
    /// Headers after `Host`, in order (names and values already checked: no CR, LF or NUL).
    pub headers: &'a [(&'a str, &'a str)],
    pub body: Option<&'a [u8]>,
    /// Ask the server to keep the connection open.
    pub keep_alive: bool,
}

/// Is `s` an HTTP token (a method or a header name)?
pub fn is_token(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 128
        && s.bytes().all(|b| {
            b.is_ascii_alphanumeric()
                || matches!(
                    b,
                    b'!' | b'#'
                        | b'$'
                        | b'%'
                        | b'&'
                        | b'\''
                        | b'*'
                        | b'+'
                        | b'-'
                        | b'.'
                        | b'^'
                        | b'_'
                        | b'`'
                        | b'|'
                        | b'~'
                )
        })
}

/// Is `s` usable as a header value (no CR, LF or NUL, at most 8 KiB)?
pub fn is_header_value(s: &str) -> bool {
    s.len() <= 8192 && !s.bytes().any(|b| b == b'\r' || b == b'\n' || b == 0)
}

/// Append the request `r` to `out`: request line, `Host`, the headers, `Content-Length` when there
/// is a body (or a `POST`/`PUT`/`PATCH` without one), `Connection`, the blank line and the body.
/// Header lines that are not valid are left out.
pub fn write_request(out: &mut Vec<u8>, r: &Request) {
    out.extend_from_slice(r.method.as_bytes());
    out.push(b' ');
    out.extend_from_slice(r.target.path.as_bytes());
    out.extend_from_slice(b" HTTP/1.1\r\nHost: ");
    out.extend_from_slice(r.target.host_header().as_bytes());
    out.extend_from_slice(b"\r\n");
    for (n, v) in r.headers {
        if !is_token(n) || !is_header_value(v) {
            continue;
        }
        out.extend_from_slice(n.as_bytes());
        out.extend_from_slice(b": ");
        out.extend_from_slice(v.as_bytes());
        out.extend_from_slice(b"\r\n");
    }
    let len = match r.body {
        Some(b) => Some(b.len()),
        None if matches!(r.method, "POST" | "PUT" | "PATCH") => Some(0),
        None => None,
    };
    if let Some(len) = len {
        out.extend_from_slice(b"Content-Length: ");
        push_dec(out, len as u64);
        out.extend_from_slice(b"\r\n");
    }
    out.extend_from_slice(if r.keep_alive {
        b"Connection: keep-alive\r\n\r\n".as_slice()
    } else {
        b"Connection: close\r\n\r\n".as_slice()
    });
    if let Some(b) = r.body {
        out.extend_from_slice(b);
    }
}
