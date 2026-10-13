//! `localStorage`: what pages keep, per origin, between visits.
//!
//! The script process keeps a copy of its origin's items and sends every change (`L` + a JSON array:
//! `["set", key, value]`, `["del", key]`, `["clear"]`, [`parse_storage_op`]); the browser applies it
//! here ([`WebStorage::apply`], with the quotas) and writes the whole store to a file in the signed-in
//! person's home folder (`~/.webstorage`, [`WebStorage::to_text`]) now and then. Nothing from the
//! process is trusted: a malformed change is dropped, one past a quota is refused.

use super::*;
use alloc::collections::BTreeMap;

/// Most bytes (keys and values, UTF-8) one origin keeps.
pub const ORIGIN_BYTES: usize = 1024 * 1024;
/// Most bytes the whole store keeps.
pub const TOTAL_BYTES: usize = 4 * 1024 * 1024;
/// Most origins kept (the one used longest ago goes first).
pub const MAX_ORIGINS: usize = 64;
/// The first line of the file.
const HEADER: &str = "kitsune-webstorage 1";

/// A change to an origin's `localStorage`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StorageOp {
    Set(String, String),
    Remove(String),
    Clear,
}

#[derive(Default)]
struct Origin {
    items: BTreeMap<String, String>,
    bytes: usize,
    /// When it was last changed (a counter): the oldest is dropped when there are too many.
    stamp: u64,
}

/// Every origin's `localStorage`.
#[derive(Default)]
pub struct WebStorage {
    origins: BTreeMap<String, Origin>,
    bytes: usize,
    clock: u64,
    dirty: bool,
}

/// The origin (`scheme://host[:port]`) of a page address, `None` for one that has none (`about:`,
/// `data:`, the browser's own pages).
pub fn origin_of(url: &str) -> Option<String> {
    crate::browsing::http1::Target::parse(url).map(|t| t.origin())
}

impl WebStorage {
    /// The items of `origin`, in key order.
    pub fn items<'a>(&'a self, origin: &str) -> impl Iterator<Item = (&'a str, &'a str)> + 'a {
        self.origins
            .get(origin)
            .into_iter()
            .flat_map(|o| o.items.iter().map(|(k, v)| (k.as_str(), v.as_str())))
    }

    /// Apply a change made by a page of `origin`. Returns `false` when it was refused (over a quota).
    pub fn apply(&mut self, origin: &str, op: StorageOp) -> bool {
        self.clock += 1;
        let stamp = self.clock;
        match op {
            StorageOp::Set(k, v) => {
                let add = k.len() + v.len();
                let old = self
                    .origins
                    .get(origin)
                    .and_then(|o| o.items.get(&k))
                    .map_or(0, |v0| k.len() + v0.len());
                let o_bytes = self.origins.get(origin).map_or(0, |o| o.bytes);
                if o_bytes - old + add > ORIGIN_BYTES || self.bytes - old + add > TOTAL_BYTES {
                    return false;
                }
                if !self.origins.contains_key(origin) && self.origins.len() >= MAX_ORIGINS {
                    self.drop_oldest();
                }
                let o = self.origins.entry(String::from(origin)).or_default();
                o.items.insert(k, v);
                o.bytes = o.bytes - old + add;
                o.stamp = stamp;
                self.bytes = self.bytes - old + add;
            }
            StorageOp::Remove(k) => {
                let Some(o) = self.origins.get_mut(origin) else {
                    return true;
                };
                if let Some(v) = o.items.remove(&k) {
                    o.bytes -= k.len() + v.len();
                    self.bytes -= k.len() + v.len();
                    o.stamp = stamp;
                }
                if o.items.is_empty() {
                    self.origins.remove(origin);
                }
            }
            StorageOp::Clear => {
                if let Some(o) = self.origins.remove(origin) {
                    self.bytes -= o.bytes;
                }
            }
        }
        self.dirty = true;
        true
    }

    fn drop_oldest(&mut self) {
        let oldest = self
            .origins
            .iter()
            .min_by_key(|(_, o)| o.stamp)
            .map(|(k, _)| k.clone());
        if let Some(k) = oldest
            && let Some(o) = self.origins.remove(&k)
        {
            self.bytes -= o.bytes;
        }
    }

    /// Did anything change since the last call?
    pub fn take_dirty(&mut self) -> bool {
        core::mem::take(&mut self.dirty)
    }

    /// The file: a header line, then one line per item, `origin TAB key TAB value`, with `\`, tab,
    /// line feed and carriage return escaped (`\\`, `\t`, `\n`, `\r`).
    pub fn to_text(&self) -> String {
        let mut out = String::from(HEADER);
        out.push('\n');
        for (origin, o) in &self.origins {
            for (k, v) in &o.items {
                escape(origin, &mut out);
                out.push('\t');
                escape(k, &mut out);
                out.push('\t');
                escape(v, &mut out);
                out.push('\n');
            }
        }
        out
    }

    /// Read the file written by [`WebStorage::to_text`] (bad lines are skipped, the quotas hold).
    pub fn from_text(text: &str) -> WebStorage {
        let mut s = WebStorage::default();
        let mut lines = text.split('\n');
        if lines.next().map(str::trim) != Some(HEADER) {
            return s;
        }
        for line in lines {
            let mut parts = line.split('\t');
            let (Some(o), Some(k), Some(v), None) =
                (parts.next(), parts.next(), parts.next(), parts.next())
            else {
                continue;
            };
            let (o, k, v) = (unescape(o), unescape(k), unescape(v));
            if origin_of(&o).as_deref() == Some(o.as_str()) {
                s.apply(&o, StorageOp::Set(k, v));
            }
        }
        s.dirty = false;
        s
    }
}

