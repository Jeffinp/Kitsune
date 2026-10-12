//! Rectangle zoom and the dock bounce.

use super::*;

/// Maximise / restore: the window rectangle travels from `from` to `to` on a
/// spring (progress 0 -> 1, slightly under-damped so it settles with a hint of
/// life). The content is drawn stretched while it runs and re-laid-out at the end.
#[derive(Clone, Copy, Debug)]
pub struct Zoom {
    pub(super) from: Rect,
    pub(super) to: Rect,
    pub(super) p: Spring,
}

impl Zoom {
    pub fn new(from: Rect, to: Rect) -> Zoom {
        let mut p = Spring::new(0.0, 300.0, 30.0);
        p.set_target(1.0);
        if reduce_motion() {
            p.jump(1.0);
        }
        Zoom { from, to, p }
    }

    /// Re-aim at `to` from wherever the window is now (interrupting a zoom).
    pub fn retarget(&mut self, to: Rect) {
        let now = self.rect();
        self.from = now;
        self.to = to;
        self.p.jump(0.0);
        self.p.set_target(1.0);
        if reduce_motion() {
            self.p.jump(1.0);
        }
    }

    pub fn step(&mut self, dt: f32) -> bool {
        self.p.step(dt)
    }

    pub fn finished(&self) -> bool {
        self.p.at_rest()
    }

    /// The rectangle to draw now.
    pub fn rect(&self) -> Rect {
        let t = self.p.value();
        let l = |a: i32, b: i32| (a as f32 + (b - a) as f32 * t + 0.5) as i32;
        Rect::new(
            l(self.from.x, self.to.x),
            l(self.from.y, self.to.y),
            l(self.from.w, self.to.w).max(1),
            l(self.from.h, self.to.h).max(1),
        )
    }

    pub fn from(&self) -> Rect {
        self.from
    }

    pub fn to(&self) -> Rect {
        self.to
    }
}

/// The launch bounce of a dock icon: hops of decreasing height, each a parabola.
/// `height` is the peak of the first hop in pixels; `t` is seconds since launch.
/// Returns the upward offset in pixels (0 when done) and whether it still runs.
pub fn bounce(t: f32, height: f32) -> (f32, bool) {
    const HOP: f32 = 0.32;
    const HOPS: u32 = 3;
    if reduce_motion() || t >= HOP * HOPS as f32 || t < 0.0 {
        return (0.0, false);
    }
    let k = (t / HOP) as u32;
    let u = (t - k as f32 * HOP) / HOP; // 0..1 within the hop
    let peak = height * (1.0 - 0.38 * k as f32);
    (4.0 * peak * u * (1.0 - u), true)
}
