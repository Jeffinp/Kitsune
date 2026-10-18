//! Supervised kernel threads: the services (fetcher, appd, shelld, ...) the supervisor brings back when
//! they die. See `docs/design/supervisao.md`.
//!
//! A thread that dies is abandoned by [`kill_current`]: its context is never resumed. The supervisor runs on
//! the compositor (it is the one thread that never dies and that may allocate), once per pass of its loop:
//!
//! 1. the service's `cleanup` puts its shared state back to a sane start and fails what was in flight;
//! 2. the stack goes back to the allocator (guard page remapped first);
//! 3. [`RestartBudget`] decides: restart after a delay that grows, or give up after three deaths in a
//!    minute;
//! 4. when the time comes the slot is reused for a new thread with the same entry point.
//!
//! What is not freed: allocations that lived only in locals of the dead stack (bounded by what one request
//! holds). Locks it held: `YieldMutex` takes them over (`sync.rs`); `SpinLock` ones cannot be held by a thread
//! that dies, because `containable` refuses faults taken with interrupts off.

use super::*;
use kitsune_core::supervise::{RestartBudget, Verdict};

/// A kernel thread the supervisor keeps alive.
pub struct Service {
    pub name: &'static str,
    pub entry: extern "C" fn() -> !,
    pub stack: usize,
    /// Runs on the compositor after the thread died and before its slot is reused: reset the shared state
    /// it owned, answer what was waiting on it. Short, no waiting on other threads.
    pub cleanup: fn(),
}

enum Phase {
    Running,
    /// Dead and cleaned up; restart at this time (ms since boot).
    Waiting(u64),
    /// Died too often: stays dead.
    GaveUp,
}

struct Entry {
    svc: &'static Service,
    slot: usize,
    budget: RestartBudget,
    phase: Phase,
}

/// Mutated only by [`spawn_service`] (boot) and [`supervise`] (the compositor loop, after boot).
static ENTRIES: RacyCell<Vec<Entry>> = RacyCell::new(Vec::new());

const MAX_SERVICES: usize = MAX_THREADS;
/// Name and slot of each service, written once while booting and read-only afterwards, so any thread (the
/// shell running `crash`) may look a service up without touching `ENTRIES`.
static REGISTRY: RacyCell<[(&str, usize); MAX_SERVICES]> = RacyCell::new([("", 0); MAX_SERVICES]);
static REGISTERED: AtomicUsize = AtomicUsize::new(0);

/// A fault requested for a slot ([`inject_fault`]): the thread panics at its next scheduler wait.
static FAULT: [AtomicBool; MAX_THREADS] = [const { AtomicBool::new(false) }; MAX_THREADS];

fn now_ms() -> u64 {
    crate::interrupts::ticks() * 4
}

/// Spawn `svc` as a supervised thread. Call it where `spawn` was called: during boot, from thread context.
pub fn spawn_service(svc: &'static Service) {
    x86_64::instructions::interrupts::without_interrupts(|| {
        let slot = scheduler().threads.len();
        spawn_with_stack(svc.name, svc.entry, svc.stack);
        let n = REGISTERED.load(Ordering::Relaxed);
        if n >= MAX_SERVICES {
            return;
        }
        // SAFETY: boot-time writes with IF=0, before the compositor loop (the only other user of
        // ENTRIES) and before any reader of REGISTRY can see slot `n`, published by the Release store.
        unsafe {
            (*ENTRIES.get()).push(Entry {
                svc,
                slot,
                budget: RestartBudget::new(),
                phase: Phase::Running,
            });
            (*REGISTRY.get())[n] = (svc.name, slot);
        }
        REGISTERED.store(n + 1, Ordering::Release);
    });
}

/// Make the service called `name` panic at its next scheduler wait (`crash <service>`); `false` if there is
/// no such service. It goes through the real path: panic, `kill_current`, then the supervisor.
pub fn inject_fault(name: &str) -> bool {
    let n = REGISTERED.load(Ordering::Acquire);
    // SAFETY: entries below `n` were written before the Release store of `REGISTERED` and never change.
    let reg = unsafe { &*REGISTRY.get() };
    match reg[..n].iter().find(|(nm, _)| *nm == name) {
        Some(&(_, slot)) => {
            FAULT[slot].store(true, Ordering::SeqCst);
            wake(slot); // a parked thread reaches its fault point at once
            true
        }
        None => false,
    }
}

