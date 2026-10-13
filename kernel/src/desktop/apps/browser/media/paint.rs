//! Painting the video surfaces of a page: the picture on screen (or the poster before playing),
//! letterboxed in the element's box, and the controls the browser draws.

use super::*;
use crate::text::{self, Weight};
use kitsune_core::iconart::Glyph;
use kitsune_core::media::controls::{self, Area};
use kitsune_core::web::{Page, VideoBox};

/// The framebuffer stores pixels as B, G, R (learnt at the first paint; the decoder is asked for
/// that order so a picture is copied row by row).
static FB_BGR: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(true);

pub(super) fn fb_bgr() -> bool {
    FB_BGR.load(core::sync::atomic::Ordering::Relaxed)
}

/// Copy an RGBA picture (`sw` x `sh`, bytes R, G, B, A, or B, G, R, A with `src_bgr`) into `dst`
/// (screen coordinates), scaled by nearest neighbour, clipped to the canvas's clip.
///
/// This runs for every picture of a playing video, so the framebuffers QEMU gives (3 or 4 bytes a
/// pixel, R, G, B or B, G, R) go a row at a time: a picture of the box's size in the framebuffer's
/// own layout is one copy per row, anything else a table of source columns (no division per
/// pixel), and a screen row showing the same source row as the one above is a copy of it. Only an
/// odd framebuffer goes pixel by pixel. (The 3-byte framebuffer of QEMU's BIOS mode used to take
/// the pixel-by-pixel path: most of the compositor's time while a video played.)
fn blit_rgba(c: &mut Canvas, src: &[u8], sw: u32, sh: u32, src_bgr: bool, dst: Rect) {
    let (sw, sh) = (sw as usize, sh as usize);
    if sw == 0 || sh == 0 || src.len() < sw * sh * 4 || dst.w <= 0 || dst.h <= 0 {
        return;
    }
    let Some(r) = dst.intersection(&c.clip_rect()) else {
        return;
    };
    let info = c.fb_info();
    let (dw, dh) = (dst.w as usize, dst.h as usize);
    let bpp = info.bytes_per_pixel;
    let fb_bgr = match info.pixel_format {
        bootloader_api::info::PixelFormat::Bgr if bpp == 3 || bpp == 4 => true,
        bootloader_api::info::PixelFormat::Rgb if bpp == 3 || bpp == 4 => false,
        _ => {
            // An odd framebuffer: the slow path.
            for y in r.y..r.bottom() {
                let sy = ((y - dst.y) as usize * sh / dh).min(sh - 1);
                for x in r.x..r.right() {
                    let sx = ((x - dst.x) as usize * sw / dw).min(sw - 1);
                    let i = (sy * sw + sx) * 4;
                    let (red, blue) = if src_bgr {
                        (src[i + 2], src[i])
                    } else {
                        (src[i], src[i + 2])
                    };
                    c.put(x as usize, y as usize, Color::rgb(red, src[i + 1], blue));
                }
            }
            return;
        }
    };
    FB_BGR.store(fb_bgr, core::sync::atomic::Ordering::Relaxed);
    // Swap R and B when the picture's order is not the framebuffer's.
    let (c0, c2) = if fb_bgr != src_bgr { (2, 0) } else { (0, 2) };
    let stride = info.stride;
    // The source column of every screen column, once.
    let cols: Vec<usize> = (r.x..r.right())
        .map(|x| ((x - dst.x) as usize * sw / dw).min(sw - 1))
        .collect();
    let same_cols = sw == dw && r.x == dst.x && cols.len() == sw;
    let len = cols.len() * bpp;
    let buf = c.buffer_mut();
    let mut above: Option<(usize, usize)> = None;
    for y in r.y..r.bottom() {
        let sy = ((y - dst.y) as usize * sh / dh).min(sh - 1);
        let start = (y as usize * stride + r.x as usize) * bpp;
        if buf.len() < start + len {
            continue;
        }
        if let Some((prev_sy, prev_start)) = above
            && prev_sy == sy
        {
            buf.copy_within(prev_start..prev_start + len, start);
            continue;
        }
        above = Some((sy, start));
        let row = &src[sy * sw * 4..(sy + 1) * sw * 4];
        let out = &mut buf[start..start + len];
        if bpp == 4 && c0 == 0 && same_cols {
            // The picture's size and order: the row as it is.
            out.copy_from_slice(row);
        } else if same_cols {
            for (o, p) in out.chunks_exact_mut(bpp).zip(row.as_chunks::<4>().0) {
                o[0] = p[c0];
                o[1] = p[1];
                o[2] = p[c2];
            }
        } else {
            for (o, &sx) in out.chunks_exact_mut(bpp).zip(&cols) {
                let p = &row[sx * 4..sx * 4 + 4];
                o[0] = p[c0];
                o[1] = p[1];
                o[2] = p[c2];
            }
        }
    }
}

