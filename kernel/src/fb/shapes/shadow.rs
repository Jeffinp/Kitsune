//! Drop shadows around a window body.

use super::*;

impl Canvas<'_> {
    /// Soft drop shadow of the rounded rectangle `body` (analytic, separable
    /// profile). Pixels inside `hole` (normally the opaque body minus its corners)
    /// are left alone: the body will overwrite them.
    pub fn draw_shadow(&mut self, body: Rect, sh: Shadow, hole: Rect) {
        if sh.alpha == 0 || body.w <= 0 || body.h <= 0 {
            return;
        }
        let Some((bpp, _)) = self.order() else {
            return;
        };
        let t0 = crate::trace::t();
        let b = sh.blur.max(1);
        let area = Rect::new(
            body.x - b,
            body.y + sh.dy - b,
            body.w + 2 * b,
            body.h + 2 * b,
        );
        let clip = self.clip_rect();
        let Some(vis) = area.intersection(&clip) else {
            return;
        };
        let px_profile = raster::shadow_profile(body.w as usize, b as usize);
        let py_profile = raster::shadow_profile(body.h as usize, b as usize);
        for y in vis.y..vis.bottom() {
            let pyv = py_profile[(y - area.y) as usize] as u32;
            if pyv == 0 {
                continue;
            }
            let pyo = pyv * sh.alpha.min(256);
            // The visible part of the row minus the hole: at most two runs.
            let in_hole = hole.w > 0 && y >= hole.y && y < hole.bottom();
            let runs: [(i32, i32); 2] = if in_hole {
                [
                    (vis.x, hole.x.min(vis.right())),
                    (hole.right().max(vis.x), vis.right()),
                ]
            } else {
                [(vis.x, vis.right()), (0, 0)]
            };
            for (x0, x1) in runs {
                if x1 <= x0 {
                    continue;
                }
                let o = (y as usize * self.info.stride + x0 as usize) * bpp;
                let n = (x1 - x0) as usize;
                let row = &mut self.buf[o..o + n * bpp];
                let base = (x0 - area.x) as usize;
                if bpp == 4 {
                    for (i, p) in row.as_chunks_mut::<4>().0.iter_mut().enumerate() {
                        let a = (px_profile[base + i] as u32 * pyo) >> 16;
                        if a != 0 {
                            let ia = 256 - (a + (a >> 7));
                            p[0] = ((p[0] as u32 * ia) >> 8) as u8;
                            p[1] = ((p[1] as u32 * ia) >> 8) as u8;
                            p[2] = ((p[2] as u32 * ia) >> 8) as u8;
                        }
                    }
                } else {
                    for (i, p) in row.as_chunks_mut::<3>().0.iter_mut().enumerate() {
                        let a = (px_profile[base + i] as u32 * pyo) >> 16;
                        if a != 0 {
                            let ia = 256 - (a + (a >> 7));
                            p[0] = ((p[0] as u32 * ia) >> 8) as u8;
                            p[1] = ((p[1] as u32 * ia) >> 8) as u8;
                            p[2] = ((p[2] as u32 * ia) >> 8) as u8;
                        }
                    }
                }
            }
        }
        crate::trace::prim(crate::trace::Prim::Alpha, t0);
    }
}
