//! A sine tone, integer only: a 32-bit phase accumulator, a quarter-wave table of 257 entries
//! computed at compile time (Taylor series in fixed point), linear interpolation between entries
//! (error below -100 dB), and a short linear fade at both ends so the tone starts and stops
//! without a click.

/// Quarter-wave entries (the table has one more, for interpolation up to the peak).
const QUARTER: usize = 256;

/// `sin(i * pi/2 / 256)` in Q15 (32767 = 1.0), for `i` in `0..=256`.
static SINE: [i16; QUARTER + 1] = quarter_table();

const fn quarter_table() -> [i16; QUARTER + 1] {
    // pi/2 in Q32.
    const HALF_PI_Q32: i128 = 6_746_518_852;
    let mut t = [0i16; QUARTER + 1];
    let mut i = 0;
    while i <= QUARTER {
        let x = HALF_PI_Q32 * i as i128 / QUARTER as i128; // Q32
        // sin x = x - x^3/3! + x^5/5! - ...: each term is the last times -x^2 / ((2k)(2k+1)).
        let x2 = (x * x) >> 32;
        let mut term = x;
        let mut sum = x;
        let mut k = 1;
        while k <= 7 {
            term = -((term * x2) >> 32) / ((2 * k) * (2 * k + 1));
            sum += term;
            k += 1;
        }
        let v = (sum * 32767 + (1 << 31)) >> 32;
        t[i] = if v > 32767 { 32767 } else { v as i16 };
        i += 1;
    }
    t
}

/// `sin(2 pi phase / 2^32)` in Q15.
pub fn sine(phase: u32) -> i16 {
    let quadrant = phase >> 30;
    let within = phase & 0x3FFF_FFFF; // 30 bits across a quarter
    let pos = if quadrant & 1 == 0 {
        within
    } else {
        0x4000_0000 - within
    };
    // 8 bits of table index, 22 bits of fraction.
    let idx = (pos >> 22) as usize;
    let frac = i32::try_from(pos & 0x3F_FFFF).unwrap_or(0);
    let a = i32::from(SINE[idx.min(QUARTER)]);
    let b = i32::from(SINE[(idx + 1).min(QUARTER)]);
    let v = a + (((b - a) as i64 * i64::from(frac)) >> 22) as i32;
    if quadrant >= 2 { -v as i16 } else { v as i16 }
}

/// A tone of one frequency for a given number of frames.
#[derive(Clone, Debug)]
pub struct Tone {
    phase: u32,
    step: u32,
    amp: i32,
    pos: u64,
    total: u64,
    fade: u64,
}

impl Tone {
    /// `freq` Hz at `rate` frames per second for `ms` milliseconds, peak `amp` (Q15), with 5 ms fades.
    pub fn new(freq: u32, rate: u32, ms: u32, amp: i16) -> Tone {
        let rate = rate.max(1);
        let total = u64::from(ms) * u64::from(rate) / 1000;
        Tone {
            phase: 0,
            step: ((u64::from(freq) << 32) / u64::from(rate)) as u32,
            amp: i32::from(amp),
            pos: 0,
            total,
            fade: (u64::from(rate) / 200).min(total / 2).max(1),
        }
    }

    /// Frames still to come.
    pub fn remaining(&self) -> u64 {
        self.total - self.pos
    }

    /// Fill `out` with the next mono samples; returns how many (0 when the tone is over).
    pub fn fill(&mut self, out: &mut [i16]) -> usize {
        let n = (out.len() as u64).min(self.remaining()) as usize;
        for o in &mut out[..n] {
            let edge = self.pos.min(self.total - 1 - self.pos).min(self.fade);
            let env = (edge * 32768 / self.fade) as i64;
            let s = (i64::from(sine(self.phase)) * i64::from(self.amp)) >> 15;
            *o = ((s * env) >> 15) as i16;
            self.phase = self.phase.wrapping_add(self.step);
            self.pos += 1;
        }
        n
    }
}
