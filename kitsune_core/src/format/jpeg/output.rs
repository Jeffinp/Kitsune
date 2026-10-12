//! From decoded components to pixels (colour conversion, chroma replication) and a cheap size peek.

use super::*;

pub(super) fn to_image(f: &Frame, adobe: Option<u8>) -> Result<Image, JpegError> {
    let (w, h) = (f.width, f.height);
    let mut img = Image::new(w, h, 0xFF00_0000)?;
    let px = img.pixels_mut();
    let sample = |c: &Comp, x: usize, y: usize| -> i32 {
        let sx = x * c.h / f.hmax;
        let sy = y * c.v / f.vmax;
        c.plane[sy * c.pw + sx] as i32
    };
    if f.comps.len() == 1 {
        let c = &f.comps[0];
        for y in 0..h {
            for x in 0..w {
                let g = sample(c, x, y) as u8;
                px[y * w + x] = rgba(g, g, g, 255);
            }
        }
        return Ok(img);
    }
    let rgb = adobe == Some(0);
    for y in 0..h {
        for x in 0..w {
            let a = sample(&f.comps[0], x, y);
            let b = sample(&f.comps[1], x, y);
            let c = sample(&f.comps[2], x, y);
            let (r, g, bl) = if rgb {
                (a, b, c)
            } else {
                let (cb, cr) = (b - 128, c - 128);
                let yy = (a << 16) + 32768;
                (
                    (yy + 91881 * cr) >> 16,
                    (yy - 22554 * cb - 46802 * cr) >> 16,
                    (yy + 116130 * cb) >> 16,
                )
            };
            px[y * w + x] = rgba(
                r.clamp(0, 255) as u8,
                g.clamp(0, 255) as u8,
                bl.clamp(0, 255) as u8,
                255,
            );
        }
    }
    Ok(img)
}

/// Width and height from the first `SOF` marker, without decoding anything. `None` when there is
/// no frame header in the bytes given.
pub fn peek_dims(bytes: &[u8]) -> Option<(usize, usize)> {
    if !is_jpeg(bytes) {
        return None;
    }
    let mut p = 2usize;
    while p + 4 <= bytes.len() {
        if bytes[p] != 0xFF {
            p += 1;
            continue;
        }
        let m = bytes[p + 1];
        if m == 0xFF {
            p += 1;
            continue;
        }
        if m == 0 || m == 0x01 || (0xD0..=0xD8).contains(&m) {
            p += 2;
            continue;
        }
        if (0xC0..=0xCF).contains(&m) && m != 0xC4 && m != 0xC8 && m != 0xCC {
            let h = u16::from_be_bytes([*bytes.get(p + 5)?, *bytes.get(p + 6)?]);
            let w = u16::from_be_bytes([*bytes.get(p + 7)?, *bytes.get(p + 8)?]);
            return Some((usize::from(w), usize::from(h)));
        }
        let len = u16::from_be_bytes([bytes[p + 2], bytes[p + 3]]) as usize;
        p += 2 + len;
    }
    None
}
