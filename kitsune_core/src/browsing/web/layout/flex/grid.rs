//! `display: grid`: columns, named areas, spans and rows.

use super::*;

/// The content of a grid container: boxes placed row by row into the columns of
/// `grid-template-columns` (`fr`, lengths, `auto`, `repeat(auto-fill, minmax())`), all of a row as
/// tall as the tallest.
pub(crate) fn grid_layout<'a>(
    p: &mut Painter<'a>,
    el: &'a Element,
    c: &Computed,
    inner: Area,
    mut y: i32,
) -> i32 {
    let Some(mut items) = gather(p, el, c) else {
        return block_content(p, el, c, inner, y);
    };
    if items.is_empty() {
        return y;
    }
    measure(p, &mut items, inner.w);
    items.sort_by_key(|it| it.c.flex.order);
    let (gap_row, gap_col) = (p.z(c.flex.gap_row), p.z(c.flex.gap_col));
    let spec = c.flex.tracks.clone();
    // The tracks.
    let tracks: Vec<Track> = match (spec.auto_min, spec.cols.is_empty()) {
        (Some(min), true) => {
            let min = p.z(min).max(1);
            let n = ((inner.w + gap_col) / (min + gap_col)).clamp(1, 12) as usize;
            alloc::vec![Track::Fr(1000); n]
        }
        (_, true) => alloc::vec![Track::Fr(1000)],
        _ => spec.cols.clone(),
    };
    // The areas of `grid-template-areas`: the cells each name covers, `(row, column, rows, columns)`.
    let area_cols = spec.areas.iter().map(Vec::len).max().unwrap_or(0);
    let ncols = tracks.len().max(area_cols);
    let mut tracks = tracks;
    tracks.resize(ncols, Track::Fr(1000));
    let find_area = |name: &str| -> Option<(usize, usize, usize, usize)> {
        let (mut r0, mut r1, mut c0, mut c1) = (usize::MAX, 0, usize::MAX, 0);
        for (r, row) in spec.areas.iter().enumerate() {
            for (k, cell) in row.iter().enumerate() {
                if cell == name {
                    r0 = r0.min(r);
                    r1 = r1.max(r);
                    c0 = c0.min(k);
                    c1 = c1.max(k);
                }
            }
        }
        (r0 != usize::MAX).then_some((r0, c0, r1 - r0 + 1, c1 - c0 + 1))
    };
    // Place the boxes: named areas where they are, the rest row by row in the free cells.
    let mut taken: Vec<Vec<bool>> = Vec::new();
    let mark = |taken: &mut Vec<Vec<bool>>, r: usize, c: usize, rs: usize, cs: usize| {
        while taken.len() < r + rs {
            taken.push(alloc::vec![false; ncols]);
        }
        for row in taken.iter_mut().skip(r).take(rs) {
            for cell in row.iter_mut().skip(c).take(cs) {
                *cell = true;
            }
        }
    };
    let mut auto: Vec<usize> = Vec::new();
    for (i, it) in items.iter_mut().enumerate() {
        let named = it.c.flex.area.as_deref().and_then(find_area);
        if let Some((r, c0, rs, cs)) = named {
            it.row = r;
            it.col = c0;
            it.rspan = rs;
            it.span = cs;
            mark(&mut taken, r, c0, rs, cs);
        } else {
            auto.push(i);
        }
    }
    let (mut col, mut row) = (0usize, 0usize);
    for i in auto {
        let span = usize::from(items[i].c.flex.span).clamp(1, ncols);
        loop {
            if col + span > ncols {
                col = 0;
                row += 1;
            }
            let free = (col..col + span).all(|k| !taken.get(row).is_some_and(|r| r[k]));
            if free {
                break;
            }
            col += 1;
        }
        items[i].row = row;
        items[i].col = col;
        items[i].rspan = 1;
        items[i].span = span;
        mark(&mut taken, row, col, 1, span);
        col += span;
    }
    let nrows = items.iter().map(|it| it.row + it.rspan).max().unwrap_or(0);
    // Track widths: lengths first, `auto` from the widest single-column box, `fr` share the rest.
    let gaps = gap_col * (ncols as i32 - 1);
    let mut widths = alloc::vec![0i32; ncols];
    let mut fr_total = 0i64;
    for (k, t) in tracks.iter().enumerate() {
        match *t {
            Track::Px(v) => widths[k] = p.z(v),
            Track::Pct(v) => widths[k] = (i64::from(inner.w) * i64::from(v) / 100) as i32,
            Track::Auto => {
                widths[k] = items
                    .iter()
                    .filter(|it| it.col == k && it.span == 1)
                    .map(|it| it.base.clamp(it.min, it.max.max(it.min)) + it.ml + it.mr)
                    .max()
                    .unwrap_or(0)
                    .min(inner.w / 2 + 1);
            }
            Track::Fr(f) => fr_total += i64::from(f.max(1)),
        }
    }
    let fixed: i32 = widths.iter().sum::<i32>() + gaps;
    let rest = (inner.w - fixed).max(0);
    for (k, t) in tracks.iter().enumerate() {
        if let Track::Fr(f) = *t {
            widths[k] = (i64::from(rest) * i64::from(f.max(1)) / fr_total.max(1)) as i32;
        }
    }
    let mut colx = Vec::with_capacity(ncols);
    let mut x = inner.x;
    for w in &widths {
        colx.push(x);
        x += w + gap_col;
    }
    // Rows, top to bottom. A box that spans rows is laid out at its first one; its last row is made
    // tall enough to hold it.
    let mut row_top = alloc::vec![y; nrows + 1];
    let mut any_before = false;
    for r in 0..nrows {
        let starting: Vec<usize> = (0..items.len()).filter(|&i| items[i].row == r).collect();
        let ending: Vec<usize> = (0..items.len())
            .filter(|&i| items[i].row + items[i].rspan - 1 == r)
            .collect();
        if starting.is_empty() && ending.is_empty() {
            row_top[r + 1] = row_top[r];
            continue;
        }
        if any_before {
            row_top[r] = adv(row_top[r], gap_row);
        }
        any_before = true;
        for &i in &starting {
            let it = &mut items[i];
            let last = (it.col + it.span).min(ncols);
            let span_w: i32 = widths[it.col..last].iter().sum::<i32>()
                + gap_col * (last - it.col) as i32
                - gap_col;
            it.w = (span_w - it.ml - it.mr).max(1);
            it.x = colx[it.col] + it.ml;
            // A box with its own width is not stretched to the cell.
            if it.c.width != Len::Auto
                && it.c.flex.align_self.unwrap_or(c.flex.align) != CrossAlign::Stretch
            {
                it.w = it.base.clamp(it.min, it.max.max(it.min)).min(it.w);
            }
            let top = adv(row_top[r], it.mt);
            place(p, it, c, inner, top);
        }
        // The height of the row: the boxes that end in it, measured from where they began.
        let mut row_h = 0;
        for &i in &ending {
            let it = &items[i];
            let need = it.placed.h + it.mt + it.mb - (row_top[r] - row_top[it.row]);
            row_h = row_h.max(need);
        }
        for &i in &ending {
            let it = &items[i];
            let total = row_h + (row_top[r] - row_top[it.row]);
            let align = it.c.flex.align_self.unwrap_or(c.flex.align);
            align_item(p, it, total, align);
        }
        row_top[r + 1] = adv(row_top[r], row_h);
    }
    y = row_top[nrows];
    y
}
