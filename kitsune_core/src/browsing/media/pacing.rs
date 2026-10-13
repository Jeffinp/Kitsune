//! Frame pacing: when the pictures of a video reach the screen, and how evenly.
//!
//! * [`SmoothClock`]: the sound clock as the pictures should follow it. The sound clock (the
//!   mixer's "played" counter) moves in steps: the device reports its position once per period
//!   (QEMU's AC'97 every 10 ms, at its audio timer) and the kernel reads it once per timer tick, when
//!   the sound thread gets the CPU. Pictures picked on that staircase come out on its grid (30 fps
//!   shown as 30/30/40 ms). The smooth clock runs on the system's fine time (the TSC, in µs) and is
//!   pulled gently towards the sound clock, so it follows the sound on average and moves evenly; it
//!   never runs more than [`MAX_LEAD_NS`] ahead of the sound (a sound that stops stops it) and jumps
//!   when the two disagree by more than [`SNAP_NS`] (a seek, a stall).
//! * [`FrameTimes`]: the instants the pictures reached the screen, with their times in the video
//!   and how late each one was against the clock; the 1-second log line and the statistics the
//!   overlay shows (frames a second, interval spread, lateness).
//! * [`Spread`]: mean, standard deviation, extremes and percentiles of a set of intervals.

use alloc::collections::VecDeque;
use alloc::string::String;
use alloc::vec::Vec;
use core::fmt::Write as _;

/// The smooth clock jumps to the sound clock when they differ by more than this.
pub const SNAP_NS: i64 = 50_000_000;
/// The smooth clock never runs further than this ahead of the sound clock.
pub const MAX_LEAD_NS: i64 = 40_000_000;
/// Time constant of the pull towards the sound clock.
const TAU_US: u64 = 250_000;

/// See the module documentation.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SmoothClock {
    /// The value returned last and the system time (µs) it was for.
    anchor: Option<(i64, u64)>,
}

impl SmoothClock {
    /// Forget the past (a seek, a pause): the next value is the sound clock's.
    pub fn reset(&mut self) {
        self.anchor = None;
    }

    /// The media time at system time `now_us`, given the sound clock's value `coarse` now.
    pub fn now(&mut self, coarse: i64, now_us: u64) -> i64 {
        let Some((last, at)) = self.anchor else {
            self.anchor = Some((coarse, now_us));
            return coarse;
        };
        let dt = now_us.saturating_sub(at);
        let predicted = last.saturating_add(dt.min(1 << 40) as i64 * 1000);
        let err = coarse - predicted;
        if err.abs() > SNAP_NS {
            self.anchor = Some((coarse, now_us));
            return coarse;
        }
        // A first-order pull: a fraction dt / TAU of the error (all of it after TAU).
        let pulled = predicted + err * dt.min(TAU_US) as i64 / TAU_US as i64;
        // Never far ahead of the sound, and never backwards.
        let t = pulled.min(coarse + MAX_LEAD_NS).max(last);
        self.anchor = Some((t, now_us));
        t
    }

    /// Is the clock following a running sound clock?
    pub fn active(&self) -> bool {
        self.anchor.is_some()
    }
}

/// Statistics of a set of intervals (µs).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Spread {
    pub n: u32,
    pub mean: u32,
    /// Standard deviation (population).
    pub sd: u32,
    pub min: u32,
    pub max: u32,
    pub p50: u32,
    pub p95: u32,
    pub p99: u32,
}

/// The integer square root of `v`.
pub fn isqrt(v: u64) -> u64 {
    if v < 2 {
        return v;
    }
    // Newton's method from an over-estimate.
    let mut x = 1u64 << (64 - v.leading_zeros()).div_ceil(2);
    loop {
        let y = (x + v / x) / 2;
        if y >= x {
            return x;
        }
        x = y;
    }
}

impl Spread {
    /// The statistics of `v` (sorted in place).
    pub fn of(v: &mut [u32]) -> Spread {
        if v.is_empty() {
            return Spread::default();
        }
        v.sort_unstable();
        let n = v.len() as u64;
        let sum: u64 = v.iter().map(|&x| u64::from(x)).sum();
        let mean = sum / n;
        let var = v
            .iter()
            .map(|&x| {
                let d = i64::from(x) - mean as i64;
                (d * d) as u64
            })
            .sum::<u64>()
            / n;
        // Nearest-rank percentiles.
        let pct = |p: u64| v[((p * n).div_ceil(100) as usize).clamp(1, v.len()) - 1];
        Spread {
            n: n as u32,
            mean: mean as u32,
            sd: isqrt(var) as u32,
            min: v[0],
            max: v[v.len() - 1],
            p50: pct(50),
            p95: pct(95),
            p99: pct(99),
        }
    }
}

