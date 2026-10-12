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
    for (i, d) in decls.iter().enumerate() {
        let Some(v) = value_of(i) else { continue };
        // A property none of the groups knows is not one the engine reads.
        let _ = apply_text(c, &d.name, v, em)
            || apply_box(c, &d.name, v, em)
            || apply_layout(c, &d.name, v, em);
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
