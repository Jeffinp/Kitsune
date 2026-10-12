//! flow (split out of `layout.rs`).

use super::*;

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

/// An inline element: special ones (`br`, `img`, controls), otherwise its
/// children join the inline run.
pub(super) fn inline_element<'a>(
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
    let y = flow(p, &el.children, c, c, area, y);
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

/// Register the form control `el` (an `<input>`, `<button>`, `<select>` or `<textarea>`) in the current
/// form and queue its box. Returns `true` when the element was consumed (a
/// control, or something to skip), `false` to let it render as ordinary content.
pub(super) fn collect_control(p: &mut Painter, el: &Element, c: &Computed) -> bool {
    let attr = |n: &str| el.attrs.get(n).map(String::as_str);
    let ty = attr("type").unwrap_or("").trim().to_ascii_lowercase();
    let name = decode_attr(attr("name").unwrap_or(""));
    let mut value = decode_attr(attr("value").unwrap_or(""));
    let mut options = Vec::new();
    let mut selected = 0;
    let mut rows = 0;
    let placeholder = fold_display(&decode_attr(attr("placeholder").unwrap_or("")));
    let checked = el.attrs.contains_key("checked");
    let (kind, label, chars) = if el.tag == "select" {
        selected = collect_options(&el.children, &mut options).unwrap_or(0);
        let widest = options
            .iter()
            .map(|o| o.label.chars().count())
            .max()
            .unwrap_or(0);
        (FieldKind::Select, String::new(), widest.clamp(6, 40))
    } else if el.tag == "textarea" {
        let n = |name: &str, default: usize, max: usize| {
            attr(name)
                .and_then(|s| s.trim().parse::<usize>().ok())
                .filter(|&n| n > 0)
                .unwrap_or(default)
                .min(max)
        };
        rows = n("rows", 2, 12);
        // The text between the tags is the initial value; one line break right after the
        // opening tag is not part of it.
        let t = text_content(&el.children);
        let t = t
            .strip_prefix("\r\n")
            .or_else(|| t.strip_prefix('\n'))
            .unwrap_or(&t);
        value = t.replace('\r', "");
        (FieldKind::TextArea, String::new(), n("cols", 20, 80).max(8))
    } else if el.tag == "button" {
        match ty.as_str() {
            "" | "submit" => {
                let t = fold_display(text_content(&el.children).trim());
                (
                    FieldKind::Submit,
                    if t.is_empty() { "Enviar".into() } else { t },
                    0,
                )
            }
            "button" | "reset" => {
                let t = fold_display(text_content(&el.children).trim());
                (FieldKind::PushButton, t, 0)
            }
            _ => return false,
        }
    } else {
        match ty.as_str() {
            "hidden" => (FieldKind::Hidden, String::new(), 0),
            "" | "text" | "search" | "url" | "email" | "tel" | "number" => {
                let n = attr("size")
                    .and_then(|s| s.trim().parse::<usize>().ok())
                    .filter(|&n| n > 0)
                    .unwrap_or(20)
                    .min(80);
                (FieldKind::Text, String::new(), n)
            }
            "password" => (FieldKind::Password, String::new(), 20),
            "submit" => {
                let l = if value.is_empty() {
                    "Enviar".into()
                } else {
                    fold_display(&value)
                };
                (FieldKind::Submit, l, 0)
            }
            "button" | "reset" => (FieldKind::PushButton, fold_display(&value), 0),
            "checkbox" => (FieldKind::Checkbox, String::new(), 0),
            "radio" => (FieldKind::Radio, String::new(), 0),
            // file, image, range, color, date, ...: not supported, not drawn.
            _ => return true,
        }
    };
    let Some(form) = p.cur_form else {
        return true; // a control outside any <form> has nowhere to go
    };
    let f = &mut p.forms[form];
    if f.fields.len() >= super::super::form::MAX_FIELDS {
        return true;
    }
    let field = f.fields.len();
    let value = if matches!(kind, FieldKind::Checkbox | FieldKind::Radio) && value.is_empty() {
        String::from("on")
    } else {
        value
    };
    f.fields.push(FieldInfo {
        name,
        kind,
        value,
        size: chars,
        label: label.clone(),
        checked,
        placeholder,
        options,
        selected,
        rows,
    });
    if kind != FieldKind::Hidden {
        let st = p.style(c);
        p.items.push(Item::Obj {
            obj: Obj::Field {
                form,
                field,
                kind,
                chars,
                rows,
                label,
            },
            st,
            sp: p.space.take(),
        });
    }
    true
}

