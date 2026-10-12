//! The ping mailbox: how any thread asks `netd` for an echo request and reads the answer (atomics only), and the statistics accessors.

use super::*;

pub(super) const P_IDLE: u8 = 0;

pub(super) const P_REQUESTED: u8 = 1;

pub(super) const P_RUNNING: u8 = 2;

pub(super) const P_DONE: u8 = 3;

/// A requester is filling in the parameters.
pub(super) const P_CLAIMED: u8 = 4;

pub(super) static P_STATE: AtomicU8 = AtomicU8::new(P_IDLE);

pub(super) static P_TARGET: AtomicU32 = AtomicU32::new(0);

pub(super) static P_TIMEOUT_MS: AtomicU32 = AtomicU32::new(0);

pub(super) static P_RESULT: AtomicU64 = AtomicU64::new(0);

/// netd exists (a NIC was found and the fetcher owns the stack).
pub(super) static UP: AtomicBool = AtomicBool::new(false);

pub(super) fn encode_result(r: Result<u64, PingError>) -> u64 {
    // bits 0..=31 rtt_us (saturated) | bits 32..=39 error kind | bits 40..=47 error arg
    match r {
        Ok(us) => us.min(u64::from(u32::MAX)),
        Err(e) => {
            let (kind, arg) = match e {
                PingError::Timeout => (1u64, 0u64),
                PingError::Unreachable(c) => (2, u64::from(c)),
                PingError::TimeExceeded => (3, 0),
                PingError::NoRoute => (4, 0),
                PingError::ArpFailed => (5, 0),
                PingError::BadTarget => (6, 0),
                PingError::NoNetwork => (7, 0),
                PingError::Busy => (8, 0),
            };
            kind << 32 | arg << 40
        }
    }
}

pub(super) fn decode_result(v: u64) -> Result<u64, PingError> {
    let arg = (v >> 40) as u8;
    match (v >> 32) as u8 {
        0 => Ok(v & u64::from(u32::MAX)),
        1 => Err(PingError::Timeout),
        2 => Err(PingError::Unreachable(arg)),
        3 => Err(PingError::TimeExceeded),
        4 => Err(PingError::NoRoute),
        5 => Err(PingError::ArpFailed),
        6 => Err(PingError::BadTarget),
        8 => Err(PingError::Busy),
        _ => Err(PingError::NoNetwork),
    }
}

/// Ask netd to ping `target` (non-blocking): the answer is collected with
/// [`ping_poll`]. `Err(Busy)` while a page is loading or another ping is
/// running, `Err(NoNetwork)` without a NIC. `timeout_ms` bounds the wait for the
/// echo reply (the ARP resolution of the next hop comes on top, at most ~1 s).
#[allow(dead_code)] // public API for the terminal and other callers (not wired to a command yet)
pub fn ping_start(target: Ipv4, timeout_ms: u32) -> Result<(), PingError> {
    if !UP.load(Ordering::Acquire) || crate::fetch::worker_dead() {
        return Err(PingError::NoNetwork);
    }
    if !crate::fetch::is_idle() {
        return Err(PingError::Busy);
    }
    // Claim the mailbox: from idle, or over a finished ping nobody collected.
    let claimed = P_STATE
        .compare_exchange(P_IDLE, P_CLAIMED, Ordering::AcqRel, Ordering::Acquire)
        .or_else(|_| {
            P_STATE.compare_exchange(P_DONE, P_CLAIMED, Ordering::AcqRel, Ordering::Acquire)
        });
    if claimed.is_err() {
        return Err(PingError::Busy);
    }
    P_TARGET.store(u32::from_be_bytes(target.0), Ordering::Relaxed);
    P_TIMEOUT_MS.store(timeout_ms.clamp(10, 60_000), Ordering::Relaxed);
    P_STATE.store(P_REQUESTED, Ordering::Release);
    crate::fetch::wake_worker();
    Ok(())
}

/// The finished ping's round trip in microseconds, or why it failed; `None` while it
/// has not finished (or none was started).
#[allow(dead_code)] // public API for the terminal and other callers (not wired to a command yet)
pub fn ping_poll_us() -> Option<Result<u64, PingError>> {
    if P_STATE
        .compare_exchange(P_DONE, P_IDLE, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        return None;
    }
    Some(decode_result(P_RESULT.load(Ordering::Acquire)))
}

/// Like [`ping_poll_us`] with the round trip in whole milliseconds (at least 1 for a
/// reply that arrived).
#[allow(dead_code)] // public API for the terminal and other callers (not wired to a command yet)
pub fn ping_poll() -> Option<Result<u32, PingError>> {
    ping_poll_us().map(|r| r.map(icmp::rtt_ms))
}

/// Ping `target` and wait for the answer: the round trip in milliseconds. The
/// caller sleeps (halted or yielding, never spinning) until the answer or the
/// timeout. Fine for a worker thread; from the compositor it would freeze the UI
/// for up to `timeout_ms` (but not deadlock), so the terminal uses [`ping_start`] +
/// [`ping_poll`] instead.
#[allow(dead_code)] // public API for the terminal and other callers (not wired to a command yet)
pub fn ping(target: Ipv4, timeout_ms: u32) -> Result<u32, PingError> {
    ping_us(target, timeout_ms).map(icmp::rtt_ms)
}

/// [`ping`] returning the round trip in microseconds.
#[allow(dead_code)] // public API for the terminal and other callers (not wired to a command yet)
pub fn ping_us(target: Ipv4, timeout_ms: u32) -> Result<u64, PingError> {
    ping_start(target, timeout_ms)?;
    // ARP (<= ~1 s) + echo timeout + one service period of slack.
    let limit = interrupts::ticks()
        + (u64::from(timeout_ms) + 1500) * u64::from(interrupts::TIMER_HZ) / 1000
        + IDLE_POLL_TICKS;
    loop {
        if let Some(r) = ping_poll_us() {
            return r;
        }
        if interrupts::ticks() >= limit {
            return Err(PingError::Timeout);
        }
        // `idle` halts until the next interrupt (or yields to a runnable thread) and, unlike
        // `block`, never parks the caller: it is safe from any thread, the compositor included
        // (the scheduler must always have a runnable thread, and the compositor is it).
        crate::sched::idle(|| P_STATE.load(Ordering::Acquire) == P_DONE);
    }
}

/// A ping request is waiting for netd (used by the fetcher's block condition).
pub fn ping_pending() -> bool {
    P_STATE.load(Ordering::Acquire) == P_REQUESTED
}

/// A copy of the interface statistics (packets, lease, resolver, ping counters).
pub fn stats() -> kitsune_core::netstats::Snapshot {
    STATS.snapshot(now_ms())
}

/// Print the statistics on the serial port (called from the `perf-trace` report).
pub fn log_stats() {
    if UP.load(Ordering::Relaxed) {
        serial_println!("[trace] {}", stats());
    }
}
