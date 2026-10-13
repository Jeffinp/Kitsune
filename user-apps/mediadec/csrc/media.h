/* Kitsune mediadec: the C side's interface to Rust (src/bin/mediadec/ffi.rs mirrors it).
 *
 * Only integers and pointers cross: the Rust side is soft-float, the C side may use SSE2. */
#ifndef KITSUNE_MEDIA_H
#define KITSUNE_MEDIA_H

#include <stddef.h>
#include <stdint.h>

/* ---- VP9 (libvpx) ---- */

typedef struct kmd_vp9 kmd_vp9;

/* A decoded picture: 8-bit planes Y, U, V (4:2:0), valid until the next decode call. */
typedef struct kmd_frame {
    const uint8_t *plane[3];
    int32_t stride[3];
    int32_t width, height;
    /* vpx_color_space_t (1: BT.601, 2: BT.709, ...) and vpx_color_range_t (0: studio, 1: full). */
    int32_t color_space, color_range;
    /* VPX_IMG_FMT_*: only I420 (0x102) is shown. */
    int32_t format;
    int32_t bit_depth;
} kmd_frame;

kmd_vp9 *kmd_vp9_new(void);
/* Decode one compressed frame (a WebM block). 0 on success, a negative libvpx error otherwise. */
int kmd_vp9_decode(kmd_vp9 *d, const uint8_t *data, size_t len);
/* The next picture the last decode produced (1), or none (0). */
int kmd_vp9_frame(kmd_vp9 *d, kmd_frame *out);
/* The text of the last error (static or owned by the decoder). */
const char *kmd_vp9_error(kmd_vp9 *d);
void kmd_vp9_free(kmd_vp9 *d);

/* ---- Opus (libopus, fixed point) ---- */

typedef struct kmd_opus kmd_opus;

/* A decoder that always gives stereo 48 kHz (a mono stream is duplicated). NULL on failure. */
kmd_opus *kmd_opus_new(void);
/* Decode one packet into `pcm` (interleaved s16 stereo, room for `max_samples` per channel; 5760
 * covers the longest packet, 120 ms). Returns samples per channel, or a negative libopus error. */
int kmd_opus_decode(kmd_opus *d, const uint8_t *data, size_t len, int16_t *pcm, int max_samples);
/* Forget the decoder state (after a seek). */
void kmd_opus_reset(kmd_opus *d);
/* The output gain of the stream's OpusHead (Q7.8 dB). */
void kmd_opus_set_gain(kmd_opus *d, int32_t gain_q8);
void kmd_opus_free(kmd_opus *d);

/* ---- pixels ---- */

/* Convert `f` to RGBA (bytes R, G, B, 255; with `bgr`, B, G, R, 255) scaled to dst_w x dst_h, rows
 * dst_stride pixels apart. Integer BT.601 or BT.709 (from the frame), studio or full range; nearest
 * neighbour when the size changes. */
void kmd_i420_to_rgba(const kmd_frame *f, uint32_t *dst, int32_t dst_w, int32_t dst_h,
                      int32_t dst_stride, int32_t bgr);

#endif
