//! Window transitions: open, close, minimize, slide and restore, as a frame of rect and alpha.

use super::*;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Phase {
    /// Window appearing: visibility goes 0 -> 1.
    Opening,
    /// Window disappearing: visibility goes 1 -> 0.
    Closing,
}

/// What the window travels to or from.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Flavor {
    /// Scale about its centre (new window, close).
    Pop,
    /// Toward / from its dock icon (minimise, restore).
    Dock,
    /// Sideways with a fade (a workspace change): the sign is the direction of the change, +1
    /// when the new workspace is to the right. Entering windows come from that side, leaving
    /// ones go to the other.
    Slide(i8),
}

/// How far, in pixels, a window travels sideways in a workspace change.
pub const SLIDE_PX: f32 = 420.0;

/// Seconds a workspace change takes.
pub const SLIDE_SECS: f32 = 0.30;

/// Scale of a window at the start of an open and the end of a close.
pub const POP_SCALE: f32 = 0.92;

/// Seconds an open takes.
pub const OPEN_SECS: f32 = 0.22;

/// Seconds a close takes.
pub const CLOSE_SECS: f32 = 0.16;

/// Seconds minimise / restore take.
pub const DOCK_SECS: f32 = 0.30;

/// The window enter/exit transition.
#[derive(Clone, Copy, Debug)]
pub struct Anim {
    pub(super) phase: Phase,
    pub(super) flavor: Flavor,
    /// Elapsed seconds.
    pub(super) t: f32,
    pub(super) dur: f32,
}

/// Where a window is drawn this frame and how opaque.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Frame {
    pub rect: Rect,
    /// 0..=256.
    pub alpha: u16,
}

impl Anim {
    pub fn open() -> Self {
        Self {
            phase: Phase::Opening,
            flavor: Flavor::Pop,
            t: 0.0,
            dur: OPEN_SECS,
        }
    }

    pub fn close() -> Self {
        Self {
            phase: Phase::Closing,
            flavor: Flavor::Pop,
            t: 0.0,
            dur: CLOSE_SECS,
        }
    }

    /// Minimise: the window flies into its dock icon.
    pub fn minimize() -> Self {
        Self {
            phase: Phase::Closing,
            flavor: Flavor::Dock,
            t: 0.0,
            dur: DOCK_SECS,
        }
    }

    /// A window of the new workspace slides in (`dir` = +1 when the workspace is to the right).
    pub fn slide_in(dir: i8) -> Self {
        Self {
            phase: Phase::Opening,
            flavor: Flavor::Slide(dir),
            t: 0.0,
            dur: SLIDE_SECS,
        }
    }

    /// A window of the old workspace slides out.
    pub fn slide_out(dir: i8) -> Self {
        Self {
            phase: Phase::Closing,
            flavor: Flavor::Slide(dir),
            t: 0.0,
            dur: SLIDE_SECS,
        }
    }

    /// Restore from the dock.
    pub fn restore() -> Self {
        Self {
            phase: Phase::Opening,
            flavor: Flavor::Dock,
            t: 0.0,
            dur: DOCK_SECS,
        }
    }

    pub fn phase(&self) -> Phase {
        self.phase
    }

    pub fn flavor(&self) -> Flavor {
        self.flavor
    }

    pub fn is_closing(&self) -> bool {
        self.phase == Phase::Closing
    }

    /// Advance by `dt` seconds (clamped at the end).
    pub fn step(&mut self, dt: f32) {
        self.t = if reduce_motion() {
            self.dur
        } else {
            (self.t + dt).min(self.dur)
        };
    }

    /// True once the transition has fully played out.
    pub fn finished(&self) -> bool {
        self.t >= self.dur
    }

    /// Linear progress `0..=1` through the transition.
    pub fn progress(&self) -> f32 {
        clamp01(self.t / self.dur)
    }

    /// Eased visibility in `0.0..=1.0` (0 = gone, 1 = fully present).
    pub fn visibility(&self) -> f32 {
        let p = self.progress();
        match (self.phase, self.flavor) {
            (Phase::Opening, Flavor::Pop | Flavor::Slide(_)) => curves::ENTER.ease(p),
            (Phase::Closing, Flavor::Pop | Flavor::Slide(_)) => 1.0 - curves::EXIT.ease(p),
            (Phase::Opening, Flavor::Dock) => curves::SWOOP.ease(p),
            (Phase::Closing, Flavor::Dock) => 1.0 - curves::SWOOP.ease(1.0 - (1.0 - p)),
        }
    }

    /// Fade factor for the window (same as visibility), `0.0..=1.0`.
    pub fn alpha(&self) -> f32 {
        self.visibility()
    }

    /// Where `rect` (the window's resting rectangle) is drawn now. `dock` is the
    /// screen rectangle of its dock icon, used by the [`Flavor::Dock`] transitions
    /// (without it they behave like [`Flavor::Pop`]).
    pub fn frame(&self, rect: Rect, dock: Option<Rect>) -> Frame {
        let v = self.visibility();
        match (self.flavor, dock) {
            (Flavor::Dock, Some(d)) => {
                // Position follows the eased value; the size shrinks a little
                // faster than the position travels (a lazy genie).
                let q = v;
                let qs = q * q;
                let lerp_i = |a: i32, b: i32, t: f32| (a as f32 + (b - a) as f32 * t + 0.5) as i32;
                let w = lerp_i(d.w, rect.w, qs.max(0.0)).max(1);
                let h = lerp_i(d.h, rect.h, qs.max(0.0)).max(1);
                // Keep the horizontal centre on the straight line between the centres.
                let cx = lerp_i(d.x + d.w / 2, rect.x + rect.w / 2, q);
                let cy = lerp_i(d.y + d.h / 2, rect.y + rect.h / 2, q);
                Frame {
                    rect: Rect::new(cx - w / 2, cy - h / 2, w, h),
                    alpha: (clamp01(v * 1.8) * 256.0) as u16,
                }
            }
            (Flavor::Slide(dir), _) => {
                // Entering windows come from the side of the change, leaving ones go away from it.
                let sign = if self.phase == Phase::Opening {
                    dir as f32
                } else {
                    -(dir as f32)
                };
                let off = (sign * (1.0 - v) * SLIDE_PX) as i32;
                Frame {
                    rect: Rect::new(rect.x + off, rect.y, rect.w, rect.h),
                    alpha: (v * 256.0) as u16,
                }
            }
            _ => {
                let s = lerp(POP_SCALE, 1.0, v);
                let w = (rect.w as f32 * s + 0.5) as i32;
                let h = (rect.h as f32 * s + 0.5) as i32;
                Frame {
                    rect: Rect::new(
                        rect.x + (rect.w - w) / 2,
                        rect.y + (rect.h - h) / 2,
                        w.max(1),
                        h.max(1),
                    ),
                    alpha: (v * 256.0) as u16,
                }
            }
        }
    }

    /// Vertical slide offset in pixels (kept for callers that only slide).
    pub fn slide(&self, max: f32) -> f32 {
        (1.0 - self.visibility()) * max
    }
}
