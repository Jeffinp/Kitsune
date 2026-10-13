//! flow (split out of `layout.rs`).

use super::*;

mod block;
mod controls;
pub(crate) use block::*;
pub(crate) use controls::*;

/// Lay out a list of sibling nodes in the formatting context of `container`.
/// Inline content is buffered in the painter; a block child first flushes it.
/// `parent` is the style the text of these nodes inherits (an inline element's
/// own style when recursing through inline elements), `container` the nearest
/// block (its strut and alignment shape the lines). Returns the y after the
/// last line or block emitted (buffered inline content is still waiting).
pub(super) fn flow<'a>(
    p: &mut Painter<'a>,
    nodes: &'a [Node],
    parent: &Computed,
    container: &Computed,
    area: Area,
    mut y: i32,
) -> i32 {
    let mut i = 0;
    while i < nodes.len() {
        let node = &nodes[i];
        i += 1;
        match node {
            Node::Text(t) => push_text(p, t, parent),
            Node::Element(el) => {
                if el.ws_before {
                    p.space = Some(p.font(parent));
                }
                let c = compute(el, p.sheet, parent, &p.anc, &mut p.budget);
                if c.float != Float::None && c.display != Disp::None && c.position != Pos::Abs {
                    // A float leaves the flow: the line being built ends, the box goes to the side.
                    y = flush_inline(p, container, area, y, None);
                    // The margin waiting above it moves the float down too, and stays for what follows.
                    let top = adv(y, p.pending);
                    place_float(p, el, &c, area, top);
                    continue;
                }
                match c.display {
                    Disp::None => {}
                    Disp::Inline => {
                        y = inline_element(p, el, &c, area, y);
                    }
                    // A row of inline-blocks is laid out as boxes; one among text is just inline.
                    Disp::InlineBlock if only_boxes(p, nodes, parent) => {
                        let (ny, used) = inline_run(p, nodes, i - 1, parent, container, area, y);
                        y = ny;
                        i = i - 1 + used;
                    }
                    Disp::InlineBlock => {
                        let mut ci = c.clone();
                        ci.display = Disp::Inline;
                        y = inline_element(p, el, &ci, area, y);
                    }
                    _ if el.tag == "img" => {
                        // A block-level picture sits on a line of its own; auto side margins
                        // centre it.
                        y = flush_inline(p, container, area, y, None);
                        p.pending = p.pending.max(side(c.margin[0], area.w, p.zoom));
                        image_element(p, el, &c);
                        let centred = c.margin[1] == Len::Auto && c.margin[3] == Len::Auto;
                        y = flush_inline(p, container, area, y, centred.then_some(Align::Center));
                        p.pending = p.pending.max(side(c.margin[2], area.w, p.zoom));
                    }
                    _ => {
                        y = flush_inline(p, container, area, y, None);
                        y = layout_block(p, el, &c, area, y);
                    }
                }
            }
        }
    }
    y
}
/// Is every child in `nodes` either blank text or an `inline-block` (or hidden) element? Then they
/// are a row of boxes (a menu, a button bar); with any other text or inline content among them they
/// stay in the line.
fn only_boxes<'a>(p: &mut Painter<'a>, nodes: &'a [Node], parent: &Computed) -> bool {
    if nodes.len() > 200 {
        return false;
    }
    nodes.iter().all(|n| match n {
        Node::Text(t) => t.chars().all(|c| c.is_whitespace() && c != '\u{a0}'),
        Node::Element(e) => {
            let c = compute(e, p.sheet, parent, &p.anc, &mut p.budget);
            matches!(c.display, Disp::InlineBlock | Disp::None)
        }
    })
}
/// The generated box of `el`: its `::before` (`kind` 1) or `::after` (2). `p.anc` ends with `el`.
/// Text joins the inline content with the box's own style; a block-level box with `content: ""`, a
/// height and a background is drawn as a bar. Returns the new y.
fn pseudo_box<'a>(
    p: &mut Painter<'a>,
    el: &'a Element,
    c: &Computed,
    area: Area,
    mut y: i32,
    kind: u8,
) -> i32 {
    if !p.sheet.has_pseudo {
        return y;
    }
    let n = p.anc.len().saturating_sub(1);
    let anc: Vec<&Element> = p.anc[..n].to_vec();
    let Some(pc) = compute_pseudo(el, p.sheet, c, &anc, kind, &mut p.budget) else {
        return y;
    };
    let text = pc.content.clone().unwrap_or_default();
    let block = !matches!(pc.display, Disp::Inline | Disp::InlineBlock);
    if block {
        y = flush_inline(p, c, area, y, None);
    }
    if text.trim().is_empty() {
        if block && pc.min_height > 0 {
            y = adv(y, core::mem::take(&mut p.pending));
            if let Some(bg) = pc.bg {
                let w = pc
                    .width
                    .resolve(area.w, p.zoom)
                    .unwrap_or(area.w)
                    .clamp(1, area.w);
                let h = p.z(pc.min_height);
                p.push_cmd(Cmd::Rect {
                    x: area.x,
                    y,
                    w,
                    h,
                    color: bg,
                    radius: p.z(pc.radius),
                });
            }
            y = adv(y, p.z(pc.min_height));
        }
        return y;
    }
    // Inline-block and inline boxes: the text takes the box's style (colour, weight, size).
    p.space = None;
    push_text(p, &text, &pc);
    if block {
        y = flush_inline(p, c, area, y, None);
    }
    y
}
/// Does the `::after` of `el` clear floats (a "clearfix")? `p.anc` holds the ancestors of `el`.
fn pseudo_clears<'a>(p: &mut Painter<'a>, el: &'a Element, c: &Computed) -> bool {
    if !p.sheet.has_pseudo {
        return false;
    }
    let anc: Vec<&Element> = p.anc.to_vec();
    let (_, au) =
        crate::browsing::web::style::matched_pseudo_decls(el, &anc, p.sheet, &mut p.budget, 2);
    let _ = c;
    au.iter()
        .any(|d| d.name == "clear" && !d.value.trim().eq_ignore_ascii_case("none"))
}
/// An inline element: special ones (`br`, `img`, controls), otherwise its
/// children join the inline run. What it lays out belongs to it (`p.node`).
pub(super) fn inline_element<'a>(
    p: &mut Painter<'a>,
    el: &'a Element,
    c: &Computed,
    area: Area,
    y: i32,
) -> i32 {
    let outer = core::mem::replace(&mut p.node, el.uid);
    let y = inline_element_in(p, el, c, area, y);
    p.node = outer;
    y
}

