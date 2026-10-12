//! cascade (split out of `style.rs`).

use super::*;

/// Collect the declarations matching `el`, split into the user-agent sheet's and the
/// page's: normal ones lowest specificity first, then the `!important` ones, so later
/// declarations override earlier ones.
pub(super) fn matched_decls<'a>(
    el: &Element,
    anc: &[&Element],
    sheet: &'a Stylesheet,
    budget: &mut u32,
    pseudo: u8,
) -> (Vec<&'a Decl>, Vec<&'a Decl>) {
    let mut hit: Vec<(Specificity, &'a super::super::css::Rule)> = Vec::new();
    let mut test = |r: &'a super::super::css::Rule| {
        if r.pseudo != pseudo || !sheet.rule_applies(r) {
            return;
        }
        let mut best: Option<Specificity> = None;
        for s in &r.selectors {
            // The cheap test first: most selectors fail on the element itself.
            if s.matches(el, anc, budget) {
                let sp = s.specificity();
                if best.is_none_or(|b| sp > b) {
                    best = Some(sp);
                }
            }
        }
        if let Some(sp) = best {
            hit.push((sp, r));
        }
    };
    let mut ix: Vec<u32> = Vec::new();
    if sheet.candidates(el, &mut ix) {
        for &i in &ix {
            test(&sheet.rules[i as usize]);
        }
    } else {
        for r in &sheet.rules {
            test(r);
        }
    }
    hit.sort_by_key(|(sp, _)| *sp);
    let group = |author: bool| -> Vec<&'a Decl> {
        let mut out: Vec<&Decl> = Vec::new();
        for (_, r) in hit.iter().filter(|(_, r)| r.author == author) {
            out.extend(r.decls.iter().filter(|d| !d.important));
        }
        for (_, r) in hit.iter().filter(|(_, r)| r.author == author) {
            out.extend(r.decls.iter().filter(|d| d.important));
        }
        out
    };
    (group(false), group(true))
}

/// Resolve the computed style for `el` given its inherited parent style and its
/// ancestors (outermost first).
pub(crate) fn compute(
    el: &Element,
    sheet: &Stylesheet,
    parent: &Computed,
    anc: &[&Element],
    budget: &mut u32,
) -> Computed {
    let mut c = parent.clone();
    reset_non_inherited(&mut c, el);

    let (ua, mut au) = matched_decls(el, anc, sheet, budget, 0);
    let parent_px = parent.font_px;
    apply_decls(&mut c, &ua, parent_px);
    // Presentational attributes beat the user-agent sheet and lose to the page's.
    presentational(el, anc, &mut c);
    // Inline `style="..."` wins over everything from the stylesheets (except !important there).
    let inline: Vec<Decl> = el
        .attrs
        .get("style")
        .map(|s| parse_decls(s))
        .unwrap_or_default();
    au.extend(inline.iter().filter(|d| !d.important));
    au.extend(inline.iter().filter(|d| d.important));
    gather_vars(&mut c, &au);
    apply_decls(&mut c, &au, parent_px);
    // What this element's children are drawn over.
    if let Some(bg) = c.bg {
        c.backdrop = bg;
    }
    if matches!(el.tag.as_str(), "html" | "body") {
        c.hidden = false;
    }
    if el.attrs.contains_key("hidden") && !matches!(c.display, Disp::None) {
        c.hidden = true;
    }
    if hidden_by_position(&c) {
        c.hidden = true;
    }
    if c.hidden {
        c.display = Disp::None;
    }
    // `<center>` and `align=center` on a table do not reach the cells in quirks mode; the
    // page-level choice is made in `Doc::layout` (see `Painter::quirks`).
    c
}

/// `position: absolute | fixed` boxes that are not meant to be seen: tiny and clipped (a "skip to
/// content" label), shifted far off the page, or without width or height.
fn hidden_by_position(c: &Computed) -> bool {
    if c.position != Pos::Abs {
        return false;
    }
    let tiny_w = matches!(c.width, Len::Px(w) if w <= 2);
    c.offscreen || (c.clipped && (tiny_w || c.bfc)) || (tiny_w && c.bfc)
}

