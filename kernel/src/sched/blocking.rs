//! The blocking API of thread context (IF=1): yield, wake, block until a deadline, idle.

use super::*;

/// Hand the CPU to the next runnable thread right now (`int 0x81`). Returns when
/// this thread is scheduled again, immediately if nothing else can run. A no-op
/// with interrupts disabled (the yield ISR would resume us with IF=1).
pub fn yield_now() {
    if !x86_64::instructions::interrupts::are_enabled() {
        return;
    }
    // SAFETY: vector 0x81 (`YIELD_VECTOR`) holds `yield_isr`, which saves and restores every GPR
    // and returns with `iretq`; IF is set, so the saved RFLAGS resumes with IF=1 as it was.
    unsafe {
        core::arch::asm!("int 0x81", options(nostack));
    }
}

/// Make thread `id` runnable (no-op if it already is). Callable from any context.
pub fn wake(id: usize) {
    if id < MAX_THREADS {
        WAKE[id].store(0, Ordering::SeqCst);
    }
}

/// `true` if some thread other than `me` is runnable right now.
pub(super) fn others_ready(me: usize) -> bool {
    let n = thread_count().min(MAX_THREADS);
    let now = crate::interrupts::ticks();
    (0..n).any(|i| i != me && runnable(i, now))
}

/// Sleep in `hlt` until the next interrupt, unless `skip()` says there is
/// already something to do. Interrupts are masked while `skip()` is evaluated
/// and `sti; hlt` re-enables them atomically, so an event that arrives between
/// the check and the halt is not lost (it just ends the `hlt` at once).
pub(super) fn halt_unless(me: usize, skip: impl Fn() -> bool) {
    x86_64::instructions::interrupts::disable();
    if skip() {
        x86_64::instructions::interrupts::enable();
        return;
    }
    let slot = me.min(MAX_THREADS - 1);
    IDLE[slot].store(true, Ordering::Relaxed);
    x86_64::instructions::interrupts::enable_and_hlt();
    IDLE[slot].store(false, Ordering::Relaxed);
}

/// Block the calling thread until tick `until` ([`FOREVER`] = no deadline) or
/// until another thread calls [`wake`] on it, whichever comes first. `still_idle`
/// is the caller's "there is still nothing to do" test: it is re-evaluated *after*
/// the block is announced, so a `wake` that raced with the announcement is not
/// lost (announce, then re-check, then sleep).
pub fn block(until: u64, still_idle: impl Fn() -> bool) {
    let me = current();
    let runnable = || WAKE[me].load(Ordering::SeqCst) <= crate::interrupts::ticks();
    WAKE[me].store(until, Ordering::SeqCst);
    while still_idle() && !runnable() {
        yield_now(); // the ISR skips us now; we return once runnable (or alone)
        if runnable() {
            break;
        }
        halt_unless(me, || runnable() || !still_idle());
    }
    WAKE[me].store(0, Ordering::SeqCst);
}

/// The compositor's idle step (replaces a bare `hlt`): if `has_work()` return
/// at once; if another thread is runnable give it the CPU now instead of
/// halting through our slice; otherwise `sti; hlt` until the next interrupt.
pub fn idle(has_work: impl Fn() -> bool) {
    let me = current();
    x86_64::instructions::interrupts::disable();
    if has_work() {
        x86_64::instructions::interrupts::enable();
    } else if others_ready(me) {
        x86_64::instructions::interrupts::enable();
        yield_now();
    } else {
        let slot = me.min(MAX_THREADS - 1);
        IDLE[slot].store(true, Ordering::Relaxed);
        x86_64::instructions::interrupts::enable_and_hlt();
        IDLE[slot].store(false, Ordering::Relaxed);
    }
}
