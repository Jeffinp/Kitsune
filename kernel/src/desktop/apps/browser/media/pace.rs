//! When the pictures of a video reach the screen, measured where it happens: the compositor uploads
//! the damage of a frame and, if a new picture of a video was in it, stamps the time
//! ([`note_present`], right after the upload). The player that picked the picture takes the stamp
//! at its next pump and keeps it in its [`FrameTimes`] (the log line once a second, the statistics
//! overlay and the 10-second report's interval spread).

use super::*;
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use kitsune_core::media::pacing::{FrameTimes, Shown};

/// A new picture of a video was marked for the next frame.
static VIDEO_DIRTY: AtomicBool = AtomicBool::new(false);
/// Frames that uploaded a new picture, and the system time (µs) of the last one.
static PRESENTS: AtomicU64 = AtomicU64::new(0);
static PRESENT_US: AtomicU64 = AtomicU64::new(0);

/// The compositor uploaded a frame (called right after the upload): if it carried a new picture,
/// this is when it reached the screen, and the compositor's appointment moves to the next one.
/// Returns whether it did (the compositor does its once-a-second work right after a picture, so
/// that work does not delay one).
pub(crate) fn note_present() -> bool {
    let shown = VIDEO_DIRTY.swap(false, Ordering::Relaxed);
    if shown {
        PRESENT_US.store(crate::netd::now_us(), Ordering::Relaxed);
        PRESENTS.fetch_add(1, Ordering::Release);
        crate::sched::present::set_deadline(
            NEXT_DEADLINE.load(Ordering::Relaxed),
            crate::netd::tsc_khz(),
        );
    }
    shown
}

/// Does a video play (as of the last [`schedule_next`])?
pub(crate) fn video_active() -> bool {
    ACTIVE.load(Ordering::Relaxed)
}

/// A new picture is in the damage of the next frame.
pub(crate) fn mark_video_dirty() {
    VIDEO_DIRTY.store(true, Ordering::Relaxed);
}

/// The system time in µs (the TSC, calibrated at boot).
pub(super) fn now_us() -> u64 {
    crate::netd::now_us()
}

/// Bring the compositor to the CPU when a picture is due (`sched::present`).
const BOOST: bool = true;
/// Run the PIT at 1 kHz while a video plays, so that happens within a millisecond.
const FAST_TIMER: bool = true;

/// The earliest picture due over the players of this loop (ns from now; `i64::MAX`: none), and
/// whether a video plays.
static NEXT_DUE_NS: core::sync::atomic::AtomicI64 = core::sync::atomic::AtomicI64::new(i64::MAX);
static PLAYING: AtomicBool = AtomicBool::new(false);
/// [`PLAYING`] as of the last loop.
static ACTIVE: AtomicBool = AtomicBool::new(false);
/// The TSC deadline of the picture after the one waiting for the frame (see [`schedule_next`]).
static NEXT_DEADLINE: AtomicU64 = AtomicU64::new(0);

/// A player's next picture is due in `ns` (it plays a video).
pub(super) fn offer_due(ns: Option<i64>) {
    PLAYING.store(true, Ordering::Relaxed);
    if let Some(ns) = ns {
        NEXT_DUE_NS.fetch_min(ns, Ordering::Relaxed);
    }
}

/// After the browser's pump: tell the scheduler when the next picture is due and run the timer
/// fast while a video plays (or put both back). A picture picked for this frame keeps the
/// compositor on the CPU until it is on screen (a deadline already passed: the round robin would
/// otherwise take the CPU away in the middle of painting it, for the decoder's and the page's
/// whole slices); [`note_present`] then sets the next picture's deadline.
pub(crate) fn schedule_next() {
    let ns = NEXT_DUE_NS.swap(i64::MAX, Ordering::Relaxed);
    let playing = PLAYING.swap(false, Ordering::Relaxed);
    ACTIVE.store(playing, Ordering::Relaxed);
    let khz = crate::netd::tsc_khz();
    let deadline = if BOOST && ns != i64::MAX && khz > 0 {
        let cycles = (ns.max(0) as u128 * u128::from(khz) / 1_000_000) as u64;
        crate::io::rdtsc().wrapping_add(cycles).max(1)
    } else {
        0
    };
    NEXT_DEADLINE.store(deadline, Ordering::Relaxed);
    let hold = BOOST && VIDEO_DIRTY.load(Ordering::Relaxed);
    let at = if hold { crate::io::rdtsc() } else { deadline };
    crate::sched::present::set_deadline(at, khz);
    crate::interrupts::set_fast(FAST_TIMER && playing);
}