/// The declarations of the rules for the `kind` generated box of `el` (author sheet, in cascade order).
pub(crate) fn matched_pseudo_decls<'a>(
    el: &Element,
    anc: &[&Element],
    sheet: &'a Stylesheet,
    budget: &mut u32,
    kind: u8,
) -> (Vec<&'a Decl>, Vec<&'a Decl>) {
    matched_decls(el, anc, sheet, budget, kind)
}

/// Reset what is not inherited to its initial value (`tag` decides the default `display`).
fn reset_non_inherited(c: &mut Computed, el: &Element) {
    let tag = el.tag.as_str();
    // Not inherited: reset to the initial values.
    c.display = default_display(tag);
    c.bg = None;
    c.margin = [Len::Px(0); 4];
    c.padding = [Len::Px(0); 4];
    c.border_w = [0; 4];
    c.radius = 0;
    c.width = Len::Auto;
    c.max_width = Len::Auto;
    c.min_height = 0;
    c.border_box = false;
    c.spacing = 2;
    c.collapse = false;
    c.valign = VAlign::Middle;
    c.vshift = 0;
    c.float = Float::None;
    c.clear = Clear::None;
    c.bfc = false;
    c.clearfix = false;
    c.position = Pos::Static;
    c.min_width = Len::Auto;
    c.flex = FlexStyle {
        // `align-items`, the gaps and the track list belong to the container; the item
        // properties start over, and the grid spec is not inherited either.
        ..FlexStyle::default()
    };
    c.hidden = false;
    c.clipped = false;
    c.offscreen = false;
    // `text-indent` is inherited; the rest above are not.
    c.content = None;
}

/// The style of the `::before` (1) or `::after` (2) box of `el`, when a rule gives it `content`;
/// `parent` is the style of `el`, `anc` its ancestors. `None` when there is no such box.
pub(crate) fn compute_pseudo(
    el: &Element,
    sheet: &Stylesheet,
    parent: &Computed,
    anc: &[&Element],
    kind: u8,
    budget: &mut u32,
) -> Option<Computed> {
    let (_, au) = matched_decls(el, anc, sheet, budget, kind);
    if au.is_empty() {
        return None;
    }
    let mut c = parent.clone();
    // A generated box is an inline child of its element: the box properties start over.
    reset_non_inherited(&mut c, el);
    c.display = Disp::Inline;
    gather_vars(&mut c, &au);
    apply_decls(&mut c, &au, parent.font_px);
    if let Some(bg) = c.bg {
        c.backdrop = bg;
    }
    if c.hidden || c.display == Disp::None {
        return None;
    }
    c.content.is_some().then_some(c)
}

/// Custom properties declared by `decls` join the inherited ones.
fn gather_vars(c: &mut Computed, decls: &[&Decl]) {
    if !decls.iter().any(|d| d.name.starts_with("--")) {
        return;
    }
    let mut m: Vars = (*c.vars).clone();
    for d in decls.iter().filter(|d| d.name.starts_with("--")) {
        if m.len() < MAX_VARS || m.contains_key(&d.name) {
            m.insert(d.name.clone(), String::from(d.value.trim()));
        }
    }
    c.vars = Rc::new(m);
}

/// Most custom properties one element sees.
const MAX_VARS: usize = 400;

/// `value` with every `var(--name, fallback)` replaced; `None` when one has neither a value nor a
/// fallback (the declaration is then invalid and ignored).
pub(super) fn subst_vars(value: &str, vars: &Vars) -> Option<String> {
    fn go(v: &str, vars: &Vars, depth: u8, out: &mut String) -> Option<()> {
        let mut rest = v;
        while let Some(p) = rest.find("var(") {
            out.push_str(&rest[..p]);
            let args = &rest[p + 4..];
            let mut depth_p = 1i32;
            let mut end = None;
            let mut comma = None;
            for (i, ch) in args.char_indices() {
                match ch {
                    '(' => depth_p += 1,
                    ')' => {
                        depth_p -= 1;
                        if depth_p == 0 {
                            end = Some(i);
                            break;
                        }
                    }
                    ',' if depth_p == 1 && comma.is_none() => comma = Some(i),
                    _ => {}
                }
            }
            let end = end?;
            let (name, fallback) = match comma {
                Some(c) => (args[..c].trim(), Some(args[c + 1..end].trim())),
                None => (args[..end].trim(), None),
            };
            if depth >= 6 || out.len() > 4096 {
                return None;
            }
            match vars.get(&name.to_ascii_lowercase()) {
                Some(val) => go(val, vars, depth + 1, out)?,
                None => go(fallback?, vars, depth + 1, out)?,
            }
            rest = &args[end + 1..];
        }
        out.push_str(rest);
        Some(())
    }
    let mut out = String::new();
    go(value, vars, 0, &mut out)?;
    Some(out)
}

