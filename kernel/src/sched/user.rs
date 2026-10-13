//! User threads (ring 3): spawning one in its own address space, waking, killing and reaping it.

use super::*;

/// What a reaped user thread leaves for its owner to free.
pub struct Released {
    /// The heap block of its kernel stack (base, size) and the guard page inside it.
    pub block: (u64, usize),
    pub guard: Option<u64>,
}

/// Spawn a user thread: ring 3 code at `rip` with stack pointer `user_rsp`, in the address space whose
/// page-table root is `cr3`. Its kernel stack has a guard page like any thread's. Returns its slot, or
/// `None` when every slot is in use. Callable from thread context (it masks interrupts itself).
pub fn spawn_user(name: &'static str, cr3: u64, rip: u64, user_rsp: u64) -> Option<usize> {
    x86_64::instructions::interrupts::without_interrupts(|| {
        let s = scheduler();
        let slot = match s.threads.iter().position(|t| t.free) {
            Some(i) => i,
            None if s.threads.len() < MAX_THREADS => s.threads.len(),
            None => return None,
        };
        let stack_size = STACK_SIZE;
        let (regions, guard) = alloc_stack(stack_size);
        let top = regions.stack_top;
        // The CPU pushes the ring 3 -> 0 frame below `RSP0`; keep it 16-aligned.
        let rsp0 = top & !0xF;
        let mut p = rsp0;
        let mut push = |val: u64| {
            p -= 8;
            // SAFETY: `p` stays inside the freshly allocated stack block (20 words below its top) and
            // is 8-aligned; nobody else sees this memory before the thread is runnable.
            unsafe { (p as *mut u64).write(val) };
        };
        push(u64::from(crate::gdt::user_data())); // SS
        push(user_rsp); // RSP
        push(0x202); // RFLAGS: IF set
        push(u64::from(crate::gdt::user_code())); // CS
        push(rip); // RIP
        for _ in 0..15 {
            push(0);
        }
        let t = Thread {
            name,
            cr3,
            rsp0,
            block: Some((
                regions.guard_start,
                paging::guarded_block_size(stack_size).unwrap_or(0),
            )),
            free: false,
            rsp: p,
            stack_bottom: regions.stack_bottom,
            guard,
            fpu: FxArea::seeded(),
        };
        if slot < s.threads.len() {
            s.threads[slot] = t;
        } else {
            s.threads.push(t);
        }
        DEAD[slot].store(false, Ordering::Release);
        IDLE[slot].store(false, Ordering::Relaxed);
        DEMOTED[slot].store(false, Ordering::Relaxed);
        TICKS[slot].store(0, Ordering::Relaxed);
        WAKE[slot].store(0, Ordering::Release);
        STACK_KIB[slot].store((stack_size / 1024) as u32, Ordering::Relaxed);
        Some(slot)
    })
}

/// Block thread `slot` until tick `until` (used by a system call, which switches away right after).
pub fn set_wake(slot: usize, until: u64) {
    if slot < MAX_THREADS {
        WAKE[slot].store(until, Ordering::SeqCst);
    }
}

/// Mark user thread `slot` dead from outside (an administrator killed the process). It stops being
/// scheduled at the next tick; the owner reaps it with [`release_dead_user`].
pub fn kill_user(slot: usize) {
    if slot < MAX_THREADS && slot != 0 {
        DEAD[slot].store(true, Ordering::Release);
    }
}

/// Take the resources of the dead user thread in `slot` so they can be freed, and let the slot be
/// reused. `None` when the slot is not a dead user thread, or it is the thread running right now
/// (its kernel stack is in use).
pub fn release_dead_user(slot: usize) -> Option<Released> {
    x86_64::instructions::interrupts::without_interrupts(|| {
        let s = scheduler();
        if slot == 0 || slot >= s.threads.len() || slot == s.current {
            return None;
        }
        let dead = DEAD[slot].load(Ordering::Acquire);
        let t = &mut s.threads[slot];
        if !dead || t.cr3 == 0 || t.free {
            return None;
        }
        t.free = true;
        let block = t.block.take()?;
        Some(Released {
            block,
            guard: t.guard,
        })
    })
}
