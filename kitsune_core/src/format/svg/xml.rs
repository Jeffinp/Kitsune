//! The XML subset SVG needs: tags and their attributes.

use super::*;

pub(super) struct Tag<'a> {
    pub(super) name: &'a str,
    /// `key="value"` pairs (the value decoded for the five predefined entities only).
    pub(super) attrs: Vec<(&'a str, &'a str)>,
    pub(super) open: bool,
    pub(super) close: bool,
}

impl<'a> Tag<'a> {
    pub(super) fn get(&self, k: &str) -> Option<&'a str> {
        self.attrs.iter().find(|(n, _)| *n == k).map(|(_, v)| *v)
    }
}

/// The next tag at or after `*pos` (comments, declarations and text are skipped).
pub(super) fn next_tag<'a>(s: &'a str, pos: &mut usize) -> Option<Tag<'a>> {
    loop {
        let i = s[*pos..].find('<')? + *pos;
        let rest = &s[i..];
        if rest.starts_with("<!--") {
            *pos = rest.find("-->").map_or(s.len(), |e| i + e + 3);
            continue;
        }
        if rest.starts_with("<![CDATA[") {
            *pos = rest.find("]]>").map_or(s.len(), |e| i + e + 3);
            continue;
        }
        if rest.starts_with("<?") || rest.starts_with("<!") {
            *pos = rest.find('>').map_or(s.len(), |e| i + e + 1);
            continue;
        }
        // Find the closing `>` outside quotes.
        let b = rest.as_bytes();
        let mut j = 1;
        let mut q = 0u8;
        while j < b.len() {
            match (q, b[j]) {
                (0, b'"' | b'\'') => q = b[j],
                (c, d) if c != 0 && c == d => q = 0,
                (0, b'>') => break,
                _ => {}
            }
            j += 1;
        }
        if j >= b.len() {
            *pos = s.len();
            return None;
        }
        let inner = &rest[1..j];
        *pos = i + j + 1;
        let (close, inner) = match inner.strip_prefix('/') {
            Some(r) => (true, r),
            None => (false, inner),
        };
        let (self_close, inner) = match inner.strip_suffix('/') {
            Some(r) => (true, r),
            None => (false, inner),
        };
        let name_end = inner
            .find(|c: char| c.is_ascii_whitespace())
            .unwrap_or(inner.len());
        let name = &inner[..name_end];
        // The namespace prefix (`svg:rect`) does not matter.
        let name = name.rsplit(':').next().unwrap_or(name);
        let mut attrs = Vec::new();
        let mut a = inner[name_end..].trim_start();
        while let Some(eq) = a.find('=') {
            let key = a[..eq].trim();
            let v = a[eq + 1..].trim_start();
            let (val, rest) = match v.chars().next() {
                Some(c @ ('"' | '\'')) => match v[1..].find(c) {
                    Some(e) => (&v[1..1 + e], &v[e + 2..]),
                    None => break,
                },
                _ => {
                    let e = v.find(|c: char| c.is_ascii_whitespace()).unwrap_or(v.len());
                    (&v[..e], &v[e..])
                }
            };
            if attrs.len() < 40 {
                attrs.push((key, val));
            }
            a = rest.trim_start();
        }
        return Some(Tag {
            name,
            attrs,
            open: !close,
            close: close || self_close,
        });
    }
}
