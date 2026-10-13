/* Kitsune mediadec: VP9's 8-tap sub-pixel convolutions (inter prediction) in SSE2 intrinsics.
 *
 * libvpx's own SSE2 versions are assembly (vpx_subpixel_8t_sse2.asm), which this build leaves out,
 * so these are written for it: the same arithmetic as vpx_dsp/vpx_convolve.c (sum of the 8 taps in
 * 32 bits, + 64, >> 7, clamped to 0..255; the averaging variants then (dst + p + 1) >> 1), hence
 * bit-exact with the C (the MD5 check against ffmpeg's libvpx covers it). They read exactly the
 * bytes the C reads. Only the unscaled case (step 16, what a stream without reference scaling uses)
 * is vectorised; anything else goes to the C. csrc/vpx/vpx_dsp_rtcd.h points vpx_convolve8*,
 * vpx_convolve_avg and vpx_convolve_copy here. */
#include <emmintrin.h>
#include <stddef.h>
#include <stdint.h>
#include <string.h>

#include "./vpx_dsp_rtcd.h"
#include "vpx_dsp/vpx_filter.h"

static inline __m128i load4(const uint8_t *p)
{
    int32_t v;
    memcpy(&v, p, 4);
    return _mm_cvtsi32_si128(v);
}

static inline void store4(uint8_t *p, __m128i v)
{
    int32_t x = _mm_cvtsi128_si32(v);
    memcpy(p, &x, 4);
}

/* The tap pairs (k, k+1) of `f` as 32-bit lanes for _mm_madd_epi16. */
static inline void taps(const int16_t *f, __m128i t[4])
{
    for (int k = 0; k < 4; k++)
        t[k] = _mm_set1_epi32((int32_t)(((uint32_t)(uint16_t)f[2 * k + 1] << 16) |
                                        (uint16_t)f[2 * k]));
}

/* Eight 16-bit rows of samples a[0..7] (a[k] holds, for each output, the k-th tap's sample)
 * filtered to 8 (or the low 4) bytes. */
static inline __m128i filter8(const __m128i a[8], const __m128i t[4])
{
    __m128i lo = _mm_setzero_si128(), hi = _mm_setzero_si128();
    for (int k = 0; k < 4; k++) {
        __m128i x = _mm_unpacklo_epi16(a[2 * k], a[2 * k + 1]);
        __m128i y = _mm_unpackhi_epi16(a[2 * k], a[2 * k + 1]);
        lo = _mm_add_epi32(lo, _mm_madd_epi16(x, t[k]));
        hi = _mm_add_epi32(hi, _mm_madd_epi16(y, t[k]));
    }
    const __m128i round = _mm_set1_epi32(1 << (FILTER_BITS - 1));
    lo = _mm_srai_epi32(_mm_add_epi32(lo, round), FILTER_BITS);
    hi = _mm_srai_epi32(_mm_add_epi32(hi, round), FILTER_BITS);
    __m128i w = _mm_packs_epi32(lo, hi);
    return _mm_packus_epi16(w, w);
}

static void horiz(const uint8_t *src, ptrdiff_t ss, uint8_t *dst, ptrdiff_t ds, const int16_t *f,
                  int w, int h, int avg)
{
    const __m128i zero = _mm_setzero_si128();
    __m128i t[4], a[8];
    taps(f, t);
    src -= SUBPEL_TAPS / 2 - 1;
    for (int y = 0; y < h; y++) {
        int x = 0;
        for (; x + 8 <= w; x += 8) {
            for (int k = 0; k < 8; k++)
                a[k] = _mm_unpacklo_epi8(_mm_loadl_epi64((const __m128i *)(src + x + k)), zero);
            __m128i r = filter8(a, t);
            if (avg)
                r = _mm_avg_epu8(r, _mm_loadl_epi64((const __m128i *)(dst + x)));
            _mm_storel_epi64((__m128i *)(dst + x), r);
        }
        for (; x + 4 <= w; x += 4) {
            for (int k = 0; k < 8; k++)
                a[k] = _mm_unpacklo_epi8(load4(src + x + k), zero);
            __m128i r = filter8(a, t);
            if (avg)
                r = _mm_avg_epu8(r, load4(dst + x));
            store4(dst + x, r);
        }
        for (; x < w; x++) {
            int sum = 0;
            for (int k = 0; k < SUBPEL_TAPS; k++)
                sum += src[x + k] * f[k];
            int v = (sum + (1 << (FILTER_BITS - 1))) >> FILTER_BITS;
            v = v < 0 ? 0 : v > 255 ? 255 : v;
            dst[x] = (uint8_t)(avg ? (dst[x] + v + 1) >> 1 : v);
        }
        src += ss;
        dst += ds;
    }
}

/* The vertical filter goes down a column strip at a time, keeping the eight rows it needs in
 * registers: each output row loads and widens one new row instead of eight (the same bytes read, the
 * same arithmetic, so still bit-exact). Under emulation this is a quarter of the decoder's time. */
