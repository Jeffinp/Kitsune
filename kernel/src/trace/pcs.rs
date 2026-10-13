//! Where the time goes: the timer interrupt notes the interrupted instruction of the running thread
//! (its scheduler slot and ring) and every few seconds the report prints, for the busiest threads,
//! the busiest addresses (`[trace] pc <slot> r<ring> 0xADDR n`), to be mapped to functions with the
//! symbols of the kernel (ring 0) or of the process's program (ring 3: `jsrun`, `mediadec`), e.g.
//! `nm -C --defined-only`. Only in `perf-trace` builds.

use super::*;
use core::sync::atomic::AtomicUsize;

const RING: usize = 8192;
/// How often the histogram is printed (seconds).
const EVERY_S: u64 = 4;
/// Addresses printed per thread each time.
const TOP: usize = 16;
/// Threads printed each time.
const THREADS: usize = 4;
/// Addresses are grouped by this many bytes (a function's hot loop lands in one or two groups).
const GROUP: u64 = 64;
/// The address bits of a sample; above them, bit 56 is ring 3 and the slot starts at bit 57.
const ADDR: u64 = (1 << 56) - 1;

static PCS: [AtomicU64; RING] = [const { AtomicU64::new(0) }; RING];
static N: AtomicUsize = AtomicUsize::new(0);
static SECONDS: AtomicU64 = AtomicU64::new(0);

/// Note `rip`, interrupted on `slot` (in ring 3 when `user`); called from the timer interrupt.
#[inline(always)]
pub(super) fn note_pc(slot: usize, user: bool, rip: u64) {
    let n = N.load(Relaxed);
    if n < RING {
        let tag = ((slot as u64 & 0x7F) << 57) | (u64::from(user) << 56);
        PCS[n].store(tag | (rip & ADDR), Relaxed);
        N.store(n + 1, Relaxed);
    }
}

/// Once every [`EVERY_S`] calls (one a second), print the busiest address groups of the busiest
/// threads and start over.
pub(super) fn report_pcs() {
    if SECONDS.fetch_add(1, Relaxed) % EVERY_S != EVERY_S - 1 {
        return;
    }
    let n = N.load(Relaxed).min(RING);
    let mut v: alloc::vec::Vec<u64> = (0..n)
        .map(|i| {
            let s = PCS[i].load(Relaxed);
            (s & !ADDR) | ((s & ADDR) / GROUP * GROUP)
        })
        .collect();
    N.store(0, Relaxed);
    v.sort_unstable();
    let mut groups: alloc::vec::Vec<(u32, u64)> = alloc::vec::Vec::new();
    for pc in v {
        match groups.last_mut() {
            Some((c, p)) if *p == pc => *c += 1,
            _ => groups.push((1, pc)),
        }
    }
    // Samples per thread (slot and ring), busiest first.
    let mut threads: alloc::vec::Vec<(u32, u64)> = alloc::vec::Vec::new();
    for &(c, pc) in &groups {
        let t = pc >> 56;
        match threads.iter_mut().find(|x| x.1 == t) {
            Some(x) => x.0 += c,
            None => threads.push((c, t)),
        }
    }
    threads.sort_unstable_by_key(|a| core::cmp::Reverse(a.0));
    groups.sort_unstable_by_key(|a| core::cmp::Reverse(a.0));
    crate::serial_println!("[trace] pc samples {n} ({EVERY_S} s)");
    for &(tc, t) in threads.iter().take(THREADS) {
        let (slot, ring) = ((t >> 1) as usize, if t & 1 == 1 { 3 } else { 0 });
        let name = crate::sched::thread_name(slot);
        crate::serial_println!("[trace] pc thread {slot} {name} r{ring}: {tc} samples");
        for (c, pc) in groups.iter().filter(|g| g.1 >> 56 == t).take(TOP) {
            crate::serial_println!("[trace] pc {slot} r{ring} {:#x} {c}", pc & ADDR);
        }
    }
}
