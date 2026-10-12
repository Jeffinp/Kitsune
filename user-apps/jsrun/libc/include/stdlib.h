/* Kitsune jsrun libc shim: <stdlib.h>. Only what QuickJS and musl's libm use (see ../libc.c). */
#ifndef KJS_STDLIB_H
#define KJS_STDLIB_H
#include <stddef.h>

#define EXIT_SUCCESS 0
#define EXIT_FAILURE 1
#define RAND_MAX 0x7fffffff

/* The heap is on the Rust side (src/bin/jsrun/heap.rs), over `brk`. */
void *malloc(size_t size);
void *calloc(size_t n, size_t size);
void *realloc(void *ptr, size_t size);
void free(void *ptr);
size_t malloc_usable_size(void *ptr);

_Noreturn void abort(void);
_Noreturn void exit(int status);

int abs(int x);
long labs(long x);
long long llabs(long long x);
int atoi(const char *s);
long strtol(const char *s, char **end, int base);
unsigned long strtoul(const char *s, char **end, int base);
long long strtoll(const char *s, char **end, int base);
unsigned long long strtoull(const char *s, char **end, int base);
void qsort(void *base, size_t n, size_t size, int (*cmp)(const void *, const void *));
char *getenv(const char *name);

#define alloca(n) __builtin_alloca(n)

#endif
