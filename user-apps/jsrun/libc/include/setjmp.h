/* Kitsune jsrun libc shim: <setjmp.h>. dtoa.c includes it without using it, so it only has the type. */
#ifndef KJS_SETJMP_H
#define KJS_SETJMP_H
typedef unsigned long jmp_buf[8];
#endif