/// One picture that reached the screen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Shown {
    /// System time it reached the screen, µs.
    pub at_us: u64,
    /// Its time in the video, ns.
    pub pts: i64,
    /// How far the clock was past its time when it reached the screen, µs (negative: early).
    pub late_us: i32,
}

/// How long [`FrameTimes`] remembers pictures.
const KEEP_US: u64 = 4_000_000;
/// The most pictures in one log line.
const LINE_MAX: usize = 120;

/// The pictures that reached the screen lately (see the module documentation).
#[derive(Default)]
pub struct FrameTimes {
    recent: VecDeque<Shown>,
    /// Not in a log line yet.
    pending: Vec<Shown>,
    /// The last picture of the previous line (the first interval of the next one starts there).
    logged: Option<Shown>,
}

impl FrameTimes {
    /// A picture reached the screen.
    pub fn push(&mut self, s: Shown) {
        while self
            .recent
            .front()
            .is_some_and(|f| s.at_us.saturating_sub(f.at_us) > KEEP_US)
        {
            self.recent.pop_front();
        }
        self.recent.push_back(s);
        if self.pending.len() < LINE_MAX {
            self.pending.push(s);
        }
    }

    /// Forget everything (a seek, a pause): the next interval does not span it.
    pub fn reset(&mut self) {
        self.recent.clear();
        self.pending.clear();
        self.logged = None;
    }

    /// The pictures of the last `window_us` before `now_us`.
    fn window(&self, now_us: u64, window_us: u64) -> impl Iterator<Item = &Shown> {
        self.recent
            .iter()
            .filter(move |f| now_us.saturating_sub(f.at_us) <= window_us)
    }

    /// Pictures a second over the last `window_us` before `now_us`, in thousandths.
    pub fn milli_fps(&self, now_us: u64, window_us: u64) -> u32 {
        let n = self.window(now_us, window_us).count() as u64;
        (n * 1_000_000_000 / window_us.max(1)) as u32
    }

    /// The intervals between the pictures of the last `window_us`.
    pub fn spread(&self, now_us: u64, window_us: u64) -> Spread {
        let w: Vec<&Shown> = self.window(now_us, window_us).collect();
        let mut v: Vec<u32> = w
            .windows(2)
            .map(|p| {
                p[1].at_us
                    .saturating_sub(p[0].at_us)
                    .min(u64::from(u32::MAX)) as u32
            })
            .collect();
        Spread::of(&mut v)
    }

    /// Mean lateness of the pictures of the last `window_us`, µs.
    pub fn mean_late(&self, now_us: u64, window_us: u64) -> i32 {
        let (n, sum) = self
            .window(now_us, window_us)
            .fold((0i64, 0i64), |(n, s), f| (n + 1, s + i64::from(f.late_us)));
        sum.checked_div(n).unwrap_or(0) as i32
    }

    /// The log line of the pictures since the last one (`None`: none): when the first reached the
    /// screen (system ms) and its time in the video (ms), then for each picture the interval since
    /// the one before (ms, one decimal), the step of its video time (ms) and how late it was (ms):
    /// `at 52340.1 pts 10040: 40.0/40/3 40.1/40/2 ...`. `tools/media/frame-intervals.py` reads it.
    pub fn take_line(&mut self) -> Option<String> {
        if self.pending.is_empty() {
            return None;
        }
        let mut s = String::new();
        let first = self.logged.unwrap_or(self.pending[0]);
        let _ = write!(
            s,
            "at {}.{} pts {}:",
            first.at_us / 1000,
            first.at_us % 1000 / 100,
            first.pts.div_euclid(1_000_000)
        );
        let mut prev = self.logged;
        for f in &self.pending {
            if let Some(p) = prev {
                let dt = f.at_us.saturating_sub(p.at_us);
                let _ = write!(
                    s,
                    " {}.{}/{}/{}",
                    dt / 1000,
                    dt % 1000 / 100,
                    (f.pts - p.pts).div_euclid(1_000_000),
                    f.late_us / 1000
                );
            }
            prev = Some(*f);
        }
        self.logged = prev;
        self.pending.clear();
        Some(s)
    }
}
