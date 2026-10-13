//! Reading a `Set-Cookie` value (RFC 6265 §5.2) and the cookie date format (§5.1.1).

use super::*;
use crate::format::unixtime::days_from_civil;

/// A `Set-Cookie` value split into its parts, before the jar applies it to an address.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct SetCookie {
    pub name: String,
    pub value: String,
    /// `Expires` as Unix milliseconds.
    pub expires: Option<i64>,
    /// `Max-Age` in seconds (wins over `Expires`).
    pub max_age: Option<i64>,
    /// `Domain`, lowercase, without its leading dot.
    pub domain: Option<String>,
    /// `Path` when it starts with `/`.
    pub path: Option<String>,
    pub secure: bool,
    pub http_only: bool,
    pub same_site: SameSite,
}

/// Is `s` free of control characters (tab included) and of `;`?
fn clean(s: &str) -> bool {
    !s.bytes().any(|b| b < 0x20 || b == 0x7f || b == b';')
}

/// Parse one `Set-Cookie` value. `None` when there is no `name=`, the name is empty, the name or
/// value holds control characters, or the two are larger than [`MAX_COOKIE_BYTES`].
pub fn parse_set_cookie(header: &str) -> Option<SetCookie> {
    let mut parts = header.split(';');
    let pair = parts.next()?;
    let eq = pair.find('=')?;
    let name = pair[..eq].trim();
    let value = pair[eq + 1..].trim();
    if name.is_empty()
        || !clean(name)
        || !clean(value)
        || name.len() + value.len() > MAX_COOKIE_BYTES
        || name.contains(['=', ' ', ',', '"'])
    {
        return None;
    }
    let mut c = SetCookie {
        name: String::from(name),
        value: String::from(value),
        expires: None,
        max_age: None,
        domain: None,
        path: None,
        secure: false,
        http_only: false,
        same_site: SameSite::Unset,
    };
    for attr in parts {
        let (k, v) = match attr.find('=') {
            Some(i) => (attr[..i].trim(), attr[i + 1..].trim()),
            None => (attr.trim(), ""),
        };
        if v.len() > 1024 {
            continue;
        }
        match k.to_ascii_lowercase().as_str() {
            "expires" => {
                if let Some(t) = parse_cookie_date(v) {
                    c.expires = Some(t);
                }
            }
            "max-age" => {
                let digits = v.strip_prefix('-').unwrap_or(v);
                if !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit()) {
                    let n = digits.parse::<i64>().unwrap_or(i64::MAX);
                    c.max_age = Some(if v.starts_with('-') { -1 } else { n });
                }
            }
            "domain" => {
                let d = v.strip_prefix('.').unwrap_or(v).to_ascii_lowercase();
                if !d.is_empty() {
                    c.domain = Some(d);
                }
            }
            "path" => {
                c.path = v.starts_with('/').then(|| String::from(v));
            }
            "secure" => c.secure = true,
            "httponly" => c.http_only = true,
            "samesite" => {
                c.same_site = match v.to_ascii_lowercase().as_str() {
                    "strict" => SameSite::Strict,
                    "lax" => SameSite::Lax,
                    "none" => SameSite::None,
                    _ => SameSite::Unset,
                }
            }
            _ => {}
        }
    }
    Some(c)
}

/// The cookie date of RFC 6265 §5.1.1 (`Wed, 21 Oct 2026 07:28:00 GMT` and the many forms servers
/// really send) as Unix milliseconds; always UTC.
pub fn parse_cookie_date(s: &str) -> Option<i64> {
    let delim = |c: char| {
        let b = c as u32;
        b == 0x09
            || (0x20..=0x2f).contains(&b)
            || (0x3b..=0x40).contains(&b)
            || (0x5b..=0x60).contains(&b)
            || (0x7b..=0x7e).contains(&b)
    };
    let (mut time, mut day, mut month, mut year) = (None, None, None, None);
    for tok in s.split(delim).filter(|t| !t.is_empty()) {
        if time.is_none()
            && let Some(t) = parse_time(tok)
        {
            time = Some(t);
            continue;
        }
        let lead: &str = &tok[..tok.find(|c: char| !c.is_ascii_digit()).unwrap_or(tok.len())];
        if day.is_none() && (1..=2).contains(&lead.len()) {
            day = lead.parse::<u8>().ok();
            continue;
        }
        if month.is_none() && tok.len() >= 3 {
            const M: [&str; 12] = [
                "jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec",
            ];
            let head = tok.get(..3).unwrap_or("").to_ascii_lowercase();
            if let Some(i) = M.iter().position(|m| *m == head) {
                month = Some(i as u8 + 1);
                continue;
            }
        }
        if year.is_none() && (2..=4).contains(&lead.len()) {
            year = lead.parse::<i32>().ok();
        }
    }
    let (h, mi, se) = time?;
    let (d, m, mut y) = (day?, month?, year?);
    if (70..=99).contains(&y) {
        y += 1900;
    } else if (0..=69).contains(&y) {
        y += 2000;
    }
    if !(1..=31).contains(&d) || y < 1601 || h > 23 || mi > 59 || se > 59 {
        return None;
    }
    if d > crate::format::unixtime::days_in_month(y, m) {
        return None;
    }
    let days = days_from_civil(y, m, d);
    Some(((days * 24 + i64::from(h)) * 60 + i64::from(mi)) * 60_000 + i64::from(se) * 1000)
}

/// `hh:mm:ss` with one or two digits each (trailing non-digits after the seconds are allowed).
fn parse_time(tok: &str) -> Option<(u8, u8, u8)> {
    let mut it = tok.split(':');
    let h = it.next()?;
    let m = it.next()?;
    let s = it.next()?;
    if it.next().is_some() {
        return None;
    }
    let s = &s[..s.find(|c: char| !c.is_ascii_digit()).unwrap_or(s.len())];
    let num = |x: &str| -> Option<u8> {
        if (1..=2).contains(&x.len()) && x.bytes().all(|b| b.is_ascii_digit()) {
            x.parse().ok()
        } else {
            None
        }
    };
    Some((num(h)?, num(m)?, num(s)?))
}