/// Apply declarations in order to `c`. The font size goes first: `em` lengths below refer to it.
pub(super) fn apply_decls(c: &mut Computed, decls: &[&Decl], parent_px: i32) {
    // Values that use `var()` are resolved first; one that cannot be is dropped.
    let vars = c.vars.clone();
    let resolved: Vec<Option<String>> = decls
        .iter()
        .map(|d| {
            if d.value.contains("var(") {
                subst_vars(&d.value, &vars)
            } else {
                None
            }
        })
        .collect();
    // Functions that pick one of their arguments here: `light-dark(light, dark)`, `color-mix()`, `env()`.
    let resolved: Vec<Option<String>> = resolved
        .into_iter()
        .zip(decls.iter())
        .map(|(r, d)| {
            let text = if d.value.contains("var(") {
                r?
            } else if needs_pick(&d.value) {
                d.value.clone()
            } else {
                return None;
            };
            Some(if needs_pick(&text) {
                pick_args(&text)
            } else {
                text
            })
        })
        .collect();
    let value_of = |i: usize| -> Option<&str> {
        match &resolved[i] {
            Some(v) => Some(v.trim()),
            // A value with `var()` that could not be resolved is dropped; the others stand as written.
            None if decls[i].value.contains("var(") => None,
            None => Some(decls[i].value.trim()),
        }
    };
    // The font size first: `em` lengths below refer to it.
    for (i, d) in decls.iter().enumerate() {
        let Some(v) = value_of(i) else { continue };
        match d.name.as_str() {
            "font-size" => {
                if let Some(px) = font_size_value(v, parent_px) {
                    c.font_px = px;
                }
            }
            "font" => apply_font_shorthand(c, v, parent_px),
            _ => {}
        }
    }
    let em = c.font_px;
    let back = c.backdrop;
    for (i, d) in decls.iter().enumerate() {
        let Some(v) = value_of(i) else { continue };
        match d.name.as_str() {
            "display" => {
                c.display = match v.to_ascii_lowercase().as_str() {
                    "block" => Disp::Block,
                    "flow-root" => {
                        c.bfc = true;
                        Disp::Block
                    }
                    "flex" | "inline-flex" => Disp::Flex,
                    "grid" | "inline-grid" => Disp::Grid,
                    "inline-block" => Disp::InlineBlock,
                    "inline" | "contents" => Disp::Inline,
                    "list-item" => Disp::ListItem,
                    "none" => Disp::None,
                    "table" | "inline-table" => Disp::Table,
                    "table-row-group" | "table-header-group" | "table-footer-group" => {
                        Disp::TableRowGroup
                    }
                    "table-row" => Disp::TableRow,
                    "table-cell" => Disp::TableCell,
                    _ => c.display,
                }
            }
            "color" => {
                if let Some(rgb) = parse_color_over(v, back) {
                    c.color = rgb;
                }
            }
            "background" => {
                // `none` / `transparent` clear it; anything with a colour (or a gradient) sets it.
                let l = v.to_ascii_lowercase();
                if l == "none" || l == "transparent" {
                    c.bg = None;
                } else if let Some(col) = first_color(v, back) {
                    c.bg = Some(col);
                }
            }
            "background-color" => {
                let l = v.to_ascii_lowercase();
                if l == "transparent" || l == "none" {
                    c.bg = None;
                } else if let Some(col) = parse_color_over(v, back) {
                    c.bg = Some(col);
                }
            }
            "background-image" => {
                if c.bg.is_none() && v.to_ascii_lowercase().contains("gradient(") {
                    c.bg = first_color(v, back);
                }
            }
            "font-weight" => {
                if let Some(b) = weight_is_bold(v) {
                    c.bold = b;
                }
            }
            "font-style" => {
                let l = v.to_ascii_lowercase();
                c.italic = l.starts_with("italic") || l.starts_with("oblique");
            }
            "font-family" => c.mono = family_is_mono(v),
            "text-align" => {
                c.align = match v.to_ascii_lowercase().as_str() {
                    "center" | "-webkit-center" => Align::Center,
                    "right" | "end" => Align::Right,
                    _ => Align::Left,
                };
            }
            "line-height" => {
                if let Some(h) = line_height_value(v, em) {
                    c.line_h = h;
                }
            }
            "text-decoration" | "text-decoration-line" => {
                let l = v.to_ascii_lowercase();
                c.underline = l.contains("underline");
                c.strike = l.contains("line-through");
            }
            "text-transform" => {
                c.transform = match v.to_ascii_lowercase().as_str() {
                    "uppercase" => Transform::Upper,
                    "lowercase" => Transform::Lower,
                    "capitalize" => Transform::Capital,
                    _ => Transform::None,
                };
            }
            "white-space" => {
                c.ws = match v.to_ascii_lowercase().as_str() {
                    "pre" | "pre-wrap" | "pre-line" | "break-spaces" => Ws::Pre,
                    "nowrap" => Ws::NoWrap,
                    _ => Ws::Normal,
                };
            }
            "list-style" | "list-style-type" => {
                let l = v.to_ascii_lowercase();
                for t in tokens(&l) {
                    c.list = match t {
                        "none" => ListStyle::None,
                        "disc" => ListStyle::Disc,
                        "circle" => ListStyle::Circle,
                        "square" => ListStyle::Square,
                        "decimal" | "decimal-leading-zero" => ListStyle::Decimal,
                        "lower-alpha" | "lower-latin" => ListStyle::LowerAlpha,
                        "upper-alpha" | "upper-latin" => ListStyle::UpperAlpha,
                        "lower-roman" => ListStyle::LowerRoman,
                        "upper-roman" => ListStyle::UpperRoman,
                        _ => c.list,
                    };
                }
            }
            "margin" => {
                if let Some(m) = four(v, em) {
                    c.margin = m;
                }
            }
            "margin-top" | "margin-right" | "margin-bottom" | "margin-left" => {
                if let Some(l) = parse_len(v, em) {
                    c.margin[side_index(&d.name)] = l;
                }
            }
            "padding" => {
                if let Some(p) = four(v, em) {
                    c.padding = p.map(non_neg);
                }
            }
            "padding-top" | "padding-right" | "padding-bottom" | "padding-left" => {
                if let Some(l) = parse_len(v, em) {
                    c.padding[side_index(&d.name)] = non_neg(l);
                }
            }
            "border" => apply_border_side(c, &[0, 1, 2, 3], v),
            "border-top" => apply_border_side(c, &[0], v),
            "border-right" => apply_border_side(c, &[1], v),
            "border-bottom" => apply_border_side(c, &[2], v),
            "border-left" => apply_border_side(c, &[3], v),
            "border-width" => {
                if let Some(w) = four(v, em) {
                    for (i, l) in w.iter().enumerate() {
                        if let Len::Px(p) = l {
                            c.border_w[i] = (*p).clamp(0, 64);
                        }
                    }
                }
            }
            "border-color" => {
                if let Some(col) = first_color(v, back) {
                    c.border_color = col;
                }
            }
            "border-style" => {
                if v.eq_ignore_ascii_case("none") || v.eq_ignore_ascii_case("hidden") {
                    c.border_w = [0; 4];
                }
            }
            "border-radius" => {
                let first = tokens(v).first().copied().unwrap_or("0");
                if let Some(Len::Px(p)) = parse_len(first, em) {
                    c.radius = p.clamp(0, 200);
                } else if let Some(Len::Pct(_)) = parse_len(first, em) {
                    c.radius = 200;
                }
            }
            "width" => {
                if let Some(l) = parse_len(v, em) {
                    c.width = non_neg_or_auto(l);
                }
            }
            "max-width" => {
                if let Some(l) = parse_len(v, em) {
                    c.max_width = non_neg_or_auto(l);
                } else if v.eq_ignore_ascii_case("none") {
                    c.max_width = Len::Auto;
                }
            }
            "height" | "min-height" => {
                if let Some(p) = parse_px(v, em) {
                    c.min_height = p.max(c.min_height.min(0)).max(0);
                }
            }
            "content" => c.content = parse_content(v),
            "position" => {
                c.position = match v.to_ascii_lowercase().as_str() {
                    "absolute" | "fixed" => Pos::Abs,
                    _ => Pos::Static,
                };
            }
            "float" => {
                c.float = match v.to_ascii_lowercase().as_str() {
                    "left" | "inline-start" => Float::Left,
                    "right" | "inline-end" => Float::Right,
                    _ => Float::None,
                };
            }
            "clear" => {
                c.clear = match v.to_ascii_lowercase().as_str() {
                    "left" | "inline-start" => Clear::Left,
                    "right" | "inline-end" => Clear::Right,
                    "both" => Clear::Both,
                    _ => Clear::None,
                };
            }
            "overflow" | "overflow-x" | "overflow-y" => {
                let l = v.to_ascii_lowercase();
                c.bfc = !matches!(l.split_whitespace().next(), Some("visible" | "clip") | None);
            }
            "visibility" => {
                c.hidden = matches!(v.to_ascii_lowercase().as_str(), "hidden" | "collapse");
            }
            "opacity" => {
                if let Some((n, _)) = parse_milli(v)
                    && n <= 0
                {
                    c.hidden = true;
                }
            }
            "left" | "top" => {
                if let Some(Len::Px(p)) = parse_len(v, em)
                    && p <= -500
                {
                    c.offscreen = true;
                }
            }
            "clip" | "clip-path" => {
                let l = v.to_ascii_lowercase();
                c.clipped = !matches!(l.as_str(), "auto" | "none");
            }
            "text-indent" => {
                if let Some(Len::Px(p)) = parse_len(v, em) {
                    c.indent = p;
                    if p <= -500 {
                        c.hidden = true;
                    }
                }
            }
            "min-width" => {
                if let Some(l) = parse_len(v, em) {
                    c.min_width = non_neg_or_auto(l);
                }
            }
            "flex-direction" => {
                let l = v.to_ascii_lowercase();
                c.flex.row = l.starts_with("row");
                c.flex.reverse = l.ends_with("reverse");
            }
            "flex-wrap" => c.flex.wrap = v.to_ascii_lowercase().starts_with("wrap"),
            "flex-flow" => {
                for t in tokens(&v.to_ascii_lowercase()) {
                    match t {
                        "row" | "row-reverse" | "column" | "column-reverse" => {
                            c.flex.row = t.starts_with("row");
                            c.flex.reverse = t.ends_with("reverse");
                        }
                        "wrap" | "wrap-reverse" => c.flex.wrap = true,
                        "nowrap" => c.flex.wrap = false,
                        _ => {}
                    }
                }
            }
            "justify-content" => c.flex.justify = parse_justify(v),
            "align-items" => c.flex.align = parse_cross(v).unwrap_or(CrossAlign::Stretch),
            "place-items" => {
                c.flex.align = tokens(v)
                    .first()
                    .and_then(|t| parse_cross(t))
                    .unwrap_or(CrossAlign::Stretch);
            }
            "align-self" => c.flex.align_self = parse_cross(v),
            "gap" | "grid-gap" => {
                let t = tokens(v);
                let one = |s: &str| parse_px(s, em).map(|p| p.clamp(0, 400));
                if let Some(r) = t.first().and_then(|s| one(s)) {
                    c.flex.gap_row = r;
                    c.flex.gap_col = t.get(1).and_then(|s| one(s)).unwrap_or(r);
                }
            }
            "row-gap" | "grid-row-gap" => {
                if let Some(p) = parse_px(v, em) {
                    c.flex.gap_row = p.clamp(0, 400);
                }
            }
            "column-gap" | "grid-column-gap" => {
                if let Some(p) = parse_px(v, em) {
                    c.flex.gap_col = p.clamp(0, 400);
                }
            }
            "flex-grow" => {
                if let Some((n, _)) = parse_milli(v) {
                    c.flex.grow = n.clamp(0, 100_000) as i32;
                }
            }
            "flex-shrink" => {
                if let Some((n, _)) = parse_milli(v) {
                    c.flex.shrink = n.clamp(0, 100_000) as i32;
                }
            }
            "flex-basis" => {
                if let Some(l) = parse_len(v, em) {
                    c.flex.basis = non_neg_or_auto(l);
                }
            }
            "flex" => apply_flex_shorthand(c, v, em),
            "order" => {
                if let Some((n, _)) = parse_milli(v) {
                    c.flex.order = (n / 1000).clamp(-1000, 1000) as i32;
                }
            }
            "grid-template-columns" => c.flex.tracks = Rc::new(parse_tracks(v, em)),
            "grid-column" | "grid-column-start" | "grid-column-end" => {
                let l = v.to_ascii_lowercase();
                if let Some(rest) = l.split_whitespace().skip_while(|t| *t != "span").nth(1)
                    && let Ok(n) = rest.parse::<u8>()
                {
                    c.flex.span = n.clamp(1, 12);
                } else if l.contains("/ -1") || l.contains("/-1") {
                    c.flex.span = 12;
                }
            }
            "box-sizing" => c.border_box = v.eq_ignore_ascii_case("border-box"),
            "border-collapse" => c.collapse = v.eq_ignore_ascii_case("collapse"),
            "border-spacing" => {
                if let Some(px) = tokens(v).first().and_then(|t| parse_px(t, em)) {
                    c.spacing = px.clamp(0, 64);
                }
            }
            "vertical-align" => {
                let l = v.to_ascii_lowercase();
                c.vshift = match l.as_str() {
                    "super" => 1,
                    "sub" => -1,
                    _ => 0,
                };
                c.valign = match l.as_str() {
                    "top" | "text-top" => VAlign::Top,
                    "bottom" | "text-bottom" => VAlign::Bottom,
                    _ => VAlign::Middle,
                };
            }
            _ => {}
        }
    }
}

