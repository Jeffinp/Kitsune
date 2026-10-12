/* Kitsune jsrun libc shim: <stdio.h>. Output only: `stdout` and `stderr` are the process output
   (write to fd 1 and 2); stdio has no files here. */
#ifndef KJS_STDIO_H
#define KJS_STDIO_H
#include <stddef.h>
#include <stdarg.h>

typedef struct KjsFile FILE;
extern FILE *const stdout;
extern FILE *const stderr;
#define stdout stdout
#define stderr stderr

#define EOF (-1)

int printf(const char *restrict fmt, ...) __attribute__((format(printf, 1, 2)));
int fprintf(FILE *restrict f, const char *restrict fmt, ...) __attribute__((format(printf, 2, 3)));
int sprintf(char *restrict buf, const char *restrict fmt, ...) __attribute__((format(printf, 2, 3)));
int snprintf(char *restrict buf, size_t n, const char *restrict fmt, ...)
    __attribute__((format(printf, 3, 4)));
int vprintf(const char *restrict fmt, va_list ap);
int vfprintf(FILE *restrict f, const char *restrict fmt, va_list ap);
int vsnprintf(char *restrict buf, size_t n, const char *restrict fmt, va_list ap);
int vsprintf(char *restrict buf, const char *restrict fmt, va_list ap);
int putchar(int c);
int puts(const char *s);
int fputc(int c, FILE *f);
int putc(int c, FILE *f);
int fputs(const char *restrict s, FILE *restrict f);
size_t fwrite(const void *restrict p, size_t size, size_t n, FILE *restrict f);
int fflush(FILE *f);

#endif
