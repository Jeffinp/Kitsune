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

// ------------------------------------------------------- runtime stats (ON)

/// Frame kinds of the compositor (see `desktop/compositor`): how the frame came about. They all
/// run the same engine; the kind only says why, so the per-second report stays comparable.
#[derive(Clone, Copy)]
pub enum Path {
    /// Animation, drag or live app that repaints most of the screen.
    AnimRebuild,
    /// Animation, drag or live app: a damage-rectangle frame.
    AnimDamage,
    /// Pointer moving over an open menu or overlay.
    OverlayHover,
    /// Reference mode, or a change that repaints most of the screen (a settle after an animation).
    Settle,
    /// Content change (keystroke, click).
    Steady,
    /// Per-second clock tick.
    ClockLocal,
    /// Cursor only: nothing recomposed.
    Cursor,
}
const PATHS: usize = 7;
const PATH_NAMES: [&str; PATHS] = [
    "animrb", "animdm", "ovhov", "settle", "steady", "clockl", "cursor",
];

/// Work stages inside a frame (cycles are summed per second).
#[derive(Clone, Copy)]
pub enum Stage {
    /// Scene recompose into the back buffer (memcpy bg + draw windows).
    Compose,
    /// Writes into the framebuffer (VRAM), full or rect.
    Blit,
    /// Mouse cursor drawn directly into the framebuffer.
    Cursor,
    /// Perf HUD overlay.
    Hud,
}
const STAGES: usize = 4;
const STAGE_NAMES: [&str; STAGES] = ["compose", "blit", "cursor", "hud"];

struct Stats {
    path_n: [u64; PATHS],
    path_sum: [u64; PATHS],
    path_cpu: [u64; PATHS],
    path_max: [u64; PATHS],
    stage_sum: [u64; STAGES],
    stage_max: [u64; STAGES],
    loops: u64,
    hlt_wakes: u64,
    lat_n: u64,
    lat_sum: u64, // IRQ -> compositor picked the event up
    lat_max: u64,
    e2e_sum: u64, // IRQ -> frame finished
    e2e_max: u64,
}

static STATS: RacyCell<Stats> = RacyCell::new(Stats {
    path_n: [0; PATHS],
    path_sum: [0; PATHS],
    path_cpu: [0; PATHS],
    path_max: [0; PATHS],
    stage_sum: [0; STAGES],
    stage_max: [0; STAGES],
    loops: 0,
    hlt_wakes: 0,
    lat_n: 0,
    lat_sum: 0,
    lat_max: 0,
    e2e_sum: 0,
    e2e_max: 0,
});

// Shared with ISRs / other threads: plain relaxed atomics.
pub static ALLOC_N: AtomicU64 = AtomicU64::new(0);
pub static ALLOC_BYTES: AtomicU64 = AtomicU64::new(0);
pub static ALLOC_CYC: AtomicU64 = AtomicU64::new(0);
pub static ALLOC_MAX: AtomicU64 = AtomicU64::new(0);
pub static ALLOC_SCANNED: AtomicU64 = AtomicU64::new(0);
pub static FREE_N: AtomicU64 = AtomicU64::new(0);
pub static FREE_CYC: AtomicU64 = AtomicU64::new(0);
pub static ISR_N: AtomicU64 = AtomicU64::new(0);
pub static ISR_CYC: AtomicU64 = AtomicU64::new(0);
pub static ISR_MAX: AtomicU64 = AtomicU64::new(0);
/// Timer-tick samples: interrupted code was an `hlt` wake-up (idle) or not.
pub static TICK_IDLE: AtomicU64 = AtomicU64::new(0);
pub static TICK_BUSY: AtomicU64 = AtomicU64::new(0);
/// Busy samples attributed to scheduler slot (0 compositor, 1 fetcher, 2 wasmapp).
pub static TICK_BUSY_THR: [AtomicU64; 4] = [const { AtomicU64::new(0) }; 4];
/// TSC of the oldest input IRQ not yet consumed by the compositor (0 = none).
pub static INPUT_TSC: AtomicU64 = AtomicU64::new(0);
/// Drawing primitives (cycles/calls per second). Nested calls are counted at
/// every level (a glyph contains `fill_rect`s), so the rows overlap on purpose.
pub static PRIM_CYC: [AtomicU64; 6] = [const { AtomicU64::new(0) }; 6];
pub static PRIM_N: [AtomicU64; 6] = [const { AtomicU64::new(0) }; 6];
/// Per-thread CPU cycles (credited at each context switch) and the TSC at
/// which the running thread was switched in. See [`cpu_now`].
pub static THREAD_CYC: [AtomicU64; 8] = [const { AtomicU64::new(0) }; 8];
pub static SLICE_START: AtomicU64 = AtomicU64::new(0);
/// Framebuffer upload accounting: bytes written to VRAM and how many of them
/// differed from what was already there.
pub static VRAM_UP: AtomicU64 = AtomicU64::new(0);
pub static VRAM_CHG: AtomicU64 = AtomicU64::new(0);
/// ATA flush accounting (cycles) and count.
pub static ATA_W_N: AtomicU64 = AtomicU64::new(0);
pub static ATA_W_CYC: AtomicU64 = AtomicU64::new(0);
pub static ATA_R_N: AtomicU64 = AtomicU64::new(0);
pub static ATA_R_CYC: AtomicU64 = AtomicU64::new(0);