/// The presentational attributes old pages still use (`align`, `bgcolor`, `<font>`,
/// `border` and `cellpadding` of tables, `type` of lists...). They apply before the
/// stylesheets, so any CSS overrides them.
pub(super) fn presentational(el: &Element, anc: &[&Element], c: &mut Computed) {
    let attr = |n: &str| el.attrs.get(n).map(|v| v.trim());
    let table = || anc.iter().rev().find(|a| a.tag == "table");
    let em = c.font_px;
    match el.tag.as_str() {
        "p" | "div" | "h1" | "h2" | "h3" | "h4" | "h5" | "h6" | "td" | "th" | "tr" | "caption"
        | "section" | "article" | "header" | "footer" | "center" => {
            match attr("align").map(str::to_ascii_lowercase).as_deref() {
                Some("center") => c.align = Align::Center,
                Some("right") => c.align = Align::Right,
                Some("left") => c.align = Align::Left,
                _ => {}
            }
        }
        "table" => {
            match attr("align").map(str::to_ascii_lowercase).as_deref() {
                Some("center") => {
                    c.margin[1] = Len::Auto;
                    c.margin[3] = Len::Auto;
                }
                Some("right") => c.margin[3] = Len::Auto,
                _ => {}
            }
            if let Some(b) = attr("border") {
                let n = if b.is_empty() {
                    1
                } else {
                    b.parse::<i32>().unwrap_or(1)
                };
                if n > 0 {
                    c.border_w = [n.clamp(1, 8); 4];
                    c.border_color = Rgb(0x80, 0x80, 0x80);
                }
            }
            if let Some(Len::Px(n)) = attr("cellspacing").and_then(|v| parse_len(v, em)) {
                c.spacing = n.clamp(0, 64);
            }
        }
        "ol" | "ul" | "li" => {
            c.list = match attr("type") {
                Some("a") => ListStyle::LowerAlpha,
                Some("A") => ListStyle::UpperAlpha,
                Some("i") => ListStyle::LowerRoman,
                Some("I") => ListStyle::UpperRoman,
                Some("1") => ListStyle::Decimal,
                Some(t) if t.eq_ignore_ascii_case("disc") => ListStyle::Disc,
                Some(t) if t.eq_ignore_ascii_case("circle") => ListStyle::Circle,
                Some(t) if t.eq_ignore_ascii_case("square") => ListStyle::Square,
                _ => c.list,
            };
        }
        "font" => {
            if let Some(col) = attr("color").and_then(parse_color) {
                c.color = col;
            }
            if let Some(n) =
                attr("size").and_then(|v| v.trim_start_matches('+').parse::<usize>().ok())
            {
                const SIZES: [i32; 8] = [10, 10, 13, 16, 18, 24, 32, 48];
                c.font_px = SIZES[n.min(7)];
            }
        }
        _ => {}
    }
    if matches!(el.tag.as_str(), "td" | "th") {
        if let Some(t) = table() {
            if t.attrs.get("border").is_some_and(|b| {
                let b = b.trim();
                b.is_empty() || b.parse::<i32>().unwrap_or(1) > 0
            }) {
                c.border_w = [1; 4];
                c.border_color = Rgb(0xA0, 0xA0, 0xA0);
            }
            if let Some(Len::Px(n)) = t.attrs.get("cellpadding").and_then(|v| parse_len(v, em)) {
                c.padding = [Len::Px(n.clamp(0, 64)); 4];
            }
        }
        match attr("valign").map(str::to_ascii_lowercase).as_deref() {
            Some("top") => c.valign = VAlign::Top,
            Some("bottom") => c.valign = VAlign::Bottom,
            Some("middle") => c.valign = VAlign::Middle,
            _ => {}
        }
    }
    if matches!(el.tag.as_str(), "table" | "td" | "th")
        && let Some(l) = attr("width").and_then(|v| parse_len(v, em))
    {
        c.width = non_neg_or_auto(l);
    }
    if let Some(col) = attr("bgcolor").and_then(parse_color) {
        c.bg = Some(col);
    }
    if el.tag == "body"
        && let Some(col) = attr("text").and_then(parse_color)
    {
        c.color = col;
    }
}

