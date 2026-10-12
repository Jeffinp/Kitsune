/* Kitsune jsrun libc shim: <fenv.h>. The rounding mode is always round-to-nearest (the SSE default). */
#ifndef KJS_FENV_H
#define KJS_FENV_H
#define FE_INVALID 1
#define FE_DIVBYZERO 4
#define FE_OVERFLOW 8
#define FE_UNDERFLOW 16
#define FE_INEXACT 32
#define FE_ALL_EXCEPT 61
#define FE_TONEAREST 0
#define FE_DOWNWARD 0x400
#define FE_UPWARD 0x800
#define FE_TOWARDZERO 0xc00
int fegetround(void);
int fesetround(int mode);
#endif