/// Drawing primitive classes for [`prim`].
#[derive(Clone, Copy)]
pub enum Prim {
    /// `crate::text::legacy::draw_char` (includes its `fill_rect`s).
    Glyph,
    /// `Canvas::fill_rect`, every call (also the nested ones).
    FillRect,
    /// `Canvas::fill_round_rect` (includes its span `fill_rect`s).
    RoundRect,
    /// `Canvas::fill_round_rect_alpha` (shadows, translucent panels).
    Alpha,
    /// `back.copy_from_slice(bg)`: wallpaper restore into the back buffer.
    BgCopy,
    /// `snapshot_region` + `blend_from_local` (fading windows).
    Fade,
}
const PRIM_NAMES: [&str; 6] = ["glyph", "fillrect", "rrect", "alpha", "bgcopy", "fade"];

/// Account one call of `p` that began at `t0` (see [`t`]).
#[inline(always)]
pub fn prim(p: Prim, t0: u64) {
    if ON {
        PRIM_CYC[p as usize].fetch_add(io::rdtsc().wrapping_sub(t0), Relaxed);
        PRIM_N[p as usize].fetch_add(1, Relaxed);
    }
}

#[inline(always)]
fn stats() -> &'static mut Stats {
    // SAFETY: STATS is only used by the compositor-thread hooks in this file (ISRs and the
    // other threads use the atomics above) and no caller holds two `stats()` refs at once.
    // NOTE: not guaranteed by the type (safe fn returning `&'static mut`); it relies on every
    // caller being the compositor thread (docs/audit/01-memoria-unsafe.md #6).
    unsafe { &mut *STATS.get() }
}

/// Start a timer: the TSC when tracing, 0 otherwise (folds away).
#[inline(always)]
pub fn t() -> u64 {
    if ON { io::rdtsc() } else { 0 }
}

#[inline(always)]
pub fn max_to(a: &AtomicU64, v: u64) {
    if v > a.load(Relaxed) {
        a.store(v, Relaxed);
    }
}

/// Add the cycles since `t0` to `stage`.
#[inline(always)]
pub fn stage(s: Stage, t0: u64) {
    if ON {
        let d = io::rdtsc().wrapping_sub(t0);
        let st = stats();
        st.stage_sum[s as usize] += d;
        st.stage_max[s as usize] = st.stage_max[s as usize].max(d);
    }
}

/// TSC cycles of CPU time consumed so far by the compositor thread (slot 0),
/// i.e. wall time minus the slices the round-robin gave to other threads. Wall
/// time per frame is inflated by those slices (idle workers `hlt` through their
/// whole slice), so the CPU figure is what a frame really costs.
pub fn cpu_now() -> u64 {
    if !ON {
        return 0;
    }
    loop {
        let a = THREAD_CYC[0].load(Relaxed);
        let start = SLICE_START.load(Relaxed);
        let cur = crate::sched::current();
        let now = io::rdtsc();
        if a == THREAD_CYC[0].load(Relaxed) {
            return a + if cur == 0 { now.wrapping_sub(start) } else { 0 };
        }
    }
}