pub(super) fn non_neg_or_auto(l: Len) -> Len {
    match l {
        Len::Px(p) => Len::Px(p.max(0)),
        Len::Pct(p) => Len::Pct(p.max(0)),
        l @ (Len::Calc { .. } | Len::Auto) => l,
    }
}

pub(super) fn side_index(name: &str) -> usize {
    if name.ends_with("top") {
        0
    } else if name.ends_with("right") {
        1
    } else if name.ends_with("bottom") {
        2
    } else {
        3
    }
}

/// `text` with `transform` applied (page text only; ASCII and Latin-1 letters via
/// the standard library's case mapping).
pub(crate) fn transform_text(text: &str, t: Transform) -> String {
    match t {
        Transform::None => String::from(text),
        Transform::Upper => text.to_uppercase(),
        Transform::Lower => text.to_lowercase(),
        Transform::Capital => {
            let mut out = String::with_capacity(text.len());
            let mut start = true;
            for ch in text.chars() {
                if start && ch.is_alphabetic() {
                    out.extend(ch.to_uppercase());
                } else {
                    out.push(ch);
                }
                start = ch.is_whitespace();
            }
            out
        }
    }
}

/// Does `v` use a function [`pick_args`] rewrites?
fn needs_pick(v: &str) -> bool {
    v.contains("light-dark(") || v.contains("color-mix(") || v.contains("env(")
}