/// Copy a decoded picture of the page (`0x00RRGGBB` pixels) into `dst`, scaled.
fn blit_image(c: &mut Canvas, img: &kitsune_core::image::Image, dst: Rect) {
    let (iw, ih) = (img.width(), img.height());
    let Some(r) = dst.intersection(&c.clip_rect()) else {
        return;
    };
    if iw == 0 || ih == 0 || dst.w <= 0 || dst.h <= 0 {
        return;
    }
    let px = img.pixels();
    for y in r.y..r.bottom() {
        let sy = ((y - dst.y) as usize * ih / dst.h as usize).min(ih - 1);
        for x in r.x..r.right() {
            let sx = ((x - dst.x) as usize * iw / dst.w as usize).min(iw - 1);
            let p = px[sy * iw + sx];
            c.put(
                x as usize,
                y as usize,
                Color::rgb((p >> 16) as u8, (p >> 8) as u8, p as u8),
            );
        }
    }
}

/// `a` (relative to the box `r`) in screen coordinates.
fn at(r: Rect, a: Area) -> Rect {
    Rect::new(r.x + a.x, r.y + a.y, a.w, a.h)
}

/// A right-pointing triangle filling `r` (the play sign).
fn triangle(c: &mut Canvas, r: Rect, color: Color) {
    let h = r.h.max(2);
    for dy in 0..h {
        let half = dy.min(h - 1 - dy);
        let w = (half * 2 * r.w / h).max(1);
        c.fill_rect(
            r.x.max(0) as usize,
            (r.y + dy).max(0) as usize,
            w as usize,
            1,
            color,
        );
    }
}

/// Is `clip` (screen coordinates) inside the box of one `<video>` of `page`? Then painting the
/// video is all that is needed there (its box is opaque: black bars and the picture).
pub(crate) fn video_covers(page: &Page, content: Rect, scroll: i32, clip: Rect) -> bool {
    page.videos.iter().any(|v| {
        let r = Rect::new(content.x + v.x, content.y - scroll + v.y, v.w, v.h);
        !v.audio
            && clip.w > 0
            && clip.x >= r.x
            && clip.y >= r.y
            && clip.right() <= r.right()
            && clip.bottom() <= r.bottom()
    })
}

impl Desktop {
    /// Paint every video surface of `page` visible in `content` (scrolled by `scroll`).
    pub(crate) fn paint_videos(
        &self,
        c: &mut Canvas,
        page: &Page,
        content: Rect,
        scroll: i32,
        bs: &BrowserState,
    ) {
        if page.videos.is_empty() {
            return;
        }
        let Some(clip) = content.intersection(&c.clip_rect()) else {
            return;
        };
        let saved = c.set_clip(clip);
        let t = bs.tabs.active();
        let zoom = i32::from(t.zoom.max(1));
        for v in &page.videos {
            let r = Rect::new(content.x + v.x, content.y - scroll + v.y, v.w, v.h);
            if r.intersection(&clip).is_none() {
                continue;
            }
            let mp = t.js.as_ref().and_then(|j| j.media_player(v.uid));
            self.paint_video(c, r, v, mp, bs);
            if v.controls {
                paint_controls(c, r, v.audio, mp, zoom);
            }
        }
        c.restore_clip(saved);
    }

    fn paint_video(
        &self,
        c: &mut Canvas,
        r: Rect,
        v: &VideoBox,
        mp: Option<&MediaPlayer>,
        bs: &BrowserState,
    ) {
        if v.audio {
            c.fill_rrect(
                r,
                (r.h / 2).min(20),
                Corner::Circle,
                Color::rgb(0x2A, 0x2D, 0x34),
                256,
            );
            return;
        }
        c.fill_rect(
            r.x.max(0) as usize,
            r.y.max(0) as usize,
            r.w.max(0) as usize,
            r.h.max(0) as usize,
            Color::rgb(0, 0, 0),
        );
        let frame = mp.and_then(|m| m.player.shown().map(|f| (m, f)));
        let poster = v
            .poster
            .and_then(|i| bs.img_keys.get(i))
            .and_then(|k| k.as_deref())
            .and_then(|k| bs.images.image(k));
        match (frame, poster) {
            (Some((m, f)), p) if m.started || p.is_none() => {
                if f.len() >= proto::FRAME_HEAD {
                    let fw = u32::from_le_bytes([f[12], f[13], f[14], f[15]]);
                    let fh = u32::from_le_bytes([f[16], f[17], f[18], f[19]]);
                    let a = controls::contain(r.w, r.h, fw, fh);
                    blit_rgba(c, &f[proto::FRAME_HEAD..], fw, fh, f[1] == 1, at(r, a));
                }
            }
            (_, Some(img)) => {
                let a = controls::contain(r.w, r.h, img.width() as u32, img.height() as u32);
                blit_image(c, img, at(r, a));
            }
            _ => {}
        }
    }
}

