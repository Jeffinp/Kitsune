//! Floats: the room they leave beside the lines, `clear`, and placing a float.

use super::*;

/// The room for a line (or a box that avoids floats) in `[y, y + h)`: `area` narrowed by the
/// floats that overlap it.
pub(crate) fn band(p: &Painter, area: Area, y: i32, h: i32) -> Area {
    let (mut l, mut r) = (area.x, area.x + area.w);
    for f in &p.floats {
        if f.top < y + h.max(1) && f.bottom > y {
            if f.left {
                l = l.max(f.x1);
            } else {
                r = r.min(f.x0);
            }
        }
    }
    Area {
        x: l,
        w: (r - l).max(1),
    }
}

/// The y of the lowest bottom edge of the floats `clear` asks to get past (or `y`).
pub(crate) fn cleared(p: &Painter, clear: Clear, y: i32) -> i32 {
    let mut out = y;
    for f in &p.floats {
        let hit = match clear {
            Clear::None => false,
            Clear::Left => f.left,
            Clear::Right => !f.left,
            Clear::Both => true,
        };
        if hit {
            out = out.max(f.bottom);
        }
    }
    out
}

/// Lowest bottom edge of the floats from index `from` on.
pub(crate) fn floats_bottom(p: &Painter, from: usize) -> i32 {
    p.floats[from.min(p.floats.len())..]
        .iter()
        .map(|f| f.bottom)
        .max()
        .unwrap_or(i32::MIN)
}

/// Put the floating element `el` (style `c`) at the left or right of `area`, at `y` or lower if it
/// does not fit beside the floats already there. It takes no room in the flow.
pub(crate) fn place_float<'a>(
    p: &mut Painter<'a>,
    el: &'a Element,
    c: &Computed,
    area: Area,
    y: i32,
) {
    if p.floats.len() >= MAX_FLOATS {
        return;
    }
    let mut items = alloc::vec![new_item(el, c.clone())];
    measure(p, &mut items, area.w);
    let mut it = items.pop().unwrap_or_else(|| unreachable!());
    // Width: its own, or what its content wants, never wider than the room.
    let room = (area.w - it.ml - it.mr).max(1);
    it.w = it.base.clamp(it.min, it.max.max(it.min)).min(room).max(1);
    let outer = it.w + it.ml + it.mr;
    let left = c.float == Float::Left;
    let mut ty = y;
    // Move down until it fits between the floats already there.
    for _ in 0..MAX_FLOATS {
        let b = band(p, area, ty, 1);
        if b.w >= outer || b.w >= area.w {
            break;
        }
        // The next edge below: the nearest bottom of a float overlapping this band.
        let next = p
            .floats
            .iter()
            .filter(|f| f.top < ty + 1 && f.bottom > ty)
            .map(|f| f.bottom)
            .min();
        match next {
            Some(n) if n > ty => ty = n,
            _ => break,
        }
    }
    let b = band(p, area, ty, 1);
    let x = if left {
        b.x
    } else {
        (b.x + b.w - outer).max(b.x)
    };
    it.x = x + it.ml;
    let saved = (p.pending, p.space.take());
    let top = adv(ty, it.mt);
    let h = place(p, &mut it, c, area, top);
    p.pending = saved.0;
    p.space = saved.1;
    p.floats.push(FloatBox {
        x0: x,
        x1: x + outer,
        top: ty,
        bottom: adv(ty, it.mt + h + it.mb),
        left,
    });
}
