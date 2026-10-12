//! Flex rows and columns: sizing a line, placing it and laying the boxes out.

use super::*;

/// Distribute the room of one flex line to the boxes `a..b` (`grow` when there is free room,
/// `shrink` when there is not, never beyond the bounds) and return what is left over.
pub(crate) fn resolve_line(items: &mut [FItem], width: i32, gap: i32) -> i32 {
    let n = items.len() as i32;
    let gaps = gap * (n - 1).max(0);
    let outer = |it: &FItem, w: i32| w + it.ml + it.mr;
    for it in items.iter_mut() {
        it.w = it.base;
    }
    let used: i32 = items.iter().map(|it| outer(it, it.w)).sum::<i32>() + gaps;
    let mut free = width - used;
    if free > 0 {
        let mut open: Vec<usize> = (0..items.len()).filter(|&i| items[i].grow > 0).collect();
        for _ in 0..4 {
            let total: i64 = open.iter().map(|&i| i64::from(items[i].grow)).sum();
            if total == 0 || free <= 0 {
                break;
            }
            let mut spent = 0;
            let mut still = Vec::new();
            for &i in &open {
                let add = (i64::from(free) * i64::from(items[i].grow) / total) as i32;
                let w = items[i].w + add;
                let cl = w.min(items[i].max);
                spent += cl - items[i].w;
                items[i].w = cl;
                if w <= items[i].max {
                    still.push(i);
                }
            }
            free -= spent;
            open = still;
        }
    } else if free < 0 {
        let mut open: Vec<usize> = (0..items.len()).filter(|&i| items[i].shrink > 0).collect();
        for _ in 0..4 {
            let total: i64 = open
                .iter()
                .map(|&i| i64::from(items[i].shrink) * i64::from(items[i].w.max(1)))
                .sum();
            if total == 0 || free >= 0 {
                break;
            }
            let mut spent = 0;
            let mut still = Vec::new();
            for &i in &open {
                let share = i64::from(items[i].shrink) * i64::from(items[i].w.max(1));
                let cut = (i64::from(-free) * share / total) as i32;
                let w = items[i].w - cut;
                let cl = w.max(items[i].min);
                spent += items[i].w - cl;
                items[i].w = cl;
                if w >= items[i].min {
                    still.push(i);
                }
            }
            free += spent;
            open = still;
        }
    }
    for it in items.iter_mut() {
        it.w = it.w.clamp(it.min, it.max.max(it.min));
    }
    let used: i32 = items.iter().map(|it| outer(it, it.w)).sum::<i32>() + gaps;
    width - used
}

/// Place the boxes of a line left to right in `[x, x + width)`, spreading the leftover room by
/// `justify` (auto margins take it first).
pub(crate) fn position_line(
    items: &mut [FItem],
    x: i32,
    leftover: i32,
    justify: Justify,
    gap: i32,
) {
    let n = items.len() as i32;
    let mut left = leftover;
    if left > 0 {
        let autos: i32 = items
            .iter()
            .map(|it| i32::from(it.ml_auto) + i32::from(it.mr_auto))
            .sum();
        if autos > 0 {
            let each = left / autos;
            let mut rem = left - each * autos;
            for it in items.iter_mut() {
                for is_left in [true, false] {
                    let flag = if is_left { it.ml_auto } else { it.mr_auto };
                    if flag {
                        let mut v = each;
                        if rem > 0 {
                            v += 1;
                            rem -= 1;
                        }
                        if is_left {
                            it.ml += v;
                        } else {
                            it.mr += v;
                        }
                    }
                }
            }
            left = 0;
        }
    }
    let left = left.max(0);
    let (offset, between) = match justify {
        Justify::Start => (0, 0),
        Justify::Center => (left / 2, 0),
        Justify::End => (left, 0),
        Justify::Between if n > 1 => (0, left / (n - 1)),
        Justify::Between => (0, 0),
        Justify::Around => (left / (2 * n.max(1)), left / n.max(1)),
        Justify::Evenly => (left / (n + 1), left / (n + 1)),
    };
    let mut cx = x + offset;
    for it in items.iter_mut() {
        it.x = cx + it.ml;
        cx += it.ml + it.w + it.mr + gap + between;
    }
}