/// What a player measures of its pictures on screen.
#[derive(Default)]
pub(super) struct Pace {
    pub times: FrameTimes,
    /// The picture picked and not yet on screen: its time in the video, the clock's lead over it
    /// when it was picked (µs) and when (µs).
    awaiting: Option<(i64, i64, u64)>,
    /// [`PRESENTS`] when the picture was picked.
    seen: u64,
    /// Intervals between pictures on screen since the 10-second report's mark (µs).
    pub window: Vec<u32>,
    last_at: Option<u64>,
    /// Average time from picking a picture to its upload (µs, 1/8 weight to each new one).
    pub delay_us: u64,
    /// For a late picture's line: the start of the compositor's loop that picked it and the
    /// deadline the scheduler had then (µs).
    picked_loop: (u64, u64),
}

/// The start of the compositor's loop (µs), for the line of a late picture.
static LOOP_US: AtomicU64 = AtomicU64::new(0);

/// The compositor starts a loop.
pub(crate) fn loop_start() {
    LOOP_US.store(now_us(), Ordering::Relaxed);
}

impl Pace {
    /// The player put a new picture on screen (picked it) at `now_us`, with the clock at `t`.
    pub fn picked(&mut self, pts: i64, t: i64, now_us: u64) {
        self.awaiting = Some((pts, (t - pts) / 1000, now_us));
        self.seen = PRESENTS.load(Ordering::Acquire);
        let khz = crate::netd::tsc_khz().max(1);
        let due = crate::sched::present::deadline();
        self.picked_loop = (
            LOOP_US.load(Ordering::Relaxed),
            (u128::from(due) * 1000 / u128::from(khz)) as u64,
        );
        mark_video_dirty();
    }

    /// Take the stamp of the frame that carried the picked picture, if it was uploaded since.
    pub fn collect(&mut self) {
        let Some((pts, lead, picked)) = self.awaiting else {
            return;
        };
        if PRESENTS.load(Ordering::Acquire) == self.seen {
            return;
        }
        let at = PRESENT_US.load(Ordering::Relaxed);
        self.awaiting = None;
        let delay = at.saturating_sub(picked).min(200_000);
        self.delay_us = (self.delay_us * 7 + delay) / 8;
        let late = lead + at.saturating_sub(picked) as i64;
        if late > 15_000 {
            // Where the time went (serial only): the scheduler (deadline to the loop's start), the
            // loop before the pick, the frame after it.
            let (lp, due) = self.picked_loop;
            crate::serial::write_str_raw(&alloc::format!(
                "media: late picture by {} ms: deadline->loop {} ms, loop->pick {} ms, pick->screen {} ms\n",
                late / 1000,
                lp as i64 / 1000 - due as i64 / 1000,
                picked.saturating_sub(lp) / 1000,
                at.saturating_sub(picked) / 1000
            ));
        }
        self.times.push(Shown {
            at_us: at,
            pts,
            late_us: late.clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32,
        });
        if let Some(prev) = self.last_at
            && self.window.len() < 4096
        {
            self.window
                .push(at.saturating_sub(prev).min(u64::from(u32::MAX)) as u32);
        }
        self.last_at = Some(at);
    }

    /// A picture was picked and is not on screen yet.
    pub fn waiting(&self) -> bool {
        self.awaiting.is_some()
    }

    /// A seek or a pause: the next interval does not span it.
    pub fn reset(&mut self) {
        self.times.reset();
        self.awaiting = None;
        self.last_at = None;
    }
}

impl MediaSet {
    /// The page's script sent a document (`bytes`), parsed from `parse_us` and laid out until now:
    /// while a video plays, what it cost the compositor goes to the serial log (the pictures wait
    /// for it).
    pub(crate) fn note_document(&self, bytes: usize, start_us: u64, parsed_us: u64) {
        if !self.players.iter().any(|p| p.player.state().running()) {
            return;
        }
        let now = now_us();
        crate::serial::write_str_raw(&alloc::format!(
            "media: page document of {} KB while playing: parse {} ms, layout {} ms\n",
            bytes / 1024,
            parsed_us.saturating_sub(start_us) / 1000,
            now.saturating_sub(parsed_us) / 1000
        ));
    }
}

impl MediaPlayer {
    /// The log line of the pictures that reached the screen since the last one (serial only: one a
    /// second while a video plays would push everything else out of the log ring).
    pub(super) fn log_frames(&mut self) {
        if let Some(line) = self.pace.times.take_line() {
            crate::serial::write_str_raw(&alloc::format!(
                "media: element {} frames {line}\n",
                self.uid
            ));
        }
    }
}
