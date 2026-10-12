//! Per-size hints: merged facets and thicker eyes and nose so that 16 and 24 px stay readable.

use super::*;

/// Per-size tuning of a mark (see [`hint`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Hint {
    /// Eyes scaled about their centres, in percent.
    pub eye_pct: i32,
    /// The nose, in percent.
    pub nose_pct: i32,
    /// Keep the dark inner facets of the ears (below 24 px they only add noise).
    pub ear_inner: bool,
    /// Supersampling factor of the rasteriser.
    pub supersample: usize,
}

/// The tuning for a mark rendered at `px` pixels square. Eyes and nose grow as the size
/// shrinks so that they keep a footprint of about two pixels; the ear facets are merged
/// below 24 px.
pub fn hint(px: usize) -> Hint {
    let (eye_pct, nose_pct, ear_inner) = match px {
        0..=16 => (250, 190, false),
        17..=24 => (165, 140, true),
        25..=32 => (135, 120, true),
        33..=48 => (112, 108, true),
        _ => (100, 100, true),
    };
    Hint {
        eye_pct,
        nose_pct,
        ear_inner,
        supersample: match px {
            0..=32 => 8,
            33..=96 => 4,
            // Large renders: the path coverage is exact already, 2x only evens out the
            // seams, and it keeps the cost of the splash mark low.
            _ => 2,
        },
    }
}

pub(super) fn centroid(pts: &[(i32, i32)]) -> (i32, i32) {
    let n = pts.len().max(1) as i64;
    (
        (pts.iter().map(|p| p.0 as i64).sum::<i64>() / n) as i32,
        (pts.iter().map(|p| p.1 as i64).sum::<i64>() / n) as i32,
    )
}

/// Scale `pts` by `pct` percent about their centroid.
pub(super) fn scaled(pts: &[(i32, i32)], pct: i32) -> Vec<(i32, i32)> {
    let (cx, cy) = centroid(pts);
    let f = |v: i32, c: i32| c + ((v - c) * pct + 50 * (v - c).signum()) / 100;
    pts.iter().map(|&(x, y)| (f(x, cx), f(y, cy))).collect()
}

/// `mark` with the tuning for `px` applied.
pub fn hinted(mark: &Mark, px: usize) -> Mark {
    let h = hint(px);
    let mut out = mark.clone();
    out.polys
        .retain(|p| h.ear_inner || p.role != Role::EarInner);
    for p in out.polys.iter_mut() {
        let pct = match p.role {
            Role::Eye => h.eye_pct,
            Role::Nose => h.nose_pct,
            // A knockout is an eye or the nose: the nose is the one with four corners.
            Role::Knockout if p.pts.len() == 4 => h.nose_pct,
            Role::Knockout => h.eye_pct,
            _ => continue,
        };
        if pct == 100 {
            continue;
        }
        // Scale the right-hand half through its mirror image so the two sides stay
        // pixel-for-pixel mirrored whatever the rounding does.
        p.pts = if centroid(&p.pts).0 > EXTENT / 2 {
            let m: Vec<_> = p.pts.iter().map(|&q| mir(q)).collect();
            scaled(&m, pct).into_iter().map(mir).collect()
        } else {
            scaled(&p.pts, pct)
        };
    }
    out
}
