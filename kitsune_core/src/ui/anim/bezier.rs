//! CSS-style cubic-bezier easing curves and the design spec's presets.

use super::*;

/// A CSS-style `cubic-bezier(x1, y1, x2, y2)` easing curve.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Bezier {
    pub x1: f32,
    pub y1: f32,
    pub x2: f32,
    pub y2: f32,
}

impl Bezier {
    pub const fn new(x1: f32, y1: f32, x2: f32, y2: f32) -> Bezier {
        Bezier { x1, y1, x2, y2 }
    }

    /// Eased value for linear progress `t` in `0..=1` (0 at 0, 1 at 1).
    pub fn ease(&self, t: f32) -> f32 {
        let t = clamp01(t);
        if t == 0.0 || t == 1.0 {
            return t;
        }
        // Solve x(s) = t for the curve parameter s: Newton steps, then bisection.
        let x = |s: f32| {
            let u = 1.0 - s;
            3.0 * u * u * s * self.x1 + 3.0 * u * s * s * self.x2 + s * s * s
        };
        let dx = |s: f32| {
            let u = 1.0 - s;
            3.0 * u * u * self.x1
                + 6.0 * u * s * (self.x2 - self.x1)
                + 3.0 * s * s * (1.0 - self.x2)
        };
        let mut s = t;
        for _ in 0..4 {
            let d = dx(s);
            if d.abs() < 1e-5 {
                break;
            }
            s = clamp01(s - (x(s) - t) / d);
        }
        if (x(s) - t).abs() > 1e-4 {
            let (mut lo, mut hi) = (0.0f32, 1.0f32);
            for _ in 0..24 {
                s = (lo + hi) / 2.0;
                if x(s) < t {
                    lo = s;
                } else {
                    hi = s;
                }
            }
        }
        let u = 1.0 - s;
        3.0 * u * u * s * self.y1 + 3.0 * u * s * s * self.y2 + s * s * s
    }
}

/// Easing presets (the design spec's motion curves).
pub mod curves {
    use super::Bezier;
    /// Decelerating entrance: windows opening, popovers, overlays.
    pub const ENTER: Bezier = Bezier::new(0.2, 0.8, 0.2, 1.0);
    /// Accelerating exit.
    pub const EXIT: Bezier = Bezier::new(0.4, 0.0, 1.0, 1.0);
    /// Fast departure, soft landing.
    pub const SWOOP: Bezier = Bezier::new(0.3, 0.7, 0.2, 1.0);
    /// Plain ease in/out.
    pub const STANDARD: Bezier = Bezier::new(0.4, 0.0, 0.2, 1.0);
    pub const LINEAR: Bezier = Bezier::new(0.0, 0.0, 1.0, 1.0);
}
