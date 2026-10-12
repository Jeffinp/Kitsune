//! The fox head: its polygons, the app-icon tile, the one-colour silhouette and the fallback mark.

use super::*;

/// Mirror a point about the symmetry axis.
pub(super) fn mir((x, y): (i32, i32)) -> (i32, i32) {
    (EXTENT - x, y)
}

pub(super) fn mirrored(pts: &[(i32, i32)]) -> Vec<(i32, i32)> {
    // Mirroring flips the winding; reverse to stay clockwise.
    pts.iter().rev().map(|&p| mir(p)).collect()
}

pub(super) const TIP: (i32, i32) = (93, 44);

pub(super) const FORE_L: (i32, i32) = (206, 153);

pub(super) const FORE_C: (i32, i32) = (256, 153);

pub(super) const EAR_BASE: (i32, i32) = (101, 246);

pub(super) const INNER_MID: (i32, i32) = (170, 185);

pub(super) const CHEEK_TIP: (i32, i32) = (65, 324);

pub(super) const CHEEK_IN: (i32, i32) = (137, 294);

pub(super) const NOSE_L: (i32, i32) = (229, 400);

pub(super) const NOSE_T: (i32, i32) = (256, 391);

pub(super) const NOSE_B: (i32, i32) = (256, 434);

pub(super) const CHIN: (i32, i32) = (256, 468);

/// The left eye (an almond: flat on top, pointing down towards the nose).
pub(super) const EYE_L: [(i32, i32); 7] = [
    (165, 280),
    (190, 277),
    (206, 287),
    (216, 311),
    (194, 312),
    (176, 301),
    (167, 289),
];

/// The head as 12 flat polygons, back to front.
pub fn fox_head(s: &Scheme) -> Vec<Poly> {
    let mut out: Vec<Poly> = Vec::with_capacity(12);
    let mut add = |role: Role, pts: Vec<(i32, i32)>| {
        out.push(Poly {
            role,
            color: s.color(role),
            pts,
        });
    };
    // Ears: the outer facet reaches under the face, so the edge they share has no seam.
    let ear_outer = [TIP, FORE_L, (200, 190), (125, 255), EAR_BASE];
    add(Role::EarOuter, ear_outer.to_vec());
    add(Role::EarOuter, mirrored(&ear_outer));
    let ear_inner = [TIP, INNER_MID, (150, 235), EAR_BASE];
    add(Role::EarInner, ear_inner.to_vec());
    add(Role::EarInner, mirrored(&ear_inner));
    // The cream V reaches up under the cheeks and the face.
    let half = [
        CHEEK_TIP,
        (CHEEK_IN.0, CHEEK_IN.1 - 16),
        (NOSE_L.0, NOSE_L.1 - 15),
    ];
    let mut muzzle: Vec<(i32, i32)> = half.to_vec();
    muzzle.push((256, 410));
    muzzle.extend(half.iter().rev().map(|&p| mir(p)));
    muzzle.push(CHIN);
    add(Role::Muzzle, muzzle);
    // Cheek facets (rust), reaching under the face.
    let cheek = [EAR_BASE, (137, 262), CHEEK_IN, CHEEK_TIP];
    add(Role::ShadeFace, cheek.to_vec());
    add(Role::ShadeFace, mirrored(&cheek));
    // The face, split down the middle (x = 256 is a pixel edge at every size).
    let face = [FORE_L, FORE_C, (256, 420), NOSE_L, CHEEK_IN, EAR_BASE];
    add(Role::LightFace, face.to_vec());
    add(Role::ShadeFace, mirrored(&face));
    add(Role::Eye, EYE_L.to_vec());
    add(Role::Eye, mirrored(&EYE_L));
    add(Role::Nose, alloc::vec![NOSE_L, NOSE_T, mir(NOSE_L), NOSE_B]);
    out
}

/// The head on its indigo tile: the app icon.
pub fn tile(s: &Scheme) -> Mark {
    Mark {
        tile: Some(s.tile),
        polys: fox_head(s),
    }
}

/// The single-colour silhouette of the head (ears, face, chin), with the eyes and the nose
/// knocked out. `color` is straight `0xRRGGBB`.
pub fn mono(color: u32) -> Mark {
    let right = |p: (i32, i32)| mir(p);
    let outline = alloc::vec![
        TIP,
        FORE_L,
        right(FORE_L),
        right(TIP),
        right(EAR_BASE),
        right(CHEEK_TIP),
        CHIN,
        CHEEK_TIP,
        EAR_BASE,
    ];
    let mut polys = alloc::vec![Poly {
        role: Role::Mark,
        color,
        pts: outline,
    }];
    // Knockouts wind the other way (counter-clockwise).
    let mut hole = |pts: Vec<(i32, i32)>| {
        polys.push(Poly {
            role: Role::Knockout,
            color,
            pts: pts.into_iter().rev().collect(),
        });
    };
    hole(EYE_L.to_vec());
    hole(mirrored(&EYE_L));
    hole(alloc::vec![NOSE_L, NOSE_T, mir(NOSE_L), NOSE_B]);
    Mark { tile: None, polys }
}

/// The neutral tile of an app that brings no icon of its own: a slate tile with the white
/// mono fox, so the fallback is recognisably "a Kitsune app".
pub fn fallback(_s: &Scheme) -> Mark {
    let mut m = mono(0xF2F3FA);
    m.tile = Some(0x4A5173);
    m
}
