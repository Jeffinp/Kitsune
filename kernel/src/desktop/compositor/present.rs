//! Getting pixels to the screen: the back buffer, uploads of damage rectangles to the
//! framebuffer, and the three things that live **only in the framebuffer** and are never part of
//! the scene: the toast banners, the performance HUD and the mouse cursor.
//!
//! Invariant: outside those three, the framebuffer equals the back buffer after every frame. The
//! compositor changes the back buffer only inside the plan's damage and uploads exactly that, so
//! the two cannot drift apart. The framebuffer-only things follow a protocol that makes each
//! restorable from the back buffer:
//! - the cursor is erased first and painted last (`kitsune_core::cursor`);
//! - after an upload that touched the HUD's or a toast's rectangle, that thing is drawn again.

use super::super::*;
use crate::perf::Perf;
use crate::trace;
use bootloader_api::info::FrameBufferInfo;
use kitsune_core::compositor::Region;
use kitsune_core::cursor::CursorTrack;

/// The three buffers of a frame and their shared layout.
pub struct Screen<'a> {
    /// The framebuffer (video memory, slow to read, never read here except by the tracer).
    pub fb: &'a mut [u8],
    /// The composed scene without the cursor, toasts and HUD.
    pub back: &'a mut [u8],
    /// The wallpaper with the panel's glass strip.
    pub bg: &'a mut [u8],
    /// Scratch buffer of the reference redraw (`verify.rs`).
    pub check: &'a mut [u8],
    pub info: FrameBufferInfo,
    /// Bytes of visible pixels.
    pub n: usize,
}

impl Screen<'_> {
    /// Upload the whole back buffer.
    pub fn upload_all(&mut self) {
        if trace::ON {
            trace::vram_upload(
                self.n as u64,
                trace::count_diff(&self.fb[..self.n], &self.back[..self.n]),
            );
        }
        let t0 = trace::t();
        let n = self.n;
        crate::fb::copy_bytes(&mut self.fb[..n], &self.back[..n]);
        trace::stage(trace::Stage::Blit, t0);
    }

    /// Upload the rectangle `r` of the back buffer.
    pub fn upload(&mut self, r: Rect) {
        let Some(r) = r.intersection(&Rect::new(
            0,
            0,
            self.info.width as i32,
            self.info.height as i32,
        )) else {
            return;
        };
        if trace::ON {
            let bpp = self.info.bytes_per_pixel;
            let (mut up, mut chg) = (0u64, 0u64);
            for row in r.y as usize..r.bottom() as usize {
                let off = (row * self.info.stride + r.x as usize) * bpp;
                let end = off + r.w as usize * bpp;
                if end <= self.n {
                    up += (end - off) as u64;
                    chg += trace::count_diff(&self.fb[off..end], &self.back[off..end]);
                }
            }
            trace::vram_upload(up, chg);
        }
        let t0 = trace::t();
        copy_rect(self.fb, self.back, self.info, r);
        trace::stage(trace::Stage::Blit, t0);
    }

    /// Upload every rectangle of a plan's damage (one memcpy when it is most of the screen).
    pub fn upload_region(&mut self, damage: &Region) {
        let screen = (self.info.width * self.info.height) as u64;
        if damage.area() * 10 >= screen * 7 {
            self.upload_all();
        } else {
            for r in damage.rects() {
                self.upload(*r);
            }
        }
    }

    /// Restore `r` of the framebuffer from the back buffer, untimed (the framebuffer-only passes).
    fn restore(&mut self, r: Rect) {
        copy_rect(self.fb, self.back, self.info, r);
    }
}

/// Copy rectangle `r` between two buffers of one layout.
fn copy_rect(dst: &mut [u8], src: &[u8], info: FrameBufferInfo, r: Rect) {
    let bpp = info.bytes_per_pixel;
    let x = r.x.max(0) as usize;
    let y = r.y.max(0) as usize;
    if x >= info.width || y >= info.height {
        return;
    }
    let x_end = (r.right().max(0) as usize).min(info.width);
    let y_end = (r.bottom().max(0) as usize).min(info.height);
    let row_len = x_end.saturating_sub(x) * bpp;
    for row in y..y_end {
        let off = (row * info.stride + x) * bpp;
        let end = off + row_len;
        if end <= dst.len() && end <= src.len() {
            crate::fb::copy_bytes(&mut dst[off..end], &src[off..end]);
        }
    }
}

