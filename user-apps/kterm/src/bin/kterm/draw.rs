//! Drawing the terminal into the window's pixel buffer with the 8x8 bitmap font at 2x.

use alloc::vec::Vec;
use kitsune_core::shell::screen::View;

include!("../../../../glyphs.rs");

/// Pixels of a character cell.
pub const CELL_W: u32 = 12;
pub const CELL_H: u32 = 18;

const BG: u32 = 0x14_16_1c;
const FG: u32 = 0xd8_dc_e6;
const ACCENT: u32 = 0x6c_b6_ff;
const MUTED: u32 = 0x8a_93_a6;

/// One character at `(x, y)` (top left of its cell).
fn put_char(frame: &mut [u8], w: u32, x: i32, y: i32, ch: char, rgb: u32) {
    let byte = if (ch as u32) < 256 { ch as u8 } else { b'?' };
    for (row, bits) in glyph(byte).iter().enumerate() {
        for col in 0..8 {
            if (bits >> (7 - col)) & 1 == 1 {
                let (px, py) = (x + col * 2, y + row as i32 * 2);
                kitsune_user::win::fill_rect(frame, w, px, py, 2, 2, rgb);
            }
        }
    }
}

/// Number of columns and rows a `w` x `h` window holds.
pub fn grid(w: u32, h: u32) -> (usize, usize) {
    (
        ((w / CELL_W) as usize).max(1),
        ((h / CELL_H) as usize).max(1),
    )
}

/// Paint `view` into `frame` (`w` x `h`, RGBA). The first `prompt_chars` characters of the live line
/// are the prompt: the folder in the accent colour, the symbol quieter.
pub fn paint(frame: &mut Vec<u8>, w: u32, h: u32, view: &View, cols: usize, prompt_chars: usize) {
    frame.resize((w as usize) * (h as usize) * 4, 0);
    for px in frame.chunks_exact_mut(4) {
        px.copy_from_slice(&[(BG >> 16) as u8, (BG >> 8) as u8, BG as u8, 255]);
    }
    let sym_at = prompt_chars.saturating_sub(2);
    for (i, row) in view.rows.iter().enumerate() {
        let y = i as i32 * CELL_H as i32 + 1;
        for (j, ch) in row.chars().enumerate() {
            let colour = match view.live_first {
                Some(lf) if i >= lf => {
                    let n = (i - lf) * cols + j;
                    if n < sym_at {
                        ACCENT
                    } else if n < prompt_chars {
                        MUTED
                    } else {
                        FG
                    }
                }
                _ => FG,
            };
            put_char(frame, w, j as i32 * CELL_W as i32, y, ch, colour);
        }
    }
    if let Some((r, c)) = view.cursor {
        let (x, y) = (c as i32 * CELL_W as i32, r as i32 * CELL_H as i32);
        kitsune_user::win::fill_rect(frame, w, x, y, CELL_W as i32, CELL_H as i32, FG);
        let ch = view
            .rows
            .get(r)
            .and_then(|s| s.chars().nth(c))
            .unwrap_or(' ');
        put_char(frame, w, x, y + 1, ch, BG);
    }
}
