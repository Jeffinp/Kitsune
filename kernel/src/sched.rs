//! Preemptive round-robin kernel scheduler with blocked threads.
//!
//! Each thread owns a heap-allocated stack and a saved stack pointer. The PIT
//! timer fires a naked ISR (see `interrupts`) that saves the full interrupted
//! context, calls [`switch_current`] to pick the next thread, and restores +
//! `iretq`s into it — so any thread is preempted on a timer tick without
//! cooperating. New threads are launched by fabricating an initial stack frame
//! that the ISR epilogue + `iretq` "resume" into the entry function.
//!
//! # Blocked threads
//!
//! Each thread has a wake-up tick in `WAKE`; it is *runnable* iff
//! `WAKE[i] <= now` (0 = ready, [`FOREVER`] = parked until [`wake`]). The ISR
//! round-robin skips threads that are not runnable, so an idle worker costs no
//! slice. A thread blocks itself with [`block`] (which also hands the CPU over
//! at once through the yield vector, [`yield_now`]) and is released by the tick
//! reaching its deadline or by [`wake`]. If nothing at all is runnable the ISR
//! leaves the current thread in place; every wait loop re-checks its condition
//! after `hlt`, so that is safe. All of this state is plain atomics, so the ISR
//! (IF=0, never allocates) and thread code can share it without locks.
//!
//! Tarefas's per-thread "CPU" counts only the timer ticks that found
//! the thread *running* (`IDLE` marks the ones spent in `hlt`).
//!
//! # Dead threads
//!
//! A thread other than the compositor (slot 0) that panics, takes a CPU
//! exception or trips its stack canary is not allowed to stop the machine: it is
//! marked *dead* (`DEAD`), never scheduled again, and the CPU is handed to
//! another thread without returning to it ([`kill_current`], or the ISR itself
//! for the canary). The compositor, a fault with interrupts off (inside an ISR or
//! a spin lock: the interrupted code held state nobody can repair), a double
//! fault, and any fault *while* a thread is being killed stay fatal
//! (`crash::die`). Resources a dead thread owned are not reclaimed: its stack and
//! whatever it had allocated stay allocated, and a lock it held without
//! disabling interrupts stays held. The heap lock cannot be one of those, because
//! it keeps interrupts off while held (see [`containable`]).

use crate::sync::RacyCell;
use alloc::alloc::{Layout, alloc_zeroed, handle_alloc_error};
use alloc::boxed::Box;
use alloc::vec;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering};
use kitsune_core::paging::{self, PAGE_SIZE};
use x86_64::registers::segmentation::{CS, SS, Segment};

// Per-thread stack. Sized generously (128 KiB) because the background fetcher
// runs the TLS 1.3 handshake (P-256 + record processing) on its own stack. Each one is
// a page-aligned heap block with an unmapped guard page below it (see `alloc_stack`).
const STACK_SIZE: usize = 128 * 1024;
const MAX_THREADS: usize = 16;

/// Sentinel written to the lowest 8 bytes of a spawned stack *that has no guard page* (the
/// fallback when the page tables cannot be edited, see `alloc_stack`). A stack overflow grows
/// *downward* past the usable region, so a mismatch seen on context switch means "this thread
/// overflowed" and its thread is killed, instead of the wild writes silently corrupting the heap.
///
/// With a guard page the canary is redundant and is not planted: an overflow cannot get past the
/// guard (rustc probes every page of a large frame, so even a 150 KiB frame touches it), and
/// the guard faults at the first bad access instead of at the next tick.
const STACK_CANARY: u64 = 0xDEAD_C0DE_CAFE_F00D;

/// 512-byte, 16-byte-aligned save area for `fxsave`/`fxrstor` (x87 + SSE state:
/// xmm0..15, MXCSR, FPU control/status). One per thread so float/SSE registers
/// survive preemption — without this, a thread preempted mid-SSE would have its
/// `xmm`/`MXCSR` clobbered by another thread.
#[repr(C, align(16))]
struct FxArea([u8; 512]);

impl FxArea {
    /// Allocate a save area seeded with the *current* (valid) FPU/SSE state, so
    /// a thread's first `fxrstor` loads a sane MXCSR/control word rather than
    /// garbage (a bad MXCSR reserved bit would `#GP` on restore).
    fn seeded() -> Box<FxArea> {
        let mut area = Box::new(FxArea([0u8; 512]));
        // SAFETY: `area` is a live 512-byte `FxArea`, align(16) as `fxsave` requires (the heap honours
        // the alignment); `fxsave` writes only those bytes (no `nomem` in the options).
        unsafe {
            core::arch::asm!(
                "fxsave [{}]", in(reg) area.0.as_mut_ptr(),
                options(nostack, preserves_flags),
            );
        }
        area
    }
}

