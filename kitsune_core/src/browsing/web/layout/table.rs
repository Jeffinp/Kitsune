//! Tables: structure, column widths from measured content, row layout.
//!
//! The model is the common subset of CSS 2.1 auto layout: columns take the widest
//! unbreakable content as their minimum and the unwrapped content as their maximum;
//! the table is as wide as its maximum (or its `width`) when that fits and otherwise
//! shares the room between the columns in proportion to how much each can still
//! shrink. `colspan` works, `rowspan` is read as 1, borders collapse by dropping the
//! shared edge, cells align vertically (`valign`, default middle).

use super::*;
use crate::browsing::web::style::VAlign;

mod measure;
pub(crate) use measure::*;

/// Widest table, in columns (a hostile `colspan` cannot allocate more).
const MAX_COLS: usize = 32;
/// Most rows laid out per table.
const MAX_ROWS: usize = 1000;
/// Deepest recursion of the content measuring.
const MAX_MEASURE_DEPTH: u32 = 40;
pub(super) struct TCell<'a> {
    el: &'a Element,
    c: Computed,
    span: usize,
    /// Narrowest and widest the cell can be (border box, pixels).
    min: i32,
    max: i32,
    /// `width` as a percent of the table, if given as one.
    pct: Option<i32>,
}
pub(super) struct TRow<'a> {
    el: &'a Element,
    c: Computed,
    cells: Vec<TCell<'a>>,
}
pub(super) struct Table<'a> {
    rows: Vec<TRow<'a>>,
    caps: Vec<(&'a Element, Computed)>,
    ncols: usize,
    colmin: Vec<i32>,
    colmax: Vec<i32>,
    colpct: Vec<Option<i32>>,
    /// A cell of the column gave it a width in pixels: spare room goes to the other columns.
    colfix: Vec<bool>,
    spacing: i32,
    collapse: bool,
    /// The captions' narrowest and widest content (the table is at least as wide).
    cap_min: i32,
    cap_max: i32,
}
impl Table<'_> {
    /// Width of the narrowest and widest the whole table can be, borders of the cells
    /// and the spacing between them included (not the table's own border or padding).
    pub(super) fn totals(&self) -> (i32, i32) {
        let gaps = self.spacing * (self.ncols as i32 + 1);
        (
            (self.colmin.iter().sum::<i32>() + gaps).max(self.cap_min),
            (self.colmax.iter().sum::<i32>() + gaps).max(self.cap_max),
        )
    }
}
/// Read the table below `el` (style `c`): rows, cells, and every column's narrowest and
/// widest width. `p.anc` must end with the table's ancestors (not the table itself).
pub(super) fn collect<'a>(p: &mut Painter<'a>, el: &'a Element, c: &Computed) -> Table<'a> {
    let mut t = Table {
        rows: Vec::new(),
        caps: Vec::new(),
        ncols: 0,
        colmin: Vec::new(),
        colmax: Vec::new(),
        colpct: Vec::new(),
        colfix: Vec::new(),
        spacing: if c.collapse { 0 } else { p.z(c.spacing) },
        collapse: c.collapse,
        cap_min: 0,
        cap_max: 0,
    };
    p.anc.push(el);
    collect_level(p, &el.children, c, &mut t, 0);
    // A caption is as wide as the table, and the table is at least as wide as its caption.
    for i in 0..t.caps.len() {
        let (el, cc) = (t.caps[i].0, t.caps[i].1.clone());
        let mut acc = Intr::default();
        p.anc.push(el);
        intrinsic(p, &el.children, &cc, &mut acc, 0);
        p.anc.pop();
        acc.end_run();
        let ex = (side(cc.padding[1], 0, p.zoom) + side(cc.padding[3], 0, p.zoom)).max(0);
        t.cap_min = t
            .cap_min
            .max((acc.min.saturating_add(Q - 1) / Q).saturating_add(ex));
        t.cap_max = t
            .cap_max
            .max((acc.max.saturating_add(Q - 1) / Q).saturating_add(ex));
    }
    p.anc.pop();
    measure_columns(p, &mut t);
    t
}
fn collect_level<'a>(
    p: &mut Painter<'a>,
    nodes: &'a [Node],
    tc: &Computed,
    t: &mut Table<'a>,
    depth: u32,
) {
    if depth > 3 {
        return;
    }
    for n in nodes {
        let Node::Element(e) = n else { continue };
        let ec = compute(e, p.sheet, tc, &p.anc, &mut p.budget);
        p.note_style(e, &ec);
        match ec.display {
            Disp::TableRowGroup => {
                p.anc.push(e);
                collect_level(p, &e.children, &ec, t, depth + 1);
                p.anc.pop();
            }
            Disp::TableRow if t.rows.len() < MAX_ROWS => {
                p.anc.push(e);
                let mut cells = Vec::new();
                let mut width = 0;
                for cn in &e.children {
                    let Node::Element(ce) = cn else { continue };
                    let cc = compute(ce, p.sheet, &ec, &p.anc, &mut p.budget);
                    p.note_style(ce, &cc);
                    if cc.display != Disp::TableCell || width >= MAX_COLS {
                        continue;
                    }
                    let span = ce
                        .attrs
                        .get("colspan")
                        .and_then(|v| v.trim().parse::<usize>().ok())
                        .unwrap_or(1)
                        .clamp(1, MAX_COLS - width);
                    width += span;
                    cells.push(TCell {
                        el: ce,
                        c: cc,
                        span,
                        min: 0,
                        max: 0,
                        pct: None,
                    });
                }
                p.anc.pop();
                t.ncols = t.ncols.max(width);
                t.rows.push(TRow {
                    el: e,
                    c: ec,
                    cells,
                });
            }
            Disp::Block if e.tag == "caption" => t.caps.push((e, ec)),
            _ => {}
        }
    }
}
/// Fill in every cell's min/max and the column arrays.
fn measure_columns<'a>(p: &mut Painter<'a>, t: &mut Table<'a>) {
    let n = t.ncols;
    t.colmin = alloc::vec![0; n];
    t.colmax = alloc::vec![0; n];
    t.colpct = alloc::vec![None; n];
    t.colfix = alloc::vec![false; n];
    let zoom = p.zoom;
    // Measure rows in place (the cells borrow the DOM, not `t`).
    for ri in 0..t.rows.len() {
        let mut col = 0;
        p.anc.push(t.rows[ri].el);
        for ci in 0..t.rows[ri].cells.len() {
            let (el, cc, span) = {
                let cell = &t.rows[ri].cells[ci];
                (cell.el, cell.c.clone(), cell.span)
            };
            let mut acc = Intr::default();
            p.anc.push(el);
            intrinsic(p, &el.children, &cc, &mut acc, 0);
            p.anc.pop();
            acc.end_run();
            let extra = (side(cc.padding[1], 0, zoom)
                + side(cc.padding[3], 0, zoom)
                + cc.border_w[1].min(1) * 2)
                .max(0);
            let mut min = (acc.min.saturating_add(Q - 1) / Q).saturating_add(extra);
            let mut max = (acc.max.saturating_add(Q - 1) / Q)
                .saturating_add(extra)
                .max(min);
            let mut pct = None;
            let mut fixed = false;
            match cc.width {
                Len::Px(w) => {
                    fixed = true;
                    let w = (p.z(w) + extra).max(min);
                    max = w;
                    min = min.min(w);
                }
                Len::Pct(v) => pct = Some(v.clamp(1, 100)),
                // `calc()` widths: sized by content, like `auto`.
                Len::Calc { .. } | Len::Auto => {}
            }
            let cell = &mut t.rows[ri].cells[ci];
            cell.min = min;
            cell.max = max;
            cell.pct = pct;
            if span == 1 {
                t.colmin[col] = t.colmin[col].max(min);
                t.colmax[col] = t.colmax[col].max(max);
                if let Some(v) = pct {
                    t.colpct[col] = Some(t.colpct[col].map_or(v, |o| o.max(v)));
                }
                t.colfix[col] |= fixed;
            }
            col += span;
        }
        p.anc.pop();
    }
    // Cells that span columns: share what the single columns cannot hold.
    for row in &t.rows {
        let mut col = 0;
        for cell in &row.cells {
            if cell.span > 1 {
                let range = col..(col + cell.span).min(n);
                let gaps = t.spacing * (cell.span as i32 - 1);
                for (need, arr) in [(cell.min, &mut t.colmin), (cell.max, &mut t.colmax)] {
                    let have: i32 = arr[range.clone()].iter().sum::<i32>() + gaps;
                    if need > have && !range.is_empty() {
                        let add = (need - have) / range.len() as i32 + 1;
                        for k in range.clone() {
                            arr[k] += add;
                        }
                    }
                }
            }
            col += cell.span;
        }
    }
    for k in 0..n {
        t.colmax[k] = t.colmax[k].max(t.colmin[k]);
    }
}
/// Column widths for a table whose columns have `avail` pixels (spacing excluded).
fn column_widths(t: &Table, avail: i32, fill: bool) -> Vec<i32> {
    let n = t.ncols;
    let avail = avail.max(n as i32);
    let (smin, smax): (i32, i32) = (t.colmin.iter().sum(), t.colmax.iter().sum());
    let mut w: Vec<i32> = if smax <= avail {
        t.colmax.clone()
    } else if smin >= avail {
        // Not even the minima fit: squeeze them in proportion (the table may not scroll).
        let scale = i64::from(avail);
        t.colmin
            .iter()
            .map(|&m| ((i64::from(m) * scale / i64::from(smin.max(1))) as i32).max(4))
            .collect()
    } else {
        let room = i64::from(avail - smin);
        let span = i64::from((smax - smin).max(1));
        t.colmin
            .iter()
            .zip(&t.colmax)
            .map(|(&lo, &hi)| lo + (i64::from(hi - lo) * room / span) as i32)
            .collect()
    };
    // Percent columns want their share.
    for (k, pct) in t.colpct.iter().enumerate() {
        if let Some(v) = pct {
            let want = (i64::from(avail) * i64::from(*v) / 100) as i32;
            if want > w[k] {
                w[k] = want.min(avail);
            }
        }
    }
    let total: i32 = w.iter().sum();
    if total < avail && fill && n > 0 {
        // A table that is meant to fill its width hands the extra to the columns in
        // proportion to their widths.
        let extra = i64::from(avail - total);
        // Columns with a pixel width keep it as long as another column can take the room.
        let all_fixed = t.colfix.iter().all(|f| *f);
        let share = |k: usize, x: i32| if all_fixed || !t.colfix[k] { x } else { 0 };
        let base = i64::from(
            w.iter()
                .enumerate()
                .map(|(k, x)| share(k, *x))
                .sum::<i32>()
                .max(1),
        );
        let mut given = 0;
        let mut last_open = 0;
        for (k, x) in w.iter_mut().enumerate() {
            let sh = share(k, *x);
            if sh > 0 {
                last_open = k;
            }
            let add = (i64::from(sh) * extra / base) as i32;
            *x += add;
            given += add;
        }
        let rest = avail - total - given;
        if let Some(l) = w.get_mut(last_open) {
            *l += rest.max(0);
        }
    } else if total > avail {
        // Pull the widest column back until it fits.
        let mut over = total - avail;
        while over > 0 {
            let Some((k, _)) = w.iter().enumerate().max_by_key(|(_, v)| **v) else {
                break;
            };
            let cut = over.min((w[k] - 4).max(0));
            if cut == 0 {
                break;
            }
            w[k] -= cut;
            over -= cut;
        }
    }
    w
}
/// Lay out the rows of `t` in `inner` (the table's content box) starting at `y`; returns
/// the y below the last row.
pub(super) fn layout<'a>(
    p: &mut Painter<'a>,
    t: &mut Table<'a>,
    c: &Computed,
    inner: Area,
    mut y: i32,
    fill: bool,
) -> i32 {
    let n = t.ncols;
    let sp = t.spacing;
    // Captions sit above the rows, as wide as the table.
    for (el, cc) in core::mem::take(&mut t.caps) {
        y = layout_block(p, el, &cc, inner, y);
    }
    if n == 0 {
        return y;
    }
    p.pending = 0;
    let avail = inner.w - sp * (n as i32 + 1);
    // A caption wider than the columns stretches them.
    let fill = fill || t.cap_max > t.colmax.iter().sum::<i32>() + sp * (n as i32 + 1);
    let widths = column_widths(t, avail, fill);
    let mut colx = Vec::with_capacity(n);
    let mut x = inner.x + sp;
    for w in &widths {
        colx.push(x);
        x += w + sp;
    }
    y = adv(y, sp);
    let rows = core::mem::take(&mut t.rows);
    let mut above_bottom = 0;
    for (ri, row) in rows.iter().enumerate() {
        let row_top = y;
        let row_bg_index = p.cmds.len();
        p.anc.push(row.el);
        // Every cell laid out at the row's top, remembering what it emitted.
        struct Placed {
            cmds_from: usize,
            cmds_to: usize,
            hits_from: usize,
            hits_to: usize,
            fields_from: usize,
            fields_to: usize,
            bg: Option<usize>,
            border: Option<usize>,
            end: i32,
            valign: VAlign,
        }
        let mut placed: Vec<Placed> = Vec::with_capacity(row.cells.len());
        let mut col = 0;
        let mut prev_right = 0;
        let mut row_bottom = 0;
        for cell in &row.cells {
            let span = cell.span.min(n - col);
            let x0 = colx[col];
            let w: i32 = widths[col..col + span].iter().sum::<i32>() + sp * (span as i32 - 1);
            let mut cc = cell.c.clone();
            if t.collapse {
                // Neighbouring cells share one edge: the one already drawn by the cell to
                // the left or the row above stays, unless this cell asks for a thicker one.
                if col > 0 && prev_right >= cc.border_w[3] {
                    cc.border_w[3] = 0;
                }
                if ri > 0 && above_bottom >= cc.border_w[0] {
                    cc.border_w[0] = 0;
                }
            }
            prev_right = cc.border_w[1];
            row_bottom = row_bottom.max(cc.border_w[2]);
            cc.margin = [Len::Px(0); 4];
            let from = (p.cmds.len(), p.hits.len(), p.fields.len());
            p.pending = 0;
            p.last_box = (None, None);
            let end = layout_block_in(p, cell.el, &cc, Area { x: x0, w }, row_top, Some((x0, w)));
            p.pending = 0;
            placed.push(Placed {
                cmds_from: from.0,
                cmds_to: p.cmds.len(),
                hits_from: from.1,
                hits_to: p.hits.len(),
                fields_from: from.2,
                fields_to: p.fields.len(),
                bg: p.last_box.0,
                border: p.last_box.1,
                end,
                valign: cc.valign,
            });
            col += span;
            if col >= n {
                break;
            }
        }
        p.anc.pop();
        above_bottom = row_bottom;
        let row_h = placed
            .iter()
            .map(|pl| pl.end - row_top)
            .max()
            .unwrap_or(0)
            .max(if row.c.min_height > 0 {
                p.z(row.c.min_height)
            } else {
                0
            });
        for pl in &placed {
            // The cell's own box stretches to the row.
            for i in [pl.bg, pl.border].into_iter().flatten() {
                match p.cmds.get_mut(i) {
                    Some(Cmd::Rect { h, .. }) | Some(Cmd::Border { h, .. }) => *h = row_h,
                    _ => {}
                }
            }
            // Content that is shorter than the row moves down for middle and bottom.
            let slack = row_h - (pl.end - row_top);
            let dy = match pl.valign {
                VAlign::Top => 0,
                VAlign::Middle => slack / 2,
                VAlign::Bottom => slack,
            };
            if dy > 0 {
                let first = pl.cmds_from + usize::from(pl.bg.is_some());
                let last = pl.border.unwrap_or(pl.cmds_to).min(pl.cmds_to);
                for cmd in p.cmds.iter_mut().take(last).skip(first) {
                    shift_cmd(cmd, dy);
                }
                for h in p.hits.iter_mut().take(pl.hits_to).skip(pl.hits_from) {
                    h.y += dy;
                }
                for f in p.fields.iter_mut().take(pl.fields_to).skip(pl.fields_from) {
                    f.y += dy;
                }
            }
        }
        // A row's own background shows through its cells' gaps and behind them.
        if let Some(bg) = row.c.bg {
            let r = Cmd::Rect {
                x: inner.x + sp,
                y: row_top,
                w: widths.iter().sum::<i32>() + sp * (n as i32 - 1),
                h: row_h,
                color: bg,
                radius: 0,
            };
            if row_bg_index <= p.cmds.len() && p.cmds.len() < MAX_CMDS {
                p.cmds.insert(row_bg_index, r);
            }
        }
        y = adv(adv(row_top, row_h), sp);
    }
    let _ = c;
    y
}
pub(super) fn shift_cmd(cmd: &mut Cmd, dy: i32) {
    match cmd {
        Cmd::Rect { y, .. }
        | Cmd::Border { y, .. }
        | Cmd::Text { y, .. }
        | Cmd::Image { y, .. }
        | Cmd::Video { y, .. } => {
            *y = y.saturating_add(dy);
        }
    }
}
