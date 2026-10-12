//! flex (split out of `layout.rs`): flexbox rows and columns, grids, runs of `inline-block`
//! boxes and floats.
//!
//! All of them place whole boxes next to each other: the width of each box is worked out first
//! (from its `width`, `flex-basis` or measured content, see `table::content_intr`), then every box is
//! laid out at its final place by `layout_block_in` with a fixed border-box width, and the boxes of a
//! line are aligned vertically afterwards by moving what they emitted. Text that is a direct child of
//! a flex or grid container is not a box here: such a container is laid out as an ordinary block.

use super::table::{content_intr, shift_cmd};
use super::*;

/// Most boxes a flex or grid container lays out (the rest are dropped).
const MAX_ITEMS: usize = 400;
/// Most floats alive at once.
const MAX_FLOATS: usize = 64;

/// A float that is still beside the lines being laid out: its outer box (margins included).
#[derive(Clone, Copy)]
pub(super) struct FloatBox {
    pub(super) x0: i32,
    pub(super) x1: i32,
    pub(super) top: i32,
    pub(super) bottom: i32,
    pub(super) left: bool,
}

struct FItem<'a> {
    el: &'a Element,
    c: Computed,
    /// Margins in pixels (`auto` counts as 0 and is flagged).
    ml: i32,
    mr: i32,
    mt: i32,
    mb: i32,
    ml_auto: bool,
    mr_auto: bool,
    /// Hypothetical, minimum and maximum border-box width.
    base: i32,
    min: i32,
    max: i32,
    /// Final border-box width and left edge.
    w: i32,
    x: i32,
    grow: i32,
    shrink: i32,
    /// Grid: first column and columns covered.
    col: usize,
    span: usize,
    /// The box and its content, once laid out.
    placed: Placed,
}

#[derive(Default, Clone, Copy)]
struct Placed {
    cmds: (usize, usize),
    hits: (usize, usize),
    fields: (usize, usize),
    bg: Option<usize>,
    border: Option<usize>,
    /// Border-box height.
    h: i32,
}

fn is_blank(t: &str) -> bool {
    t.chars().all(|c| c.is_whitespace() && c != '\u{a0}')
}

/// The boxes of a flex or grid container: its element children that are shown. `None` when a direct
/// child is text (the container is then laid out as a block).
fn gather<'a>(p: &mut Painter<'a>, el: &'a Element, c: &Computed) -> Option<Vec<FItem<'a>>> {
    let mut out = Vec::new();
    for node in &el.children {
        match node {
            Node::Text(t) if is_blank(t) => {}
            Node::Text(_) => return None,
            Node::Element(e) => {
                let ec = compute(e, p.sheet, c, &p.anc, &mut p.budget);
                if ec.display == Disp::None {
                    continue;
                }
                if out.len() >= MAX_ITEMS {
                    break;
                }
                out.push(new_item(e, ec));
            }
        }
    }
    Some(out)
}

fn new_item<'a>(el: &'a Element, c: Computed) -> FItem<'a> {
    FItem {
        el,
        grow: c.flex.grow,
        shrink: c.flex.shrink,
        c,
        ml: 0,
        mr: 0,
        mt: 0,
        mb: 0,
        ml_auto: false,
        mr_auto: false,
        base: 0,
        min: 0,
        max: 0,
        w: 0,
        x: 0,
        col: 0,
        span: 1,
        placed: Placed::default(),
    }
}