fn escape(s: &str, out: &mut String) {
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\t' => out.push_str("\\t"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            c => out.push(c),
        }
    }
}

fn unescape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut it = s.chars();
    while let Some(c) = it.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match it.next() {
            Some('t') => out.push('\t'),
            Some('n') => out.push('\n'),
            Some('r') => out.push('\r'),
            Some(o) => out.push(o),
            None => {}
        }
    }
    out
}

/// Read an `L` message: a JSON array of strings (`["set", k, v]`, `["del", k]`, `["clear"]`).
pub fn parse_storage_op(m: &[u8]) -> Option<StorageOp> {
    let text = core::str::from_utf8(m).ok()?;
    let list = json_strings(text.trim())?;
    match list.as_slice() {
        [op, k, v] if op == "set" => Some(StorageOp::Set(k.clone(), v.clone())),
        [op, k] if op == "del" => Some(StorageOp::Remove(k.clone())),
        [op] if op == "clear" => Some(StorageOp::Clear),
        _ => None,
    }
}

/// A JSON array of strings (as `JSON.stringify` writes it), or `None`.
fn json_strings(s: &str) -> Option<Vec<String>> {
    let b = s.as_bytes();
    let mut i = 0;
    let ws = |i: &mut usize| {
        while b.get(*i).is_some_and(|c| c.is_ascii_whitespace()) {
            *i += 1;
        }
    };
    if b.first() != Some(&b'[') {
        return None;
    }
    i += 1;
    let mut out = Vec::new();
    ws(&mut i);
    if b.get(i) == Some(&b']') {
        return (i + 1 == b.len()).then_some(out);
    }
    loop {
        ws(&mut i);
        if b.get(i) != Some(&b'"') {
            return None;
        }
        i += 1;
        let mut v = String::new();
        let mut pending_high: Option<u32> = None;
        loop {
            let c = *b.get(i)?;
            match c {
                b'"' => {
                    i += 1;
                    break;
                }
                b'\\' => {
                    let e = *b.get(i + 1)?;
                    i += 2;
                    let ch = match e {
                        b'"' => '"',
                        b'\\' => '\\',
                        b'/' => '/',
                        b'b' => '\u{8}',
                        b'f' => '\u{c}',
                        b'n' => '\n',
                        b'r' => '\r',
                        b't' => '\t',
                        b'u' => {
                            let hex = core::str::from_utf8(b.get(i..i + 4)?).ok()?;
                            let u = u32::from_str_radix(hex, 16).ok()?;
                            i += 4;
                            if (0xd800..0xdc00).contains(&u) {
                                pending_high = Some(u);
                                continue;
                            }
                            if (0xdc00..0xe000).contains(&u) {
                                let hi = pending_high.take().unwrap_or(0xd800);
                                char::from_u32(0x10000 + ((hi - 0xd800) << 10) + (u - 0xdc00))
                                    .unwrap_or('\u{fffd}')
                            } else {
                                char::from_u32(u).unwrap_or('\u{fffd}')
                            }
                        }
                        _ => return None,
                    };
                    if pending_high.take().is_some() && e != b'u' {
                        v.push('\u{fffd}');
                    }
                    v.push(ch);
                }
                _ => {
                    // A run of plain bytes (the input is valid UTF-8).
                    let start = i;
                    while b.get(i).is_some_and(|&c| c != b'"' && c != b'\\') {
                        i += 1;
                    }
                    v.push_str(core::str::from_utf8(&b[start..i]).ok()?);
                }
            }
        }
        out.push(v);
        ws(&mut i);
        match b.get(i) {
            Some(b',') => i += 1,
            Some(b']') => return (i + 1 == b.len()).then_some(out),
            _ => return None,
        }
    }
}
