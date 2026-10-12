//! One lookup across several servers ([`Resolve`]) and the resolver that remembers the server that answered.

use super::*;

/// Final result of a [`Resolve`].
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Outcome {
    Resolved {
        addr: Ipv4,
        ttl: u32,
    },
    NxDomain,
    NoData,
    /// Every attempt failed or timed out.
    Failed,
    /// No DNS server is configured.
    NoServers,
}

/// What the driver should do next for a [`Resolve`].
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Step {
    /// Send the query (built with [`build_query`] and `id`) to `server`.
    Send {
        server: Ipv4,
        id: u16,
    },
    /// Nothing to do before this time (ms).
    Wait(u64),
    Done(Outcome),
}

/// One lookup in progress.
#[derive(Clone, Debug)]
pub struct Resolve {
    pub(super) name: String,
    pub(super) servers: DnsServers,
    pub(super) id: u16,
    pub(super) start: usize,
    /// Attempts started so far.
    pub(super) attempts: usize,
    /// When the current attempt is abandoned.
    pub(super) attempt_deadline: u64,
    /// Whether the first attempt has been issued.
    pub(super) started: bool,
    pub(super) total_deadline: u64,
    pub(super) done: Option<Outcome>,
    /// Index (into `servers`) that produced the final answer.
    pub(super) answered_by: Option<usize>,
    /// A failure notice from the server being waited on forces the next attempt.
    pub(super) advance: bool,
}

impl Resolve {
    /// Start resolving `name` against `servers`, first asking
    /// `servers[start % len]`. `id` is the DNS transaction id (any value, but
    /// unpredictable in the kernel: it is the only defense against an off-path
    /// forger).
    pub fn new(name: &str, servers: DnsServers, start: usize, id: u16, now_ms: u64) -> Resolve {
        let normalized = normalize_name(name);
        let mut r = Resolve {
            name: normalized.clone().unwrap_or_default(),
            servers,
            id,
            start,
            attempts: 0,
            attempt_deadline: 0,
            started: false,
            total_deadline: now_ms + TOTAL_TIMEOUT_MS,
            done: None,
            answered_by: None,
            advance: false,
        };
        if normalized.is_none() {
            r.done = Some(Outcome::Failed);
        } else if servers.is_empty() {
            r.done = Some(Outcome::NoServers);
        }
        r
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn id(&self) -> u16 {
        self.id
    }

    /// Attempts started so far (a value above 1 means a failover happened).
    pub fn attempts(&self) -> usize {
        self.attempts
    }

    pub(super) fn max_attempts(&self) -> usize {
        self.servers.len() * ROUNDS
    }

    /// The server the current (or last) attempt goes to.
    pub(super) fn server_index(&self, attempt: usize) -> usize {
        (self.start + attempt) % self.servers.len().max(1)
    }

    /// Index of the server that supplied the final answer (for the caller to
    /// remember as the preferred one).
    pub fn answered_by(&self) -> Option<usize> {
        self.answered_by
    }

    /// Time-driven step.
    pub fn poll(&mut self, now_ms: u64) -> Step {
        if let Some(o) = self.done {
            return Step::Done(o);
        }
        if now_ms >= self.total_deadline {
            self.done = Some(Outcome::Failed);
            return Step::Done(Outcome::Failed);
        }
        if !self.started || self.advance || now_ms >= self.attempt_deadline {
            if self.attempts >= self.max_attempts() {
                self.done = Some(Outcome::Failed);
                return Step::Done(Outcome::Failed);
            }
            let idx = self.server_index(self.attempts);
            self.started = true;
            self.advance = false;
            self.attempts += 1;
            self.attempt_deadline = (now_ms + ATTEMPT_TIMEOUT_MS).min(self.total_deadline);
            return Step::Send {
                server: self.servers.as_slice()[idx],
                id: self.id,
            };
        }
        Step::Wait(self.attempt_deadline)
    }

    /// Feed a UDP payload received from `from`. Returns the final outcome if
    /// this ends the lookup. Packets from addresses that are not configured
    /// servers, with the wrong id or question, or malformed, are ignored.
    pub fn on_response(&mut self, from: Ipv4, msg: &[u8]) -> Option<Outcome> {
        if self.done.is_some() || !self.started {
            return None;
        }
        let from_idx = self.servers.as_slice().iter().position(|&s| s == from)?;
        let reply = parse_response(msg, self.id, &self.name)?;
        let out = match reply {
            Reply::Addrs { addrs, ttl } => Outcome::Resolved {
                addr: addrs.first()?,
                ttl,
            },
            Reply::NxDomain => Outcome::NxDomain,
            Reply::NoData => Outcome::NoData,
            Reply::ServerFail(_) => {
                // Another server may do better: move on right away.
                self.advance = true;
                return None;
            }
        };
        self.done = Some(out);
        self.answered_by = Some(from_idx);
        Some(out)
    }
}

/// Servers + cache + the rotation memory. The kernel keeps one of these.
#[derive(Clone, Debug)]
pub struct Resolver {
    pub(super) servers: DnsServers,
    pub(super) cache: DnsCache,
    /// Server to try first on the next lookup.
    pub(super) preferred: usize,
}

impl Resolver {
    pub fn new(servers: DnsServers) -> Resolver {
        Resolver {
            servers,
            cache: DnsCache::new(),
            preferred: 0,
        }
    }

    pub fn servers(&self) -> &DnsServers {
        &self.servers
    }

    /// Replace the server list (a lease change): the cache is dropped too, since
    /// the new servers may answer differently (split-horizon, a new network).
    pub fn set_servers(&mut self, servers: DnsServers) {
        if servers != self.servers {
            self.cache.clear();
        }
        self.servers = servers;
        self.preferred = 0;
    }

    pub fn cache(&self) -> &DnsCache {
        &self.cache
    }

    pub fn preferred(&self) -> usize {
        self.preferred
    }

    /// Cached answer for `name`, if any.
    pub fn lookup(&self, name: &str, now_ms: u64) -> Option<Cached> {
        self.cache.get(name, now_ms)
    }

    /// Begin a network lookup for `name`.
    pub fn begin(&self, name: &str, id: u16, now_ms: u64) -> Resolve {
        Resolve::new(name, self.servers, self.preferred, id, now_ms)
    }

    /// Record a finished lookup: cache it and remember which server answered
    /// (or, after a total failure, start with the next one next time).
    pub fn finish(&mut self, r: &Resolve, outcome: Outcome, now_ms: u64) {
        match outcome {
            Outcome::Resolved { addr, ttl } => {
                self.cache.insert_addr(r.name(), addr, ttl, now_ms);
            }
            Outcome::NxDomain => self.cache.insert_nxdomain(r.name(), now_ms),
            Outcome::NoData | Outcome::Failed | Outcome::NoServers => {}
        }
        let n = self.servers.len();
        if n > 0 {
            self.preferred = match (outcome, r.answered_by()) {
                (_, Some(i)) => i,
                (Outcome::Failed, None) => (self.preferred + 1) % n,
                _ => self.preferred,
            };
        }
    }
}