/// The `<option>`s under a `<select>` (also inside `<optgroup>`; an option that was never
/// closed holds the ones after it, which the parser already ends). Returns the index of the
/// first one marked `selected`.
fn collect_options(
    nodes: &[crate::browsing::web::dom::Node],
    out: &mut Vec<crate::browsing::web::form::SelectOption>,
) -> Option<usize> {
    use crate::browsing::web::dom::Node;
    let mut chosen = None;
    for n in nodes {
        let Node::Element(e) = n else { continue };
        if out.len() >= crate::browsing::web::form::MAX_OPTIONS {
            break;
        }
        if e.tag == "option" {
            let label = fold_display(text_content(&e.children).trim());
            let value = match e.attrs.get("value") {
                Some(v) => decode_attr(v),
                None => label.clone(),
            };
            if e.attrs.contains_key("selected") && chosen.is_none() {
                chosen = Some(out.len());
            }
            out.push(crate::browsing::web::form::SelectOption { value, label });
        } else if let Some(i) = collect_options(&e.children, out) {
            chosen = chosen.or(Some(i));
        }
    }
    chosen
}

/// Resolve a margin or padding side against the container width.
pub(super) fn side(l: Len, base: i32, zoom: i32) -> i32 {
    l.resolve(base, zoom).unwrap_or(0)
}

/// Lay out a single block-level element: margins, border, padding, background and
/// content. Returns the y below its border box (its bottom margin waits in
/// `p.pending` to collapse with what follows).
pub(super) fn layout_block<'a>(
    p: &mut Painter<'a>,
    el: &'a Element,
    c: &Computed,
    area: Area,
    y: i32,
) -> i32 {
    layout_block_in(p, el, c, area, y, None)
}

