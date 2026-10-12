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
//! | 6 | `brk` | new end of the heap (0 = just ask) | the heap's end after the call (unchanged when refused) |
//! | 16–21 | `host_*` | only for a WebAssembly host process, see [`HostLink`] | |
//! | 48–51 | `win_open`, `win_present`, `win_event`, `win_title` | one window on the desktop, see `kitsune_userabi` and `crate::wasm::manager::native` | |
//! | 32–42 | `open`, `close`, `read`, `write`, `seek`, `stat`, `readdir`, `mkdir`, `remove`, `rename`, `getargs` | files and arguments, see `kitsune_userabi` and [`crate::userfs`] | |
//!
//! Every pointer is translated through the process's own page tables before the kernel touches
//! it (`AddressSpace::copy_in`), so a bad pointer is an error return, never a kernel fault.
//! The calls run with interrupts off, entered through an interrupt gate, and are short.
//!
//! A finished or killed process is reaped from the compositor loop ([`reap`]): its frames and kernel
//! stack are freed and its scheduler slot is reused.

use crate::allocator::SpinLock;
use crate::uvm::{AddressSpace, USER_BASE, USER_END};
use alloc::collections::VecDeque;
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU32, Ordering};
use kitsune_core::appabi::remote::{self, Done, HostCall, Job};
use kitsune_core::elf;
use kitsune_core::paging::PAGE_SIZE;
use kitsune_core::perm::Cred;
use kitsune_userabi as abi;

/// Pages of user stack, and for a WebAssembly host (its interpreter and module translator recurse).
const STACK_PAGES: u64 = 16;
const HOST_STACK_PAGES: u64 = 256;
/// Programs that run the shell engine (deep recursion in the parser and executor) get a larger stack.
const SHELL_STACK_PAGES: u64 = 128;
const BIG_STACK: &[&str] = &["kterm"];
/// The stack's top: the last page of the user range, with the guard page (unmapped) below the stack.
const STACK_TOP: u64 = USER_END - PAGE_SIZE;
/// What an ELF may occupy: the first gigabyte (so the small code model reaches everything).
const IMAGE_END: u64 = 0x4000_0000;
/// Most memory (bytes) a program's segments may ask for.
const MAX_IMAGE: u64 = 8 * 1024 * 1024;
/// Most bytes one `write` accepts.
const MAX_WRITE: usize = 4096;
/// Where a process's heap starts, and the most it may grow to.
const HEAP_BASE: u64 = 0x1000_0000;
const MAX_HEAP: u64 = 8 * 1024 * 1024;
/// The most a WebAssembly host may use (guest memory included).
const MAX_HOST_HEAP: u64 = 96 * 1024 * 1024;
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
    /// The image has no WebAssembly host embedded.
    NoHost,
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
    /// End of the heap (`HEAP_BASE` when it has none).
    heap_end: u64,
    /// What the process wrote and nobody has collected yet (at most [`MAX_OUT`] bytes).
    out: Vec<u8>,
    /// Tick at which the process was seen to be over (0 = not yet), so its output can be collected.
    over_tick: u64,
    /// The most the heap may grow to (end address).
    heap_max: u64,
    /// Set for a WebAssembly host process (`wasm::remote`).
    host: Option<HostLink>,
    /// Who the program acts as when it touches files: the user that started it (`None` before any
    /// account exists: full rights, like the rest of the desktop in early boot).
    cred: Option<Cred>,
    /// The process that started it (`0` = none) and whether it must wait for that parent to collect its end.
    parent: u32,
    keep: bool,
    /// The folder it was started in, and the arguments blob `getargs` returns.
    cwd: String,
    args: Vec<u8>,
    /// What was piped to it (descriptor 0) and how much it has read.
    stdin: Vec<u8>,
    stdin_pos: usize,
    /// Open files (descriptor `FIRST_FD + index`).
    fds: Vec<Option<Fd>>,
    /// The file request in flight (served by the `procfsd` thread, see `userfs`).
    fs: FsState,
    /// Its window (the handle in the app table, `0` = none), the input waiting for it and the tick
    /// at which a `win_event` that waits gives up.
    win: u32,
    events: VecDeque<abi::Event>,
    event_deadline: Option<u64>,
}

/// An open file: where it is and where the next read or write happens.
#[derive(Clone)]
pub struct Fd {
    pub path: Vec<u8>,
    pub pos: u64,
    pub write: bool,
    pub append: bool,
}