/// Account a finished frame on `path` that began at wall `t0` / CPU `cpu0`.
#[inline(always)]
pub fn frame(p: Path, t0: u64, cpu0: u64) {
    if ON {
        let d = io::rdtsc().wrapping_sub(t0);
        let st = stats();
        st.path_n[p as usize] += 1;
        st.path_sum[p as usize] += d;
        st.path_cpu[p as usize] += cpu_now().wrapping_sub(cpu0);
        st.path_max[p as usize] = st.path_max[p as usize].max(d);
    }
}

#[inline(always)]
pub fn loop_iter() {
    if ON {
        stats().loops += 1;
    }
}

#[inline(always)]
pub fn hlt_wake() {
    if ON {
        stats().hlt_wakes += 1;
    }
}

/// Called when the compositor drained input: returns the IRQ timestamp (or 0).
#[inline(always)]
pub fn input_taken() -> u64 {
    if ON {
        let t0 = INPUT_TSC.swap(0, Relaxed);
        if t0 != 0 {
            let d = io::rdtsc().wrapping_sub(t0);
            let st = stats();
            st.lat_n += 1;
            st.lat_sum += d;
            st.lat_max = st.lat_max.max(d);
        }
        t0
    } else {
        0
    }
}

/// Called after the frame that handled that input is on screen.
#[inline(always)]
pub fn input_done(irq_tsc: u64) {
    if ON && irq_tsc != 0 {
        let d = io::rdtsc().wrapping_sub(irq_tsc);
        let st = stats();
        st.e2e_sum += d;
        st.e2e_max = st.e2e_max.max(d);
    }
}

/// Keyboard/mouse ISR hook: remember when the first unconsumed input arrived.
#[inline(always)]
pub fn input_irq() {
    if ON {
        let _ = INPUT_TSC.compare_exchange(0, io::rdtsc(), Relaxed, Relaxed);
    }
}

/// Timer-ISR hook. `rsp` points at the 15 saved GPRs; the `iretq` frame (RIP
/// first) follows. An interrupted `hlt` leaves RIP just past the `0xF4` opcode.
#[inline(always)]
pub fn timer_sample(rsp: u64, cur_slot: usize) {
    if ON {
        // SAFETY: `rsp` is the block of 15 GPRs pushed by `timer_isr`, so `rsp + 15*8` is the RIP
        // slot of the CPU-pushed interrupt frame of the interrupted context: mapped, 8-aligned, read-only.
        let rip = unsafe { *((rsp + 15 * 8) as *const u64) };
        // SAFETY: the CS slot follows the RIP slot in the same CPU-pushed frame.
        let cs = unsafe { *((rsp + 16 * 8) as *const u64) };
        // A ring 3 process was running: busy, and its code is not ours to read (SMAP would fault).
        // SAFETY: in ring 0 `rip` is the interrupted instruction pointer, kernel text, so the byte
        // before it is mapped and readable.
        // NOTE: not checked: assumes `rip` is not the very first byte of the mapped text.
        let idle = cs & 3 == 0 && unsafe { *((rip - 1) as *const u8) } == 0xF4;
        if idle {
            TICK_IDLE.fetch_add(1, Relaxed);
        } else {
            TICK_BUSY.fetch_add(1, Relaxed);
            TICK_BUSY_THR[cur_slot.min(3)].fetch_add(1, Relaxed);
        }
    }
}

/// Record one framebuffer upload of `bytes`, of which `changed` differ from the
/// previous framebuffer contents.
#[inline(always)]
pub fn vram_upload(bytes: u64, changed: u64) {
    if ON {
        VRAM_UP.fetch_add(bytes, Relaxed);
        VRAM_CHG.fetch_add(changed, Relaxed);
    }
}

