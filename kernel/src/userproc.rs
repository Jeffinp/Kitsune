//! User processes: ELF programs that run in ring 3 behind a small system-call interface.
//!
//! A process is a scheduler thread (see `sched::spawn_user`) plus an [`AddressSpace`](crate::uvm)
//! that holds only what the ELF and the process itself mapped. Its code cannot read or write kernel
//! memory, other processes' memory, or I/O ports: any attempt is a page fault or a general-protection
//! fault, which kills that process and nothing else. The only way out is `int 0x80`:
//!
//! | `rax` | call | arguments (`rdi`, `rsi`, `rdx`) | returns |
//! |---|---|---|---|
//! | 0 | `exit` | status | never |
//! | 1 | `write` | fd (1 or 2), pointer, length | bytes written, or a negative error |
//! | 2 | `yield` | | 0 |
//! | 3 | `getpid` | | the process id |
//! | 4 | `sleep_ms` | milliseconds | 0 |
//! | 5 | `uptime_ms` | | milliseconds since boot |
//!
//! Every pointer is translated through the process's own page tables before the kernel touches
//! it (`AddressSpace::copy_in`), so a bad pointer is an error return, never a kernel fault.
//! The calls run with interrupts off, entered through an interrupt gate, and are short.
//!
//! A finished or killed process is reaped from the compositor loop ([`reap`]): its frames and kernel
//! stack are freed and its scheduler slot is reused.

use crate::allocator::SpinLock;
use crate::uvm::{AddressSpace, USER_BASE, USER_END};
use alloc::string::String;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU32, Ordering};
use kitsune_core::elf;
use kitsune_core::paging::PAGE_SIZE;

/// Pages of user stack.
const STACK_PAGES: u64 = 16;
/// The stack's top: the last page of the user range, with the guard page (unmapped) below the stack.
const STACK_TOP: u64 = USER_END - PAGE_SIZE;
/// What an ELF may occupy: the first gigabyte (so the small code model reaches everything).
const IMAGE_END: u64 = 0x4000_0000;
/// Most memory (bytes) a program's segments may ask for.
const MAX_IMAGE: u64 = 8 * 1024 * 1024;
/// Most bytes one `write` accepts.
const MAX_WRITE: usize = 4096;
/// Most uncollected output kept per process.
const MAX_OUT: usize = 64 * 1024;
/// How long a finished process waits for its output to be collected, in ticks (2 s).
const LINGER_TICKS: u64 = 500;
/// Processes alive at once.
const MAX_PROCS: usize = 8;

const ENOSYS: i64 = -38;
const EINVAL: i64 = -22;
const EFAULT: i64 = -14;
const EBADF: i64 = -9;

/// Why a program could not start.
#[derive(Debug)]
pub enum SpawnError {
    BadElf(elf::ElfError),
    OutOfMemory,
    TooMany,
}

/// How a process ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum End {
    Running,
    Exited(i32),
    Killed,
}

struct Proc {
    pid: u32,
    name: String,
    slot: usize,
    aspace: Option<AddressSpace>,
    /// Set by `exit`; a process whose thread died without it was killed.
    exit: Option<i32>,
    pages: usize,
    /// What the process wrote and nobody has collected yet (at most [`MAX_OUT`] bytes).
    out: Vec<u8>,
    /// Tick at which the process was seen to be over (0 = not yet), so its output can be collected.
    over_tick: u64,
}

/// Process ids start high, clear of the desktop's own process table (`ps` lists both).
static NEXT_PID: AtomicU32 = AtomicU32::new(5000);
static PROCS: SpinLock<Vec<Proc>> = SpinLock::new(Vec::new());
/// Thread names are `&'static str`; the few distinct program names are interned (and leaked once).
static NAMES: SpinLock<Vec<&'static str>> = SpinLock::new(Vec::new());

fn intern(name: &str) -> &'static str {
    let mut n = NAMES.lock();
    if let Some(s) = n.iter().find(|s| **s == name) {
        return s;
    }
    if n.len() >= 32 {
        return "user";
    }
    let leaked: &'static str = alloc::boxed::Box::leak(String::from(name).into_boxed_str());
    n.push(leaked);
    leaked
}

include!(concat!(env!("OUT_DIR"), "/user_gen.rs"));

/// The embedded program called `name`, if any.
pub fn embedded(name: &str) -> Option<&'static [u8]> {
    USER_PROGRAMS
        .iter()
        .find(|(n, _)| *n == name)
        .map(|(_, b)| *b)
}

/// Start the ELF program `image` as process `name`. Returns its process id.
pub fn spawn_elf(name: &str, image: &[u8]) -> Result<u32, SpawnError> {
    if PROCS.lock().len() >= MAX_PROCS {
        return Err(SpawnError::TooMany);
    }
    let prog = elf::parse(image, USER_BASE, IMAGE_END, MAX_IMAGE).map_err(SpawnError::BadElf)?;
    let mut asp = AddressSpace::new().map_err(|_| SpawnError::OutOfMemory)?;
    let mut pages = 0usize;
    let build = (|| -> Result<(), crate::uvm::UvmError> {
        for seg in &prog.segments {
            let first = seg.vaddr & !(PAGE_SIZE - 1);
            let last = (seg.vaddr + seg.mem_size).div_ceil(PAGE_SIZE) * PAGE_SIZE;
            let mut va = first;
            while va < last {
                asp.map_new(va, seg.write, seg.exec)?;
                pages += 1;
                va += PAGE_SIZE;
            }
            asp.poke(
                seg.vaddr,
                &image[seg.file_off..seg.file_off + seg.file_size],
            )?;
        }
        for i in 0..STACK_PAGES {
            asp.map_new(STACK_TOP - (i + 1) * PAGE_SIZE, true, false)?;
            pages += 1;
        }
        Ok(())
    })();
    if build.is_err() {
        asp.destroy();
        return Err(SpawnError::OutOfMemory);
    }
    let root = asp.root();
    let tname = intern(name);
    let Some(slot) = crate::sched::spawn_user(tname, root, prog.entry, STACK_TOP) else {
        asp.destroy();
        return Err(SpawnError::TooMany);
    };
    let pid = NEXT_PID.fetch_add(1, Ordering::Relaxed);
    PROCS.lock().push(Proc {
        pid,
        name: String::from(name),
        slot,
        aspace: Some(asp),
        exit: None,
        pages,
        out: Vec::new(),
        over_tick: 0,
    });
    crate::klog!(
        Info,
        "proc: started '{name}' pid {pid} (slot {slot}, {pages} pages, entry {:#x})",
        prog.entry
    );
    Ok(pid)
}