/// Resolve margins, padding and the width bounds of every item against a container `aw` wide.
fn measure<'a>(p: &mut Painter<'a>, items: &mut [FItem<'a>], aw: i32) {
    let zoom = p.zoom;
    for it in items.iter_mut() {
        let c = &it.c;
        it.ml_auto = c.margin[3] == Len::Auto;
        it.mr_auto = c.margin[1] == Len::Auto;
        it.ml = side(c.margin[3], aw, zoom);
        it.mr = side(c.margin[1], aw, zoom);
        it.mt = side(c.margin[0], aw, zoom);
        it.mb = side(c.margin[2], aw, zoom);
        let extra = side(c.padding[1], aw, zoom)
            + side(c.padding[3], aw, zoom)
            + if c.border_w[1] > 0 {
                p.z(c.border_w[1]).max(1)
            } else {
                0
            }
            + if c.border_w[3] > 0 {
                p.z(c.border_w[3]).max(1)
            } else {
                0
            };
        // Content-based sizes.
        let (cmin, cmax) = content_intr(p, it.el, &it.c, 0);
        let cmin = (cmin.saturating_add(Q - 1) / Q).saturating_add(extra);
        let cmax = (cmax.saturating_add(Q - 1) / Q)
            .saturating_add(extra)
            .max(cmin);
        let to_border = |w: i32| if c.border_box { w } else { w + extra };
        let width_given = c.width.resolve(aw, zoom).map(to_border);
        let specified = match c.flex.basis {
            Len::Auto => width_given,
            b => b.resolve(aw, zoom).map(to_border),
        };
        let base = specified.unwrap_or(cmax).max(extra);
        let min = match c.min_width.resolve(aw, zoom) {
            Some(m) => to_border(m),
            // The automatic minimum: the narrowest content, but never more than the `width`.
            None => width_given.map_or(cmin, |s| cmin.min(s)),
        };
        let mut max = c.max_width.resolve(aw, zoom).map_or(i32::MAX, to_border);
        if max < min {
            max = min;
        }
        // The flex base size stays as given (a `flex-basis` of 0 is 0, whatever the content needs):
        // the free room is worked out from it, and the bounds clamp the result.
        it.base = base;
        it.min = min;
        it.max = max;
        it.w = base.clamp(min, max);
    }
}

/// Lay out one box at `(x, y)` with border-box width `w`; returns its border-box height.
fn place<'a>(
    p: &mut Painter<'a>,
    it: &mut FItem<'a>,
    parent: &Computed,
    area: Area,
    y: i32,
) -> i32 {
    let from = (p.cmds.len(), p.hits.len(), p.fields.len());
    p.pending = 0;
    p.last_box = (None, None);
    let end = if matches!(
        it.el.tag.as_str(),
        "img" | "input" | "button" | "select" | "textarea"
    ) {
        // Replaced content: it is a line of its own, as wide as the box.
        let a = Area { x: it.x, w: it.w };
        p.space = None;
        let mut ci = it.c.clone();
        ci.display = Disp::Inline;
        let mut e = inline_element(p, it.el, &ci, a, y);
        e = flush_inline(p, parent, a, e, None);
        e
    } else {
        layout_block_in(p, it.el, &it.c, area, y, Some((it.x, it.w)))
    };
    p.pending = 0;
    let h = (end - y).max(0);
    it.placed = Placed {
        cmds: (from.0, p.cmds.len()),
        hits: (from.1, p.hits.len()),
        fields: (from.2, p.fields.len()),
        bg: p.last_box.0,
        border: p.last_box.1,
        h,
    };
    h
}

/// Stretch the box of `it` to `h` (its background and border), or move its content down by `dy`.
fn align_item(p: &mut Painter, it: &FItem, line_h: i32, align: CrossAlign) {
    let outer = it.placed.h + it.mt + it.mb;
    let slack = (line_h - outer).max(0);
    match align {
        CrossAlign::Stretch => {
            let h = (line_h - it.mt - it.mb).max(it.placed.h);
            for i in [it.placed.bg, it.placed.border].into_iter().flatten() {
                match p.cmds.get_mut(i) {
                    Some(Cmd::Rect { h: hh, .. }) | Some(Cmd::Border { h: hh, .. }) => *hh = h,
                    _ => {}
                }
            }
        }
        CrossAlign::Start => {}
        CrossAlign::Center | CrossAlign::End => {
            let dy = if align == CrossAlign::Center {
                slack / 2
            } else {
                slack
            };
            if dy > 0 {
                let first = it.placed.cmds.0 + usize::from(it.placed.bg.is_some());
                let last = it
                    .placed
                    .border
                    .unwrap_or(it.placed.cmds.1)
                    .min(it.placed.cmds.1);
                for cmd in p.cmds.iter_mut().take(last).skip(first) {
                    shift_cmd(cmd, dy);
                }
                for h in p
                    .hits
                    .iter_mut()
                    .take(it.placed.hits.1)
                    .skip(it.placed.hits.0)
                {
                    h.y += dy;
                }
                for f in p
                    .fields
                    .iter_mut()
                    .take(it.placed.fields.1)
                    .skip(it.placed.fields.0)
                {
                    f.y += dy;
                }
            }
        }
    }
}