/// The controls of a box `r`: the bar with play/pause, the time, the time bar, mute and volume, and
/// a large play sign in the middle of a video that is not playing.
fn paint_controls(c: &mut Canvas, r: Rect, audio: bool, mp: Option<&MediaPlayer>, zoom: i32) {
    let l = controls::layout(r.w, r.h, zoom);
    let white = Color::rgb(0xFF, 0xFF, 0xFF);
    let st = mp.map(|m| m.player.state());
    let paused = st.is_none_or(|s| s.paused);
    if !audio {
        c.blend_rect(at(r, l.bar), Color::rgb(0, 0, 0), 150);
        if paused && r.h > l.bar.h * 3 {
            // The big play sign.
            let d = (r.h / 4).clamp(24, 72);
            let o = Rect::new(r.x + (r.w - d) / 2, r.y + (r.h - l.bar.h - d) / 2, d, d);
            c.fill_rrect(o, d / 2, Corner::Circle, Color::rgb(0, 0, 0), 140);
            let tw = d * 2 / 5;
            triangle(
                c,
                Rect::new(o.x + (d - tw) / 2 + d / 12, o.y + (d - tw) / 2, tw, tw),
                white,
            );
        }
    }
    // Play / pause.
    let pb = at(r, l.play);
    let s = (pb.h * 2 / 5).max(6);
    let g = Rect::new(pb.x + (pb.w - s) / 2, pb.y + (pb.h - s) / 2, s, s);
    if paused {
        triangle(c, Rect::new(g.x + s / 10, g.y, s, s), white);
    } else {
        let bw = (s / 3).max(2);
        c.fill_rect(
            g.x.max(0) as usize,
            g.y.max(0) as usize,
            bw as usize,
            s as usize,
            white,
        );
        c.fill_rect(
            (g.x + s - bw).max(0) as usize,
            g.y.max(0) as usize,
            bw as usize,
            s as usize,
            white,
        );
    }
    let (pos, dur) = mp.map_or((0, None), |m| {
        (m.player.position(), m.player.state().duration)
    });
    // The time.
    if l.time.w > 0 {
        let px = (12 * zoom / 100).clamp(8, 40) as u16;
        text::draw_centered(
            c,
            at(r, l.time),
            &controls::time_text(pos, dur),
            px,
            Weight::Medium,
            white,
        );
    }
    // The time bar: the track, what played, the knob.
    if l.seek.w > 0 {
        let sb = at(r, l.seek);
        let th = (4 * zoom / 100).max(2);
        let ty = sb.y + (sb.h - th) / 2;
        c.blend_rect(Rect::new(sb.x, ty, sb.w, th), white, 90);
        let done = dur.map_or(0, |d| l.progress_px(pos, d));
        if done > 0 {
            c.fill_rect(
                sb.x.max(0) as usize,
                ty.max(0) as usize,
                done as usize,
                th as usize,
                white,
            );
        }
        let k = (th * 3).max(8);
        c.fill_rrect(
            Rect::new(sb.x + done - k / 2, ty + th / 2 - k / 2, k, k),
            k / 2,
            Corner::Circle,
            white,
            256,
        );
    }
    // Mute and the volume.
    let (vol, muted) = st.map_or((100, false), |s| (s.volume, s.muted));
    let mb = at(r, l.mute);
    let gs = (mb.h * 3 / 5).max(8);
    crate::desktop::kit::ui::draw_glyph(
        c,
        if muted || vol == 0 {
            Glyph::SpeakerOff
        } else {
            Glyph::Speaker
        },
        mb.x + (mb.w - gs) / 2,
        mb.y + (mb.h - gs) / 2,
        gs,
        0xFFFF_FFFF,
    );
    if l.volume.w > 0 {
        let vb = at(r, l.volume);
        let th = (4 * zoom / 100).max(2);
        let ty = vb.y + (vb.h - th) / 2;
        c.blend_rect(Rect::new(vb.x, ty, vb.w, th), white, 90);
        let fill = if muted {
            0
        } else {
            vb.w * i32::from(vol) / 100
        };
        if fill > 0 {
            c.fill_rect(
                vb.x.max(0) as usize,
                ty.max(0) as usize,
                fill as usize,
                th as usize,
                white,
            );
        }
    }
}
