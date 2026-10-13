//! Where the compositor spends its time: the timer interrupt notes the interrupted instruction of
//! the compositor thread (slot 0, ring 0) and every few seconds the report prints the busiest
//! addresses (`[trace] pc 0xADDR n`), to be mapped to functions with the kernel's symbols
//! (`nm -C --defined-only kernel | sort`). Only in `perf-trace` builds.

use super::*;
use core::sync::atomic::AtomicUsize;

const RING: usize = 4096;
/// How often the histogram is printed (seconds).
const EVERY_S: u64 = 4;
/// Addresses printed each time.
const TOP: usize = 24;
/// Addresses are grouped by this many bytes (a function's hot loop lands in one or two groups).
const GROUP: u64 = 64;

static PCS: [AtomicU64; RING] = [const { AtomicU64::new(0) }; RING];
static N: AtomicUsize = AtomicUsize::new(0);
static SECONDS: AtomicU64 = AtomicU64::new(0);

/// Note `rip`, interrupted on the compositor thread in ring 0 (called from the timer interrupt).
#[inline(always)]
pub(super) fn note_pc(rip: u64) {
    let n = N.load(Relaxed);
    if n < RING {
        PCS[n].store(rip, Relaxed);
        N.store(n + 1, Relaxed);
    }
}

/// Once every [`EVERY_S`] calls (one a second), print the busiest address groups and start over.
pub(super) fn report_pcs() {
    if SECONDS.fetch_add(1, Relaxed) % EVERY_S != EVERY_S - 1 {
        return;
    }
    let n = N.load(Relaxed).min(RING);
    let mut v: alloc::vec::Vec<u64> = (0..n)
        .map(|i| PCS[i].load(Relaxed) / GROUP * GROUP)
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
    groups.sort_unstable_by(|a, b| b.0.cmp(&a.0));
    crate::serial_println!("[trace] pc samples {n} (compositor, ring 0, {EVERY_S} s)");
    for (c, pc) in groups.iter().take(TOP) {
        crate::serial_println!("[trace] pc {pc:#x} {c}");
    }
}