/// Number of differing bytes between two equally long slices (trace builds only;
/// compares 8 bytes at a time).
pub fn count_diff(a: &[u8], b: &[u8]) -> u64 {
    let mut n = 0u64;
    let (ac, ar) = a.as_chunks::<8>();
    let (bc, br) = b.as_chunks::<8>();
    for (x, y) in ac.iter().zip(bc) {
        if x != y {
            n += x.iter().zip(y).filter(|(p, q)| p != q).count() as u64;
        }
    }
    n + ar.iter().zip(br).filter(|(p, q)| p != q).count() as u64
}

/// Reset-and-read helper for the atomics.
fn take(a: &AtomicU64) -> u64 {
    a.swap(0, Relaxed)
}

fn us(cycles: u64, khz: u64) -> u64 {
    cycles * 1000 / khz.max(1)
}

/// Print and reset the per-second statistics (call once per wall-clock second
/// from the compositor, outside any timed region).
pub fn report(khz: u64, ticks: u64) {
    if !ON {
        return;
    }
    let st = stats();
    crate::serial_println!(
        "[trace] t={}ticks loops={} hltwakes={}",
        ticks,
        st.loops,
        st.hlt_wakes
    );
    crate::netd::log_stats();
    crate::wasm::remote::log_stats();
    for p in 0..PATHS {
        if st.path_n[p] > 0 {
            crate::serial_println!(
                "[trace]   path {:<6} n={:<4} avg={}us max={}us cpu={}us",
                PATH_NAMES[p],
                st.path_n[p],
                us(st.path_sum[p] / st.path_n[p], khz),
                us(st.path_max[p], khz),
                us(st.path_cpu[p] / st.path_n[p], khz)
            );
        }
    }
    if st.stage_sum.iter().any(|&v| v > 0) {
        crate::serial_println!(
            "[trace]   stage sum/s: {}={}us {}={}us {}={}us {}={}us  (max {}us {}us {}us {}us)",
            STAGE_NAMES[0],
            us(st.stage_sum[0], khz),
            STAGE_NAMES[1],
            us(st.stage_sum[1], khz),
            STAGE_NAMES[2],
            us(st.stage_sum[2], khz),
            STAGE_NAMES[3],
            us(st.stage_sum[3], khz),
            us(st.stage_max[0], khz),
            us(st.stage_max[1], khz),
            us(st.stage_max[2], khz),
            us(st.stage_max[3], khz),
        );
    }
    if st.lat_n > 0 {
        crate::serial_println!(
            "[trace]   input n={} irq->pickup avg={}us max={}us  irq->frame avg={}us max={}us",
            st.lat_n,
            us(st.lat_sum / st.lat_n, khz),
            us(st.lat_max, khz),
            us(st.e2e_sum / st.lat_n, khz),
            us(st.e2e_max, khz)
        );
    }
    if PRIM_N.iter().any(|n| n.load(Relaxed) > 0) {
        crate::serial_println!(
            "[trace]   prim {}={}us/{} {}={}us/{} {}={}us/{} {}={}us/{} {}={}us/{} {}={}us/{}",
            PRIM_NAMES[0],
            us(take(&PRIM_CYC[0]), khz),
            take(&PRIM_N[0]),
            PRIM_NAMES[1],
            us(take(&PRIM_CYC[1]), khz),
            take(&PRIM_N[1]),
            PRIM_NAMES[2],
            us(take(&PRIM_CYC[2]), khz),
            take(&PRIM_N[2]),
            PRIM_NAMES[3],
            us(take(&PRIM_CYC[3]), khz),
            take(&PRIM_N[3]),
            PRIM_NAMES[4],
            us(take(&PRIM_CYC[4]), khz),
            take(&PRIM_N[4]),
            PRIM_NAMES[5],
            us(take(&PRIM_CYC[5]), khz),
            take(&PRIM_N[5]),
        );
    }
    let (vu, vc) = (take(&VRAM_UP), take(&VRAM_CHG));
    if vu > 0 {
        crate::serial_println!("[trace]   vram upload={}B changed={}B", vu, vc);
    }
    // Exact heap occupancy (the HUD only shows whole percent of 64 MiB): lets a
    // window open/close soak prove nothing leaks.
    let used = crate::HEAP_SIZE - crate::ALLOCATOR.free_bytes().min(crate::HEAP_SIZE);
    crate::serial_println!("[trace]   heap used={}B", used);
    let an = take(&ALLOC_N);
    let fnn = take(&FREE_N);
    if an + fnn > 0 {
        crate::serial_println!(
            "[trace]   alloc n={} bytes={} avg={}cyc max={}cyc scanned/alloc={} | free n={} avg={}cyc",
            an,
            take(&ALLOC_BYTES),
            take(&ALLOC_CYC) / an.max(1),
            take(&ALLOC_MAX),
            take(&ALLOC_SCANNED) / an.max(1),
            fnn,
            take(&FREE_CYC) / fnn.max(1)
        );
    }
    let isr_n = take(&ISR_N);
    let idle = take(&TICK_IDLE);
    let busy = take(&TICK_BUSY);
    crate::serial_println!(
        "[trace]   timer isr n={} avg={}cyc max={}cyc | cpu samples idle={} busy={} (comp={} fetch={} wasm={})",
        isr_n,
        take(&ISR_CYC) / isr_n.max(1),
        take(&ISR_MAX),
        idle,
        busy,
        take(&TICK_BUSY_THR[0]),
        take(&TICK_BUSY_THR[1]),
        take(&TICK_BUSY_THR[2])
    );
    let (wn, wc, rn, rc) = (
        take(&ATA_W_N),
        take(&ATA_W_CYC),
        take(&ATA_R_N),
        take(&ATA_R_CYC),
    );
    if wn + rn > 0 {
        crate::serial_println!(
            "[trace]   ata write n={} avg={}us | read n={} avg={}us",
            wn,
            us(wc / wn.max(1), khz),
            rn,
            us(rc / rn.max(1), khz)
        );
    }
    *st = Stats {
        path_n: [0; PATHS],
        path_sum: [0; PATHS],
        path_cpu: [0; PATHS],
        path_max: [0; PATHS],
        stage_sum: [0; STAGES],
        stage_max: [0; STAGES],
        loops: 0,
        hlt_wakes: 0,
        lat_n: 0,
        lat_sum: 0,
        lat_max: 0,
        e2e_sum: 0,
        e2e_max: 0,
    };
}

