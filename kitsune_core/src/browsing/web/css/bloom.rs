//! A small Bloom filter of the tags, ids and classes on the path from the root to an element, so a
//! selector with ancestors (`.a .b span`) is turned down without walking the path when one of the
//! ancestors it needs is certainly not there. False positives only cost the walk they replace.

use super::*;

/// 512 bits, two per key: a path of twenty elements with a few classes each stays well under a
/// third full.
pub(crate) type Bloom = [u64; 8];

/// The bits of one key (`kind` keeps tags, ids and classes apart).
fn bits(kind: u8, s: &str) -> [usize; 2] {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325 ^ u64::from(kind);
    for b in s.bytes() {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    [(h % 512) as usize, ((h >> 24) % 512) as usize]
}

fn set(f: &mut Bloom, kind: u8, s: &str) {
    for i in bits(kind, s) {
        f[i / 64] |= 1 << (i % 64);
    }
}

/// Add the keys of the element itself.
pub(crate) fn add_element(f: &mut Bloom, el: &Element) {
    set(f, 0, &el.tag);
    if let Some(id) = el.id() {
        set(f, 1, id);
    }
    for c in el.classes() {
        set(f, 2, c);
    }
}

/// Add the keys a compound needs from the element it is matched against.
pub(crate) fn add_compound(f: &mut Bloom, c: &Compound) {
    if let Some(t) = &c.tag
        && t != "*"
    {
        set(f, 0, t);
    }
    if let Some(id) = &c.id {
        set(f, 1, id);
    }
    for class in &c.classes {
        set(f, 2, class);
    }
}

/// Could a path that produced `path` hold everything in `need`?
pub(crate) fn may_hold(path: &Bloom, need: &Bloom) -> bool {
    path.iter().zip(need).all(|(p, n)| n & !p == 0)
}
