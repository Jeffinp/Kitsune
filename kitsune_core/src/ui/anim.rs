//! Motion: easing curves, springs, retargetable tweens and the window
//! transitions, all driven by real elapsed seconds (`dt`), never by frame counts.
//!
//! Everything is pure state advanced by [`step`](Spring::step) / [`Anim::step`]:
//! the compositor feeds it the time since the last frame, asks `active()` to decide
//! whether another frame is needed (an idle desktop schedules nothing), and reads
//! values to draw. Animations are interruptible: [`Spring::set_target`] and
//! [`Tween::retarget`] keep the current value (and velocity) and continue toward
//! the new goal. A global *reduce motion* switch makes every step land on its end
//! value immediately.
//!
//! `f32` is fine here: a handful of values per frame, never per pixel (the kernel
//! emulates floats in software).

use crate::windowing::window::Rect;
#[cfg(not(test))]
use core::sync::atomic::{AtomicBool, Ordering};

mod bezier;
mod caret;
mod spring;
mod transition;
mod zoom;
pub use bezier::*;
pub use caret::*;
pub use spring::*;
pub use transition::*;
pub use zoom::*;

#[cfg(not(test))]
static REDUCE_MOTION: AtomicBool = AtomicBool::new(false);

// Unit tests run in parallel threads: give each its own switch so a test that turns
// reduce motion on cannot disturb the others.
#[cfg(test)]
std::thread_local! {
    static REDUCE_MOTION_TL: core::cell::Cell<bool> = const { core::cell::Cell::new(false) };
}
/// Turn the global *reduce motion* switch on or off (the Settings toggle).
pub fn set_reduce_motion(on: bool) {
    #[cfg(not(test))]
    REDUCE_MOTION.store(on, Ordering::Relaxed);
    #[cfg(test)]
    REDUCE_MOTION_TL.with(|c| c.set(on));
}
/// Whether *reduce motion* is on.
pub fn reduce_motion() -> bool {
    #[cfg(not(test))]
    return REDUCE_MOTION.load(Ordering::Relaxed);
    #[cfg(test)]
    return REDUCE_MOTION_TL.with(|c| c.get());
}
fn clamp01(t: f32) -> f32 {
    t.clamp(0.0, 1.0)
}
fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

#[cfg(test)]
mod tests;
