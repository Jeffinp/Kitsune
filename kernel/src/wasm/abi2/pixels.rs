//! Pixel host functions: the framebuffer offset and the blits (1:1 and alpha) into the guest's surface.

use super::*;

/// Byte offset of pixel `(x, y)` in the surface, or `None` outside it.
pub(super) fn offset(
    info: &bootloader_api::info::FrameBufferInfo,
    x: i64,
    y: i64,
) -> Option<usize> {
    if x < 0 || y < 0 || x >= info.width as i64 || y >= info.height as i64 {
        return None;
    }
    Some((y as usize * info.stride + x as usize) * info.bytes_per_pixel)
}

/// Writes one RGB pixel in the surface's native order.
#[inline]
pub(super) fn put_px(
    buf: &mut [u8],
    info: &bootloader_api::info::FrameBufferInfo,
    o: usize,
    r: u8,
    g: u8,
    b: u8,
) {
    use bootloader_api::info::PixelFormat;
    let bpp = info.bytes_per_pixel;
    if o + bpp > buf.len() || bpp < 3 {
        return;
    }
    match info.pixel_format {
        PixelFormat::Bgr => {
            buf[o] = b;
            buf[o + 1] = g;
            buf[o + 2] = r;
        }
        _ => {
            buf[o] = r;
            buf[o + 1] = g;
            buf[o + 2] = b;
        }
    }
}

/// `osj.blit_rgba`: 1:1 copy of an RGBA image (alpha ignored), clipped to the surface.
pub(super) fn blit_1to1(st: &mut HostState, px: &[u8], w: i32, h: i32, dx: i32, dy: i32) {
    let Some(info) = st.info else { return };
    if st.fb.is_null() {
        return;
    }
    // SAFETY: `fb`/`fb_len` describe the live back surface set by `appd` right before this guest call
    // (see `HostState::set_surface`); only the `appd` thread, which is running this call, touches it.
    let buf = unsafe { core::slice::from_raw_parts_mut(st.fb, st.fb_len) };
    for row in 0..h as i64 {
        let y = dy as i64 + row;
        if y < 0 || y >= st.ch as i64 {
            continue;
        }
        for col in 0..w as i64 {
            let x = dx as i64 + col;
            if x < 0 || x >= st.cw as i64 {
                continue;
            }
            let Some(o) = offset(&info, x, y) else {
                continue;
            };
            let s = ((row * w as i64 + col) * 4) as usize;
            put_px(buf, &info, o, px[s], px[s + 1], px[s + 2]);
        }
    }
}

/// `osj.draw_image_png`: alpha-blended draw, clipped to the surface.
pub(super) fn blit_alpha(st: &HostState, px: &[u8], w: i32, h: i32, dx: i32, dy: i32) {
    let Some(info) = st.info else { return };
    if st.fb.is_null() {
        return;
    }
    // SAFETY: same contract as `blit_1to1` (the `appd` thread is the only one touching the surface).
    let buf = unsafe { core::slice::from_raw_parts_mut(st.fb, st.fb_len) };
    for row in 0..h as i64 {
        let y = dy as i64 + row;
        if y < 0 || y >= st.ch as i64 {
            continue;
        }
        for col in 0..w as i64 {
            let x = dx as i64 + col;
            if x < 0 || x >= st.cw as i64 {
                continue;
            }
            let s = ((row * w as i64 + col) * 4) as usize;
            let a = px[s + 3] as u32;
            if a == 0 {
                continue;
            }
            let Some(o) = offset(&info, x, y) else {
                continue;
            };
            if a == 255 {
                put_px(buf, &info, o, px[s], px[s + 1], px[s + 2]);
                continue;
            }
            // read back the destination in RGB order
            use bootloader_api::info::PixelFormat;
            let (dr, dg, db) = match info.pixel_format {
                PixelFormat::Bgr => (buf[o + 2], buf[o + 1], buf[o]),
                _ => (buf[o], buf[o + 1], buf[o + 2]),
            };
            let mix = |s: u8, d: u8| ((s as u32 * a + d as u32 * (255 - a)) / 255) as u8;
            put_px(
                buf,
                &info,
                o,
                mix(px[s], dr),
                mix(px[s + 1], dg),
                mix(px[s + 2], db),
            );
        }
    }
}
