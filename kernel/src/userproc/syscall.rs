//! The `int 0x80` entry: the register frame, the call table, `brk` and `write`.

use super::*;

/// The registers `switch.s` saved, lowest address first, followed by the `iretq` frame.
#[repr(C)]
pub(super) struct Regs {
    pub(super) r15: u64,
    pub(super) r14: u64,
    pub(super) r13: u64,
    pub(super) r12: u64,
    pub(super) r11: u64,
    pub(super) r10: u64,
    pub(super) r9: u64,
    pub(super) r8: u64,
    pub(super) rbp: u64,
    pub(super) rdi: u64,
    pub(super) rsi: u64,
    pub(super) rdx: u64,
    pub(super) rcx: u64,
    pub(super) rbx: u64,
    pub(super) rax: u64,
    pub(super) rip: u64,
    pub(super) cs: u64,
    pub(super) rflags: u64,
    pub(super) rsp: u64,
    pub(super) ss: u64,
}

/// Rust half of the `int 0x80` entry (`switch.s`): returns the stack pointer to resume, which is the
/// caller's own unless the call blocked or ended the process.
pub extern "C" fn syscall_entry(rsp: u64) -> u64 {
    // SAFETY: `rsp` is the stack pointer the ISR just built: 15 saved registers plus the CPU's frame,
    // all on the current kernel stack, 8-aligned, live for this call and not aliased.
    let r = unsafe { &mut *(rsp as *mut Regs) };
    if r.cs & 3 != 3 {
        // Only ring 3 may call; a kernel `int 0x80` is a bug, answered without acting on it.
        r.rax = EINVAL as u64;
        return rsp;
    }
    let slot = crate::sched::current();
    let (a, b, c) = (r.rdi, r.rsi, r.rdx);
    match r.rax {
        0 => {
            let code = a as i32;
            if let Some(p) = PROCS.lock().iter_mut().find(|p| p.slot == slot) {
                p.exit = Some(code);
            }
            crate::sched::kill_user(slot);
            crate::sched::yield_switch(rsp)
        }
        1 => {
            r.rax = sys_write(slot, a, b, c) as u64;
            rsp
        }
        2 => {
            r.rax = 0;
            crate::sched::yield_switch(rsp)
        }
        3 => {
            r.rax = PROCS
                .lock()
                .iter()
                .find(|p| p.slot == slot)
                .map_or(0, |p| u64::from(p.pid));
            rsp
        }
        4 => {
            r.rax = 0;
            let ticks = (a.min(60_000) * u64::from(crate::interrupts::TIMER_HZ)).div_ceil(1000);
            crate::sched::set_wake(slot, crate::interrupts::ticks() + ticks.max(1));
            crate::sched::yield_switch(rsp)
        }
        5 => {
            r.rax = crate::netd::now_ms();
            rsp
        }
        6 => {
            r.rax = sys_brk(slot, a);
            rsp
        }
        7 => {
            // Wall clock: seconds since the Unix epoch in the zone the settings chose (what `date` shows).
            let secs = crate::clock::trusted_unix_secs()
                .or_else(|| crate::clock::local_unix_ms().map(|ms| ms / 1000))
                .unwrap_or(0);
            r.rax = (secs as i64 + i64::from(crate::rtc::tz_minutes()) * 60) as u64;
            rsp
        }
        8 => {
            // The interface language (index for `i18n::Lang::from_index`).
            r.rax = u64::from(kitsune_core::i18n::lang() as u8);
            rsp
        }
        9 => {
            // Wall clock in UTC milliseconds (`Date.now()`): the SNTP-corrected time when there is a
            // plausible one, else the uncorrected clock, else 0.
            r.rax = crate::clock::trusted_unix_ms()
                .or_else(crate::clock::local_unix_ms)
                .unwrap_or(0);
            rsp
        }
        remote::SYS_HOST_MODULE_LEN..=remote::SYS_HOST_BATCH => host_call(r, rsp, slot),
        abi::SYS_OPEN..=abi::SYS_GETARGS => file_call(r, rsp, slot),
        abi::SYS_WIN_OPEN..=abi::SYS_WIN_TITLE => file_call(r, rsp, slot),
        abi::SYS_SPAWN..=abi::SYS_PROC_KILL => file_call(r, rsp, slot),
        abi::SYS_PAGE_RECV..=abi::SYS_PAGE_SEND => page_call(r, rsp, slot),
        abi::SYS_AUDIO_OPEN..=abi::SYS_AUDIO_WAIT => audio_call(r, rsp, slot),
        abi::SYS_GETRANDOM => {
            r.rax = sys_getrandom(slot, a, b) as u64;
            rsp
        }
        _ => {
            r.rax = ENOSYS as u64;
            rsp
        }
    }
}