/// [`layout_block`], optionally in a box of exactly `fixed = (x, border-box width)`
/// (a table cell: no margins, the width is the column's).
pub(super) fn layout_block_in<'a>(
    p: &mut Painter<'a>,
    el: &'a Element,
    c: &Computed,
    area: Area,
    mut y: i32,
    fixed: Option<(i32, i32)>,
) -> i32 {
    let zoom = p.zoom;
    // Past the floats `clear` asks to get below.
    if c.clear != Clear::None {
        let below = cleared(p, c.clear, adv(y, p.pending));
        if below > adv(y, p.pending) {
            y = below;
            p.pending = 0;
        }
    }
    // Boxes that make their own formatting context do not run under a float: they sit beside it.
    let beside = fixed.is_none()
        && (c.bfc
            || matches!(c.display, Disp::Table | Disp::Flex | Disp::Grid)
            || c.float != Float::None);
    let area = if beside && !p.floats.is_empty() {
        band(p, area, y, 1)
    } else {
        area
    };
    let floats_before = p.floats.len();
    let aw = area.w;
    let [mt_l, mr_l, mb_l, ml_l] = c.margin;
    let (mt, mb) = if fixed.is_some() {
        (0, 0)
    } else {
        (side(mt_l, aw, zoom), side(mb_l, aw, zoom))
    };
    let pad_t = side(c.padding[0], aw, zoom);
    let pad_r = side(c.padding[1], aw, zoom);
    let pad_b = side(c.padding[2], aw, zoom);
    let pad_l = side(c.padding[3], aw, zoom);
    let [bt, br, bb, bl] = c.border_w.map(|w| if w > 0 { p.z(w).max(1) } else { 0 });
    let extra = pad_l + pad_r + bl + br;

    // A table is as wide as its columns want to be (or its `width`); read it first.
    // Without a doctype a table does not inherit `text-align` (`<center><table>` keeps its cells left).
    let table_style;
    let mut table = if c.display == Disp::Table && fixed.is_none() {
        let tc = if p.quirks && c.align != Align::Left {
            let mut t = c.clone();
            t.align = Align::Left;
            table_style = t;
            &table_style
        } else {
            c
        };
        Some(table::collect(p, el, tc))
    } else {
        None
    };

    // ---- width and horizontal position ----
    let ml_fixed = if ml_l == Len::Auto {
        0
    } else {
        side(ml_l, aw, zoom)
    };
    let mr_fixed = if mr_l == Len::Auto {
        0
    } else {
        side(mr_l, aw, zoom)
    };
    let room = (aw - ml_fixed - mr_fixed).max(1);
    let mut total = room;
    if let Some(t) = &table
        && c.width == Len::Auto
    {
        total = t.totals().1.saturating_add(extra).min(room);
    }
    if let Some(w) = c.width.resolve(aw, zoom) {
        total = if c.border_box { w } else { w + extra };
    }
    if let Some(mw) = c.max_width.resolve(aw, zoom) {
        let cap = if c.border_box { mw } else { mw + extra };
        total = total.min(cap);
    }
    total = total.clamp(extra.max(1), room.max(extra).max(1));
    let leftover = (aw - ml_fixed - mr_fixed - total).max(0);
    let ml = match (ml_l == Len::Auto, mr_l == Len::Auto) {
        (true, true) => leftover / 2,
        (true, false) => leftover,
        _ => ml_fixed,
    };
    let (bx, bw) = fixed.unwrap_or((area.x + ml, total));
    let cx = bx + bl + pad_l;
    let cw = (bw - extra).max(1);

    // ---- top ----
    p.pending = p.pending.max(mt);
    let is_canvas = el.tag == "html" || el.tag == "body";
    if is_canvas && let Some(bg) = c.bg {
        // The page background belongs to the canvas, not to this box.
        if el.tag == "html" || p.canvas.is_none() {
            p.canvas = Some(bg);
        }
    }
    let paint_bg = c.bg.filter(|_| !is_canvas);
    let boxed_top = paint_bg.is_some() || bt > 0 || pad_t > 0;
    let boxed_bottom = paint_bg.is_some() || bb > 0 || pad_b > 0;
    if boxed_top {
        y = adv(y, core::mem::take(&mut p.pending));
    }
    let top = y;
    let bg_index = p.cmds.len();
    y = adv(y, bt + pad_t);

    // ---- list marker ----
    let mut own_marker = false;
    if c.display == Disp::ListItem {
        let n = match p.lists.last_mut() {
            Some(l) => {
                if let Some(v) = el
                    .attrs
                    .get("value")
                    .and_then(|v| v.trim().parse::<i32>().ok())
                {
                    l.next = v;
                }
                let n = l.next;
                l.next = l.next.saturating_add(1);
                Some((l.ordered, n))
            }
            None => None,
        };
        let (ordered, n) = n.unwrap_or((false, 0));
        let kind = if ordered
            || matches!(
                c.list,
                ListStyle::Decimal
                    | ListStyle::LowerAlpha
                    | ListStyle::UpperAlpha
                    | ListStyle::LowerRoman
                    | ListStyle::UpperRoman
            ) {
            MarkerKind::Number
        } else {
            MarkerKind::Bullet(c.list)
        };
        if c.list != ListStyle::None {
            p.marker = Some(Marker {
                kind,
                text: if kind == MarkerKind::Number {
                    let mut s = counter_text(n, c.list);
                    s.push('.');
                    s
                } else {
                    String::new()
                },
                color: c.color,
                right: cx - p.z(8),
            });
            own_marker = true;
        }
    }

    // ---- scopes opened by this element ----
    let saved_form = p.cur_form;
    if el.tag == "form" && p.forms.len() < super::super::form::MAX_FORMS {
        p.forms.push(FormInfo::from_attrs(
            el.attrs.get("method").map(String::as_str),
            el.attrs.get("action").map(|a| decode_attr(a)),
        ));
        p.cur_form = Some(p.forms.len() - 1);
    }
    let is_list = matches!(el.tag.as_str(), "ul" | "ol" | "menu" | "dir");
    if is_list {
        let start = el
            .attrs
            .get("start")
            .and_then(|v| v.trim().parse::<i32>().ok())
            .unwrap_or(1);
        if p.lists.len() < 32 {
            p.lists.push(ListCtx {
                ordered: el.tag == "ol",
                next: start,
            });
        }
    }

    // ---- content ----
    p.anc.push(el);
    let inner = Area { x: cx, w: cw };
    let content_top = y;
    if let Some(t) = &mut table {
        y = table::layout(p, t, c, inner, y, c.width != Len::Auto);
    } else if c.display == Disp::Flex {
        y = flex_layout(p, el, c, inner, y);
    } else if c.display == Disp::Grid {
        y = grid_layout(p, el, c, inner, y);
    } else {
        y = flow(p, &el.children, c, c, inner, y);
        y = flush_inline(p, c, inner, y, None);
    }
    p.anc.pop();
    // The box holds its floats when it makes its own context, is cleared by a `::after` rule, is a
    // float or a cell itself, or has nothing in the flow but floats (a row of columns).
    if p.floats.len() > floats_before {
        let holds = c.bfc
            || c.clearfix
            || c.float != Float::None
            || fixed.is_some()
            || matches!(
                c.display,
                Disp::Table | Disp::Flex | Disp::Grid | Disp::TableCell | Disp::InlineBlock
            )
            || y <= content_top;
        if holds {
            y = y.max(floats_bottom(p, floats_before));
            p.floats.truncate(floats_before);
        }
    }
    if is_list && !p.lists.is_empty() {
        p.lists.pop();
    }
    p.cur_form = saved_form;

    // A marker that never found a line (an empty item) sits on a line of its own.
    if own_marker && let Some(mk) = p.marker.take() {
        let f = p.font(c);
        let strut = strut_of(p, c);
        emit_marker(p, &mk, top + bt + pad_t + strut.0, f);
        y = adv(y, strut.0 + strut.1);
    }

    // ---- bottom ----
    if boxed_bottom {
        y = adv(y, core::mem::take(&mut p.pending));
        y = adv(y, pad_b + bb);
    }
    if c.min_height > 0 {
        y = y.max(top + p.z(c.min_height));
    }
    let height = (y - top).max(0);

    // Background behind the content, border on top of it.
    p.last_box = (None, None);
    if let Some(bg) = paint_bg {
        let rect = Cmd::Rect {
            x: bx,
            y: top,
            w: bw,
            h: height,
            color: bg,
            radius: p.z(c.radius).min(bw / 2).min(height / 2),
        };
        if bg_index <= p.cmds.len() && p.cmds.len() < MAX_CMDS {
            p.cmds.insert(bg_index, rect);
            p.last_box.0 = Some(bg_index);
        }
    }
    if bt + br + bb + bl > 0 {
        p.last_box.1 = Some(p.cmds.len());
        p.push_cmd(Cmd::Border {
            x: bx,
            y: top,
            w: bw,
            h: height,
            widths: [bt, br, bb, bl],
            radius: p.z(c.radius).min(bw / 2).min(height / 2),
            color: c.border_color,
        });
    }
    p.pending = p.pending.max(mb);
    y
}