/// A file request, with the paths already absolute and copied out of the process.
pub enum FsReq {
    Open {
        path: Vec<u8>,
        flags: u64,
    },
    Read {
        fd: i64,
        ptr: u64,
        len: usize,
    },
    Write {
        fd: i64,
        ptr: u64,
        len: usize,
    },
    Seek {
        fd: i64,
        off: i64,
        whence: u64,
    },
    Stat {
        path: Vec<u8>,
        out: u64,
    },
    ReadDir {
        path: Vec<u8>,
        index: u64,
        out: u64,
        cap: usize,
    },
    Mkdir {
        path: Vec<u8>,
    },
    Remove {
        path: Vec<u8>,
    },
    Rename {
        from: Vec<u8>,
        to: Vec<u8>,
    },
    WinOpen {
        w: i32,
        h: i32,
        title: Vec<u8>,
    },
    WinPresent {
        ptr: u64,
        w: i32,
        h: i32,
    },
    WinTitle {
        title: Vec<u8>,
    },
    /// Start a child program; the bytes are a `kitsune_userabi::pack_spawn` request.
    Spawn {
        req: Vec<u8>,
    },
}

enum FsState {
    Idle,
    Pending(FsReq),
    /// The `procfsd` thread took it.
    Running,
    Done(i64),
}

/// What a program is started with besides its image.
#[derive(Default)]
pub struct Launch {
    /// The process that starts it (`0` = nobody: the shell or the desktop). A parent collects the
    /// child's output and end with `SYS_PROC_POLL`, so the child is kept until it has.
    pub parent: u32,
    pub cwd: String,
    /// Made by `kitsune_userabi::pack_args`.
    pub args: Vec<u8>,
    pub stdin: Vec<u8>,
    pub cred: Option<Cred>,
}

/// The kernel's side of a WebAssembly host process: the module it runs, the job the kernel posted
/// and the host call the process is waiting on. The process blocks in `int 0x80` (the call is
/// restarted when it is woken) until the `appd` thread answers; see `wasm::remote`.
pub struct HostLink {
    module: Arc<[u8]>,
    /// How much of the module the process has read.
    module_pos: usize,
    job: Option<Job>,
    call: CallState,
    done: Option<(Done, Vec<u8>)>,
}

enum CallState {
    Idle,
    /// One call, or a batch of calls without results.
    Pending(Vec<HostCall>),
    /// `appd` took the call and is working on it.
    Serving,
    Served(i64),
}

