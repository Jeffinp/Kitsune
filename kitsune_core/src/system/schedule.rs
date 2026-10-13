//! Pure round-robin choice for the kernel scheduler.
//!
//! The kernel keeps per-thread state in atomics (wake tick, dead flag); this
//! module only decides *which slot runs next* given a "may this slot run?"
//! predicate, so the ordering rules (and the "dead threads are never picked"
//! guarantee) are unit-tested on the host.

/// The next thread to run after `cur` out of `n` slots, round-robin.
///
/// `runnable(i)` says whether slot `i` may be scheduled (ready and not dead).
/// Slots are tried in the order `cur + 1, cur + 2, ..., cur` — the current
/// thread is the *last* candidate, so it keeps the CPU when nobody else can
/// run, but only if it is still runnable itself. Returns `None` when no slot at
/// all is runnable (for example the current thread has just died and every
/// other slot is blocked); the caller must then have another way to make
/// progress (the kernel never lets the compositor, slot 0, die).
pub fn next_runnable(cur: usize, n: usize, runnable: impl Fn(usize) -> bool) -> Option<usize> {
    if n == 0 {
        return None;
    }
    (1..=n).map(|step| (cur + step) % n).find(|&c| runnable(c))
}

/// Like [`next_runnable`], but a slot for which `held(i)` is true (a demoted thread whose turn has
/// not come) is passed over while another thread than `cur` can run. It never leaves the CPU to
/// `cur` or idle because of that: when nobody else can run, the held slot runs.
pub fn next_runnable_held(
    cur: usize,
    n: usize,
    runnable: impl Fn(usize) -> bool,
    held: impl Fn(usize) -> bool,
) -> Option<usize> {
    match next_runnable(cur, n, |i| runnable(i) && !held(i)) {
        Some(i) if i != cur => Some(i),
        _ => next_runnable(cur, n, runnable),
    }
}

#[cfg(test)]
mod tests;
