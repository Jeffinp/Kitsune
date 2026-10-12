//! Measurement instrumentation for the performance audit.
//!
//! Two layers, both writing to COM1 (so they work headless, without a screen):
//!
//! * **Boot milestones** ([`mark`]) are always compiled in: a handful of lines
//!   printed once, with milliseconds since kernel entry. The TSC is not
//!   calibrated until the PIT runs, so marks taken earlier are buffered and
//!   flushed by [`calibrated`] (printing them live would perturb the very thing
//!   being timed).
//! * **Per-second runtime stats** (frame paths and stages, allocator, timer ISR,
//!   idle sampling, input latency) exist only with the `perf-trace` cargo
//!   feature. Without it [`ON`] is `false`, every hook folds to nothing and the
//!   statics are discarded by the linker.
//!
//! Build with stats:  `cargo build --release -p os --features perf-trace`
//! Output lines start with `[trace]`; `tools/perf/` helpers parse them.

use crate::io;
use crate::sync::RacyCell;
use core::sync::atomic::{AtomicU64, Ordering::Relaxed};

mod bench;
mod report;
mod stats;
pub use bench::*;
pub use report::*;
pub use stats::*;

/// `true` when the per-second runtime stats are compiled in.
pub const ON: bool = cfg!(feature = "perf-trace");

// ---------------------------------------------------------------- boot marks

const MAX_MARKS: usize = 32;

struct Marks {
    n: usize,
    khz: u64, // 0 until the TSC is calibrated
    names: [&'static str; MAX_MARKS],
    tsc: [u64; MAX_MARKS],
}

static MARKS: RacyCell<Marks> = RacyCell::new(Marks {
    n: 0,
    khz: 0,
    names: [""; MAX_MARKS],
    tsc: [0; MAX_MARKS],
});

fn print_mark(name: &str, tsc: u64, first: u64, prev: u64, khz: u64) {
    let since = tsc.wrapping_sub(first) * 1000 / khz; // microseconds
    let step = tsc.wrapping_sub(prev) * 1000 / khz;
    crate::serial_println!(
        "[trace] boot +{:>5}.{:03} ms (step {:>5}.{:03} ms)  {}",
        since / 1000,
        since % 1000,
        step / 1000,
        step % 1000,
        name
    );
}

/// Record a boot milestone. Buffered until [`calibrated`], printed live after.
pub fn mark(name: &'static str) {
    let now = io::rdtsc();
    // Single-threaded use: the compositor/boot thread only.
    // SAFETY: MARKS is only touched by `mark`/`calibrated`, both called from `kernel_main`
    // (compositor/boot thread), never from an ISR or another thread, and no other `&mut Marks`
    // is live while this one is, so the reference is unique.
    let m = unsafe { &mut *MARKS.get() };
    if m.n >= MAX_MARKS {
        return;
    }
    let i = m.n;
    m.names[i] = name;
    m.tsc[i] = now;
    m.n += 1;
    if m.khz != 0 {
        let prev = m.tsc[i.saturating_sub(1)];
        print_mark(name, now, m.tsc[0], prev, m.khz);
    }
}

/// Print `label` with the microseconds since `t0` (see [`t`]) and the extra `detail`
/// (`perf-trace` builds only; used by the browser for load, layout and first paint).
pub fn note(label: &str, t0: u64, detail: u64) {
    if ON {
        // SAFETY: as in `mark`: compositor thread only, nothing else holds `&mut Marks`.
        let khz = unsafe { &*MARKS.get() }.khz.max(1);
        let us = io::rdtsc().wrapping_sub(t0) * 1000 / khz;
        crate::serial_println!("[trace] browser {label}: {us} us ({detail})");
    }
}

/// Provide the calibrated TSC rate and flush the milestones buffered so far.
/// Also reports how long the VM had been running when the kernel started
/// (TSC counts from reset, so `tsc[0]` is the firmware + bootloader time).
pub fn calibrated(khz: u64) {
    // SAFETY: as in `mark`: compositor/boot thread only, no other `&mut Marks` live.
    let m = unsafe { &mut *MARKS.get() };
    m.khz = khz.max(1);
    if m.n == 0 {
        return;
    }
    let pre = m.tsc[0] * 1000 / m.khz;
    crate::serial_println!(
        "[trace] boot firmware+bootloader before kernel entry (TSC since reset): {}.{:03} ms",
        pre / 1000,
        pre % 1000
    );
    for i in 0..m.n {
        let prev = m.tsc[i.saturating_sub(1)];
        print_mark(m.names[i], m.tsc[i], m.tsc[0], prev, m.khz);
    }
}
