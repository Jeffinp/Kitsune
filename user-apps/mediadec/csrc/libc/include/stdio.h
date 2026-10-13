/* Kitsune mediadec libc shim: <stdio.h>. Only the formatting libvpx uses for its error texts
 * (`snprintf`, `vsnprintf`: %d %i %u %x %s %c %% with l/ll/z sizes) and a `fprintf` that writes to the
 * process output. */
#ifndef KMD_STDIO_H
#define KMD_STDIO_H
#include <stdarg.h>
#include <stddef.h>

typedef struct kmd_file FILE;
extern FILE *const stderr;
extern FILE *const stdout;

int vsnprintf(char *restrict buf, size_t cap, const char *restrict fmt, va_list ap);
int snprintf(char *restrict buf, size_t cap, const char *restrict fmt, ...);
int fprintf(FILE *restrict f, const char *restrict fmt, ...);
int printf(const char *restrict fmt, ...);

#endif
