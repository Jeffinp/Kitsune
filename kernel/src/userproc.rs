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
//! | 7 | `time` | | seconds since 1970 on the wall clock, in the user's time zone |
//! | 8 | `language` | | the interface language |
//! | 9 | `unix_ms` | | milliseconds since 1970 in UTC (`0` without a clock) |
//! | 16–21 | `host_*` | only for a WebAssembly host process, see [`HostLink`] | |
//! | 48–51 | `win_open`, `win_present`, `win_event`, `win_title` | one window on the desktop, see `kitsune_userabi` and `crate::wasm::manager::native` | |
//! | 32–42 | `open`, `close`, `read`, `write`, `seek`, `stat`, `readdir`, `mkdir`, `remove`, `rename`, `getargs` | files and arguments, see `kitsune_userabi` and [`crate::userfs`] | |
//! | 52–54 | `spawn`, `proc_poll`, `proc_kill` | child processes | |
//! | 56–57 | `page_recv`, `page_send` | only the script process of a browser page or a media server (`spawn_page`, `spawn_media`), see `page.rs` | |
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

mod files;
mod host;
mod page;
mod queue;
mod syscall;
use files::*;
pub use host::*;
use page::*;
pub use page::{page_post, page_take, spawn_media, spawn_page};
pub(crate) use queue::*;
pub use syscall::syscall_entry;
use syscall::*;

/// Pages of user stack, and for a WebAssembly host (its interpreter and module translator recurse).
const STACK_PAGES: u64 = 16;
const HOST_STACK_PAGES: u64 = 256;
/// Embedded programs that recurse deeply get a larger stack, in pages: the shell engine's parser and
/// executor (512 KiB) and the JavaScript engine (4 MiB: QuickJS uses ~1 KiB of C stack per JavaScript
/// call, so this allows ~3800 nested calls), and the media decoder (1 MiB: libopus keeps its scratch
/// buffers on the stack as C99 arrays).
const BIG_STACK: &[(&str, u64)] = &[("kterm", 128), ("jsrun", 1024), ("mediadec", 256)];
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
/// Embedded programs allowed a larger heap: the JavaScript engine (scripts and their objects) and the
/// media decoder (VP9 keeps up to 8 reference pictures and a few in flight: ~10 MiB at 640x360, ~40
/// MiB at 1280x720, more at 1920x1080, the largest it accepts). The heap only grows as it is used.
const BIG_HEAP: &[(&str, u64)] = &[("jsrun", 64 * 1024 * 1024), ("mediadec", 128 * 1024 * 1024)];
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
    /// The link with the browser, for the script process of a page (`page.rs`).
    page: Option<PageLink>,
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
    /// Give it the link with the browser (the script process of a page).
    pub page: bool,
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
    spawn(name, image, heap_limit(name, image), None, launch)
}
/// True when `image` is the embedded program `name` itself, not a file that happens to have its
/// name: only those get the larger stack and heap of [`BIG_STACK`] and [`BIG_HEAP`].
fn is_embedded(name: &str, image: &[u8]) -> bool {
    embedded(name)
        .is_some_and(|e| core::ptr::eq(e.as_ptr(), image.as_ptr()) && e.len() == image.len())
}
/// The most the heap of program `name` may grow to.
fn heap_limit(name: &str, image: &[u8]) -> u64 {
    match BIG_HEAP.iter().find(|(n, _)| *n == name) {
        Some(&(_, bytes)) if is_embedded(name, image) => bytes,
        _ => MAX_HEAP,
    }
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
    let stack_pages = match BIG_STACK.iter().find(|(n, _)| *n == name) {
        _ if host.is_some() => HOST_STACK_PAGES,
        Some(&(_, pages)) if is_embedded(name, image) => pages,
        _ => STACK_PAGES,
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
        page: launch.page.then(PageLink::default),
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
