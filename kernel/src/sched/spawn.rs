//! Spawning kernel threads: the guarded stack and the fabricated first frame.

use super::*;

/// Allocate a thread stack: a 4096-aligned heap block of one guard page plus `STACK_SIZE` bytes, then
/// take the guard page out of the page tables so running off the stack faults (see `vm`). Returns the
/// block's regions and the guard page's address, or `None` for the guard if it could not be installed
/// (no physical-memory mapping, huge page), in which case the canary is planted instead.
pub(super) fn alloc_stack(stack_size: usize) -> (paging::StackRegions, Option<u64>) {
    let size = paging::guarded_block_size(stack_size).expect("stack size");
    let layout = Layout::from_size_align(size, PAGE_SIZE as usize).expect("stack layout");
    // SAFETY: `layout` has a non-zero size.
    let base = unsafe { alloc_zeroed(layout) };
    if base.is_null() {
        handle_alloc_error(layout);
    }
    let regions = paging::stack_regions(base as u64, stack_size).expect("page-aligned stack block");
    // The block is never freed, so the unmapped page never gets back to the allocator (whose free-list
    // node writes would fault on it): a dead thread's stack stays allocated for good.
    match crate::vm::unmap_page(regions.guard_start) {
        Ok(()) => (regions, Some(regions.guard_start)),
        Err(e) => {
            crate::klog!(
                Warn,
                "stack guard page unavailable ({e:?}): using the canary instead"
            );
            // SAFETY: `stack_bottom` is 8-aligned and inside the zeroed block we own.
            unsafe { (regions.stack_bottom as *mut u64).write(STACK_CANARY) };
            (regions, None)
        }
    }
}

/// Spawn a preemptible kernel thread starting at `entry` (must never return).
pub fn spawn(name: &'static str, entry: extern "C" fn() -> !) {
    spawn_with_stack(name, entry, STACK_SIZE);
}

/// Like [`spawn`], on a stack of `stack_size` bytes (a multiple of the page size). For threads whose
/// work is deep (the app runtime validates and translates WebAssembly modules).
pub fn spawn_with_stack(name: &'static str, entry: extern "C" fn() -> !, stack_size: usize) {
    let s = scheduler();
    assert!(s.threads.len() < MAX_THREADS, "too many threads");
    STACK_KIB[s.threads.len()].store((stack_size / 1024) as u32, Ordering::Relaxed);

    let (regions, guard) = alloc_stack(stack_size);
    match guard {
        Some(g) => crate::klog!(
            Info,
            "sched: '{name}' stack {:#x}..{:#x}, guard page {g:#x}",
            regions.stack_bottom,
            regions.stack_top
        ),
        None => crate::klog!(
            Info,
            "sched: '{name}' stack {:#x}..{:#x}, no guard page (canary)",
            regions.stack_bottom,
            regions.stack_top
        ),
    }

    // Running stack pointer once the thread is live (≡ 8 mod 16, as if just
    // called, per the SysV ABI).
    let thread_rsp = paging::initial_rsp(regions.stack_top);

    let cs = CS::get_reg().0 as u64;
    let ss = SS::get_reg().0 as u64;

    // Build, from `thread_rsp` downward: an `iretq` frame, then a 15-register
    // block of zeros. The timer ISR epilogue pops the 15 regs then `iretq`s.
    let mut p = thread_rsp;
    let mut push = |val: u64| {
        p -= 8;
        // SAFETY: `p` lies inside the stack block (20 words below `thread_rsp`, far above the bottom, in
        // memory we own) and is 8-aligned (`thread_rsp` is 8 mod 16, steps of 8). The new thread is not
        // on the scheduler list yet, so nobody else reads this memory.
        unsafe { (p as *mut u64).write(val) };
    };
    push(ss); // SS
    push(thread_rsp); // RSP after iretq
    push(0x202); // RFLAGS: IF set + reserved bit
    push(cs); // CS
    push(entry as usize as u64); // RIP
    for _ in 0..15 {
        push(0); // rax..r15 (already zero, but advance the pointer)
    }
    let rsp = p;

    s.threads.push(Thread {
        name,
        cr3: 0,
        rsp0: 0,
        block: None,
        free: false,
        rsp,
        stack_bottom: regions.stack_bottom,
        guard,
        fpu: FxArea::seeded(),
    });
}

/// Called from the timer ISR (clock already advanced): credit the tick to the
/// thread that was running (unless it was idle in `hlt`), then pick the next
/// runnable thread. Touches only scheduler state (no allocation, no locks).
pub extern "C" fn switch_current(rsp: u64) -> u64 {
    reschedule(rsp, true)
}

/// Called from the timer ISR on a sub-tick (`interrupts::set_fast`): the running thread keeps the
/// CPU unless a picture is due and the compositor must put it on screen (`present`).
pub fn sub_tick_switch(rsp: u64) -> u64 {
    if present::due() && current() != 0 {
        reschedule(rsp, false)
    } else {
        rsp
    }
}

/// Called from the yield vector (`int 0x81`): same, but no tick is credited.
pub extern "C" fn yield_switch(rsp: u64) -> u64 {
    reschedule(rsp, false)
}
