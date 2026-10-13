/* Kitsune mediadec: I420 (YUV 4:2:0, 8 bits) to RGBA, in integers, with scaling to the window.
 *
 * The matrix is BT.601 or BT.709 (what the VP9 stream says; YouTube's HD streams are BT.709),
 * studio range (Y 16..235) unless the stream says full range. Coefficients are Q14. When the size is
 * the same the inner loop is a straight conversion (clang vectorises it with SSE2); when it changes,
 * luma is bilinear (Q8 weights, centre-aligned) and chroma is the nearest sample. */
#include "media.h"

#include <stdint.h>

#define MAX_SIDE 4096

typedef struct {
    int32_t yoff, ky, rv, gu, gv, bu;
} matrix;

static const matrix M601_STUDIO = {16, 19077, 26149, -6419, -13320, 33050};
static const matrix M709_STUDIO = {16, 19077, 29372, -3494, -8731, 34610};
static const matrix M601_FULL = {0, 16384, 22970, -5638, -11700, 29032};
static const matrix M709_FULL = {0, 16384, 25802, -3069, -7670, 30402};

static const matrix *pick(const kmd_frame *f)
{
    /* vpx_color_space_t: 2 BT.709, 4 SMPTE-240, 5 BT.2020 (drawn as 709); the rest as 601. */
    int hd = f->color_space == 2 || f->color_space == 4 || f->color_space == 5;
    if (f->color_range == 1)
        return hd ? &M709_FULL : &M601_FULL;
    return hd ? &M709_STUDIO : &M601_STUDIO;
}

static inline uint32_t clamp8(int32_t v)
{
    return v < 0 ? 0 : v > 255 ? 255 : (uint32_t)v;
}

static inline uint32_t pixel(const matrix *m, int32_t y, int32_t u, int32_t v)
{
    int32_t l = (y - m->yoff) * m->ky + (1 << 13);
    u -= 128;
    v -= 128;
    uint32_t r = clamp8((l + m->rv * v) >> 14);
    uint32_t g = clamp8((l + m->gu * u + m->gv * v) >> 14);
    uint32_t b = clamp8((l + m->bu * u) >> 14);
    return r | g << 8 | b << 16 | 0xFF000000u;
}

/* One row at the frame's own size. */
static void row_same(const matrix *m, const uint8_t *y, const uint8_t *u, const uint8_t *v,
                     uint32_t *out, int32_t w)
{
    for (int32_t x = 0; x < w; x++)
        out[x] = pixel(m, y[x], u[x >> 1], v[x >> 1]);
}

/* Source position of output coordinate `i` of `dst` over `src`, centre-aligned, in Q8. */
static inline int32_t src_q8(int32_t i, int32_t src, int32_t dst)
{
    int64_t p = ((int64_t)(2 * i + 1) * src - dst) * 256 / (2 * (int64_t)dst);
    return p < 0 ? 0 : (int32_t)p;
}

void kmd_i420_to_rgba(const kmd_frame *f, uint32_t *dst, int32_t dst_w, int32_t dst_h,
                      int32_t dst_stride)
{
    const int32_t sw = f->width, sh = f->height;
    if (sw <= 0 || sh <= 0 || dst_w <= 0 || dst_h <= 0 || dst_w > MAX_SIDE || dst_h > MAX_SIDE)
        return;
    const matrix *m = pick(f);
    const uint8_t *py = f->plane[0], *pu = f->plane[1], *pv = f->plane[2];
    const int32_t sy = f->stride[0], su = f->stride[1], sv = f->stride[2];

    if (sw == dst_w && sh == dst_h) {
        for (int32_t r = 0; r < sh; r++)
            row_same(m, py + (int64_t)r * sy, pu + (int64_t)(r >> 1) * su,
                     pv + (int64_t)(r >> 1) * sv, dst + (int64_t)r * dst_stride, sw);
        return;
    }

    /* Column tables: luma left sample and Q8 weight, chroma sample. */
    int32_t x0[MAX_SIDE];
    uint8_t xw[MAX_SIDE];
    int32_t xc[MAX_SIDE];
    for (int32_t x = 0; x < dst_w; x++) {
        int32_t p = src_q8(x, sw, dst_w);
        int32_t i = p >> 8;
        if (i >= sw - 1) {
            x0[x] = sw - 1;
            xw[x] = 0;
        } else {
            x0[x] = i;
            xw[x] = (uint8_t)(p & 255);
        }
        int32_t c = p >> 9;
        int32_t cmax = (sw - 1) >> 1;
        xc[x] = c > cmax ? cmax : c;
    }
    for (int32_t r = 0; r < dst_h; r++) {
        int32_t p = src_q8(r, sh, dst_h);
        int32_t y0 = p >> 8, yw = p & 255;
        if (y0 >= sh - 1) {
            y0 = sh - 1;
            yw = 0;
        }
        int32_t y1 = y0 + (yw ? 1 : 0);
        int32_t cy = p >> 9;
        if (cy > (sh - 1) >> 1)
            cy = (sh - 1) >> 1;
        const uint8_t *a = py + (int64_t)y0 * sy, *b = py + (int64_t)y1 * sy;
        const uint8_t *cu = pu + (int64_t)cy * su, *cv = pv + (int64_t)cy * sv;
        uint32_t *out = dst + (int64_t)r * dst_stride;
        for (int32_t x = 0; x < dst_w; x++) {
            int32_t i = x0[x], w = xw[x];
            int32_t j = i + (w ? 1 : 0);
            int32_t top = a[i] * (256 - w) + a[j] * w;
            int32_t bot = b[i] * (256 - w) + b[j] * w;
            int32_t lum = (top * (256 - yw) + bot * yw + 32768) >> 16;
            out[x] = pixel(m, lum, cu[xc[x]], cv[xc[x]]);
        }
    }
}
