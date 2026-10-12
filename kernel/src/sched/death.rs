//! Thread death: which thread a stack-guard fault belongs to, and killing the current thread without returning to it.

use super::*;

unsafe extern "C" {
    /// `switch.s`: load `rsp` (a context saved by the ISR: 15 GPRs, then an `iretq` frame), pop it and
    /// `iretq` into that thread. Never returns.
    fn resume_context(rsp: u64) -> !;
}

/// The thread whose stack guard page contains `addr`, if any: a fault there is that thread's stack
/// overflow. Touches only scheduler fields the ISR leaves alone; callable from the #PF handler.
pub fn guard_owner(addr: u64) -> Option<&'static str> {
    // SAFETY: read-only walk of `threads`, which `spawn` resizes only with IF=0 and the fault handler
    // runs on one core, so it is not resized under us; `name`/`guard` never change after `spawn`.
    // NOTE: this shared ref can overlap the ISR's `&mut` (not guaranteed by the type).
    let s = unsafe { (*SCHED.get()).as_ref() }?;
    s.threads
        .iter()
        .find(|t| t.guard.is_some_and(|g| paging::guard_hit(addr, g)))
        .map(|t| t.name)
}

/// `true` if thread slot `id` has died (see [`kill_current`]).
pub fn is_dead(id: usize) -> bool {
    id < MAX_THREADS && DEAD[id].load(Ordering::Acquire)
}

/// Can a failure in the running thread be contained to that thread?
///
/// `if_was_set` is the interrupt flag of the failing context (RFLAGS.IF of the
/// faulting code, or the live flag for a panic). Containment needs all of:
///
/// - **not the compositor** (slot 0): the desktop is the one thread whose loss is
///   the machine's loss; it keeps the red error screen;
/// - **IF was set**: plain thread context. Interrupt gates and [`SpinLock`] both
///   run with IF clear, so IF=1 proves we were in neither an ISR (a half-served IRQ,
///   no EOI) nor inside the heap lock or another critical section, so no lock
///   is left held by a thread that vanishes;
/// - **not already killing**: a fault inside [`kill_current`] is fatal.
///
/// [`SpinLock`]: crate::allocator::SpinLock
pub fn containable(if_was_set: bool) -> bool {
    if_was_set && current() != 0 && !KILLING.load(Ordering::Acquire) && thread_count() > 1
}

/// Kill the running thread and switch to another one; never returns.
///
/// Logs the thread's name and `reason` on COM1, marks the slot dead and jumps
/// straight into the next runnable thread's saved context with `resume_context`,
/// abandoning the current stack (which may be the exhausted one that just
/// overflowed, or the IST stack of the fault). Callers check [`containable`] first.
pub fn kill_current(reason: core::fmt::Arguments<'_>) -> ! {
    x86_64::instructions::interrupts::disable();
    if KILLING.swap(true, Ordering::AcqRel) {
        crate::crash::halt(); // fault while killing (callers should have checked)
    }
    let cur = current();
    crate::klog!(Error, "thread '{}' died: {}", thread_name(cur), reason);
    if cur < MAX_THREADS {
        DEAD[cur].store(true, Ordering::Release);
        IDLE[cur].store(false, Ordering::Relaxed);
    }

    let s = scheduler();
    let now = crate::interrupts::ticks();
    let Some(next) =
        kitsune_core::schedule::next_runnable(cur, s.threads.len(), |i| runnable(i, now))
    else {
        crate::serial_println!("no runnable thread left");
        crate::crash::halt();
    };
    s.current = next;
    CURRENT.store(next, Ordering::Relaxed);
    // SAFETY: `fpu_ptr()` is the 16-aligned 512-byte area of `next` (never freed), seeded or written by
    // `fxsave`, so `fxrstor` cannot #GP; same as the restore in `reschedule`.
    unsafe {
        core::arch::asm!(
            "fxrstor [{}]", in(reg) s.threads[next].fpu_ptr(),
            options(nostack, readonly, preserves_flags),
        );
    }
    activate(&s.threads[next]);
    KILLING.store(false, Ordering::Release);
    // SAFETY: `next != cur` is runnable and not the running thread, so its `rsp` was saved by the ISR
    // (timer or yield) when it was switched out: 15 GPRs followed by an `iretq` frame, exactly what
    // `resume_context` pops. IF=0 until that `iretq`, so no tick can intervene.
    unsafe { resume_context(s.threads[next].rsp) }
}