/// Physical address of the kernel's own page-table root (the bootloader's), captured by [`init`].
static KERNEL_CR3: AtomicU64 = AtomicU64::new(0);

/// Make `t`'s address space and ring 3 entry stack the live ones. Runs inside the context-switch
/// ISR / `kill_current` (IF=0), right before the new thread's registers are restored.
///
/// A kernel thread (`cr3 == 0`) keeps whatever root is loaded: every user root carries the kernel's
/// own mappings, and the kernel never touches the user half (SMAP), so reloading CR3 only to run
/// kernel code would flush the TLB for nothing (the exchange between a process and `appd` happens
/// at every host call). [`release_cr3`] undoes the borrow before a root is freed.
fn activate(t: &Thread) {
    let (cur, flags) = x86_64::registers::control::Cr3::read();
    let want = if t.cr3 == 0 {
        cur.start_address().as_u64()
    } else {
        t.cr3
    };
    if cur.start_address().as_u64() != want {
        let frame =
            x86_64::structures::paging::PhysFrame::containing_address(x86_64::PhysAddr::new(want));
        // SAFETY: `want` is the kernel's own root or a user root built by `uvm`, which copies every
        // kernel mapping into it, so the code, stacks and data running now stay mapped.
        unsafe { x86_64::registers::control::Cr3::write(frame, flags) };
    }
    if t.rsp0 != 0 {
        crate::gdt::set_rsp0(t.rsp0);
    }
}

/// Make sure the root `root` is not the live one (a kernel thread may be borrowing it), so it can be
/// freed. Interrupts must be off or the caller must be a kernel thread that does not mind the switch.
pub fn release_cr3(root: u64) {
    x86_64::instructions::interrupts::without_interrupts(|| {
        let (cur, flags) = x86_64::registers::control::Cr3::read();
        let kernel = KERNEL_CR3.load(Ordering::Relaxed);
        if cur.start_address().as_u64() == root && kernel != 0 {
            let frame = x86_64::structures::paging::PhysFrame::containing_address(
                x86_64::PhysAddr::new(kernel),
            );
            // SAFETY: `kernel` is the root the kernel booted with; it maps everything the caller runs.
            unsafe { x86_64::registers::control::Cr3::write(frame, flags) };
        }
    });
}

/// Index of the running thread (read by Tarefas).
static CURRENT: AtomicUsize = AtomicUsize::new(0);
/// Timer ticks that found each thread slot *running* (not parked in `hlt`).
static TICKS: [AtomicU64; MAX_THREADS] = [const { AtomicU64::new(0) }; MAX_THREADS];
/// Wake-up tick per slot: runnable iff `WAKE[i] <= now`. 0 = ready.
static WAKE: [AtomicU64; MAX_THREADS] = [const { AtomicU64::new(0) }; MAX_THREADS];
/// `WAKE` value meaning "blocked until [`wake`] is called".
pub const FOREVER: u64 = u64::MAX;
/// Threads that died (see [`kill_current`]). A dead slot is never scheduled again.
static DEAD: [AtomicBool; MAX_THREADS] = [const { AtomicBool::new(false) }; MAX_THREADS];
/// Set while [`kill_current`] runs, so a fault inside it is not "contained" a second time.
static KILLING: AtomicBool = AtomicBool::new(false);
/// Set by a thread while it sits in `hlt` waiting for an interrupt, so the tick
/// sampler does not charge that time as CPU use.
static IDLE: [AtomicBool; MAX_THREADS] = [const { AtomicBool::new(false) }; MAX_THREADS];

struct Thread {
    name: &'static str,
    /// Physical address of the page-table root of a user process; `0` = the kernel's own tables.
    cr3: u64,
    /// Top of the kernel stack the CPU loads on a ring 3 -> ring 0 transition (`0`: kernel thread).
    rsp0: u64,
    /// The heap block the stack lives in (base, size in bytes), for freeing a reaped user thread.
    block: Option<(u64, usize)>,
    /// A dead user thread whose resources were released: the slot may be reused by `spawn_user`.
    free: bool,
    rsp: u64,
    /// Lowest usable stack address, `0` for the boot thread (its stack is the bootloader's).
    stack_bottom: u64,
    /// The unmapped guard page just below the stack (`None`: no guard, the canary stands in).
    guard: Option<u64>,
    fpu: Box<FxArea>, // x87/SSE save area, swapped on context switch
}

