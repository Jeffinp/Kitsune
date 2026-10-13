//! The jar as a text file (`~/.cookies`): one persistent cookie per line, fields separated by tabs.
//!
//! ```text
//! # kitsune cookies 1
//! DOMAIN  HOST_ONLY(0|1)  PATH  SECURE(0|1)  HTTP_ONLY(0|1)  SAMESITE(S|L|N|U)  EXPIRES_MS  NAME  VALUE
//! ```
//!
//! Names and values never hold a tab or a newline (the parser refuses control characters), so the
//! format needs no escaping. A damaged line is skipped; session cookies are never written.

use super::*;
use alloc::format;

const HEADER: &str = "# kitsune cookies 1";

impl Jar {
    /// The persistent cookies still valid at `now`, as text.
    pub fn to_text(&self, now: i64) -> Vec<u8> {
        let mut s = String::from(HEADER);
        s.push('\n');
        for c in &self.cookies {
            let Some(e) = c.expires else { continue };
            if e <= now {
                continue;
            }
            s.push_str(&format!(
                "{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\n",
                c.domain,
                u8::from(c.host_only),
                c.path,
                u8::from(c.secure),
                u8::from(c.http_only),
                c.same_site.code(),
                e,
                c.name,
                c.value
            ));
        }
        s.into_bytes()
    }

    /// A jar from [`Jar::to_text`]'s format; expired and damaged lines are dropped, the limits apply.
    pub fn parse_text(text: &[u8], now: i64) -> Jar {
        let mut jar = Jar::new();
        let Ok(text) = core::str::from_utf8(text) else {
            return jar;
        };
        for line in text.lines() {
            if line.starts_with('#') || line.is_empty() {
                continue;
            }
            let f: Vec<&str> = line.split('\t').collect();
            if f.len() != 9 {
                continue;
            }
            let bit = |s: &str| match s {
                "0" => Some(false),
                "1" => Some(true),
                _ => None,
            };
            let (Some(host_only), Some(secure), Some(http_only), Some(same_site), Ok(expires)) = (
                bit(f[1]),
                bit(f[3]),
                bit(f[4]),
                SameSite::from_code(f[5]),
                f[6].parse::<i64>(),
            ) else {
                continue;
            };
            let (domain, path, name, value) = (f[0], f[2], f[7], f[8]);
            let ok = !domain.is_empty()
                && domain.len() <= 253
                && path.starts_with('/')
                && !name.is_empty()
                && name.len() + value.len() <= MAX_COOKIE_BYTES
                && !line.bytes().any(|b| b < 0x20 && b != b'\t')
                && expires > now;
            if !ok {
                continue;
            }
            let created = jar.next;
            jar.next += 1;
            jar.cookies.push(Cookie {
                name: String::from(name),
                value: String::from(value),
                domain: domain.to_ascii_lowercase(),
                host_only,
                path: String::from(path),
                expires: Some(expires.min(now.saturating_add(MAX_AGE_MS))),
                secure,
                http_only,
                same_site,
                created,
            });
        }
        jar.evict(now);
        jar.dirty = false;
        jar
    }
}
