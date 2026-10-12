//! Resampling (area and bilinear taps) and the separable box blur.

use super::*;

/// Scale all four premultiplied channels by `a256 / 256`.
#[inline]
pub fn scale_premul(p: u32, a256: u32) -> u32 {
    let rb = (((p & 0x00FF_00FF) * a256 + 0x0080_0080) >> 8) & 0x00FF_00FF;
    let ag = ((((p >> 8) & 0x00FF_00FF) * a256 + 0x0080_0080) >> 8) & 0x00FF_00FF;
    rb | (ag << 8)
}

// Resampling taps: for every destination index the first source index and the
// weights (Q16, summing to exactly 65536).

pub(super) struct Tap {
    pub(super) start: usize,
    pub(super) w: Vec<u32>,
}

pub(super) fn taps(src: usize, dst: usize) -> Vec<Tap> {
    let mut out = Vec::with_capacity(dst);
    for d in 0..dst {
        if dst < src {
            // Box filter over [d*src/dst, (d+1)*src/dst) in Q8.
            let a = d * src * 256 / dst;
            let b = ((d + 1) * src * 256 / dst).max(a + 1);
            let first = a / 256;
            let last = b.div_ceil(256).min(src);
            let mut w = Vec::with_capacity(last - first);
            for i in first..last {
                let lo = a.max(i * 256);
                let hi = b.min((i + 1) * 256);
                w.push(((hi - lo) * 65536 / (b - a)) as u32);
            }
            normalise(&mut w);
            out.push(Tap { start: first, w });
        } else {
            // Bilinear: sample position of the destination pixel centre in the source.
            let pos = ((2 * d + 1) * src * 128 / dst) as i64 - 128; // Q8 of (x+0.5)*s - 0.5
            let pos = pos.clamp(0, ((src - 1) * 256) as i64) as usize;
            let i = pos / 256;
            let f = (pos % 256) as u32;
            if i + 1 >= src || f == 0 {
                out.push(Tap {
                    start: i.min(src - 1),
                    w: vec![65536],
                });
            } else {
                out.push(Tap {
                    start: i,
                    w: vec![(256 - f) * 256, f * 256],
                });
            }
        }
    }
    out
}

pub(super) fn normalise(w: &mut [u32]) {
    let sum: u32 = w.iter().sum();
    if let Some(last) = w.last_mut() {
        if sum < 65536 {
            *last += 65536 - sum;
        } else {
            *last -= (sum - 65536).min(*last);
        }
    }
}

pub(super) fn weigh(t: &Tap, get: impl Fn(usize) -> u32) -> u32 {
    let (mut a, mut r, mut g, mut b) = (0u32, 0u32, 0u32, 0u32);
    for (k, &wt) in t.w.iter().enumerate() {
        let p = get(t.start + k);
        a += (p >> 24) * wt;
        r += ((p >> 16) & 0xFF) * wt;
        g += ((p >> 8) & 0xFF) * wt;
        b += (p & 0xFF) * wt;
    }
    let q = |v: u32| (v + 32768) >> 16;
    (q(a).min(255) << 24) | (q(r).min(255) << 16) | (q(g).min(255) << 8) | q(b).min(255)
}

/// Separable box blur on packed 4-channel pixels (alpha-premultiplied or opaque).
pub fn blur_u32(px: &mut [u32], w: usize, h: usize, radius: usize, passes: usize) {
    if radius == 0 || w == 0 || h == 0 || px.len() < w * h {
        return;
    }
    let radius = radius.min(127);
    let mut tmp = vec![0u32; w.max(h)];
    for _ in 0..passes.max(1) {
        for y in 0..h {
            blur_line(&mut px[y * w..(y + 1) * w], radius, &mut tmp);
        }
        // Columns: gather, blur, scatter.
        let mut col = vec![0u32; h];
        for x in 0..w {
            for y in 0..h {
                col[y] = px[y * w + x];
            }
            blur_line(&mut col, radius, &mut tmp);
            for y in 0..h {
                px[y * w + x] = col[y];
            }
        }
    }
}

pub(super) fn blur_line(line: &mut [u32], radius: usize, tmp: &mut [u32]) {
    let n = line.len();
    let win = 2 * radius + 1;
    let inv = (1u32 << 16) / win as u32;
    let at = |i: isize| -> u32 { line[i.clamp(0, n as isize - 1) as usize] };
    let (mut a, mut r, mut g, mut b) = (0u32, 0u32, 0u32, 0u32);
    for k in -(radius as isize)..=(radius as isize) {
        let p = at(k);
        a += p >> 24;
        r += (p >> 16) & 0xFF;
        g += (p >> 8) & 0xFF;
        b += p & 0xFF;
    }
    for (i, slot) in tmp.iter_mut().enumerate().take(n) {
        let q = |v: u32| ((v * inv + 32768) >> 16).min(255);
        *slot = (q(a) << 24) | (q(r) << 16) | (q(g) << 8) | q(b);
        let out = at(i as isize - radius as isize);
        let inn = at(i as isize + radius as isize + 1);
        a = a + (inn >> 24) - (out >> 24);
        r = r + ((inn >> 16) & 0xFF) - ((out >> 16) & 0xFF);
        g = g + ((inn >> 8) & 0xFF) - ((out >> 8) & 0xFF);
        b = b + (inn & 0xFF) - (out & 0xFF);
    }
    line.copy_from_slice(&tmp[..n]);
}