impl Thread {
    #[inline]
    fn fpu_ptr(&self) -> *mut u8 {
        // Box gives a stable, 16-byte-aligned address (FxArea is align(16)).
        &*self.fpu as *const FxArea as *mut u8
    }

    /// `true` if this thread's stack canary is intact (or it has none: the boot thread, or a stack
    /// with a guard page). A `false` means the stack overflowed its bounds.
    #[inline]
    fn stack_intact(&self) -> bool {
        if self.stack_bottom == 0 || self.guard.is_some() {
            return true;
        }
        // SAFETY: `stack_bottom` is the base of a stack block `spawn` allocated and never frees, 8-aligned,
        // with the canary written there; reading 8 bytes stays inside the block.
        unsafe { (self.stack_bottom as *const u64).read_volatile() == STACK_CANARY }
    }
}

struct Scheduler {
    threads: Vec<Thread>,
    current: usize,
}

static SCHED: RacyCell<Option<Scheduler>> = RacyCell::new(None);

/// Register the current (boot) context as thread 0. Run before interrupts.
pub fn init() {
    // The boot thread runs on the bootloader's stack, which has a guard page of its own: find it so a
    // stack overflow in the compositor is reported as such (and stays fatal).
    let boot_guard = crate::vm::find_guard_below(current_sp());
    KERNEL_CR3.store(
        x86_64::registers::control::Cr3::read()
            .0
            .start_address()
            .as_u64(),
        Ordering::Relaxed,
    );
    let boot = Thread {
        name: "compositor",
        cr3: 0,
        rsp0: 0,
        block: None,
        free: false,
        rsp: 0, // captured on the first preemption
        stack_bottom: 0,
        guard: boot_guard,
        fpu: FxArea::seeded(),
    };
    // SAFETY: runs on the boot thread before `interrupts::init()` enables IF (see `kernel_main`),
    // and the timer ISR is the only other user of SCHED, so nothing accesses it concurrently.
    unsafe {
        *SCHED.get() = Some(Scheduler {
            threads: vec![boot],
            current: 0,
        });
    }
}

/// Current stack pointer (to locate the boot stack).
fn current_sp() -> u64 {
    let sp: u64;
    // SAFETY: reads RSP into a register; no memory access, no stack use, no flags change.
    unsafe {
        core::arch::asm!("mov {}, rsp", out(reg) sp, options(nomem, nostack, preserves_flags));
    }
    sp
}