/// The content of a flex container: rows (wrapping or not) or a column of boxes. Returns the y
/// below it.
pub(crate) fn flex_layout<'a>(
    p: &mut Painter<'a>,
    el: &'a Element,
    c: &Computed,
    inner: Area,
    y: i32,
) -> i32 {
    let Some(mut items) = gather(p, el, c) else {
        return block_content(p, el, c, inner, y);
    };
    if items.is_empty() {
        return y;
    }
    measure(p, &mut items, inner.w);
    items.sort_by_key(|it| it.c.flex.order);
    if c.flex.reverse {
        items.reverse();
    }
    let (gap_row, gap_col) = (p.z(c.flex.gap_row), p.z(c.flex.gap_col));
    if !c.flex.row {
        return column_layout(p, &mut items, c, inner, y, gap_row);
    }
    let mut justify = c.flex.justify;
    if c.flex.reverse {
        justify = match justify {
            Justify::Start => Justify::End,
            Justify::End => Justify::Start,
            j => j,
        };
    }
    lines_layout(
        p,
        &mut items,
        c,
        inner,
        y,
        gap_col,
        gap_row,
        justify,
        c.flex.wrap,
    )
}

/// Rows of boxes: break into lines when `wrap`, size each line, place and align it.
#[allow(clippy::too_many_arguments)]
pub(crate) fn lines_layout<'a>(
    p: &mut Painter<'a>,
    items: &mut [FItem<'a>],
    c: &Computed,
    inner: Area,
    mut y: i32,
    gap_col: i32,
    gap_row: i32,
    justify: Justify,
    wrap: bool,
) -> i32 {
    let mut start = 0;
    let mut first_line = true;
    while start < items.len() {
        // The boxes of this line.
        let mut end = start;
        let mut w = 0;
        while end < items.len() {
            let it = &items[end];
            let hyp = it.base.clamp(it.min, it.max.max(it.min));
            let next = w + hyp + it.ml + it.mr + if end > start { gap_col } else { 0 };
            if wrap && end > start && next > inner.w {
                break;
            }
            w = next;
            end += 1;
        }
        let line = &mut items[start..end];
        let leftover = resolve_line(line, inner.w, gap_col);
        position_line(line, inner.x, leftover, justify, gap_col);
        if !first_line {
            y = adv(y, gap_row);
        }
        first_line = false;
        let mut line_h = 0;
        for it in line.iter_mut() {
            let h = place(p, it, c, inner, adv(y, it.mt));
            line_h = line_h.max(h + it.mt + it.mb);
        }
        for it in line.iter() {
            let align = it.c.flex.align_self.unwrap_or(c.flex.align);
            align_item(p, it, line_h, align);
        }
        y = adv(y, line_h);
        start = end;
    }
    y
}

/// A column of boxes: stacked, each as wide as the container (or as its content when the
/// alignment is not `stretch`).
pub(crate) fn column_layout<'a>(
    p: &mut Painter<'a>,
    items: &mut [FItem<'a>],
    c: &Computed,
    inner: Area,
    mut y: i32,
    gap: i32,
) -> i32 {
    let n = items.len();
    for (i, it) in items.iter_mut().enumerate() {
        let align = it.c.flex.align_self.unwrap_or(c.flex.align);
        let room = (inner.w - it.ml - it.mr).max(1);
        let explicit = it.c.width != Len::Auto || it.c.flex.basis != Len::Auto;
        let w = if align == CrossAlign::Stretch && !explicit {
            room
        } else {
            it.base.clamp(it.min, it.max.max(it.min)).min(room)
        }
        .clamp(it.min.min(room), it.max.max(1));
        it.w = w;
        let spare = (room - w).max(0);
        let off = match (it.ml_auto, it.mr_auto, align) {
            (true, true, _) => spare / 2,
            (true, false, _) => spare,
            (false, true, _) => 0,
            (_, _, CrossAlign::Center) => spare / 2,
            (_, _, CrossAlign::End) => spare,
            _ => 0,
        };
        it.x = inner.x + it.ml + off;
        let h = place(p, it, c, inner, adv(y, it.mt));
        y = adv(y, it.mt + h + it.mb);
        if i + 1 < n {
            y = adv(y, gap);
        }
    }
    y
}