/// State of the framebuffer-only passes.
pub struct FbPasses {
    pub cursor: CursorTrack,
    /// Area the toasts covered when they were last drawn.
    prev_toast: Rect,
    /// Tick of the last HUD refresh, and whether it was on at the last frame.
    last_hud: u64,
    hud_was_on: bool,
}

impl FbPasses {
    pub fn new() -> Self {
        Self {
            cursor: CursorTrack::new(CURSOR_W, CURSOR_H),
            prev_toast: Rect::new(0, 0, 0, 0),
            last_hud: 0,
            hud_was_on: false,
        }
    }

    /// Does the cursor or the HUD switch need a frame (the sprite is not where the pointer is,
    /// or the HUD was just toggled)?
    pub fn stale(&self, desk: &Desktop) -> bool {
        self.cursor.is_stale(desk.pointer()) || desk.hud_visible() != self.hud_was_on
    }

    /// Step 1 of the cursor protocol: erase the sprite from the framebuffer. Returns the box that
    /// was restored.
    pub fn erase_cursor(&mut self, scr: &mut Screen) -> Option<Rect> {
        let r = self
            .cursor
            .erase(scr.info.width as i32, scr.info.height as i32)?;
        scr.restore(r);
        Some(r)
    }

    /// Toasts: restore what they covered (now and at the last draw) and draw them on top.
    /// `due` says a frame was rendered or the toasts changed.
    pub fn toasts(&mut self, desk: &Desktop, scr: &mut Screen, changed: bool, due: bool) -> bool {
        if !(changed || (due && !desk.toasts_idle())) {
            return false;
        }
        let tt = trace::t();
        let cur = desk.toast_bounds();
        let r = if self.prev_toast.is_empty() {
            cur
        } else if cur.is_empty() {
            self.prev_toast
        } else {
            self.prev_toast.union(&cur)
        };
        scr.restore(r);
        let n = scr.n;
        let info = scr.info;
        let mut c = Canvas::new(&mut scr.fb[..n], info);
        desk.draw_toasts(&mut c);
        self.prev_toast = cur;
        trace::stage(trace::Stage::Hud, tt);
        true
    }

    /// The performance HUD: refreshed about ten times a second, and at once when something
    /// wiped it (an upload over it, or the cursor's old box). Returns whether it was drawn.
    pub fn hud(
        &mut self,
        desk: &Desktop,
        scr: &mut Screen,
        perf: &Perf,
        tick: u64,
        wiped: bool,
    ) -> bool {
        let on = desk.hud_visible();
        let hr = Perf::rect(scr.info.width as i32);
        if on != self.hud_was_on {
            // Switched on or off: draw it, or restore what it covered.
            self.hud_was_on = on;
            self.last_hud = 0;
            if !on {
                scr.restore(hr);
            }
        }
        if !(on && (tick.saturating_sub(self.last_hud) >= 25 || wiped)) {
            return false;
        }
        self.last_hud = tick;
        let used = crate::HEAP_SIZE - crate::ALLOCATOR.free_bytes().min(crate::HEAP_SIZE);
        let heap_pct = (used * 100 / crate::HEAP_SIZE) as u32;
        let th = trace::t();
        scr.restore(hr);
        let n = scr.n;
        let info = scr.info;
        let mut c = Canvas::new(&mut scr.fb[..n], info);
        perf.draw(&mut c, heap_pct, sched::thread_count());
        trace::stage(trace::Stage::Hud, th);
        true
    }

    /// Step 3 of the cursor protocol: paint the sprite, always the last thing of a frame.
    pub fn paint_cursor(&mut self, desk: &Desktop, scr: &mut Screen) {
        let t0 = trace::t();
        self.cursor.paint(
            desk.pointer(),
            scr.info.width as i32,
            scr.info.height as i32,
        );
        let n = scr.n;
        let info = scr.info;
        let mut c = Canvas::new(&mut scr.fb[..n], info);
        desk.draw_cursor(&mut c);
        trace::stage(trace::Stage::Cursor, t0);
    }

    /// The HUD rectangle (for "did an upload wipe it").
    pub fn hud_rect(width: i32) -> Rect {
        Perf::rect(width)
    }
}