/// Allocate a thread stack: a 4096-aligned heap block of one guard page plus `STACK_SIZE` bytes, then
/// take the guard page out of the page tables so running off the stack faults (see `vm`). Returns the
/// block's regions and the guard page's address, or `None` for the guard if it could not be installed
/// (no physical-memory mapping, huge page), in which case the canary is planted instead.
fn alloc_stack(stack_size: usize) -> (paging::StackRegions, Option<u64>) {
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

/// Called from the yield vector (`int 0x81`): same, but no tick is credited.
pub extern "C" fn yield_switch(rsp: u64) -> u64 {
    reschedule(rsp, false)
}

/// The thread the next voluntary switch should go to (`usize::MAX` = none): a direct hand-off, so two
/// threads that wait on each other (a user process and `appd`) do not queue behind unrelated work
/// at every exchange.
static HANDOFF: AtomicUsize = AtomicUsize::new(usize::MAX);

/// Ask that the next yield (not a timer preemption) go to `slot`, if it can run then.
pub fn hand_off(slot: usize) {
    HANDOFF.store(slot, Ordering::Relaxed);
}

/// May slot `i` be scheduled at tick `now`: not dead and not blocked.
#[inline]
fn runnable(i: usize, now: u64) -> bool {
    i >= MAX_THREADS || (!DEAD[i].load(Ordering::Acquire) && WAKE[i].load(Ordering::Acquire) <= now)
}

fn reschedule(rsp: u64, timer_tick: bool) -> u64 {
    // SAFETY: runs inside an ISR with IF=0, so it is neither re-entered nor preempted.
    // Other users run with IF=0 (`spawn`, via `without_interrupts`) or only read fields the ISR
    // leaves alone (`thread_count`/`thread_name`).
    // NOTE: those readers' shared refs can overlap this `&mut` (not guaranteed by the type).
    let s = match unsafe { (*SCHED.get()).as_mut() } {
        Some(s) => s,
        None => return rsp,
    };
    let cur = s.current;
    if crate::trace::ON {
        let now = crate::io::rdtsc();
        let start = crate::trace::SLICE_START.swap(now, Ordering::Relaxed);
        if cur < MAX_THREADS && start != 0 {
            crate::trace::THREAD_CYC[cur].fetch_add(now.wrapping_sub(start), Ordering::Relaxed);
        }
    }

    // Catch a stack overflow the instant the offending thread is preempted,
    // before its wild writes corrupt the heap and detonate elsewhere. The thread
    // is killed right here (we are in the ISR, IF=0): it is marked dead and the
    // pick below simply never chooses it again. No `panic!` in the ISR.
    if cur != 0 && !DEAD[cur].load(Ordering::Relaxed) && !s.threads[cur].stack_intact() {
        crate::klog!(
            Error,
            "thread '{}' died: stack overflow (canary clobbered, rsp {:#x})",
            s.threads[cur].name,
            rsp
        );
        DEAD[cur].store(true, Ordering::Release);
    }

    if timer_tick && cur < MAX_THREADS && !IDLE[cur].load(Ordering::Relaxed) {
        TICKS[cur].fetch_add(1, Ordering::Relaxed);
    }

    // Next runnable thread after `cur`, round-robin; `cur` itself is the last
    // candidate, so it keeps the CPU when nobody else can run (unless it died).
    let n = s.threads.len();
    let now = crate::interrupts::ticks();
    let hint = HANDOFF.swap(usize::MAX, Ordering::Relaxed);
    let hinted = (!timer_tick && hint < n && hint != cur && runnable(hint, now)).then_some(hint);
    let next = match hinted
        .or_else(|| kitsune_core::schedule::next_runnable(cur, n, |i| runnable(i, now)))
    {
        Some(next) => next,
        // Only possible if the compositor were dead or missing, which `kill_current` forbids.
        None => crate::crash::halt(),
    };
    if next == cur {
        return rsp; // nothing else to run: state unchanged, `cur` resumes
    }

    // Save the outgoing thread's x87/SSE state. This must run before any xmm
    // use: the path from ISR entry to here (GP-reg pushes, integer scheduler
    // glue) touches no SSE register, so the interrupted thread's xmm/MXCSR are
    // still live here and captured intact.
    // SAFETY: `fpu_ptr()` is the 16-aligned 512-byte `FxArea` owned by thread `cur` (threads are
    // never removed, so it is never freed); `fxsave` writes only that area.
    unsafe {
        core::arch::asm!(
            "fxsave [{}]", in(reg) s.threads[cur].fpu_ptr(),
            options(nostack, preserves_flags),
        );
    }

    s.threads[cur].rsp = rsp;
    s.current = next;
    CURRENT.store(next, Ordering::Relaxed);

    // Load the incoming thread's x87/SSE state. Nothing below uses xmm before
    // the ISR `iretq`s into that thread, so its registers resume correctly.
    // SAFETY: same area guarantees for `next`. Its contents were written by `FxArea::seeded` or a
    // previous `fxsave`, so MXCSR has no reserved bits set and `fxrstor` cannot #GP.
    unsafe {
        core::arch::asm!(
            "fxrstor [{}]", in(reg) s.threads[next].fpu_ptr(),
            options(nostack, readonly, preserves_flags),
        );
    }

    activate(&s.threads[next]);
    s.threads[next].rsp
}

// ---- thread death ----

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

// ---- blocking API (thread context, IF=1) ----

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
fn others_ready(me: usize) -> bool {
    let n = thread_count().min(MAX_THREADS);
    let now = crate::interrupts::ticks();
    (0..n).any(|i| i != me && runnable(i, now))
}

/// Sleep in `hlt` until the next interrupt, unless `skip()` says there is
/// already something to do. Interrupts are masked while `skip()` is evaluated
/// and `sti; hlt` re-enables them atomically, so an event that arrives between
/// the check and the halt is not lost (it just ends the `hlt` at once).
fn halt_unless(me: usize, skip: impl Fn() -> bool) {
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

// ---- introspection for Tarefas ----

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
static STACK_KIB: [AtomicU32; MAX_THREADS] = [const { AtomicU32::new(0) }; MAX_THREADS];

const _: () = assert!(MAX_THREADS == kitsune_core::sysmon::MAX_THREADS);

fn scheduler() -> &'static mut Scheduler {
    // SAFETY: only called from `spawn`, which every caller runs with IF=0 (`without_interrupts` in
    // `kernel_main`), so the timer ISR cannot touch SCHED meanwhile.
    // NOTE: not guaranteed by the type: safe fn returning `&'static mut`, and `spawn`'s IF=0
    // precondition is not enforced (docs/audit/01-memoria-unsafe.md #6).
    unsafe { (*SCHED.get()).as_mut().expect("scheduler not initialized") }
}

// ---- user threads (ring 3) ----

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
