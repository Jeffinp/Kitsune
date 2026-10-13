/* Kitsune mediadec: I420 (YUV 4:2:0, 8 bits) to RGBA, in integers, with scaling to the window.
 *
 * The matrix is BT.601 or BT.709 (what the VP9 stream says; YouTube's HD streams are BT.709),
 * studio range (Y 16..235) unless the stream says full range. Coefficients are Q13 and the sums 32
 * bits: SSE2 does 8 pixels at a time with _mm_madd_epi16 (the scalar tail computes the same thing).
 * Scaling is nearest-neighbour in integers: each source row needed is converted once, its pixels
 * picked by a column table, and an output row that maps to the same source row as the one before
 * is a copy. That keeps the cost per output pixel at about one load and one store, which matters
 * most under emulation. */
#include "media.h"

#include <emmintrin.h>
#include <stdint.h>
#include <string.h>

#define MAX_SIDE 4096
#define Q 13

typedef struct {
    int16_t yoff, ky, rv, gu, gv, bu;
} matrix;

static const matrix M601_STUDIO = {16, 9539, 13075, -3209, -6660, 16525};
static const matrix M709_STUDIO = {16, 9539, 14686, -1747, -4365, 17305};
static const matrix M601_FULL = {0, 8192, 11485, -2819, -5850, 14516};
static const matrix M709_FULL = {0, 8192, 12901, -1534, -3835, 15201};

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
    int32_t l = (y - m->yoff) * m->ky + (1 << (Q - 1));
    u -= 128;
    v -= 128;
    uint32_t r = clamp8((l + m->rv * v) >> Q);
    uint32_t g = clamp8((l + m->gu * u + m->gv * v) >> Q);
    uint32_t b = clamp8((l + m->bu * u) >> Q);
    return r | g << 8 | b << 16 | 0xFF000000u;
}

static inline __m128i pair(int16_t lo, int16_t hi)
{
    return _mm_set1_epi32((int32_t)(((uint32_t)(uint16_t)hi << 16) | (uint16_t)lo));
}

/* 32-bit lanes >> Q, packed to 8 unsigned bytes (low half) with saturation (= clamp8). */
static inline __m128i narrow(__m128i lo, __m128i hi)
{
    __m128i w = _mm_packs_epi32(_mm_srai_epi32(lo, Q), _mm_srai_epi32(hi, Q));
    return _mm_packus_epi16(w, w);
}

