/* Kitsune mediadec libc shim: <math.h>. The decoders are integer (VP9, and Opus in fixed point); the
 * headers that include this only need the names to exist. What is used maps to compiler builtins. */
#ifndef KMD_MATH_H
#define KMD_MATH_H

#define floor(x) __builtin_floor(x)
#define floorf(x) __builtin_floorf(x)
#define ceil(x) __builtin_ceil(x)
#define sqrt(x) __builtin_sqrt(x)
#define sqrtf(x) __builtin_sqrtf(x)
#define fabs(x) __builtin_fabs(x)
#define fabsf(x) __builtin_fabsf(x)

#endif
