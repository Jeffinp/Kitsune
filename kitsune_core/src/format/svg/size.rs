//! The size of a picture: `width`, `height` and `viewBox` of the root element.

use super::*;

/// `(width, height)` of the picture in pixels from the root element, without rendering.
pub fn dims(bytes: &[u8]) -> Option<(usize, usize)> {
    let s = core::str::from_utf8(bytes).ok()?;
    let mut pos = 0;
    while let Some(t) = next_tag(s, &mut pos) {
        if t.name.eq_ignore_ascii_case("svg") && t.open {
            return root_size(&t).map(|(w, h, ..)| (w, h));
        }
    }
    None
}

/// `(width, height, view box)` of the root: the declared size, else the view box, else 300 x 150.
pub(super) fn root_size(t: &Tag) -> Option<(usize, usize, [i64; 4])> {
    let vb = t.get("viewBox").or_else(|| t.get("viewbox")).and_then(|v| {
        let mut s = v.trim();
        let mut out = [0i64; 4];
        for o in &mut out {
            let (n, r) = num(skip_sep(s))?;
            *o = n;
            s = r;
        }
        (out[2] > 0 && out[3] > 0).then_some(out)
    });
    let w = t
        .get("width")
        .and_then(|w| length(w, vb.map_or(300 << 16, |v| v[2])));
    let h = t
        .get("height")
        .and_then(|h| length(h, vb.map_or(150 << 16, |v| v[3])));
    let (w, h) = match (w, h, vb) {
        (Some(w), Some(h), _) => (w, h),
        (Some(w), None, Some(v)) => (w, w * v[3] / v[2].max(1)),
        (None, Some(h), Some(v)) => (h * v[2] / v[3].max(1), h),
        (None, None, Some(v)) => (v[2], v[3]),
        _ => (300 << 16, 150 << 16),
    };
    let (w, h) = (((w + 32768) >> 16) as usize, ((h + 32768) >> 16) as usize);
    if w == 0 || h == 0 || w > 16384 || h > 16384 {
        return None;
    }
    let view = vb.unwrap_or([0, 0, (w as i64) << 16, (h as i64) << 16]);
    Some((w, h, view))
}
