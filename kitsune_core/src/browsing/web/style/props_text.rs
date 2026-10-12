//! Properties of text and paint: colour, backgrounds, fonts, alignment, lists.

use super::*;

/// Apply the property `name` with value `v` (`em` is the element's font size); `false` when it is not one of these.
pub(super) fn apply_text(c: &mut Computed, name: &str, v: &str, em: i32) -> bool {
    match name {
        "color" => {
            if let Some(rgb) = parse_color_over(v, c.backdrop) {
                c.color = rgb;
            }
        }
        "background" => {
            // `none` / `transparent` clear it; anything with a colour (or a gradient) sets it.
            let l = v.to_ascii_lowercase();
            if l == "none" || l == "transparent" {
                c.bg = None;
            } else if let Some(col) = first_color(v, c.backdrop) {
                c.bg = Some(col);
            }
        }
        "background-color" => {
            let l = v.to_ascii_lowercase();
            if l == "transparent" || l == "none" {
                c.bg = None;
            } else if let Some(col) = parse_color_over(v, c.backdrop) {
                c.bg = Some(col);
            }
        }
        "background-image" => {
            if c.bg.is_none() && v.to_ascii_lowercase().contains("gradient(") {
                c.bg = first_color(v, c.backdrop);
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
        "text-indent" => {
            if let Some(Len::Px(p)) = parse_len(v, em) {
                c.indent = p;
                if p <= -500 {
                    c.hidden = true;
                }
            }
        }
        _ => return false,
    }
    true
}