/// What the process did since `appd` last looked.
pub enum HostEvent {
    Nothing,
    Call(Vec<HostCall>),
    Done(Done, Vec<u8>),
    /// The process is gone (exited, faulted or was killed).
    Dead,
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
pub fn spawn_elf(name: &str, image: &[u8], launch: Launch) -> Result<u32, SpawnError> {
    spawn(name, image, MAX_HEAP, None, launch)
}

/// Start the embedded WebAssembly host as process `name`, to run `module`. `heap` is how much memory
/// it may use for the interpreter, the module and the guest's linear memory.
pub fn spawn_host(name: &str, module: Arc<[u8]>, heap: u64) -> Result<u32, SpawnError> {
    let image = embedded("wasmhost").ok_or(SpawnError::NoHost)?;
    let link = HostLink {
        module,
        module_pos: 0,
        job: None,
        call: CallState::Idle,
        done: None,
    };
    spawn(
        name,
        image,
        heap.min(MAX_HOST_HEAP),
        Some(link),
        Launch::default(),
    )
}

fn spawn(
    name: &str,
    image: &[u8],
    heap: u64,
    host: Option<HostLink>,
    launch: Launch,
) -> Result<u32, SpawnError> {
    if PROCS.lock().len() >= MAX_PROCS {
        return Err(SpawnError::TooMany);
    }
    let stack_pages = if host.is_some() {
        HOST_STACK_PAGES
    } else if BIG_STACK.contains(&name) {
        SHELL_STACK_PAGES
    } else {
        STACK_PAGES
    };
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
        for i in 0..stack_pages {
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
    let tname = if host.is_some() {
        intern("wasmhost")
    } else {
        intern(name)
    };
    let pid = NEXT_PID.fetch_add(1, Ordering::Relaxed);
    let mut proc = Proc {
        pid,
        name: String::from(name),
        slot: usize::MAX,
        aspace: Some(asp),
        exit: None,
        pages,
        heap_end: HEAP_BASE,
        out: Vec::new(),
        over_tick: 0,
        heap_max: HEAP_BASE + heap,
        host,
        cred: launch.cred,
        parent: launch.parent,
        keep: launch.parent != 0,
        cwd: launch.cwd,
        args: launch.args,
        stdin: launch.stdin,
        stdin_pos: 0,
        fds: Vec::new(),
        fs: FsState::Idle,
        win: 0,
        events: VecDeque::new(),
        event_deadline: None,
    };
    // The entry goes into the table in the same critical section that makes the thread runnable (the
    // lock keeps interrupts off): the thread's first system call looks its process up by slot, and
    // must find it (it used to find nothing, or the previous owner of the slot, when a tick fell
    // between the two).
    let mut procs = PROCS.lock();
    let Some(slot) = crate::sched::spawn_user(tname, root, prog.entry, STACK_TOP) else {
        drop(procs);
        if let Some(a) = proc.aspace.take() {
            a.destroy();
        }
        return Err(SpawnError::TooMany);
    };
    proc.slot = slot;
    procs.push(proc);
    drop(procs);
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
            // A WebAssembly app waits on its process: let `appd` see it is gone.
            crate::wasm::wake_worker();
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
            // A child is kept until its parent has collected its end (unless the parent is gone).
            .filter(|p| !(p.keep && procs.iter().any(|q| q.pid == p.parent)))
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
        // Free the thread slot and drop the entry in one critical section: once the slot is free a
        // new process may take it, and its system calls (which look the process up by slot) must
        // never find this one.
        let mut procs = PROCS.lock();
        let Some(i) = procs.iter().position(|p| p.pid == pid) else {
            continue;
        };
        let Some(rel) = crate::sched::release_dead_user(slot) else {
            continue; // still the running thread: next time
        };
        let mut p = procs.remove(i);
        drop(procs);
        let how = match end_of(&p) {
            End::Exited(c) => alloc::format!("exited with status {c}"),
            _ => String::from("was killed"),
        };
        if let Some(a) = p.aspace.take() {
            // A kernel thread may be running on this root (`sched::activate` lends it): leave it first.
            crate::sched::release_cr3(a.root());
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
        remote::SYS_HOST_MODULE_LEN..=remote::SYS_HOST_BATCH => host_call(r, rsp, slot),
        abi::SYS_OPEN..=abi::SYS_GETARGS => file_call(r, rsp, slot),
        abi::SYS_WIN_OPEN..=abi::SYS_WIN_TITLE => file_call(r, rsp, slot),
        abi::SYS_SPAWN..=abi::SYS_PROC_KILL => file_call(r, rsp, slot),
        _ => {
            r.rax = ENOSYS as u64;
            rsp
        }
    }
}

/// `brk`: grow the heap to `want` (page-rounded) with zeroed pages, within [`MAX_HEAP`]. The heap
/// only grows; asking for less than the current end, or for too much, changes nothing. Returns the
/// end of the heap after the call.
fn sys_brk(slot: usize, want: u64) -> u64 {
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
    while p.heap_end < target {
        if asp.map_new(p.heap_end, true, false).is_err() {
            break; // out of memory: keep what was mapped
        }
        p.heap_end += PAGE_SIZE;
        p.pages += 1;
    }
    p.heap_end
}

fn sys_write(slot: usize, fd: u64, ptr: u64, len: u64) -> i64 {
    if fd != 1 && fd != 2 {
        return EBADF;
    }
    write_out(slot, ptr, len)
}

/// The standard output: kept for the shell to collect, and logged.
fn write_out(slot: usize, ptr: u64, len: u64) -> i64 {
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
fn block_restart(r: &mut Regs, rsp: u64, slot: usize, number: u64) -> u64 {
    r.rax = number; // the restarted instruction needs the call number again
    r.rip -= 2;
    crate::sched::set_wake(slot, crate::sched::FOREVER);
    crate::sched::yield_switch(rsp)
}

/// Calls 16–20 (see [`remote`]), for a process that has a [`HostLink`].
fn host_call(r: &mut Regs, rsp: u64, slot: usize) -> u64 {
    let (number, a, b) = (r.rax, r.rdi, r.rsi);
    let mut procs = PROCS.lock();
    let Some(p) = procs.iter_mut().find(|p| p.slot == slot) else {
        r.rax = EINVAL as u64;
        return rsp;
    };
    let (Some(asp), Some(link)) = (p.aspace.as_ref(), p.host.as_mut()) else {
        r.rax = ENOSYS as u64;
        return rsp;
    };
    let mut wake_appd = false;
    let mut block = false;
    r.rax = match r.rax {
        remote::SYS_HOST_MODULE_LEN => link.module.len() as u64,
        remote::SYS_HOST_MODULE_READ => {
            let n = (b as usize).min(link.module.len() - link.module_pos);
            match asp.poke(a, &link.module[link.module_pos..link.module_pos + n]) {
                Ok(()) => {
                    link.module_pos += n;
                    n as u64
                }
                Err(_) => EFAULT as u64,
            }
        }
        remote::SYS_HOST_BATCH => match link.call {
            CallState::Idle => {
                let n = b as usize;
                let mut raw = alloc::vec![0u8; n.min(remote::MAX_BATCH) * 72];
                if n == 0 || n > remote::MAX_BATCH || asp.copy_in(a, &mut raw).is_err() {
                    r.rax = EINVAL as u64;
                    return rsp;
                }
                let calls = raw
                    .as_chunks::<72>()
                    .0
                    .iter()
                    .map(HostCall::from_bytes)
                    .collect();
                link.call = CallState::Pending(calls);
                wake_appd = true;
                block = true;
                0
            }
            CallState::Served(v) => {
                link.call = CallState::Idle;
                v as u64
            }
            _ => {
                block = true;
                0
            }
        },
        remote::SYS_HOST_JOB_WAIT => match link.job {
            Some(job) => match asp.poke(a, &job.to_bytes()) {
                Ok(()) => {
                    link.job = None;
                    0
                }
                Err(_) => EFAULT as u64,
            },
            None => {
                block = true;
                0
            }
        },
        remote::SYS_HOST_CALL => match core::mem::replace(&mut link.call, CallState::Idle) {
            CallState::Idle => {
                let mut raw = [0u8; 72];
                if asp.copy_in(a, &mut raw).is_err() {
                    r.rax = EFAULT as u64;
                    return rsp;
                }
                link.call = CallState::Pending(alloc::vec![HostCall::from_bytes(&raw)]);
                wake_appd = true;
                block = true;
                0
            }
            CallState::Served(v) => v as u64,
            waiting => {
                link.call = waiting;
                block = true;
                0
            }
        },
        _ => {
            // SYS_HOST_JOB_DONE
            let mut raw = [0u8; 40];
            if asp.copy_in(a, &mut raw).is_err() {
                r.rax = EFAULT as u64;
                return rsp;
            }
            let done = Done::from_bytes(&raw);
            let mut msg = alloc::vec![0u8; (done.msg_len as usize).min(remote::MAX_MSG)];
            if !msg.is_empty() && asp.copy_in(done.msg_ptr, &mut msg).is_err() {
                msg.clear();
            }
            link.done = Some((done, msg));
            wake_appd = true;
            0
        }
    };
    drop(procs);
    if wake_appd {
        crate::wasm::wake_worker();
        crate::wasm::hand_off_to_worker();
    }
    if block {
        return block_restart(r, rsp, slot, number);
    }
    rsp
}

fn host_of<T>(pid: u32, f: impl FnOnce(&mut Proc, &mut HostLink) -> T) -> Option<T> {
    let mut procs = PROCS.lock();
    let p = procs.iter_mut().find(|p| p.pid == pid)?;
    let mut link = p.host.take()?;
    let out = f(p, &mut link);
    p.host = Some(link);
    Some(out)
}

/// Is process `pid` still there and running?
pub fn alive(pid: u32) -> bool {
    PROCS
        .lock()
        .iter()
        .any(|p| p.pid == pid && end_of(p) == End::Running)
}

/// Give the host process `pid` its next job. `false` if it is gone or the previous job is unread.
pub fn host_post(pid: u32, job: Job) -> bool {
    let slot = host_of(pid, |p, l| {
        if l.job.is_some() || end_of(p) != End::Running {
            return None;
        }
        l.job = Some(job);
        Some(p.slot)
    });
    match slot.flatten() {
        Some(s) => {
            crate::sched::wake(s);
            crate::sched::hand_off(s);
            true
        }
        None => false,
    }
}

/// What the host process did since the last look (takes a finished job, marks a call as taken).
pub fn host_poll(pid: u32) -> HostEvent {
    host_of(pid, |p, l| {
        if let Some((d, m)) = l.done.take() {
            return HostEvent::Done(d, m);
        }
        if matches!(l.call, CallState::Pending(_))
            && let CallState::Pending(calls) = core::mem::replace(&mut l.call, CallState::Serving)
        {
            return HostEvent::Call(calls);
        }
        if end_of(p) != End::Running {
            return HostEvent::Dead;
        }
        HostEvent::Nothing
    })
    .unwrap_or(HostEvent::Dead)
}

/// Is there something for `appd` to look at? (the condition `appd` blocks on)
pub fn host_ready(pid: u32) -> bool {
    host_of(pid, |p, l| {
        l.done.is_some() || matches!(l.call, CallState::Pending(_)) || end_of(p) != End::Running
    })
    .unwrap_or(true)
}

/// Answer the call `appd` is serving and let the process go on.
pub fn host_reply(pid: u32, value: i64) {
    let slot = host_of(pid, |p, l| {
        l.call = CallState::Served(value);
        p.slot
    });
    if let Some(s) = slot {
        crate::sched::wake(s);
        crate::sched::hand_off(s);
    }
}

/// Bytes copied per lock hold, so interrupts are never off for long.
const CHUNK: usize = 4096;

/// Copy `len` bytes at user address `va` of process `pid`. `None` if any page is not mapped.
pub fn host_read(pid: u32, va: u64, len: usize) -> Option<Vec<u8>> {
    let mut out = alloc::vec![0u8; len];
    let mut done = 0;
    while done < len {
        let n = (len - done).min(CHUNK);
        let at = va.checked_add(done as u64)?;
        let procs = PROCS.lock();
        let asp = procs.iter().find(|p| p.pid == pid)?.aspace.as_ref()?;
        asp.copy_in(at, &mut out[done..done + n]).ok()?;
        done += n;
    }
    Some(out)
}

/// Copy `bytes` to user address `va` of process `pid`. `false` if any page is not mapped.
pub fn host_write(pid: u32, va: u64, bytes: &[u8]) -> bool {
    for (k, chunk) in bytes.chunks(CHUNK).enumerate() {
        let Some(at) = va.checked_add((k * CHUNK) as u64) else {
            return false;
        };
        let procs = PROCS.lock();
        let Some(asp) = procs
            .iter()
            .find(|p| p.pid == pid)
            .and_then(|p| p.aspace.as_ref())
        else {
            return false;
        };
        if asp.poke(at, chunk).is_err() {
            return false;
        }
    }
    true
}

// ---- files and arguments (calls 32-42) ----

fn copy_path(asp: &AddressSpace, cwd: &str, ptr: u64, len: u64) -> Result<Vec<u8>, i64> {
    let len = usize::try_from(len).map_err(|_| abi::errno::EINVAL)?;
    if len == 0 {
        return Err(abi::errno::EINVAL);
    }
    if len > abi::MAX_PATH {
        return Err(abi::errno::ENAMETOOLONG);
    }
    let mut raw = alloc::vec![0u8; len];
    asp.copy_in(ptr, &mut raw).map_err(|_| abi::errno::EFAULT)?;
    let text = core::str::from_utf8(&raw).map_err(|_| abi::errno::EINVAL)?;
    abi::resolve_string(cwd, text)
        .map(String::into_bytes)
        .ok_or(abi::errno::ENAMETOOLONG)
}

/// Calls 32–42. The ones that touch the disk are only *queued* here (this runs with interrupts off,
/// where a disk transfer must not happen and a held lock could never be waited for): the process
/// blocks, the `procfsd` thread does the work in thread context and the call is restarted to pick
/// the result up, the same way the WebAssembly host's calls wait.
fn file_call(r: &mut Regs, rsp: u64, slot: usize) -> u64 {
    let number = r.rax;
    let (a, b, c, d, e) = (r.rdi, r.rsi, r.rdx, r.r10, r.r8);
    let mut procs = PROCS.lock();
    let Some(p) = procs.iter_mut().find(|p| p.slot == slot) else {
        r.rax = EINVAL as u64;
        return rsp;
    };
    // A request already in flight (this is the restarted call): wait, or take its result.
    match core::mem::replace(&mut p.fs, FsState::Idle) {
        FsState::Done(v) => {
            r.rax = v as u64;
            return rsp;
        }
        busy @ (FsState::Pending(_) | FsState::Running) => {
            p.fs = busy;
            drop(procs);
            return block_restart(r, rsp, slot, number);
        }
        FsState::Idle => {}
    }
    let Some(asp) = p.aspace.as_ref() else {
        r.rax = EINVAL as u64;
        return rsp;
    };
    let cwd = p.cwd.as_str();
    let req = match number {
        abi::SYS_GETARGS => {
            let n = p.args.len().min(b as usize);
            r.rax = match asp.poke(a, &p.args[..n]) {
                Ok(()) => p.args.len() as u64,
                Err(_) => EFAULT as u64,
            };
            return rsp;
        }
        abi::SYS_CLOSE => {
            r.rax = close_fd(p, a as i64) as u64;
            return rsp;
        }
        abi::SYS_READ if a == 0 => {
            // Standard input: what was piped in; no disk involved.
            let len = (c as usize).min(abi::MAX_IO);
            let have = p.stdin.len().saturating_sub(p.stdin_pos);
            let n = len.min(have);
            r.rax = match asp.poke(b, &p.stdin[p.stdin_pos..p.stdin_pos + n]) {
                Ok(()) => {
                    p.stdin_pos += n;
                    n as u64
                }
                Err(_) => EFAULT as u64,
            };
            return rsp;
        }
        abi::SYS_WRITE if a == 1 || a == 2 => {
            drop(procs);
            r.rax = write_out(slot, b, c) as u64;
            return rsp;
        }
        abi::SYS_PROC_POLL => {
            drop(procs);
            r.rax = child_poll(slot, a, b, c) as u64;
            return rsp;
        }
        abi::SYS_PROC_KILL => {
            drop(procs);
            r.rax = child_kill(slot, a) as u64;
            return rsp;
        }
        abi::SYS_SPAWN => {
            // (req_ptr, req_len)
            let len = b as usize;
            if !(abi::SPAWN_HEAD..=abi::MAX_SPAWN).contains(&len) {
                Err(abi::errno::EINVAL)
            } else {
                let mut raw = alloc::vec![0u8; len];
                asp.copy_in(a, &mut raw)
                    .map(|()| FsReq::Spawn { req: raw })
                    .map_err(|_| abi::errno::EFAULT)
            }
        }
        abi::SYS_WIN_EVENT => {
            // (ptr, timeout_ms): one input event, or wait for one up to the timeout.
            let now = crate::interrupts::ticks();
            if let Some(ev) = p.events.pop_front() {
                p.event_deadline = None;
                r.rax = match asp.poke(a, &ev.to_bytes()) {
                    Ok(()) => 1,
                    Err(_) => EFAULT as u64,
                };
                return rsp;
            }
            let deadline = match p.event_deadline {
                Some(d) => d,
                None => {
                    let ticks = if b == u64::MAX {
                        crate::sched::FOREVER
                    } else {
                        now.saturating_add(
                            (b.min(3_600_000) * u64::from(crate::interrupts::TIMER_HZ))
                                .div_ceil(1000),
                        )
                    };
                    p.event_deadline = Some(ticks);
                    ticks
                }
            };
            if b == 0 || now >= deadline {
                p.event_deadline = None;
                r.rax = 0;
                return rsp;
            }
            drop(procs);
            // Restart this call when woken (an event arrived) or when the deadline passes.
            r.rax = number;
            r.rip -= 2;
            crate::sched::set_wake(slot, deadline);
            return crate::sched::yield_switch(rsp);
        }
        abi::SYS_WIN_OPEN => {
            // (w, h, title_ptr, title_len)
            let (w, h) = (a as i64, b as i64);
            if w < 1 || h < 1 || w > i64::from(abi::WIN_MAX_W) || h > i64::from(abi::WIN_MAX_H) {
                Err(abi::errno::EINVAL)
            } else if p.win != 0 {
                Err(abi::errno::EEXIST)
            } else {
                copy_title(asp, c, d).map(|title| FsReq::WinOpen {
                    w: w as i32,
                    h: h as i32,
                    title,
                })
            }
        }
        abi::SYS_WIN_PRESENT => {
            let (w, h) = (b as i64, c as i64);
            if p.win == 0 {
                Err(abi::errno::EBADF)
            } else if w < 1
                || h < 1
                || w > i64::from(abi::WIN_MAX_W)
                || h > i64::from(abi::WIN_MAX_H)
            {
                Err(abi::errno::EINVAL)
            } else {
                Ok(FsReq::WinPresent {
                    ptr: a,
                    w: w as i32,
                    h: h as i32,
                })
            }
        }
        abi::SYS_WIN_TITLE => {
            if p.win == 0 {
                Err(abi::errno::EBADF)
            } else {
                copy_title(asp, a, b).map(|title| FsReq::WinTitle { title })
            }
        }
        abi::SYS_OPEN => copy_path(asp, cwd, a, b).map(|path| FsReq::Open { path, flags: c }),
        abi::SYS_READ => Ok(FsReq::Read {
            fd: a as i64,
            ptr: b,
            len: (c as usize).min(abi::MAX_IO),
        }),
        abi::SYS_WRITE => Ok(FsReq::Write {
            fd: a as i64,
            ptr: b,
            len: (c as usize).min(abi::MAX_IO),
        }),
        abi::SYS_SEEK => Ok(FsReq::Seek {
            fd: a as i64,
            off: b as i64,
            whence: c,
        }),
        abi::SYS_STAT => copy_path(asp, cwd, a, b).map(|path| FsReq::Stat { path, out: c }),
        abi::SYS_READDIR => copy_path(asp, cwd, a, b).map(|path| FsReq::ReadDir {
            path,
            index: c,
            out: d,
            cap: e as usize,
        }),
        abi::SYS_MKDIR => copy_path(asp, cwd, a, b).map(|path| FsReq::Mkdir { path }),
        abi::SYS_REMOVE => copy_path(asp, cwd, a, b).map(|path| FsReq::Remove { path }),
        // SYS_RENAME: (from, from_len, to, to_len)
        _ => copy_path(asp, cwd, a, b)
            .and_then(|from| copy_path(asp, cwd, c, d).map(|to| FsReq::Rename { from, to })),
    };
    match req {
        Ok(req) => {
            p.fs = FsState::Pending(req);
            drop(procs);
            crate::userfs::wake();
            crate::userfs::hand_off();
            block_restart(r, rsp, slot, number)
        }
        Err(e) => {
            r.rax = e as u64;
            rsp
        }
    }
}

/// `SYS_PROC_POLL`: move up to `cap` bytes of child `pid`'s output to the caller and say how it stands.
/// The child counts as over only once its output is all taken; then it may be reaped.
fn child_poll(slot: usize, pid: u64, ptr: u64, cap: u64) -> i64 {
    let cap = (cap as usize).min(abi::MAX_IO);
    let mut procs = PROCS.lock();
    let Some(me) = procs.iter().find(|p| p.slot == slot).map(|p| p.pid) else {
        return abi::errno::EINVAL;
    };
    let Some(c) = procs
        .iter_mut()
        .find(|p| u64::from(p.pid) == pid && p.parent == me)
    else {
        return abi::errno::ESRCH;
    };
    let n = c.out.len().min(cap);
    let chunk: Vec<u8> = c.out.drain(..n).collect();
    let state = match end_of(c) {
        _ if !c.out.is_empty() => abi::ChildState::Running,
        End::Running => abi::ChildState::Running,
        End::Exited(code) => abi::ChildState::Exited(code as i16),
        _ => abi::ChildState::Killed,
    };
    if state != abi::ChildState::Running {
        c.keep = false;
    }
    let ok = procs
        .iter()
        .find(|p| p.pid == me)
        .and_then(|p| p.aspace.as_ref())
        .is_some_and(|a| a.poke(ptr, &chunk).is_ok());
    if ok {
        abi::pack_poll(n, state)
    } else {
        abi::errno::EFAULT
    }
}

/// `SYS_PROC_KILL`: stop a child of the caller.
fn child_kill(slot: usize, pid: u64) -> i64 {
    let target = {
        let procs = PROCS.lock();
        let Some(me) = procs.iter().find(|p| p.slot == slot).map(|p| p.pid) else {
            return abi::errno::EINVAL;
        };
        procs
            .iter()
            .find(|p| u64::from(p.pid) == pid && p.parent == me)
            .map(|p| p.pid)
    };
    match target {
        Some(t) if kill(t) => 0,
        Some(_) => 0, // already over
        None => abi::errno::ESRCH,
    }
}

fn copy_title(asp: &AddressSpace, ptr: u64, len: u64) -> Result<Vec<u8>, i64> {
    let len = usize::try_from(len).map_err(|_| abi::errno::EINVAL)?;
    if len > abi::WIN_MAX_TITLE {
        return Err(abi::errno::ENAMETOOLONG);
    }
    let mut raw = alloc::vec![0u8; len];
    asp.copy_in(ptr, &mut raw).map_err(|_| abi::errno::EFAULT)?;
    // Printable ASCII only (the title bar draws it as it is).
    if raw.iter().any(|b| !(0x20..0x7F).contains(b)) {
        return Err(abi::errno::EINVAL);
    }
    Ok(raw)
}

fn close_fd(p: &mut Proc, fd: i64) -> i64 {
    let Ok(i) = usize::try_from(fd - abi::FIRST_FD) else {
        return abi::errno::EBADF;
    };
    match p.fds.get_mut(i) {
        Some(slot @ Some(_)) => {
            *slot = None;
            0
        }
        _ => abi::errno::EBADF,
    }
}

// What the `procfsd` thread (see `userfs`) does with the processes' tables.

/// Take a queued file request: (pid, request, who it acts as). `None` when nothing is waiting.
pub(crate) fn fs_take() -> Option<(u32, FsReq, Option<Cred>)> {
    let mut procs = PROCS.lock();
    for p in procs.iter_mut() {
        if end_of(p) != End::Running {
            continue;
        }
        if matches!(p.fs, FsState::Pending(_))
            && let FsState::Pending(req) = core::mem::replace(&mut p.fs, FsState::Running)
        {
            return Some((p.pid, req, p.cred.clone()));
        }
    }
    None
}

/// Is a request waiting (what the daemon blocks on)?
pub(crate) fn fs_pending() -> bool {
    PROCS
        .lock()
        .iter()
        .any(|p| matches!(p.fs, FsState::Pending(_)) && end_of(p) == End::Running)
}

/// A copy of descriptor `fd` of process `pid`.
pub(crate) fn fs_fd(pid: u32, fd: i64) -> Option<Fd> {
    let i = usize::try_from(fd - abi::FIRST_FD).ok()?;
    PROCS
        .lock()
        .iter()
        .find(|p| p.pid == pid)?
        .fds
        .get(i)?
        .clone()
}

/// Replace descriptor `fd` of `pid` (after a read, write or seek moved it).
pub(crate) fn fs_set_fd(pid: u32, fd: i64, new: Fd) {
    let Ok(i) = usize::try_from(fd - abi::FIRST_FD) else {
        return;
    };
    if let Some(slot) = PROCS
        .lock()
        .iter_mut()
        .find(|p| p.pid == pid)
        .and_then(|p| p.fds.get_mut(i))
    {
        *slot = Some(new);
    }
}

/// Give `pid` the new descriptor `fd`: its number, or `EMFILE`.
pub(crate) fn fs_new_fd(pid: u32, fd: Fd) -> i64 {
    let mut procs = PROCS.lock();
    let Some(p) = procs.iter_mut().find(|p| p.pid == pid) else {
        return abi::errno::EBADF;
    };
    if let Some(i) = p.fds.iter().position(Option::is_none) {
        p.fds[i] = Some(fd);
        return abi::FIRST_FD + i as i64;
    }
    if p.fds.len() >= abi::MAX_FDS {
        return abi::errno::EMFILE;
    }
    p.fds.push(Some(fd));
    abi::FIRST_FD + p.fds.len() as i64 - 1
}

/// Finish the request of `pid` with `value` and let the process go on.
pub(crate) fn fs_done(pid: u32, value: i64) {
    let slot = {
        let mut procs = PROCS.lock();
        let Some(p) = procs.iter_mut().find(|p| p.pid == pid) else {
            return;
        };
        p.fs = FsState::Done(value);
        p.slot
    };
    crate::sched::wake(slot);
    crate::sched::hand_off(slot);
}

// ---- the window of a process ----

/// Queue an input event for the window of process `pid` and wake it if it waits (pointer motion
/// replaces the one before it).
pub(crate) fn win_push(pid: u32, ev: abi::Event) {
    let slot = {
        let mut procs = PROCS.lock();
        let Some(p) = procs.iter_mut().find(|p| p.pid == pid) else {
            return;
        };
        if ev.kind == abi::EV_POINTER
            && let Some(last) = p.events.back_mut()
            && last.kind == abi::EV_POINTER
            && last.c == ev.c
        {
            *last = ev;
        } else if ev.kind == abi::EV_RESIZE
            && let Some(last) = p.events.back_mut()
            && last.kind == abi::EV_RESIZE
        {
            *last = ev;
        } else if p.events.len() < 256 {
            p.events.push_back(ev);
        }
        p.slot
    };
    crate::sched::wake(slot);
}

/// How process `pid` ended (`Running` while it lives; a process no longer in the table exited).
pub(crate) fn end(pid: u32) -> End {
    PROCS
        .lock()
        .iter()
        .find(|p| p.pid == pid)
        .map_or(End::Exited(0), end_of)
}

/// The program's name and the window it owns (`0` = none).
pub(crate) fn win_of(pid: u32) -> Option<(String, u32)> {
    let procs = PROCS.lock();
    let p = procs.iter().find(|p| p.pid == pid)?;
    Some((p.name.clone(), p.win))
}

/// Remember the window `handle` of process `pid`.
pub(crate) fn set_win(pid: u32, handle: u32) {
    if let Some(p) = PROCS.lock().iter_mut().find(|p| p.pid == pid) {
        p.win = handle;
    }
}