// ------------------------------------------------------------ micro-benches

/// Boot-time micro-benchmark of the kernel heap (`perf-trace` only): cost of a
/// small allocation on a clean heap, then of a large one when the free list is
/// fragmented into thousands of holes (the first-fit scan is O(holes), and so is
/// the address-sorted insertion on `dealloc`). Leaves the heap as it found it.
pub fn bench_alloc(khz: u64) {
    if !ON {
        return;
    }
    use alloc::alloc::{Layout, alloc, dealloc};
    use alloc::vec::Vec;
    let small = Layout::from_size_align(64, 8).unwrap();
    let big = Layout::from_size_align(4096, 8).unwrap();
    let avg = |total: u64, n: u64| total / n.max(1);

    // 1. clean heap: 1000 small allocs, then free them.
    let mut ptrs: Vec<*mut u8> = Vec::with_capacity(4000);
    let t0 = io::rdtsc();
    for _ in 0..1000 {
        // SAFETY: `small` has non-zero size and a power-of-two align, as `alloc` requires.
        // NOTE: the result is not null-checked; harmless at boot with an almost empty 64 MiB heap.
        ptrs.push(unsafe { alloc(small) });
    }
    let a_clean = avg(io::rdtsc() - t0, 1000);
    let t0 = io::rdtsc();
    for p in ptrs.drain(..) {
        // SAFETY: `p` came from `alloc(small)` above and is freed once (drained), with the same layout.
        unsafe { dealloc(p, small) };
    }
    let f_clean = avg(io::rdtsc() - t0, 1000);

    // 2. fragment: 4000 small blocks, free every other one => 2000 holes.
    for _ in 0..4000 {
        // SAFETY: `small` has non-zero size and a power-of-two align (see the first loop).
        ptrs.push(unsafe { alloc(small) });
    }
    let t0 = io::rdtsc();
    let mut kept: Vec<*mut u8> = Vec::with_capacity(2000);
    for (i, p) in ptrs.drain(..).enumerate() {
        if i % 2 == 0 {
            // SAFETY: `p` came from `alloc(small)` (drained from `ptrs`) and is freed once, same layout.
            unsafe { dealloc(p, small) };
        } else {
            kept.push(p);
        }
    }
    let f_frag = avg(io::rdtsc() - t0, 2000);
    // 3. big allocations must skip all 2000 holes; their frees insert at the tail.
    let mut bigs: Vec<*mut u8> = Vec::with_capacity(100);
    let t0 = io::rdtsc();
    for _ in 0..100 {
        // SAFETY: `big` has non-zero size and a power-of-two align.
        bigs.push(unsafe { alloc(big) });
    }
    let a_frag = avg(io::rdtsc() - t0, 100);
    let t0 = io::rdtsc();
    for p in bigs.drain(..) {
        // SAFETY: `p` came from `alloc(big)` (drained from `bigs`) and is freed once, same layout.
        unsafe { dealloc(p, big) };
    }
    let f_big = avg(io::rdtsc() - t0, 100);
    for p in kept.drain(..) {
        // SAFETY: `p` is a `small` block kept from the loop above; freed once, same layout.
        unsafe { dealloc(p, small) };
    }
    crate::serial_println!(
        "[trace] bench alloc: clean 64B alloc {} cyc / free {} cyc | 2000 holes: 4KiB alloc {} cyc / free {} cyc, 64B free(in list) {} cyc | khz {}",
        a_clean,
        f_clean,
        a_frag,
        f_big,
        f_frag,
        khz
    );
}

