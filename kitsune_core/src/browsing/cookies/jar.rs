//! [`Jar`]: storing, matching and evicting cookies.

use super::*;
use crate::browsing::shields::site_of;

/// The browser's cookies.
#[derive(Default, Debug)]
pub struct Jar {
    pub(super) cookies: Vec<Cookie>,
    pub(super) next: u64,
    /// A persistent cookie changed since the last [`Jar::take_dirty`].
    pub(super) dirty: bool,
}

/// Does `host` domain-match `domain` (RFC 6265 §5.1.3)?
pub fn domain_match(host: &str, domain: &str) -> bool {
    host == domain
        || (host.len() > domain.len()
            && host.ends_with(domain)
            && host.as_bytes()[host.len() - domain.len() - 1] == b'.'
            && !is_ip(host))
}

/// Does `req` path-match the cookie path `cookie` (RFC 6265 §5.1.4)?
pub fn path_match(req: &str, cookie: &str) -> bool {
    req == cookie
        || (req.starts_with(cookie)
            && (cookie.ends_with('/') || req.as_bytes().get(cookie.len()) == Some(&b'/')))
}

/// The default cookie path of a request path: its directory (RFC 6265 §5.1.4).
pub fn default_path(path: &str) -> String {
    match path.rfind('/') {
        Some(i) if path.starts_with('/') && i > 0 => String::from(&path[..i]),
        _ => String::from("/"),
    }
}

fn is_ip(host: &str) -> bool {
    host.split('.').count() == 4
        && host
            .split('.')
            .all(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()))
}

/// Are two hosts of the same site (`shields::site_of`)?
pub fn same_site(a: &str, b: &str) -> bool {
    site_of(a.as_bytes()) == site_of(b.as_bytes())
}

impl Jar {
    pub fn new() -> Jar {
        Jar::default()
    }

    /// How many cookies the jar holds (expired ones included until the next change).
    pub fn len(&self) -> usize {
        self.cookies.len()
    }

    pub fn is_empty(&self) -> bool {
        self.cookies.is_empty()
    }

    /// Every cookie, for inspection.
    pub fn cookies(&self) -> &[Cookie] {
        &self.cookies
    }

    /// Was a persistent cookie added, changed or removed since the last call? (Then the file is due.)
    pub fn take_dirty(&mut self) -> bool {
        core::mem::take(&mut self.dirty)
    }

    /// Apply a `Set-Cookie` header received from `site` in `ctx` at `now` (Unix ms). Returns whether
    /// the jar took it (a deletion counts).
    pub fn set_from_header(&mut self, site: Site, header: &str, ctx: Context, now: i64) -> bool {
        match parse_set_cookie(header) {
            Some(c) => self.store(site, c, ctx, now),
            None => false,
        }
    }

    /// `document.cookie = text` on a page at `site`.
    pub fn document_set(&mut self, site: Site, text: &str, now: i64) -> bool {
        self.set_from_header(site, text, Context::SCRIPT, now)
    }

    /// `document.cookie` of a page at `site`: what a same-site script may read (no `HttpOnly`).
    pub fn document_get(&mut self, site: Site, now: i64) -> String {
        self.header_for(site, Context::SCRIPT, now)
    }

    /// The `Cookie` request header value for `site` in `ctx` (empty: send none).
    pub fn header_for(&mut self, site: Site, ctx: Context, now: i64) -> String {
        self.purge(now);
        let mut hits: Vec<&Cookie> = self
            .cookies
            .iter()
            .filter(|c| {
                let host_ok = if c.host_only {
                    site.host == c.domain
                } else {
                    domain_match(site.host, &c.domain)
                };
                host_ok
                    && path_match(site.path, &c.path)
                    && (!c.secure || site.https)
                    && (ctx.http || !c.http_only)
                    && match c.same_site {
                        SameSite::None => true,
                        SameSite::Strict => ctx.same_site,
                        SameSite::Lax | SameSite::Unset => {
                            ctx.same_site || (ctx.top_level && ctx.safe_method)
                        }
                    }
            })
            .collect();
        hits.sort_by(|a, b| {
            b.path
                .len()
                .cmp(&a.path.len())
                .then(a.created.cmp(&b.created))
        });
        let mut out = String::new();
        for c in hits {
            if !out.is_empty() {
                out.push_str("; ");
            }
            out.push_str(&c.name);
            out.push('=');
            out.push_str(&c.value);
        }
        out
    }