/// Rewrite `light-dark(a, b)` as `a` (the page is always shown light), `color-mix(in srgb, a ..., b ...)` as
/// its first colour and `env(name, fallback)` as the fallback (or 0), at any depth.
#[cfg(test)]
pub(super) fn pick_args_for_test(v: &str) -> String {
    pick_args(v)
}

fn pick_args(v: &str) -> String {
    let mut out = String::with_capacity(v.len());
    let mut rest = v;
    'outer: while !rest.is_empty() {
        for name in ["light-dark(", "color-mix(", "env("] {
            if let Some(p) = rest.find(name) {
                // Only the earliest function is rewritten in this round.
                let earliest = ["light-dark(", "color-mix(", "env("]
                    .iter()
                    .filter_map(|n| rest.find(n))
                    .min()
                    .unwrap_or(p);
                if p != earliest {
                    continue;
                }
                out.push_str(&rest[..p]);
                let args = &rest[p + name.len()..];
                let mut depth = 1i32;
                let mut end = args.len();
                let mut commas: Vec<usize> = Vec::new();
                for (i, ch) in args.char_indices() {
                    match ch {
                        '(' => depth += 1,
                        ')' => {
                            depth -= 1;
                            if depth == 0 {
                                end = i;
                                break;
                            }
                        }
                        ',' if depth == 1 => commas.push(i),
                        _ => {}
                    }
                }
                let parts: Vec<&str> = {
                    let mut v = Vec::new();
                    let mut start = 0;
                    for &c in &commas {
                        v.push(&args[start..c]);
                        start = c + 1;
                    }
                    v.push(&args[start..end]);
                    v
                };
                let chosen = match name {
                    "light-dark(" => parts.first().copied().unwrap_or(""),
                    "color-mix(" => parts
                        .get(1)
                        .map(|c| c.split_whitespace().next().unwrap_or(""))
                        .unwrap_or(""),
                    _ => parts.get(1).copied().unwrap_or("0"),
                };
                out.push_str(&pick_args(chosen.trim()));
                rest = args.get(end + 1..).unwrap_or("");
                continue 'outer;
            }
        }
        out.push_str(rest);
        break;
    }
    out
}