/// A line of the process table for Tarefas.
pub struct Info {
    pub pid: u32,
    pub name: String,
    pub pages: usize,
    pub end: End,
}

/// Every process still in the table (running, or finished and not reaped yet).
pub fn list() -> Vec<Info> {
    PROCS
        .lock()
        .iter()
        .map(|p| Info {
            pid: p.pid,
            name: p.name.clone(),
            pages: p.pages,
            end: end_of(p),
        })
        .collect()
}

fn end_of(p: &Proc) -> End {
    match p.exit {
        Some(c) => End::Exited(c),
        None if crate::sched::is_dead(p.slot) => End::Killed,
        None => End::Running,
    }
}

/// What process `pid` wrote since the last call, and whether it is over. A process that is not in the
/// table any more (reaped) reads as over with no output.
pub fn poll(pid: u32) -> (Vec<u8>, Option<End>) {
    let mut procs = PROCS.lock();
    match procs.iter_mut().find(|p| p.pid == pid) {
        Some(p) => {
            let out = core::mem::take(&mut p.out);
            let end = end_of(p);
            (out, (end != End::Running).then_some(end))
        }
        None => (Vec::new(), Some(End::Exited(0))),
    }
}

/// Stop process `pid` (it dies at the next tick). `false` when there is no such running process.
pub fn kill(pid: u32) -> bool {
    let procs = PROCS.lock();
    match procs.iter().find(|p| p.pid == pid) {
        Some(p) if end_of(p) == End::Running => {
            crate::sched::kill_user(p.slot);
            true
        }
        _ => false,
    }
}

/// Free everything a finished process owned and forget it. Called from the compositor loop.
pub fn reap() {
    // Cheap exit for the common case: nothing to reap.
    let candidates: Vec<(u32, usize)> = {
        let procs = PROCS.lock();
        procs
            .iter()
            .filter(|p| end_of(p) != End::Running)
            .filter(|p| {
                p.out.is_empty()
                    || p.over_tick != 0
                        && crate::interrupts::ticks().saturating_sub(p.over_tick) > LINGER_TICKS
            })
            .map(|p| (p.pid, p.slot))
            .collect()
    };
    // Note when each finished process was first seen over (its output lingers a little).
    for p in PROCS.lock().iter_mut() {
        if p.over_tick == 0 && end_of(p) != End::Running {
            p.over_tick = crate::interrupts::ticks().max(1);
        }
    }
    for (pid, slot) in candidates {
        let Some(rel) = crate::sched::release_dead_user(slot) else {
            continue; // still the running thread: next time
        };
        let mut procs = PROCS.lock();
        let Some(i) = procs.iter().position(|p| p.pid == pid) else {
            continue;
        };
        let mut p = procs.remove(i);
        drop(procs);
        let how = match end_of(&p) {
            End::Exited(c) => alloc::format!("exited with status {c}"),
            _ => String::from("was killed"),
        };
        if let Some(a) = p.aspace.take() {
            a.destroy();
        }
        let (used, total) = crate::uvm::frame_stats();
        crate::klog!(
            Info,
            "proc: '{}' pid {} {how} (frames in use: {used} of {total})",
            p.name,
            p.pid
        );
        // The kernel stack: restore the guard page, then give the block back to the heap.
        if let Some(g) = rel.guard {
            let _ = crate::vm::remap_page(g);
        }
        let (base, size) = rel.block;
        if let Ok(layout) = core::alloc::Layout::from_size_align(size, PAGE_SIZE as usize) {
            // SAFETY: `base`/`layout` are exactly what `sched::alloc_stack` allocated for this thread,
            // the thread is dead and not running (`release_dead_user`), and its guard page is mapped
            // again so the allocator may write a free-list node there.
            unsafe { alloc::alloc::dealloc(base as *mut u8, layout) };
        }
    }
}

// ---- system calls ----

/// The registers `switch.s` saved, lowest address first, followed by the `iretq` frame.
#[repr(C)]
struct Regs {
    r15: u64,
    r14: u64,
    r13: u64,
    r12: u64,
    r11: u64,
    r10: u64,
    r9: u64,
    r8: u64,
    rbp: u64,
    rdi: u64,
    rsi: u64,
    rdx: u64,
    rcx: u64,
    rbx: u64,
    rax: u64,
    rip: u64,
    cs: u64,
    rflags: u64,
    rsp: u64,
    ss: u64,
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
        _ => {
            r.rax = ENOSYS as u64;
            rsp
        }
    }
}

fn sys_write(slot: usize, fd: u64, ptr: u64, len: u64) -> i64 {
    if fd != 1 && fd != 2 {
        return EBADF;
    }
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