/// Called by [`block`] on entry and on exit: a requested fault becomes a panic of the calling thread.
pub(super) fn fault_point(me: usize) {
    if me < MAX_THREADS && FAULT[me].swap(false, Ordering::SeqCst) {
        panic!("fault injected by `crash`");
    }
}

/// One pass of the supervisor. Cheap when nothing died: a few atomic loads.
pub fn supervise() {
    // SAFETY: see ENTRIES: the compositor loop is the only caller, one at a time.
    let entries = unsafe { &mut *ENTRIES.get() };
    for e in entries.iter_mut() {
        match e.phase {
            Phase::Running if is_dead(e.slot) => on_death(e),
            Phase::Waiting(at) if now_ms() >= at => respawn(e),
            _ => {}
        }
    }
}

fn on_death(e: &mut Entry) {
    let name = e.svc.name;
    (e.svc.cleanup)();
    let freed = reap_stack(e.slot);
    match e.budget.on_death(now_ms()) {
        Verdict::RestartAt { at_ms, attempt } => {
            // Info, not Warn: a Warn would also become a raw English toast next to the localized one.
            crate::klog!(
                Info,
                "supervisor: '{}' died; stack {}, restart in {} ms (attempt {} of {})",
                name,
                if freed { "freed" } else { "kept" },
                at_ms.saturating_sub(now_ms()),
                attempt,
                kitsune_core::supervise::MAX_RESTARTS
            );
            e.phase = Phase::Waiting(at_ms);
        }
        Verdict::GiveUp => {
            crate::klog!(
                Error,
                "supervisor: '{}' died {} times in {} s: not restarted",
                name,
                kitsune_core::supervise::MAX_RESTARTS + 1,
                kitsune_core::supervise::WINDOW_MS / 1000
            );
            crate::notify::notify_key(
                crate::klog::Level::Error,
                kitsune_core::tk!("notify.service_stopped"),
                &[("name", kitsune_core::i18n::Arg::Str(name))],
            );
            e.phase = Phase::GaveUp;
        }
    }
}

/// Give the dead thread's stack block back to the allocator. `false` if it had to be kept.
fn reap_stack(slot: usize) -> bool {
    x86_64::instructions::interrupts::without_interrupts(|| {
        let s = scheduler();
        if slot == 0 || slot >= s.threads.len() || slot == s.current {
            return false;
        }
        let t = &mut s.threads[slot];
        let Some((base, size)) = t.block else {
            return false;
        };
        // The guard page is unmapped; the allocator would fault writing its free-list node there.
        if let Some(g) = t.guard
            && crate::vm::remap_page(g).is_err()
        {
            return false;
        }
        t.block = None;
        t.guard = None;
        t.stack_bottom = 0;
        let Ok(layout) = Layout::from_size_align(size, PAGE_SIZE as usize) else {
            return false;
        };
        // SAFETY: `base`/`layout` are exactly what `alloc_stack` allocated for this thread (`block` holds
        // them), the guard page is mapped again, the thread is dead and not running (`slot != current`,
        // `DEAD` set so the scheduler never resumes it), and `block` was just cleared so it is freed once.
        unsafe { alloc::alloc::dealloc(base as *mut u8, layout) };
        true
    })
}

/// Put a new thread with the service's entry point into the dead one's slot.
fn respawn(e: &mut Entry) {
    let svc = e.svc;
    let slot = e.slot;
    x86_64::instructions::interrupts::without_interrupts(|| {
        let s = scheduler();
        s.threads[slot] = new_kernel_thread(svc.name, svc.entry, svc.stack);
        FAULT[slot].store(false, Ordering::Relaxed);
        IDLE[slot].store(false, Ordering::Relaxed);
        DEMOTED[slot].store(false, Ordering::Relaxed);
        TICKS[slot].store(0, Ordering::Relaxed);
        WAKE[slot].store(0, Ordering::Release);
        STACK_KIB[slot].store((svc.stack / 1024) as u32, Ordering::Relaxed);
        DEAD[slot].store(false, Ordering::Release);
    });
    e.phase = Phase::Running;
    crate::klog!(
        Info,
        "supervisor: '{}' restarted in slot {}",
        svc.name,
        slot
    );
    crate::notify::notify_key(
        crate::klog::Level::Info,
        kitsune_core::tk!("notify.service_restarted"),
        &[("name", kitsune_core::i18n::Arg::Str(svc.name))],
    );
}
