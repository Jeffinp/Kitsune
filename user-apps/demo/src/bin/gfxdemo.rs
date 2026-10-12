//! A window of its own: a square that bounces, a cross that follows the pointer, a bar that lights
//! up with every key (its colour is the key code), a frame around the content that shows the size
//! the window has now. Esc, the close button or a click on the red square finish it.
#![no_std]
#![no_main]

use kitsune_user::abi::{EV_CLOSE, EV_KEY, EV_POINTER, EV_RESIZE};
use kitsune_user::win::{self, Window};
use kitsune_user::*;

entry!(main);

const MAX: usize = 640 * 480 * 4;
static mut FRAME: [u8; MAX] = [0; MAX];

fn main() -> i32 {
    let (mut w, mut h) = (320u32, 240u32);
    let Ok(window) = Window::open(w, h, "Demo grafico") else {
        println!("gfxdemo: could not open the window");
        return 1;
    };
    // SAFETY: single-threaded program; this is the only reference to the buffer.
    let frame: &mut [u8] = unsafe { &mut *core::ptr::addr_of_mut!(FRAME) };
    let (mut px, mut py) = (w as i32 / 2, h as i32 / 2);
    let (mut x, mut y, mut dx, mut dy) = (20i32, 20i32, 3i32, 2i32);
    let mut key_color = 0x404040u32;
    let mut frames = 0u32;
    loop {
        // Input first: wait a frame's time for the next event, then take whatever else is there.
        let mut first = true;
        loop {
            let timeout = if first { 16 } else { 0 };
            first = false;
            match win::next_event(timeout) {
                Ok(Some(e)) => match e.kind {
                    EV_CLOSE => return 0,
                    EV_KEY => {
                        if e.a == 27 {
                            return 0;
                        }
                        key_color =
                            0x3060c0 ^ ((e.a as u32).wrapping_mul(0x00_9e_37_79) & 0x00_ff_ff_ff);
                    }
                    EV_POINTER => {
                        (px, py) = (e.a, e.b);
                        if e.c & 1 != 0 && (e.a - x).abs() < 12 && (e.b - y).abs() < 12 {
                            return 0;
                        }
                    }
                    EV_RESIZE => {
                        (w, h) = ((e.a as u32).clamp(64, 640), (e.b as u32).clamp(64, 480));
                    }
                    _ => {}
                },
                _ => break,
            }
        }
        // Move and draw.
        x += dx;
        y += dy;
        if x < 0 || x + 24 > w as i32 {
            dx = -dx;
            x += 2 * dx;
        }
        if y < 0 || y + 24 > h as i32 {
            dy = -dy;
            y += 2 * dy;
        }
        let n = (w * h * 4) as usize;
        let f = &mut frame[..n];
        win::fill_rect(f, w, 0, 0, w as i32, h as i32, 0x10162a);
        win::fill_rect(f, w, 0, 0, w as i32, 2, 0x4cc9f0);
        win::fill_rect(f, w, 0, h as i32 - 2, w as i32, 2, 0x4cc9f0);
        win::fill_rect(f, w, 0, 0, 2, h as i32, 0x4cc9f0);
        win::fill_rect(f, w, w as i32 - 2, 0, 2, h as i32, 0x4cc9f0);
        win::fill_rect(f, w, 8, h as i32 - 20, w as i32 - 16, 10, key_color);
        win::fill_rect(f, w, x, y, 24, 24, 0xe05050);
        win::fill_rect(f, w, px - 10, py, 21, 1, 0xf0f0f0);
        win::fill_rect(f, w, px, py - 10, 1, 21, 0xf0f0f0);
        if window.present(f, w, h).is_err() {
            return 2;
        }
        frames += 1;
        if frames == 1 {
            println!("gfxdemo: first frame {w}x{h}");
        }
    }
}