    /// Store `c` received from `site` (RFC 6265 §5.3 with the 6265bis rules on `Secure`, prefixes
    /// and `SameSite`).
    pub(super) fn store(&mut self, site: Site, c: SetCookie, ctx: Context, now: i64) -> bool {
        if !ctx.http && c.http_only {
            return false;
        }
        // A cross-site response (that is not a top-level navigation) may not set Strict/Lax cookies.
        if !ctx.same_site && !ctx.top_level && c.same_site != SameSite::None {
            return false;
        }
        if c.secure && !site.https {
            return false;
        }
        if c.same_site == SameSite::None && !c.secure {
            return false;
        }
        let (domain, host_only) = match &c.domain {
            Some(d) => {
                // Not a bare top-level name or a suffix such as co.uk: the domain must be the host's
                // site or below it.
                let ok = *d == site.host
                    || (!is_ip(site.host)
                        && d.contains('.')
                        && domain_match(site.host, d)
                        && same_site(d, site.host));
                if !ok {
                    return false;
                }
                (d.clone(), false)
            }
            None => (String::from(site.host), true),
        };
        let path = c.path.clone().unwrap_or_else(|| default_path(site.path));
        if c.name.starts_with("__Secure-") && !c.secure {
            return false;
        }
        if c.name.starts_with("__Host-") && (!c.secure || !host_only || path != "/") {
            return false;
        }
        let expires = match (c.max_age, c.expires) {
            (Some(s), _) if s <= 0 => Some(i64::MIN),
            (Some(s), _) => Some(now.saturating_add(s.saturating_mul(1000).min(MAX_AGE_MS))),
            (None, Some(t)) => Some(t.min(now.saturating_add(MAX_AGE_MS))),
            (None, None) => None,
        };
        // An http: page may not touch a Secure cookie of the same name over its path.
        if !site.https
            && self.cookies.iter().any(|o| {
                o.secure
                    && o.name == c.name
                    && (domain_match(&domain, &o.domain) || domain_match(&o.domain, &domain))
                    && path_match(&path, &o.path)
            })
        {
            return false;
        }
        let old = self
            .cookies
            .iter()
            .position(|o| o.name == c.name && o.domain == domain && o.path == path);
        if let Some(i) = old {
            if self.cookies[i].http_only && !ctx.http {
                return false;
            }
            let o = self.cookies.remove(i);
            if o.expires.is_some() {
                self.dirty = true;
            }
            if expires.is_some_and(|e| e <= now) {
                return true;
            }
            let created = o.created;
            self.push(c, domain, host_only, path, expires, created);
            return true;
        }
        if expires.is_some_and(|e| e <= now) {
            return false;
        }
        let created = self.next;
        self.next += 1;
        self.push(c, domain, host_only, path, expires, created);
        self.evict(now);
        true
    }

    fn push(
        &mut self,
        c: SetCookie,
        domain: String,
        host_only: bool,
        path: String,
        expires: Option<i64>,
        created: u64,
    ) {
        if expires.is_some() {
            self.dirty = true;
        }
        self.cookies.push(Cookie {
            name: c.name,
            value: c.value,
            domain,
            host_only,
            path,
            expires,
            secure: c.secure,
            http_only: c.http_only,
            same_site: c.same_site,
            created,
        });
    }

    /// Drop expired cookies.
    pub fn purge(&mut self, now: i64) {
        let before = self.cookies.len();
        let mut lost_persistent = false;
        self.cookies.retain(|c| {
            let keep = c.expires.is_none_or(|e| e > now);
            lost_persistent |= !keep;
            keep
        });
        if lost_persistent && self.cookies.len() != before {
            self.dirty = true;
        }
    }

    /// Keep within [`MAX_PER_DOMAIN`] and [`MAX_COOKIES`]: expired ones first, then the oldest.
    pub(super) fn evict(&mut self, now: i64) {
        self.purge(now);
        // Newest first within each domain; everything past the first MAX_PER_DOMAIN goes.
        let mut order: Vec<usize> = (0..self.cookies.len()).collect();
        order.sort_by(|&a, &b| {
            let (x, y) = (&self.cookies[a], &self.cookies[b]);
            x.domain.cmp(&y.domain).then(y.created.cmp(&x.created))
        });
        let mut drop = alloc::vec![false; self.cookies.len()];
        let mut run = 0;
        for (k, &i) in order.iter().enumerate() {
            let same = k > 0 && self.cookies[order[k - 1]].domain == self.cookies[i].domain;
            run = if same { run + 1 } else { 1 };
            drop[i] = run > MAX_PER_DOMAIN;
        }
        let kept = drop.iter().filter(|d| !**d).count();
        if kept > MAX_COOKIES {
            // Then the oldest overall.
            let mut by_age: Vec<usize> = (0..self.cookies.len()).filter(|&i| !drop[i]).collect();
            by_age.sort_by_key(|&i| self.cookies[i].created);
            for &i in by_age.iter().take(kept - MAX_COOKIES) {
                drop[i] = true;
            }
        }
        let mut i = 0;
        let dirty = &mut self.dirty;
        self.cookies.retain(|c| {
            let gone = drop[i];
            i += 1;
            *dirty |= gone && c.expires.is_some();
            !gone
        });
    }
}
