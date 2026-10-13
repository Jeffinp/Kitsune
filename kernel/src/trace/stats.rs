//! Per-second runtime statistics (`perf-trace`): the frame paths and stages, the allocator, timer ISR and idle samples, input latency and the hooks that feed them.

use super::*;

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

pub(super) const PATHS: usize = 7;

pub(super) const PATH_NAMES: [&str; PATHS] = [
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

pub(super) const STAGES: usize = 4;

pub(super) const STAGE_NAMES: [&str; STAGES] = ["compose", "blit", "cursor", "hud"];

pub(super) struct Stats {
    pub(super) path_n: [u64; PATHS],
    pub(super) path_sum: [u64; PATHS],
    pub(super) path_cpu: [u64; PATHS],
    pub(super) path_max: [u64; PATHS],
    pub(super) stage_sum: [u64; STAGES],
    pub(super) stage_max: [u64; STAGES],
    pub(super) loops: u64,
    pub(super) hlt_wakes: u64,
    pub(super) lat_n: u64,
    pub(super) lat_sum: u64, // IRQ -> compositor picked the event up
    pub(super) lat_max: u64,
    pub(super) e2e_sum: u64, // IRQ -> frame finished
    pub(super) e2e_max: u64,
}

pub(super) static STATS: RacyCell<Stats> = RacyCell::new(Stats {
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
/// which the running thread was switched in. See [`cpu_now`]. One per scheduler slot (a page with
/// a video has the browser's script process, a worker or two and the decoders: more than 8).
pub static THREAD_CYC: [AtomicU64; kitsune_core::sysmon::MAX_THREADS] =
    [const { AtomicU64::new(0) }; kitsune_core::sysmon::MAX_THREADS];
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

pub(super) const PRIM_NAMES: [&str; 6] = ["glyph", "fillrect", "rrect", "alpha", "bgcopy", "fade"];

/// Account one call of `p` that began at `t0` (see [`t`]).
#[inline(always)]
pub fn prim(p: Prim, t0: u64) {
    if ON {
        PRIM_CYC[p as usize].fetch_add(io::rdtsc().wrapping_sub(t0), Relaxed);
        PRIM_N[p as usize].fetch_add(1, Relaxed);
    }
}

#[inline(always)]
pub(super) fn stats() -> &'static mut Stats {
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