static void vert(const uint8_t *src, ptrdiff_t ss, uint8_t *dst, ptrdiff_t ds, const int16_t *f,
                 int w, int h, int avg)
{
    const __m128i zero = _mm_setzero_si128();
    __m128i t[4], a[8];
    taps(f, t);
    src -= ss * (SUBPEL_TAPS / 2 - 1);
    int x = 0;
    for (; x + 8 <= w; x += 8) {
        const uint8_t *s = src + x;
        uint8_t *d = dst + x;
        for (int k = 0; k < 7; k++)
            a[k] = _mm_unpacklo_epi8(_mm_loadl_epi64((const __m128i *)(s + k * ss)), zero);
        for (int y = 0; y < h; y++) {
            a[7] = _mm_unpacklo_epi8(_mm_loadl_epi64((const __m128i *)(s + (y + 7) * ss)), zero);
            __m128i r = filter8(a, t);
            if (avg)
                r = _mm_avg_epu8(r, _mm_loadl_epi64((const __m128i *)d));
            _mm_storel_epi64((__m128i *)d, r);
            for (int k = 0; k < 7; k++)
                a[k] = a[k + 1];
            d += ds;
        }
    }
    for (; x + 4 <= w; x += 4) {
        const uint8_t *s = src + x;
        uint8_t *d = dst + x;
        for (int k = 0; k < 7; k++)
            a[k] = _mm_unpacklo_epi8(load4(s + k * ss), zero);
        for (int y = 0; y < h; y++) {
            a[7] = _mm_unpacklo_epi8(load4(s + (y + 7) * ss), zero);
            __m128i r = filter8(a, t);
            if (avg)
                r = _mm_avg_epu8(r, load4(d));
            store4(d, r);
            for (int k = 0; k < 7; k++)
                a[k] = a[k + 1];
            d += ds;
        }
    }
    for (; x < w; x++) {
        const uint8_t *s = src + x;
        uint8_t *d = dst + x;
        for (int y = 0; y < h; y++) {
            int sum = 0;
            for (int k = 0; k < SUBPEL_TAPS; k++)
                sum += s[(y + k) * ss] * f[k];
            int v = (sum + (1 << (FILTER_BITS - 1))) >> FILTER_BITS;
            v = v < 0 ? 0 : v > 255 ? 255 : v;
            *d = (uint8_t)(avg ? (*d + v + 1) >> 1 : v);
            d += ds;
        }
    }
}

void kmd_convolve8_horiz_sse2(const uint8_t *src, ptrdiff_t ss, uint8_t *dst, ptrdiff_t ds,
                              const InterpKernel *filter, int x0_q4, int x_step_q4, int y0_q4,
                              int y_step_q4, int w, int h)
{
    if (x_step_q4 != 16) {
        vpx_convolve8_horiz_c(src, ss, dst, ds, filter, x0_q4, x_step_q4, y0_q4, y_step_q4, w, h);
        return;
    }
    horiz(src + (x0_q4 >> SUBPEL_BITS), ss, dst, ds, filter[x0_q4 & SUBPEL_MASK], w, h, 0);
}

void kmd_convolve8_avg_horiz_sse2(const uint8_t *src, ptrdiff_t ss, uint8_t *dst, ptrdiff_t ds,
                                  const InterpKernel *filter, int x0_q4, int x_step_q4, int y0_q4,
                                  int y_step_q4, int w, int h)
{
    if (x_step_q4 != 16) {
        vpx_convolve8_avg_horiz_c(src, ss, dst, ds, filter, x0_q4, x_step_q4, y0_q4, y_step_q4, w,
                                  h);
        return;
    }
    horiz(src + (x0_q4 >> SUBPEL_BITS), ss, dst, ds, filter[x0_q4 & SUBPEL_MASK], w, h, 1);
}

void kmd_convolve8_vert_sse2(const uint8_t *src, ptrdiff_t ss, uint8_t *dst, ptrdiff_t ds,
                             const InterpKernel *filter, int x0_q4, int x_step_q4, int y0_q4,
                             int y_step_q4, int w, int h)
{
    if (y_step_q4 != 16) {
        vpx_convolve8_vert_c(src, ss, dst, ds, filter, x0_q4, x_step_q4, y0_q4, y_step_q4, w, h);
        return;
    }
    vert(src + (y0_q4 >> SUBPEL_BITS) * ss, ss, dst, ds, filter[y0_q4 & SUBPEL_MASK], w, h, 0);
}

void kmd_convolve8_avg_vert_sse2(const uint8_t *src, ptrdiff_t ss, uint8_t *dst, ptrdiff_t ds,
                                 const InterpKernel *filter, int x0_q4, int x_step_q4, int y0_q4,
                                 int y_step_q4, int w, int h)
{
    if (y_step_q4 != 16) {
        vpx_convolve8_avg_vert_c(src, ss, dst, ds, filter, x0_q4, x_step_q4, y0_q4, y_step_q4, w,
                                 h);
        return;
    }
    vert(src + (y0_q4 >> SUBPEL_BITS) * ss, ss, dst, ds, filter[y0_q4 & SUBPEL_MASK], w, h, 1);
}

