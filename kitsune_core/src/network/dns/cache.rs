//! The TTL cache of answers and of NXDOMAIN.

use super::*;

/// A cache hit.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Cached {
    Addr(Ipv4),
    NxDomain,
}

#[derive(Clone, Debug)]
pub(super) struct Entry {
    pub(super) name: String,
    pub(super) what: Cached,
    pub(super) expires_ms: u64,
}

/// TTL cache of lookups, keyed by normalized name.
#[derive(Clone, Debug, Default)]
pub struct DnsCache {
    pub(super) entries: Vec<Entry>,
}

impl DnsCache {
    pub fn new() -> DnsCache {
        DnsCache::default()
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn clear(&mut self) {
        self.entries.clear();
    }

    /// Drop expired entries.
    pub fn purge(&mut self, now_ms: u64) {
        self.entries.retain(|e| e.expires_ms > now_ms);
    }

    /// The live entry for `name`, if any. An entry is live strictly before its
    /// expiry time.
    pub fn get(&self, name: &str, now_ms: u64) -> Option<Cached> {
        let n = normalize_name(name)?;
        self.entries
            .iter()
            .find(|e| e.name == n && e.expires_ms > now_ms)
            .map(|e| e.what)
    }

    /// Remaining life of the entry for `name` in milliseconds.
    pub fn remaining_ms(&self, name: &str, now_ms: u64) -> Option<u64> {
        let n = normalize_name(name)?;
        self.entries
            .iter()
            .find(|e| e.name == n && e.expires_ms > now_ms)
            .map(|e| e.expires_ms - now_ms)
    }

    /// Cache `addr` for `name` for `ttl_secs` (clamped to [`MAX_TTL_SECS`]; 0 is
    /// not cached, and removes any older entry).
    pub fn insert_addr(&mut self, name: &str, addr: Ipv4, ttl_secs: u32, now_ms: u64) {
        self.insert(name, Cached::Addr(addr), ttl_secs.min(MAX_TTL_SECS), now_ms);
    }

    /// Cache the non-existence of `name` for [`NEGATIVE_TTL_SECS`].
    pub fn insert_nxdomain(&mut self, name: &str, now_ms: u64) {
        self.insert(name, Cached::NxDomain, NEGATIVE_TTL_SECS, now_ms);
    }

    pub(super) fn insert(&mut self, name: &str, what: Cached, ttl_secs: u32, now_ms: u64) {
        let Some(n) = normalize_name(name) else {
            return;
        };
        self.entries.retain(|e| e.name != n); // replace
        if ttl_secs == 0 {
            return;
        }
        self.purge(now_ms);
        if self.entries.len() >= CACHE_CAP {
            // Evict the entry closest to expiry.
            if let Some(i) = self
                .entries
                .iter()
                .enumerate()
                .min_by_key(|(_, e)| e.expires_ms)
                .map(|(i, _)| i)
            {
                self.entries.swap_remove(i);
            }
        }
        self.entries.push(Entry {
            name: n,
            what,
            expires_ms: now_ms.saturating_add(u64::from(ttl_secs) * 1000),
        });
    }
}