/// Boot-time micro-benchmark of the drawing primitives on the back buffer
/// (`perf-trace` only). Prints cycles and cycles/pixel of the best of 4 runs,
/// which separates "this primitive is slow" from "it is called too often".
pub fn bench_prims(back: &mut [u8], info: bootloader_api::info::FrameBufferInfo, khz: u64) {
    if !ON {
        return;
    }
    use crate::fb::{Canvas, Color};
    let best = |f: &mut dyn FnMut()| -> u64 {
        let mut b = u64::MAX;
        for _ in 0..4 {
            let t0 = io::rdtsc();
            f();
            b = b.min(io::rdtsc().wrapping_sub(t0));
        }
        b
    };
    let (w, h) = (512usize, 320usize);
    let px = (w * h) as u64;
    let mut c = Canvas::new(back, info);
    let col = Color::rgb(0x20, 0x40, 0x80);
    let s = "The quick brown fox jumps over the lazy dog 0123";

    let fill = best(&mut || c.fill_rect(100, 100, w, h, col));
    let rr = best(&mut || c.fill_round_rect(100, 100, w, h, 12, col));
    let al = best(&mut || c.fill_round_rect_alpha(100, 100, w, h, 12, col, 28));
    let al_small = best(&mut || c.fill_round_rect_alpha(100, 100, 60, 30, 8, col, 28));
    let txt = best(&mut || crate::text::legacy::draw_text(&mut c, 100, 100, s, col, 2));
    crate::serial_println!(
        "[trace] bench ({}x{} px, bpp {}): fill_rect {} cyc ({}/px) | round_rect {} ({}/px) | alpha {} ({}/px) | alpha 60x30 {} ({}/px) | text {} chars {} cyc ({}/char) | khz {}",
        w,
        h,
        info.bytes_per_pixel,
        fill,
        fill / px,
        rr,
        rr / px,
        al,
        al / px,
        al_small,
        al_small / (60 * 30),
        s.len(),
        txt,
        txt / s.len() as u64,
        khz
    );
    bench_ui(&mut c, best_fn(), khz);
}

fn best_fn() -> impl Fn(&mut dyn FnMut()) -> u64 {
    |f: &mut dyn FnMut()| -> u64 {
        let mut b = u64::MAX;
        for _ in 0..4 {
            let t0 = io::rdtsc();
            f();
            b = b.min(io::rdtsc().wrapping_sub(t0));
        }
        b
    }
}