fn inline_element_in<'a>(
    p: &mut Painter<'a>,
    el: &'a Element,
    c: &Computed,
    area: Area,
    y: i32,
) -> i32 {
    match el.tag.as_str() {
        "br" => {
            let st = p.style(c);
            p.items.push(Item::Br(st));
            p.space = None;
            return y;
        }
        "wbr" => return y,
        "img" => {
            image_element(p, el, c);
            return y;
        }
        "input" | "button" | "select" | "textarea" => {
            if collect_control(p, el, c) {
                return y;
            }
        }
        _ => {}
    }
    let saved_link = p.link;
    if el.tag == "a"
        && let Some(href) = el.attrs.get("href")
        && p.links.len() < MAX_LINKS
    {
        p.link = Some(p.links.len());
        p.links.push(decode_attr(href));
    }
    // Horizontal padding of an inline box: a gap its background shows through.
    let (pl, pr) = (side(c.padding[3], 0, p.zoom), side(c.padding[1], 0, p.zoom));
    let st = p.style(c);
    if pl > 0 {
        // The space before the box goes before its padding, not inside it.
        let sp = p.space.take();
        p.items.push(Item::Gap(pl, st, sp));
    }
    p.anc.push(el);
    let y = pseudo_box(p, el, c, area, y, 1);
    let y = flow(p, &el.children, c, c, area, y);
    let y = pseudo_box(p, el, c, area, y, 2);
    p.anc.pop();
    if pr > 0 {
        p.items.push(Item::Gap(pr, st, None));
    }
    p.link = saved_link;
    y
}
pub(super) fn image_element(p: &mut Painter, el: &Element, c: &Computed) {
    if p.images.len() >= MAX_IMAGES {
        return;
    }
    let src = decode_attr(el.attrs.get("src").map(String::as_str).unwrap_or(""));
    let alt = fold_display(&decode_attr(
        el.attrs.get("alt").map(String::as_str).unwrap_or(""),
    ));
    let state = if src.trim().is_empty() {
        ImgState::Failed
    } else {
        p.lookup.lookup(&src)
    };
    let idx = p.images.len();
    p.images.push(ImgRef {
        src,
        alt: alt.clone(),
    });
    // The ad blocker left it out: no box, no gap (the entry stays so indices agree).
    if state == ImgState::Blocked {
        return;
    }
    let (dw, dh) = (p.z(attr_px(el, "width")), p.z(attr_px(el, "height")));
    // A CSS pixel width overrides the attribute.
    let dw = match c.width {
        Len::Px(w) => p.z(w),
        _ => dw,
    };
    let st = p.style(c);
    let sp = p.space.take();
    p.items.push(Item::Obj {
        obj: Obj::Img {
            idx,
            dw,
            dh,
            state,
            alt,
        },
        st,
        sp,
    });
}
