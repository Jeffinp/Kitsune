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
/// this is when it reached the screen.
pub(crate) fn note_present() {
    if VIDEO_DIRTY.swap(false, Ordering::Relaxed) {
        PRESENT_US.store(crate::netd::now_us(), Ordering::Relaxed);
        PRESENTS.fetch_add(1, Ordering::Release);
    }
}

/// A new picture is in the damage of the next frame.
pub(crate) fn mark_video_dirty() {
    VIDEO_DIRTY.store(true, Ordering::Relaxed);
}

/// The system time in µs (the TSC, calibrated at boot).
pub(super) fn now_us() -> u64 {
    crate::netd::now_us()
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
}

impl Pace {
    /// The player put a new picture on screen (picked it) at `now_us`, with the clock at `t`.
    pub fn picked(&mut self, pts: i64, t: i64, now_us: u64) {
        self.awaiting = Some((pts, (t - pts) / 1000, now_us));
        self.seen = PRESENTS.load(Ordering::Acquire);
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
        let late = lead + at.saturating_sub(picked) as i64;
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

    /// A seek or a pause: the next interval does not span it.
    pub fn reset(&mut self) {
        self.times.reset();
        self.awaiting = None;
        self.last_at = None;
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