/// The UI toolkit's primitives (anti-aliased shapes, shadows, text, surfaces,
/// blur, scaling), best of 4, in cycles and cycles per pixel.
fn bench_ui(c: &mut crate::fb::Canvas, best: impl Fn(&mut dyn FnMut()) -> u64, khz: u64) {
    use crate::fb::{Color, Corner, Shadow};
    use crate::text::{self, Weight};
    use kitsune_core::Rect;
    use kitsune_core::raster::{self, Paint, Surface};
    let col = Color::rgb(0x20, 0x40, 0x80);
    let col2 = Color::rgb(0xF0, 0xF4, 0xFF);
    let rect = Rect::new(100, 100, 512, 320);
    let px = (rect.w * rect.h) as u64;
    let rr_solid = best(&mut || c.fill_rrect(rect, 12, Corner::Circle, col, 256));
    let rr_alpha = best(&mut || c.fill_rrect(rect, 12, Corner::Circle, col, 180));
    let rr_grad = best(&mut || c.fill_rrect_vgrad(rect, 12, Corner::Circle, col2, col, 256));
    let stroke = best(&mut || c.stroke_rrect(rect, 12, Corner::Circle, col2, 120));
    let body = Rect::new(150, 120, 600, 400);
    let hole = Rect::new(150, 132, 600, 376);
    let sh_area = ((body.w + 40) * (body.h + 40) - hole.w * hole.h) as u64;
    let shadow = best(&mut || {
        c.draw_shadow(
            body,
            Shadow {
                blur: 20,
                dy: 14,
                alpha: 70,
            },
            hole,
        );
        c.draw_shadow(
            body,
            Shadow {
                blur: 6,
                dy: 4,
                alpha: 64,
            },
            hole,
        );
    });
    let s = "The quick brown fox jumps over the lazy dog 0123";
    let t13 = best(&mut || {
        text::draw(c, 100, 100, s, 13, Weight::Regular, col);
    });
    let masks = crate::fb::masks();
    let mut icon = Surface::new(128, 128);
    icon.fill_rrect(
        0,
        0,
        128,
        128,
        29,
        raster::Corner::Squircle,
        Paint::Vertical(raster::rgb(0x60A5FA), raster::rgb(0x2563EB)),
        masks,
    );
    let scaled = icon.resized(56, 56);
    let blit = best(&mut || c.blit_surface(&scaled, 300, 300, 256));
    let resize = best(&mut || {
        core::hint::black_box(icon.resized(56, 56));
    });
    let mut bd = Surface::new(360, 80);
    bd.fill(raster::rgb(0x305080));
    let blur = best(&mut || bd.blur(8, 3));
    let mut buf = alloc::vec::Vec::new();
    let region = Rect::new(100, 100, 360, 80);
    let read = best(&mut || c.read_region(region, &mut buf));
    let write = best(&mut || c.write_region(region, &buf));
    let lut = raster::glow_lut();
    let glow = best(&mut || c.glow(400, 300, 300, col2, 40, &lut));
    let masks_t0 = io::rdtsc();
    core::hint::black_box(raster::CornerMasks::with_max_radius(48));
    let masks_build = io::rdtsc().wrapping_sub(masks_t0);
    crate::serial_println!(
        "[trace] bench ui ({}x{}): rrect solid {} ({}/px) | rrect a180 {} ({}/px) | rrect vgrad {} ({}/px) | stroke {} | shadow x2 {} ({}/px of ring) | text 48ch@13 {} ({}/ch) | blit 56x56 {} ({}/px) | resize 128->56 {} | blur 360x80 r8x3 {} ({}/px) | read 360x80 {} | write {} | glow r300 {} | masks(48) build {} | khz {}",
        rect.w,
        rect.h,
        rr_solid,
        rr_solid / px,
        rr_alpha,
        rr_alpha / px,
        rr_grad,
        rr_grad / px,
        stroke,
        shadow,
        shadow / sh_area.max(1),
        t13,
        t13 / s.len() as u64,
        blit,
        blit / (56 * 56),
        resize,
        blur,
        blur / (360 * 80),
        read,
        write,
        glow,
        masks_build,
        khz
    );
}
