//! Introspection for Tarefas: thread count, names, CPU ticks and stack sizes.

use super::*;

/// Index of the thread that is running right now (timer-ISR attribution).
pub fn current() -> usize {
    CURRENT.load(Ordering::Relaxed)
}

pub fn thread_count() -> usize {
    // SAFETY: read-only, compositor thread. `threads` is only resized by `spawn` (IF=0), and the
    // ISR changes `current`/`rsp`/fpu contents, not the length.
    // NOTE: this shared ref can overlap the ISR's `&mut` (not guaranteed by the type).
    unsafe { (*SCHED.get()).as_ref() }.map_or(0, |s| s.threads.len())
}

pub fn thread_name(i: usize) -> &'static str {
    // SAFETY: as in `thread_count` (`name` is never changed after `spawn`).
    unsafe { (*SCHED.get()).as_ref() }
        .and_then(|s| s.threads.get(i))
        .map_or("", |t| t.name)
}

/// `true` if the thread in slot `i` died (shown as DEAD in Tarefas).
pub fn thread_dead(i: usize) -> bool {
    is_dead(i)
}

/// Cumulative "ticks found running" of every scheduler slot (the resource monitor
/// turns the deltas into CPU shares).
pub fn busy_ticks() -> [u64; kitsune_core::sysmon::MAX_THREADS] {
    core::array::from_fn(|i| TICKS[i].load(Ordering::Relaxed))
}

/// Stack size of slot `i` in KiB: the compositor runs on the bootloader's stack
/// (`BOOT_CONFIG.kernel_stack_size`), the others on a `STACK_SIZE` block.
pub fn thread_stack_kib(i: usize) -> u32 {
    if i == 0 {
        512
    } else {
        match STACK_KIB.get(i).map(|k| k.load(Ordering::Relaxed)) {
            Some(k) if k != 0 => k,
            _ => (STACK_SIZE / 1024) as u32,
        }
    }
}

/// Stack size of each slot in KiB (0 = the default `STACK_SIZE`), set by `spawn_with_stack`.
pub(super) static STACK_KIB: [AtomicU32; MAX_THREADS] = [const { AtomicU32::new(0) }; MAX_THREADS];

const _: () = assert!(MAX_THREADS == kitsune_core::sysmon::MAX_THREADS);

pub(super) fn scheduler() -> &'static mut Scheduler {
    // SAFETY: only called from `spawn`, which every caller runs with IF=0 (`without_interrupts` in
    // `kernel_main`), so the timer ISR cannot touch SCHED meanwhile.
    // NOTE: not guaranteed by the type: safe fn returning `&'static mut`, and `spawn`'s IF=0
    // precondition is not enforced (docs/audit/01-memoria-unsafe.md #6).
    unsafe { (*SCHED.get()).as_mut().expect("scheduler not initialized") }
}
