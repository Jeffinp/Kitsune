//! 2D affine transforms (16.16 fixed point), the `transform` attribute and the integer sine.

use super::*;

/// `x' = a x + c y + e`, `y' = b x + d y + f`, all 16.16.
#[derive(Clone, Copy)]
pub(super) struct Mat {
    pub(super) a: i64,
    pub(super) b: i64,
    pub(super) c: i64,
    pub(super) d: i64,
    pub(super) e: i64,
    pub(super) f: i64,
}

impl Mat {
    const ID: Mat = Mat {
        a: 65536,
        b: 0,
        c: 0,
        d: 65536,
        e: 0,
        f: 0,
    };

    /// `self` applied after `o` (the point goes through `o` first).
    pub(super) fn mul(self, o: Mat) -> Mat {
        let m = |x: i64, y: i64| (x * y) >> 16;
        Mat {
            a: m(self.a, o.a) + m(self.c, o.b),
            b: m(self.b, o.a) + m(self.d, o.b),
            c: m(self.a, o.c) + m(self.c, o.d),
            d: m(self.b, o.c) + m(self.d, o.d),
            e: m(self.a, o.e) + m(self.c, o.f) + self.e,
            f: m(self.b, o.e) + m(self.d, o.f) + self.f,
        }
    }

    /// A 16.16 point to device 24.8.
    pub(super) fn apply(&self, x: i64, y: i64) -> (i64, i64) {
        let m = |p: i64, q: i64| (p * q) >> 16;
        let (nx, ny) = (
            m(self.a, x) + m(self.c, y) + self.e,
            m(self.b, x) + m(self.d, y) + self.f,
        );
        (nx >> 8, ny >> 8)
    }

    /// How much lengths grow (square root of the area scale), 16.16.
    pub(super) fn scale(&self) -> i64 {
        let det = ((self.a * self.d - self.b * self.c) >> 16).abs();
        isqrt(det << 16)
    }
}

pub(super) fn isqrt(n: i64) -> i64 {
    if n <= 0 {
        return 0;
    }
    let mut x = n;
    let mut y = (x + 1) / 2;
    while y < x {
        x = y;
        y = (x + n / x) / 2;
    }
    x
}

/// Sine and cosine of `deg` degrees (16.16), by a quarter-wave table of 16 steps and linear
/// interpolation: good to about 0.2 %, which is a fraction of a pixel on a logo.
pub(super) fn sin_cos(deg: i64) -> (i64, i64) {
    const TABLE: [i64; 17] = [
        0, 6393, 12540, 18205, 23170, 27246, 30274, 32138, 32768, 32138, 30274, 27246, 23170,
        18205, 12540, 6393, 0,
    ];
    // sin(k * 5.625 deg) * 65536 for k in 0..=16 is TABLE-like only up to 90: build it properly.
    let _ = TABLE;
    const SIN: [i64; 17] = [
        0, 6424, 12785, 19024, 25080, 30893, 36410, 41576, 46341, 50660, 54491, 57797, 60547,
        62714, 64277, 65220, 65536,
    ];
    let sin_q = |d: i64| -> i64 {
        // d in 0..=90 degrees.
        let pos = d * 16 * 256 / 90;
        let i = (pos >> 8) as usize;
        let f = pos & 255;
        if i >= 16 {
            SIN[16]
        } else {
            SIN[i] + ((SIN[i + 1] - SIN[i]) * f >> 8)
        }
    };
    let d = deg.rem_euclid(360);
    let sin = match d {
        0..=90 => sin_q(d),
        91..=180 => sin_q(180 - d),
        181..=270 => -sin_q(d - 180),
        _ => -sin_q(360 - d),
    };
    let cd = (d + 90) % 360;
    let cos = match cd {
        0..=90 => sin_q(cd),
        91..=180 => sin_q(180 - cd),
        181..=270 => -sin_q(cd - 180),
        _ => -sin_q(360 - cd),
    };
    (sin, cos)
}

pub(super) fn parse_transform(s: &str) -> Mat {
    let mut m = Mat::ID;
    let mut rest = s;
    while let Some(open) = rest.find('(') {
        let name = rest[..open].trim().trim_start_matches([',', ' ']);
        let Some(close) = rest[open..].find(')') else {
            break;
        };
        let args_s = &rest[open + 1..open + close];
        rest = &rest[open + close + 1..];
        let mut args = [0i64; 6];
        let mut n = 0;
        let mut a = args_s;
        while n < 6 {
            match num(skip_sep(a)) {
                Some((v, r)) => {
                    args[n] = v;
                    n += 1;
                    a = r;
                }
                None => break,
            }
        }
        let t = match (name, n) {
            ("matrix", 6) => Mat {
                a: args[0],
                b: args[1],
                c: args[2],
                d: args[3],
                e: args[4],
                f: args[5],
            },
            ("translate", 1) => Mat {
                e: args[0],
                ..Mat::ID
            },
            ("translate", 2) => Mat {
                e: args[0],
                f: args[1],
                ..Mat::ID
            },
            ("scale", 1) => Mat {
                a: args[0],
                d: args[0],
                ..Mat::ID
            },
            ("scale", 2) => Mat {
                a: args[0],
                d: args[1],
                ..Mat::ID
            },
            ("rotate", 1 | 3) => {
                let (s, c) = sin_cos(args[0] >> 16);
                let r = Mat {
                    a: c,
                    b: s,
                    c: -s,
                    d: c,
                    e: 0,
                    f: 0,
                };
                if n == 3 {
                    let (cx, cy) = (args[1], args[2]);
                    let to = Mat {
                        e: cx,
                        f: cy,
                        ..Mat::ID
                    };
                    let back = Mat {
                        e: -cx,
                        f: -cy,
                        ..Mat::ID
                    };
                    to.mul(r).mul(back)
                } else {
                    r
                }
            }
            ("skewX", 1) => {
                let (s, c) = sin_cos(args[0] >> 16);
                Mat {
                    c: if c == 0 { 0 } else { (s << 16) / c },
                    ..Mat::ID
                }
            }
            ("skewY", 1) => {
                let (s, c) = sin_cos(args[0] >> 16);
                Mat {
                    b: if c == 0 { 0 } else { (s << 16) / c },
                    ..Mat::ID
                }
            }
            _ => Mat::ID,
        };
        m = m.mul(t);
    }
    m
}
