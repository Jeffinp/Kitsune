/* Kitsune mediadec: libopus build configuration (what `./configure --enable-fixed-point
 * --disable-float-api` would write, cut to the decoder).
 *
 * Fixed point: no float anywhere in the decoder, so it is the same code whatever the CPU has; the
 * output is 16-bit PCM. VAR_ARRAYS puts the scratch buffers on the stack as C99 arrays (mediadec has
 * a large stack). No assertions, no float API, no deep PLC/DRED/OSCE (the 1.5 neural extensions are
 * off unless configured on). */
#ifndef KITSUNE_OPUS_CONFIG_H
#define KITSUNE_OPUS_CONFIG_H

#define OPUS_BUILD 1
#define FIXED_POINT 1
#define DISABLE_FLOAT_API 1
#define VAR_ARRAYS 1
#define PACKAGE_VERSION "1.5.2"

#endif