/// `brk`: grow the heap to `want` (page-rounded) with zeroed pages, within the process's limit
/// ([`MAX_HEAP`], or its entry in [`BIG_HEAP`]). The heap
/// only grows; asking for less than the current end, or for too much, changes nothing. Returns the
/// end of the heap after the call.
pub(super) fn sys_brk(slot: usize, want: u64) -> u64 {
    let mut procs = PROCS.lock();
    let Some(p) = procs.iter_mut().find(|p| p.slot == slot) else {
        return 0;
    };
    let Some(asp) = p.aspace.as_mut() else {
        return 0;
    };
    if want <= p.heap_end || want > p.heap_max {
        return p.heap_end;
    }
    let target = want.div_ceil(PAGE_SIZE) * PAGE_SIZE;
    let before = p.heap_end;
    while p.heap_end < target {
        if asp.map_new(p.heap_end, true, false).is_err() {
            break; // out of memory: keep what was mapped
        }
        p.heap_end += PAGE_SIZE;
        p.pages += 1;
    }
    // How big a process's heap gets, in the log every 16 MiB (what a page's scripts cost).
    const MARK: u64 = 16 * 1024 * 1024;
    if (p.heap_end - HEAP_BASE) / MARK != (before - HEAP_BASE) / MARK {
        crate::klog!(
            Info,
            "proc: '{}' pid {} heap {} MiB",
            p.name,
            p.pid,
            (p.heap_end - HEAP_BASE) / (1024 * 1024)
        );
    }
    p.heap_end
}

/// `getrandom(ptr, len)`: `len` bytes (at most `MAX_RANDOM`) from the kernel generator into the
/// process's memory. Returns `len` or `-errno`.
fn sys_getrandom(slot: usize, ptr: u64, len: u64) -> i64 {
    let len = len as usize;
    if len > abi::MAX_RANDOM {
        return EINVAL;
    }
    let mut buf = alloc::vec![0u8; len];
    crate::rng::fill(&mut buf);
    let procs = PROCS.lock();
    let Some(asp) = procs
        .iter()
        .find(|p| p.slot == slot)
        .and_then(|p| p.aspace.as_ref())
    else {
        return EINVAL;
    };
    match asp.poke(ptr, &buf) {
        Ok(()) => len as i64,
        Err(_) => EFAULT,
    }
}

pub(super) fn sys_write(slot: usize, fd: u64, ptr: u64, len: u64) -> i64 {
    if fd != 1 && fd != 2 {
        return EBADF;
    }
    write_out(slot, ptr, len)
}

/// The standard output: kept for the shell to collect, and logged.
pub(super) fn write_out(slot: usize, ptr: u64, len: u64) -> i64 {
    let len = (len as usize).min(MAX_WRITE);
    let mut procs = PROCS.lock();
    let Some(p) = procs.iter_mut().find(|p| p.slot == slot) else {
        return EINVAL;
    };
    let Some(asp) = p.aspace.as_ref() else {
        return EINVAL;
    };
    let mut buf = alloc::vec![0u8; len];
    if asp.copy_in(ptr, &mut buf).is_err() {
        return EFAULT;
    }
    let room = MAX_OUT.saturating_sub(p.out.len());
    p.out.extend_from_slice(&buf[..buf.len().min(room)]);
    let text = String::from_utf8_lossy(&buf);
    for line in text.trim_end_matches('\n').split('\n') {
        crate::klog!(Info, "[{}:{}] {}", p.name, p.pid, line);
    }
    len as i64
}

// ---- WebAssembly host processes ----

/// Block the calling process until woken, then run the same `int 0x80` again (`rip` goes back over
/// the two-byte instruction). Everything the call decides is re-evaluated on restart.
pub(super) fn block_restart(r: &mut Regs, rsp: u64, slot: usize, number: u64) -> u64 {
    r.rax = number; // the restarted instruction needs the call number again
    r.rip -= 2;
    crate::sched::set_wake(slot, crate::sched::FOREVER);
    crate::sched::yield_switch(rsp)
}