/// The text of a `content` value: its quoted strings (CSS escapes decoded), `open-quote` and
/// `close-quote`; `attr()`, counters and `url()` give nothing. `None` for `none` / `normal`. Private-use
/// characters (icon fonts) are dropped: there is no font for them.
fn parse_content(v: &str) -> Option<String> {
    let l = v.trim();
    if l.eq_ignore_ascii_case("none") || l.eq_ignore_ascii_case("normal") {
        return None;
    }
    let mut out = String::new();
    let b = l.as_bytes();
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            q @ (b'"' | b'\'') => {
                i += 1;
                while i < b.len() && b[i] != q {
                    if b[i] == b'\\' && i + 1 < b.len() {
                        i += 1;
                        let hex: String = l[i..]
                            .chars()
                            .take_while(char::is_ascii_hexdigit)
                            .take(6)
                            .collect();
                        if hex.is_empty() {
                            // An escaped character stands for itself (a newline escape is nothing).
                            let ch = l[i..].chars().next().unwrap_or(' ');
                            if ch != '\n' {
                                out.push(ch);
                            }
                            i += ch.len_utf8();
                        } else {
                            let cp = u32::from_str_radix(&hex, 16).unwrap_or(0xfffd);
                            if let Some(ch) = char::from_u32(cp) {
                                out.push(ch);
                            }
                            i += hex.len();
                            // One space after a hex escape belongs to it.
                            if b.get(i) == Some(&b' ') {
                                i += 1;
                            }
                        }
                    } else {
                        let ch = l[i..].chars().next().unwrap_or(' ');
                        out.push(ch);
                        i += ch.len_utf8();
                    }
                }
                i += 1;
            }
            _ => {
                let rest = &l[i..];
                if rest.starts_with("open-quote") {
                    out.push('\u{201c}');
                    i += 10;
                } else if rest.starts_with("close-quote") {
                    out.push('\u{201d}');
                    i += 11;
                } else {
                    i += rest.chars().next().map_or(1, char::len_utf8);
                }
            }
        }
    }
    out.retain(|c| !('\u{e000}'..='\u{f8ff}').contains(&c));
    Some(out)
}