/* Horizontal into a 64-wide buffer, then vertical out of it, as vpx_convolve8_c does. */
void kmd_convolve8_sse2(const uint8_t *src, ptrdiff_t ss, uint8_t *dst, ptrdiff_t ds,
                        const InterpKernel *filter, int x0_q4, int x_step_q4, int y0_q4,
                        int y_step_q4, int w, int h)
{
    if (x_step_q4 != 16 || y_step_q4 != 16 || w > 64 || h > 64) {
        vpx_convolve8_c(src, ss, dst, ds, filter, x0_q4, x_step_q4, y0_q4, y_step_q4, w, h);
        return;
    }
    _Alignas(16) uint8_t temp[64 * 135];
    const int rows = (((h - 1) * y_step_q4 + y0_q4) >> SUBPEL_BITS) + SUBPEL_TAPS;
    horiz(src - ss * (SUBPEL_TAPS / 2 - 1) + (x0_q4 >> SUBPEL_BITS), ss, temp, 64,
          filter[x0_q4 & SUBPEL_MASK], w, rows, 0);
    vert(temp + 64 * (SUBPEL_TAPS / 2 - 1) + (y0_q4 >> SUBPEL_BITS) * 64, 64, dst, ds,
         filter[y0_q4 & SUBPEL_MASK], w, h, 0);
}

void kmd_convolve8_avg_sse2(const uint8_t *src, ptrdiff_t ss, uint8_t *dst, ptrdiff_t ds,
                            const InterpKernel *filter, int x0_q4, int x_step_q4, int y0_q4,
                            int y_step_q4, int w, int h)
{
    if (x_step_q4 != 16 || y_step_q4 != 16 || w > 64 || h > 64) {
        vpx_convolve8_avg_c(src, ss, dst, ds, filter, x0_q4, x_step_q4, y0_q4, y_step_q4, w, h);
        return;
    }
    _Alignas(16) uint8_t temp[64 * 64];
    kmd_convolve8_sse2(src, ss, temp, 64, filter, x0_q4, x_step_q4, y0_q4, y_step_q4, w, h);
    kmd_convolve_avg_sse2(temp, 64, dst, ds, NULL, 0, 0, 0, 0, w, h);
}

void kmd_convolve_avg_sse2(const uint8_t *src, ptrdiff_t ss, uint8_t *dst, ptrdiff_t ds,
                           const InterpKernel *filter, int x0_q4, int x_step_q4, int y0_q4,
                           int y_step_q4, int w, int h)
{
    (void)filter;
    (void)x0_q4;
    (void)x_step_q4;
    (void)y0_q4;
    (void)y_step_q4;
    for (int y = 0; y < h; y++) {
        int x = 0;
        for (; x + 16 <= w; x += 16) {
            __m128i a = _mm_loadu_si128((const __m128i *)(src + x));
            __m128i b = _mm_loadu_si128((const __m128i *)(dst + x));
            _mm_storeu_si128((__m128i *)(dst + x), _mm_avg_epu8(a, b));
        }
        for (; x + 8 <= w; x += 8) {
            __m128i a = _mm_loadl_epi64((const __m128i *)(src + x));
            __m128i b = _mm_loadl_epi64((const __m128i *)(dst + x));
            _mm_storel_epi64((__m128i *)(dst + x), _mm_avg_epu8(a, b));
        }
        for (; x < w; x++)
            dst[x] = (uint8_t)((dst[x] + src[x] + 1) >> 1);
        src += ss;
        dst += ds;
    }
}

/* A block copied as it is (a whole-pixel motion vector): 16 or 8 bytes at a time instead of a memcpy
 * call per row (each one a `rep movs` that an emulator runs byte by byte). */
void kmd_convolve_copy_sse2(const uint8_t *src, ptrdiff_t ss, uint8_t *dst, ptrdiff_t ds,
                            const InterpKernel *filter, int x0_q4, int x_step_q4, int y0_q4,
                            int y_step_q4, int w, int h)
{
    (void)filter;
    (void)x0_q4;
    (void)x_step_q4;
    (void)y0_q4;
    (void)y_step_q4;
    for (int y = 0; y < h; y++) {
        int x = 0;
        for (; x + 16 <= w; x += 16)
            _mm_storeu_si128((__m128i *)(dst + x), _mm_loadu_si128((const __m128i *)(src + x)));
        for (; x + 8 <= w; x += 8)
            _mm_storel_epi64((__m128i *)(dst + x), _mm_loadl_epi64((const __m128i *)(src + x)));
        for (; x + 4 <= w; x += 4)
            store4(dst + x, load4(src + x));
        for (; x < w; x++)
            dst[x] = src[x];
        src += ss;
        dst += ds;
    }
}