/// The text of list counter `n`: `1`, `a`, `iv`.
pub(super) fn counter_text(n: i32, style: ListStyle) -> String {
    match style {
        ListStyle::LowerAlpha | ListStyle::UpperAlpha => {
            if n < 1 {
                return alloc::format!("{n}");
            }
            let mut s = String::new();
            let mut k = n - 1;
            loop {
                let d = (k % 26) as u8;
                s.insert(
                    0,
                    char::from(if style == ListStyle::LowerAlpha {
                        b'a' + d
                    } else {
                        b'A' + d
                    }),
                );
                k = k / 26 - 1;
                if k < 0 || s.len() > 8 {
                    break;
                }
            }
            s
        }
        ListStyle::LowerRoman | ListStyle::UpperRoman => {
            if !(1..4000).contains(&n) {
                return alloc::format!("{n}");
            }
            let table = [
                (1000, "m"),
                (900, "cm"),
                (500, "d"),
                (400, "cd"),
                (100, "c"),
                (90, "xc"),
                (50, "l"),
                (40, "xl"),
                (10, "x"),
                (9, "ix"),
                (5, "v"),
                (4, "iv"),
                (1, "i"),
            ];
            let mut s = String::new();
            let mut k = n;
            for (v, r) in table {
                while k >= v {
                    s.push_str(r);
                    k -= v;
                }
            }
            if style == ListStyle::UpperRoman {
                s.to_uppercase()
            } else {
                s
            }
        }
        _ => alloc::format!("{n}"),
    }
}
