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

mod float;
mod grid;
mod line;
pub(crate) use float::*;
pub(crate) use grid::*;
pub(crate) use line::*;

/// Most boxes a flex or grid container lays out (the rest are dropped).
const MAX_ITEMS: usize = 400;
/// Most floats alive at once.
const MAX_FLOATS: usize = 64;
/// A float that is still beside the lines being laid out: its outer box (margins included).
#[derive(Clone, Copy)]
pub(crate) struct FloatBox {
    pub(crate) x0: i32,
    pub(crate) x1: i32,
    pub(crate) top: i32,
    pub(crate) bottom: i32,
    pub(crate) left: bool,
}
pub(crate) struct FItem<'a> {
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
    /// Grid: first row and column, rows and columns covered.
    row: usize,
    rspan: usize,
    col: usize,
    span: usize,
    /// The box and its content, once laid out.
    placed: Placed,
}
#[derive(Default, Clone, Copy)]
pub(crate) struct Placed {
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
                p.note_style(e, &ec);
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
        row: 0,
        rspan: 1,
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
        "img" | "input" | "button" | "select" | "textarea" | "video" | "audio"
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
                p.note_style(e, &ec);
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
