//! A window on the desktop: the program draws into its own pixel buffer and hands it over.
//!
//! One window per program. [`Window::open`] asks the desktop for it, [`Window::present`] shows a frame
//! (RGBA, 4 bytes a pixel, tightly packed, at most 1280 x 800), [`next_event`] gives the keys, the
//! pointer, a new content size and the request to close (see `kitsune_userabi::Event`). When the
//! program ends its window goes with it.

use super::{syscall3, syscall5};
use kitsune_userabi::{self as abi, Event};

/// What a failed call returned (`-errno`).
pub type Errno = i64;

/// The program's window.
pub struct Window {
    _private: (),
}

impl Window {
    /// Ask for a window of `w` x `h` pixels of content with `title` (printable ASCII, at most 64 bytes).
    pub fn open(w: u32, h: u32, title: &str) -> Result<Window, Errno> {
        // SAFETY: the pointer and length describe `title`.
        let r = unsafe {
            syscall5(
                abi::SYS_WIN_OPEN,
                u64::from(w),
                u64::from(h),
                title.as_ptr() as u64,
                title.len() as u64,
                0,
            )
        };
        if r < 0 {
            Err(r)
        } else {
            Ok(Window { _private: () })
        }
    }

    /// Show a frame: `rgba` holds `w * h` pixels. The window may be a different size than the frame
    /// (a frame is drawn at its top left corner).
    pub fn present(&self, rgba: &[u8], w: u32, h: u32) -> Result<(), Errno> {
        if rgba.len() < (w as usize) * (h as usize) * 4 {
            return Err(abi::errno::EINVAL);
        }
        // SAFETY: the pointer describes `rgba`, which holds at least `w * h * 4` bytes.
        let r = unsafe {
            syscall3(
                abi::SYS_WIN_PRESENT,
                rgba.as_ptr() as u64,
                u64::from(w),
                u64::from(h),
            )
        };
        if r < 0 { Err(r) } else { Ok(()) }
    }

    /// Change the title.
    pub fn set_title(&self, title: &str) -> Result<(), Errno> {
        // SAFETY: the pointer and length describe `title`.
        let r = unsafe {
            syscall3(
                abi::SYS_WIN_TITLE,
                title.as_ptr() as u64,
                title.len() as u64,
                0,
            )
        };
        if r < 0 { Err(r) } else { Ok(()) }
    }
}

/// The next input event, waiting up to `timeout_ms` for one (`0` only looks, `u64::MAX` waits
/// without limit). `Ok(None)` when none came.
pub fn next_event(timeout_ms: u64) -> Result<Option<Event>, Errno> {
    let mut raw = [0u8; abi::EVENT_LEN];
    // SAFETY: the pointer describes the 16-byte `raw`.
    let r = unsafe { syscall3(abi::SYS_WIN_EVENT, raw.as_mut_ptr() as u64, timeout_ms, 0) };
    match r {
        1 => Ok(Some(Event::from_bytes(&raw))),
        0 => Ok(None),
        e => Err(e),
    }
}

/// Set pixel `(x, y)` of a `w`-wide RGBA frame (outside the frame: nothing).
pub fn put(frame: &mut [u8], w: u32, x: i32, y: i32, rgb: u32) {
    if x < 0 || y < 0 || x as u32 >= w {
        return;
    }
    let o = ((y as usize) * (w as usize) + x as usize) * 4;
    if let Some(px) = frame.get_mut(o..o + 4) {
        px.copy_from_slice(&[(rgb >> 16) as u8, (rgb >> 8) as u8, rgb as u8, 255]);
    }
}

/// Fill a rectangle of a `w`-wide RGBA frame with `0xRRGGBB`.
pub fn fill_rect(frame: &mut [u8], w: u32, x: i32, y: i32, rw: i32, rh: i32, rgb: u32) {
    for yy in y..y + rh {
        for xx in x..x + rw {
            put(frame, w, xx, yy, rgb);
        }
    }
}
