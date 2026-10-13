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

mod blocking;
mod death;
mod introspect;
mod spawn;
mod user;
pub use blocking::*;
pub use death::*;
pub use introspect::*;
pub use spawn::*;
pub use user::*;

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
/// Demoted slots ([`set_demoted`]) and the tick from which each may run again.
static DEMOTED: [AtomicBool; MAX_THREADS] = [const { AtomicBool::new(false) }; MAX_THREADS];
static NEXT_TURN: [AtomicU64; MAX_THREADS] = [const { AtomicU64::new(0) }; MAX_THREADS];
/// Ticks a demoted thread waits after it ran while others want the CPU (it gets at most one tick
/// in `DEMOTED_GAP + 1` when it competes; all the CPU nobody else wants).
const DEMOTED_GAP: u64 = 6;

/// Demote slot `slot` (or stop): while other threads can run, it gets at most one timer tick in
/// [`DEMOTED_GAP`] + 1. The browser demotes a page's script process while its video plays with
/// plenty buffered, so the decoder and the compositor come first.
pub fn set_demoted(slot: usize, on: bool) {
    if slot < MAX_THREADS {
        DEMOTED[slot].store(on, Ordering::Relaxed);
    }
}

/// Is slot `i` demoted and waiting for its turn at tick `now`?
#[inline]
fn held(i: usize, now: u64) -> bool {
    i < MAX_THREADS
        && DEMOTED[i].load(Ordering::Relaxed)
        && NEXT_TURN[i].load(Ordering::Relaxed) > now
}
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
    let next = match hinted.or_else(|| {
        kitsune_core::schedule::next_runnable_held(cur, n, |i| runnable(i, now), |i| held(i, now))
    }) {
        Some(next) => next,
        // Only possible if the compositor were dead or missing, which `kill_current` forbids.
        None => crate::crash::halt(),
    };
    if next < MAX_THREADS && DEMOTED[next].load(Ordering::Relaxed) {
        NEXT_TURN[next].store(now + DEMOTED_GAP, Ordering::Relaxed);
    }
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
