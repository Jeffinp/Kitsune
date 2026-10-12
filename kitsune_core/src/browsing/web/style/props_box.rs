//! The box model: margins, padding, borders, sizes, tables and `vertical-align`.

use super::*;

/// Apply the property `name` with value `v` (`em` is the element's font size); `false` when it is not one of these.
pub(super) fn apply_box(c: &mut Computed, name: &str, v: &str, em: i32) -> bool {
    match name {
        "margin" => {
            if let Some(m) = four(v, em) {
                c.margin = m;
            }
        }
        "margin-top" | "margin-right" | "margin-bottom" | "margin-left" => {
            if let Some(l) = parse_len(v, em) {
                c.margin[side_index(name)] = l;
            }
        }
        "padding" => {
            if let Some(p) = four(v, em) {
                c.padding = p.map(non_neg);
            }
        }
        "padding-top" | "padding-right" | "padding-bottom" | "padding-left" => {
            if let Some(l) = parse_len(v, em) {
                c.padding[side_index(name)] = non_neg(l);
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
            if let Some(col) = first_color(v, c.backdrop) {
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
        "min-width" => {
            if let Some(l) = parse_len(v, em) {
                c.min_width = non_neg_or_auto(l);
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
        _ => return false,
    }
    true
}
