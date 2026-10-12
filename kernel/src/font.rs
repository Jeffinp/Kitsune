//! Minimal 8x8 bitmap font: uppercase, lowercase, digits, and symbols.
//! Unknown chars render as blank. Glyphs occupy the top-left 5x7 of each cell.

use crate::fb::{Canvas, Color};

include!("../../user-apps/glyphs.rs");

/// Draws a single character. `scale` enlarges each font pixel into a square block.
pub fn draw_char(c: &mut Canvas, x: usize, y: usize, ch: u8, color: Color, scale: usize) {
    let t0 = crate::trace::t();
    draw_char_inner(c, x, y, ch, color, scale);
    crate::trace::prim(crate::trace::Prim::Glyph, t0);
}

fn draw_char_inner(c: &mut Canvas, x: usize, y: usize, ch: u8, color: Color, scale: usize) {
    let g = glyph(ch);
    for (row, bits) in g.iter().enumerate() {
        for col in 0..8usize {
            if (bits >> (7 - col)) & 1 == 1 {
                c.fill_rect(x + col * scale, y + row * scale, scale, scale, color);
            }
        }
    }
}

/// Draws a left-to-right string. Advances 6*scale px per glyph (5px glyph + 1px gap).
pub fn draw_text(c: &mut Canvas, x: usize, y: usize, text: &str, color: Color, scale: usize) {
    draw_bytes(c, x, y, text.as_bytes(), color, scale);
}

/// Draws raw ASCII bytes (terminal/editor content is stored as `[u8]`).
pub fn draw_bytes(c: &mut Canvas, x: usize, y: usize, bytes: &[u8], color: Color, scale: usize) {
    let mut cx = x;
    for &b in bytes {
        draw_char(c, cx, y, b, color, scale);
        cx += 6 * scale;
    }
}

/// Horizontal advance per character cell.
pub const fn cell_w(scale: usize) -> usize {
    6 * scale
}
