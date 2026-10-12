//! The halo: the head in front of a half-circle crown of curved tails (boot splash, About page).

use super::*;

/// The spine of a tail pointing up from the pivot at the origin, in coordinate units:
/// `(x, y, half width)` from the base to the tip. The spine bends to the right (the tails
/// on the left are mirrored, so every tail curls outwards) and the tail swells in the
/// middle like a brush before it tapers to the tip.
pub(super) const TAIL_SPINE: [(i32, i32, i32); 9] = [
    (0, 0, 14),
    (2, -30, 22),
    (5, -62, 27),
    (10, -94, 30),
    (17, -126, 30),
    (26, -156, 27),
    (36, -186, 21),
    (48, -212, 13),
    (62, -236, 0),
];

/// Index of the spine point where the cream tip starts.
pub(super) const TAIL_TIP_FROM: usize = 6;

/// A tail's outline (left edge up, right edge down) and its cream tip.
pub(super) fn tail_shapes() -> (Pts, Pts) {
    let left = |i: usize| (TAIL_SPINE[i].0 - TAIL_SPINE[i].2, TAIL_SPINE[i].1);
    let right = |i: usize| (TAIL_SPINE[i].0 + TAIL_SPINE[i].2, TAIL_SPINE[i].1);
    let last = TAIL_SPINE.len() - 1;
    let mut outline: Vec<(i32, i32)> = (0..last).map(left).collect();
    outline.push((TAIL_SPINE[last].0, TAIL_SPINE[last].1));
    outline.extend((0..last).rev().map(right));
    let mut tip: Vec<(i32, i32)> = (TAIL_TIP_FROM..last).map(left).collect();
    tip.push((TAIL_SPINE[last].0, TAIL_SPINE[last].1));
    tip.extend((TAIL_TIP_FROM..last).rev().map(right));
    (outline, tip)
}

/// Where the tails fan out from (behind the head).
pub(super) const HALO_PIVOT: (i32, i32) = (256, 372);

/// Scale of the head in front of the crown, in 1/256.
pub(super) const HALO_HEAD_K: i32 = 150;

/// Where the chin of the small head sits.
pub(super) const HALO_CHIN_Y: i32 = 478;

pub(super) fn rotate_about(pivot: (i32, i32), p: (i32, i32), deg: i32) -> (i32, i32) {
    let (s, c) = (sin_q14(deg) as i64, cos_q14(deg) as i64);
    let (x, y) = (p.0 as i64, p.1 as i64);
    (
        pivot.0 + ((x * c - y * s + (1 << 13)) >> 14) as i32,
        pivot.1 + ((x * s + y * c + (1 << 13)) >> 14) as i32,
    )
}

/// The head and a crown of [`HALO_TAILS`] tails (the mark of the boot splash and the About
/// page); no tile, a transparent background.
pub fn halo(s: &Scheme) -> Mark {
    halo_n(s, HALO_TAILS)
}

/// Like [`halo`] with `n` tails (clamped to 1..=9), spread over a half circle.
pub fn halo_n(s: &Scheme, n: usize) -> Mark {
    let n = n.clamp(1, 9);
    let mut polys: Vec<Poly> = Vec::new();
    let span = 136; // degrees between the outermost tails
    // Draw from the sides inwards so the middle tails lie over their neighbours.
    let mut order: Vec<usize> = (0..n).collect();
    order.sort_by_key(|&i| core::cmp::Reverse((2 * i as i32 - (n as i32 - 1)).abs()));
    for &i in &order {
        let deg = if n == 1 {
            0
        } else {
            -span / 2 + span * i as i32 / (n as i32 - 1)
        };
        let (role, tip_role) = if i % 2 == 0 {
            (Role::Tail, Role::TailTip)
        } else {
            (Role::TailDark, Role::TailTip)
        };
        // The tails left of the axis curl left, the others right: the crown is symmetric.
        let flip = deg < 0;
        let place = |pts: &[(i32, i32)]| -> Vec<(i32, i32)> {
            let it = pts
                .iter()
                .map(|&(x, y)| rotate_about(HALO_PIVOT, (if flip { -x } else { x }, y), deg));
            if flip {
                it.rev().collect()
            } else {
                it.collect()
            }
        };
        let (outline, tip) = tail_shapes();
        polys.push(Poly {
            role,
            color: s.color(role),
            pts: place(&outline),
        });
        polys.push(Poly {
            role: tip_role,
            color: s.color(tip_role),
            pts: place(&tip),
        });
    }
    for mut p in fox_head(s) {
        for q in p.pts.iter_mut() {
            q.0 = EXTENT / 2 + ((q.0 - EXTENT / 2) * HALO_HEAD_K + 128) / 256;
            q.1 = HALO_CHIN_Y + ((q.1 - 468) * HALO_HEAD_K - 128) / 256;
        }
        polys.push(p);
    }
    Mark { tile: None, polys }
}
