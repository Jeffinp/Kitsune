//! Springs (damped, retargetable) and tweens (fixed duration along a curve).

use super::*;

/// A damped spring (semi-implicit Euler with fixed sub-steps: stable for any
/// frame time). Interruptible: change the target at any moment.
#[derive(Clone, Copy, Debug)]
pub struct Spring {
    pub(super) pos: f32,
    pub(super) vel: f32,
    pub(super) target: f32,
    pub stiffness: f32,
    pub damping: f32,
    /// Resting tolerance on position and velocity.
    pub epsilon: f32,
}

impl Spring {
    /// A spring at rest at `pos`.
    pub const fn new(pos: f32, stiffness: f32, damping: f32) -> Spring {
        Spring {
            pos,
            vel: 0.0,
            target: pos,
            stiffness,
            damping,
            epsilon: 0.002,
        }
    }

    /// Pixel-scale springs rest within a twentieth of a pixel.
    pub const fn pixels(pos: f32, stiffness: f32, damping: f32) -> Spring {
        let mut s = Spring::new(pos, stiffness, damping);
        s.epsilon = 0.05;
        s
    }

    pub fn value(&self) -> f32 {
        self.pos
    }

    pub fn velocity(&self) -> f32 {
        self.vel
    }

    pub fn target(&self) -> f32 {
        self.target
    }

    /// Aim at a new target, keeping position and velocity.
    pub fn set_target(&mut self, t: f32) {
        self.target = t;
    }

    /// Snap to `v` at rest.
    pub fn jump(&mut self, v: f32) {
        self.pos = v;
        self.target = v;
        self.vel = 0.0;
    }

    /// Give the spring a kick (`v` units per second).
    pub fn impulse(&mut self, v: f32) {
        self.vel += v;
    }

    pub fn at_rest(&self) -> bool {
        (self.pos - self.target).abs() <= self.epsilon && self.vel.abs() <= self.epsilon * 10.0
    }

    /// Advance by `dt` seconds. Returns `true` while it still moves.
    pub fn step(&mut self, dt: f32) -> bool {
        if self.at_rest() {
            self.pos = self.target;
            self.vel = 0.0;
            return false;
        }
        if reduce_motion() || dt >= 2.0 {
            self.jump(self.target);
            return false;
        }
        // Sub-steps of at most 4 ms keep the integration stable for stiff springs.
        let n = ((dt / 0.004) as u32 + 1).min(64);
        let h = dt / n as f32;
        for _ in 0..n {
            let a = -self.stiffness * (self.pos - self.target) - self.damping * self.vel;
            self.vel += a * h;
            self.pos += self.vel * h;
        }
        if self.at_rest() {
            self.pos = self.target;
            self.vel = 0.0;
            return false;
        }
        true
    }
}

/// A time-based interpolation between two values along a [`Bezier`], which can be
/// retargeted mid-flight (it then starts from the value it shows right now).
#[derive(Clone, Copy, Debug)]
pub struct Tween {
    pub(super) from: f32,
    pub(super) to: f32,
    pub(super) t: f32,
    pub(super) dur: f32,
    pub(super) curve: Bezier,
}

impl Tween {
    /// A finished tween resting at `v`.
    pub const fn at(v: f32) -> Tween {
        Tween {
            from: v,
            to: v,
            t: 1.0,
            dur: 1.0,
            curve: curves::LINEAR,
        }
    }

    /// Start moving from the current value to `to` over `dur` seconds.
    pub fn retarget(&mut self, to: f32, dur: f32, curve: Bezier) {
        if to == self.to && !self.finished() {
            return;
        }
        self.from = self.value();
        self.to = to;
        self.t = 0.0;
        self.dur = dur.max(0.001);
        self.curve = curve;
        if reduce_motion() {
            self.t = self.dur;
        }
    }

    pub fn value(&self) -> f32 {
        if self.t >= self.dur {
            self.to
        } else {
            lerp(self.from, self.to, self.curve.ease(self.t / self.dur))
        }
    }

    pub fn target(&self) -> f32 {
        self.to
    }

    pub fn finished(&self) -> bool {
        self.t >= self.dur
    }

    /// Advance; `true` while still running.
    pub fn step(&mut self, dt: f32) -> bool {
        if self.finished() {
            return false;
        }
        self.t = if reduce_motion() {
            self.dur
        } else {
            self.t + dt
        };
        !self.finished()
    }
}
