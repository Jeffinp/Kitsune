//! Properties of boxes' placement: `display`, `position`, `float`, `overflow`, flexbox and grid, generated `content`.

use super::*;

/// Apply the property `name` with value `v` (`em` is the element's font size); `false` when it is not one of these.
pub(super) fn apply_layout(c: &mut Computed, name: &str, v: &str, em: i32) -> bool {
    match name {
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
        "grid-template-columns" => {
            let t = parse_tracks(v, em);
            let g = Rc::make_mut(&mut c.flex.tracks);
            g.cols = t.cols;
            g.auto_min = t.auto_min;
        }
        // `grid-template: <rows> / <columns>` (the rows are not used).
        "grid-template" => {
            if let Some((_, cols)) = v.rsplit_once('/') {
                let t = parse_tracks(cols, em);
                let g = Rc::make_mut(&mut c.flex.tracks);
                g.cols = t.cols;
                g.auto_min = t.auto_min;
            }
        }
        "grid-template-areas" => {
            let g = Rc::make_mut(&mut c.flex.tracks);
            g.areas = parse_areas(v);
        }
        "grid-area" => {
            let n = v.trim();
            // A single name; the four-line form is not supported.
            if !n.is_empty()
                && !n.contains('/')
                && n.bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
            {
                c.flex.area = Some(Rc::from(n));
            }
        }
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
        _ => return false,
    }
    true
}
