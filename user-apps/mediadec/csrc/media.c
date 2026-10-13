/* Kitsune mediadec: thin wrappers over libvpx's VP9 decoder and libopus's decoder (media.h). */
#include "media.h"

#include <limits.h>
#include <stdlib.h>
#include <string.h>

#include "opus.h"
#include "vpx/vp8dx.h"
#include "vpx/vpx_decoder.h"

/* ---- VP9 ---- */

struct kmd_vp9 {
    vpx_codec_ctx_t ctx;
    vpx_codec_iter_t iter;
};

kmd_vp9 *kmd_vp9_new(void)
{
    kmd_vp9 *d = calloc(1, sizeof *d);
    if (!d)
        return NULL;
    vpx_codec_dec_cfg_t cfg;
    memset(&cfg, 0, sizeof cfg);
    cfg.threads = 1;
    if (vpx_codec_dec_init(&d->ctx, vpx_codec_vp9_dx(), &cfg, 0) != VPX_CODEC_OK) {
        free(d);
        return NULL;
    }
    return d;
}

int kmd_vp9_decode(kmd_vp9 *d, const uint8_t *data, size_t len)
{
    d->iter = NULL;
    if (len == 0 || len > UINT_MAX)
        return -(int)VPX_CODEC_INVALID_PARAM;
    vpx_codec_err_t e = vpx_codec_decode(&d->ctx, data, (unsigned int)len, NULL, 0);
    return e == VPX_CODEC_OK ? 0 : -(int)e;
}

int kmd_vp9_frame(kmd_vp9 *d, kmd_frame *out)
{
    vpx_image_t *img = vpx_codec_get_frame(&d->ctx, &d->iter);
    if (!img)
        return 0;
    for (int i = 0; i < 3; i++) {
        out->plane[i] = img->planes[i];
        out->stride[i] = img->stride[i];
    }
    out->width = (int32_t)img->d_w;
    out->height = (int32_t)img->d_h;
    out->color_space = (int32_t)img->cs;
    out->color_range = (int32_t)img->range;
    out->format = (int32_t)img->fmt;
    out->bit_depth = (int32_t)img->bit_depth;
    return 1;
}

const char *kmd_vp9_error(kmd_vp9 *d)
{
    const char *detail = vpx_codec_error_detail(&d->ctx);
    return detail ? detail : vpx_codec_error(&d->ctx);
}

void kmd_vp9_free(kmd_vp9 *d)
{
    if (!d)
        return;
    vpx_codec_destroy(&d->ctx);
    free(d);
}

/* ---- Opus ---- */

kmd_opus *kmd_opus_new(void)
{
    int err = 0;
    OpusDecoder *d = opus_decoder_create(48000, 2, &err);
    if (err != OPUS_OK) {
        if (d)
            opus_decoder_destroy(d);
        return NULL;
    }
    return (kmd_opus *)d;
}

int kmd_opus_decode(kmd_opus *d, const uint8_t *data, size_t len, int16_t *pcm, int max_samples)
{
    if (len == 0 || len > INT_MAX)
        return OPUS_BAD_ARG;
    return opus_decode((OpusDecoder *)d, data, (opus_int32)len, pcm, max_samples, 0);
}

void kmd_opus_reset(kmd_opus *d)
{
    opus_decoder_ctl((OpusDecoder *)d, OPUS_RESET_STATE);
}

void kmd_opus_set_gain(kmd_opus *d, int32_t gain_q8)
{
    opus_decoder_ctl((OpusDecoder *)d, OPUS_SET_GAIN(gain_q8));
}

void kmd_opus_free(kmd_opus *d)
{
    if (d)
        opus_decoder_destroy((OpusDecoder *)d);
}
