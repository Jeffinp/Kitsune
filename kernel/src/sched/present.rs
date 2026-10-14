//! The compositor's appointment with the next picture of a video.
//!
//! While a video plays, the compositor tells the scheduler when its next picture is due (a TSC
//! value, [`set_deadline`]). From that instant the compositor is the thread that runs: at the next
//! timer interrupt (a tick, or a sub-tick while the PIT runs fast, `interrupts::set_fast`) the
//! scheduler switches to it whoever's turn it was, or keeps it running, so the picture reaches the
//! screen within a millisecond of its time instead of waiting for its round-robin turn behind the
//! decoder and the page (up to several ticks under load). The compositor sets the next deadline
//! (or none) every loop, and the favour lasts [`HOLD_MS`] at most: a compositor busy with something
//! long (a page's document) goes back to its turns, so the sound thread and the decoder are never
//! kept off the CPU for long.

use core::sync::atomic::{AtomicU64, Ordering};

/// How long past the deadline the compositor is favoured.
pub const HOLD_MS: u64 = 12;

/// TSC value at which the next picture is due (0: none), and when the favour ends.
static DEADLINE: AtomicU64 = AtomicU64::new(0);
static UNTIL: AtomicU64 = AtomicU64::new(0);

/// The next picture is due at TSC `tsc` (0: no picture is waiting); `khz` is the TSC rate.
pub fn set_deadline(tsc: u64, khz: u64) {
    // The timer interrupt may come between the stores: never a new deadline with an old end.
    DEADLINE.store(0, Ordering::Relaxed);
    UNTIL.store(tsc.saturating_add(HOLD_MS * khz), Ordering::Relaxed);
    DEADLINE.store(tsc, Ordering::Relaxed);
}

/// The deadline in force (TSC; 0: none).
pub fn deadline() -> u64 {
    DEADLINE.load(Ordering::Relaxed)
}

/// Is a picture due now? Cheap enough for the timer interrupt (one TSC read).
#[inline]
pub fn due() -> bool {
    let d = DEADLINE.load(Ordering::Relaxed);
    if d == 0 {
        return false;
    }
    let now = crate::io::rdtsc();
    now >= d && now < UNTIL.load(Ordering::Relaxed)
}
