//! DNS client logic: query/response messages (RFC 1035, A records), a TTL cache
//! and a resolver session that rotates through several servers with timeouts.
//!
//! Like the rest of the network code this is pure: the kernel owns the UDP
//! socket and the clock (milliseconds, injected), this module builds and parses
//! the bytes and decides what to send next and when to give up.
//!
//! * [`build_query`] / [`parse_response`]: one `A IN` question, recursion
//!   desired. Responses are only accepted when the transaction id, the question
//!   and (in [`Resolve`]) the server address match, and answer records only count
//!   if their owner is the name asked for or a CNAME target reached from it, so
//!   an off-path spoofer needs the id and a plausible message, and unrelated
//!   records are never trusted. Compression pointers must point strictly
//!   backwards (no loops).
//! * [`DnsCache`]: positive entries live for the record TTL (clamped to
//!   [`MAX_TTL_SECS`]; a TTL of 0 is not cached), NXDOMAIN for
//!   [`NEGATIVE_TTL_SECS`]; bounded capacity, the entry closest to expiry is the
//!   one evicted.
//! * [`Resolve`]: one lookup. Attempt `k` goes to server `(start + k) mod n`; an
//!   attempt waits [`ATTEMPT_TIMEOUT_MS`], a SERVFAIL/REFUSED/garbled answer moves
//!   to the next server at once, NXDOMAIN and NODATA are final. Each server is
//!   tried up to [`ROUNDS`] times within [`TOTAL_TIMEOUT_MS`].
//! * [`Resolver`] ties them together and remembers which server answered last so
//!   the next lookup starts there (a dead first server costs one timeout, not one
//!   per lookup).

use crate::network::net::{DnsServers, Ipv4};
use alloc::string::String;
use alloc::vec::Vec;

mod cache;
mod message;
mod resolve;
pub use cache::*;
pub use message::*;
pub use resolve::*;

pub const DNS_PORT: u16 = 53;
const HDR: usize = 12;
const TYPE_A: u16 = 1;
const TYPE_CNAME: u16 = 5;
const CLASS_IN: u16 = 1;
/// Longest name on the wire in presentation form (RFC 1035: 255 incl. dots and
/// the root; 253 visible characters).
pub const MAX_NAME: usize = 253;
const MAX_LABEL: usize = 63;
/// Compression pointers followed per name before giving up.
const MAX_JUMPS: usize = 16;
/// CNAME hops accepted in one answer.
const MAX_CNAMES: usize = 8;
/// A records kept from one answer.
pub const MAX_ADDRS: usize = 4;
/// Longest cache lifetime honored for a positive answer.
pub const MAX_TTL_SECS: u32 = 3600;
/// Lifetime of a cached NXDOMAIN.
pub const NEGATIVE_TTL_SECS: u32 = 30;
/// Cache capacity (entries).
pub const CACHE_CAP: usize = 32;
/// How long one attempt (one server) is waited for.
pub const ATTEMPT_TIMEOUT_MS: u64 = 1500;
/// Times each server is tried before the lookup fails.
pub const ROUNDS: usize = 2;
/// Hard bound for a whole lookup.
pub const TOTAL_TIMEOUT_MS: u64 = 8000;
#[cfg(test)]
mod tests;