/* One row of `w` pixels at the frame's own size. */
static void row(const matrix *m, const uint8_t *y, const uint8_t *u, const uint8_t *v,
                uint32_t *out, int32_t w)
{
    const __m128i zero = _mm_setzero_si128();
    const __m128i yoff = _mm_set1_epi16(m->yoff), c128 = _mm_set1_epi16(128);
    const __m128i round = _mm_set1_epi32(1 << (Q - 1));
    const __m128i k_r = pair(m->ky, m->rv), k_b = pair(m->ky, m->bu), k_g = pair(m->ky, m->gu);
    const __m128i k_gv = pair(m->gv, 0), alpha = _mm_set1_epi8((char)0xFF);
    int32_t x = 0;
    for (; x + 8 <= w; x += 8) {
        __m128i Y = _mm_sub_epi16(_mm_unpacklo_epi8(_mm_loadl_epi64((const __m128i *)(y + x)), zero), yoff);
        int32_t ui, vi;
        memcpy(&ui, u + (x >> 1), 4);
        memcpy(&vi, v + (x >> 1), 4);
        __m128i U = _mm_cvtsi32_si128(ui), V = _mm_cvtsi32_si128(vi);
        U = _mm_sub_epi16(_mm_unpacklo_epi8(_mm_unpacklo_epi8(U, U), zero), c128);
        V = _mm_sub_epi16(_mm_unpacklo_epi8(_mm_unpacklo_epi8(V, V), zero), c128);
        __m128i yv_lo = _mm_unpacklo_epi16(Y, V), yv_hi = _mm_unpackhi_epi16(Y, V);
        __m128i yu_lo = _mm_unpacklo_epi16(Y, U), yu_hi = _mm_unpackhi_epi16(Y, U);
        __m128i v0_lo = _mm_unpacklo_epi16(V, zero), v0_hi = _mm_unpackhi_epi16(V, zero);
        __m128i r = narrow(_mm_add_epi32(_mm_madd_epi16(yv_lo, k_r), round),
                           _mm_add_epi32(_mm_madd_epi16(yv_hi, k_r), round));
        __m128i b = narrow(_mm_add_epi32(_mm_madd_epi16(yu_lo, k_b), round),
                           _mm_add_epi32(_mm_madd_epi16(yu_hi, k_b), round));
        __m128i g = narrow(
            _mm_add_epi32(_mm_add_epi32(_mm_madd_epi16(yu_lo, k_g), _mm_madd_epi16(v0_lo, k_gv)), round),
            _mm_add_epi32(_mm_add_epi32(_mm_madd_epi16(yu_hi, k_g), _mm_madd_epi16(v0_hi, k_gv)), round));
        __m128i rg = _mm_unpacklo_epi8(r, g), ba = _mm_unpacklo_epi8(b, alpha);
        _mm_storeu_si128((__m128i *)(out + x), _mm_unpacklo_epi16(rg, ba));
        _mm_storeu_si128((__m128i *)(out + x + 4), _mm_unpackhi_epi16(rg, ba));
    }
    for (; x < w; x++)
        out[x] = pixel(m, y[x], u[x >> 1], v[x >> 1]);
}

/* Source index of output coordinate `i` of `dst` over `src` (centre-aligned, nearest). */
static inline int32_t nearest(int32_t i, int32_t src, int32_t dst)
{
    int32_t p = (int32_t)(((int64_t)(2 * i + 1) * src) / (2 * (int64_t)dst));
    return p >= src ? src - 1 : p;
}

void kmd_i420_to_rgba(const kmd_frame *f, uint32_t *dst, int32_t dst_w, int32_t dst_h,
                      int32_t dst_stride)
{
    const int32_t sw = f->width, sh = f->height;
    if (sw <= 0 || sh <= 0 || sw > MAX_SIDE || sh > MAX_SIDE || dst_w <= 0 || dst_h <= 0 ||
        dst_w > MAX_SIDE || dst_h > MAX_SIDE)
        return;
    const matrix *m = pick(f);
    const uint8_t *py = f->plane[0], *pu = f->plane[1], *pv = f->plane[2];
    const int32_t sy = f->stride[0], su = f->stride[1], sv = f->stride[2];

    if (sw == dst_w && sh == dst_h) {
        for (int32_t r = 0; r < sh; r++)
            row(m, py + (int64_t)r * sy, pu + (int64_t)(r >> 1) * su, pv + (int64_t)(r >> 1) * sv,
                dst + (int64_t)r * dst_stride, sw);
        return;
    }
    int32_t xs[MAX_SIDE];
    uint32_t line[MAX_SIDE + 8];
    for (int32_t x = 0; x < dst_w; x++)
        xs[x] = nearest(x, sw, dst_w);
    int32_t last = -1;
    for (int32_t r = 0; r < dst_h; r++) {
        uint32_t *out = dst + (int64_t)r * dst_stride;
        int32_t s = nearest(r, sh, dst_h);
        if (s == last) {
            memcpy(out, out - dst_stride, (size_t)dst_w * 4);
            continue;
        }
        row(m, py + (int64_t)s * sy, pu + (int64_t)(s >> 1) * su, pv + (int64_t)(s >> 1) * sv,
            line, sw);
        if (sw == dst_w)
            memcpy(out, line, (size_t)dst_w * 4);
        else
            for (int32_t x = 0; x < dst_w; x++)
                out[x] = line[xs[x]];
        last = s;
    }
}