/// Distribute the room of one flex line to the boxes `a..b` (`grow` when there is free room,
/// `shrink` when there is not, never beyond the bounds) and return what is left over.
fn resolve_line(items: &mut [FItem], width: i32, gap: i32) -> i32 {
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
fn position_line(items: &mut [FItem], x: i32, leftover: i32, justify: Justify, gap: i32) {
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
pub(super) fn flex_layout<'a>(
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
fn lines_layout<'a>(
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
fn column_layout<'a>(
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

/// The content of a grid container: boxes placed row by row into the columns of
/// `grid-template-columns` (`fr`, lengths, `auto`, `repeat(auto-fill, minmax())`), all of a row as
/// tall as the tallest.
pub(super) fn grid_layout<'a>(
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
    let ncols = tracks.len();
    // Which column each box starts in, row by row.
    let (mut col, mut row) = (0usize, 0usize);
    let mut rows_of: Vec<usize> = Vec::with_capacity(items.len());
    for it in items.iter_mut() {
        let span = usize::from(it.c.flex.span).clamp(1, ncols);
        if col + span > ncols {
            col = 0;
            row += 1;
        }
        it.col = col;
        it.span = span;
        rows_of.push(row);
        col += span;
        if col >= ncols {
            col = 0;
            row += 1;
        }
    }
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
    // Rows.
    let mut idx = 0;
    let mut first = true;
    while idx < items.len() {
        let r = rows_of[idx];
        let mut end = idx;
        while end < items.len() && rows_of[end] == r {
            end += 1;
        }
        if !first {
            y = adv(y, gap_row);
        }
        first = false;
        let mut row_h = 0;
        for it in items[idx..end].iter_mut() {
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
            let h = place(p, it, c, inner, adv(y, it.mt));
            row_h = row_h.max(h + it.mt + it.mb);
        }
        for it in items[idx..end].iter() {
            let align = it.c.flex.align_self.unwrap_or(c.flex.align);
            align_item(p, it, row_h, align);
        }
        y = adv(y, row_h);
        idx = end;
    }
    y
}

/// The children of `el` laid out as ordinary block content.
fn block_content<'a>(
    p: &mut Painter<'a>,
    el: &'a Element,
    c: &Computed,
    inner: Area,
    y: i32,
) -> i32 {
    let y = flow(p, &el.children, c, c, inner, y);
    flush_inline(p, c, inner, y, None)
}

/// A run of `inline-block` siblings (and the blanks between them) starting at `nodes[start]`, laid
/// out as wrapping rows aligned by the container's `text-align`. Returns the y below it and the
/// number of nodes consumed.
pub(super) fn inline_run<'a>(
    p: &mut Painter<'a>,
    nodes: &'a [Node],
    start: usize,
    parent: &Computed,
    container: &Computed,
    area: Area,
    y: i32,
) -> (i32, usize) {
    let mut items: Vec<FItem<'a>> = Vec::new();
    let mut used = 0;
    for (k, node) in nodes[start..].iter().enumerate() {
        match node {
            Node::Text(t) if is_blank(t) => {}
            Node::Text(_) => break,
            Node::Element(e) => {
                let ec = compute(e, p.sheet, parent, &p.anc, &mut p.budget);
                if ec.display == Disp::None {
                    used = k + 1;
                    continue;
                }
                if ec.display != Disp::InlineBlock || items.len() >= MAX_ITEMS {
                    break;
                }
                items.push(new_item(e, ec));
                used = k + 1;
            }
        }
    }
    if items.is_empty() {
        return (y, used.max(1));
    }
    let y = flush_inline(p, container, area, y, None);
    measure(p, &mut items, area.w);
    let justify = match container.align {
        Align::Left => Justify::Start,
        Align::Center => Justify::Center,
        Align::Right => Justify::End,
    };
    let gap = p.z(4);
    let mut c2 = container.clone();
    c2.flex.align = CrossAlign::Start;
    let y = lines_layout(p, &mut items, &c2, area, y, gap, 0, justify, true);
    (y, used.max(1))
}

// ---- floats ----

/// The room for a line (or a box that avoids floats) in `[y, y + h)`: `area` narrowed by the
/// floats that overlap it.
pub(super) fn band(p: &Painter, area: Area, y: i32, h: i32) -> Area {
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
pub(super) fn cleared(p: &Painter, clear: Clear, y: i32) -> i32 {
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
pub(super) fn floats_bottom(p: &Painter, from: usize) -> i32 {
    p.floats[from.min(p.floats.len())..]
        .iter()
        .map(|f| f.bottom)
        .max()
        .unwrap_or(i32::MIN)
}

/// Put the floating element `el` (style `c`) at the left or right of `area`, at `y` or lower if it
/// does not fit beside the floats already there. It takes no room in the flow.
pub(super) fn place_float<'a>(
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
