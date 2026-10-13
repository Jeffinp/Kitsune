/* Kitsune mediadec libc shim: <stdlib.h>. The allocator is the Rust side's (src/bin/mediadec/heap.rs). */
#ifndef KMD_STDLIB_H
#define KMD_STDLIB_H
#include <stddef.h>

void *malloc(size_t size);
void *calloc(size_t n, size_t size);
void *realloc(void *p, size_t size);
void free(void *p);
_Noreturn void abort(void);
void qsort(void *base, size_t n, size_t size, int (*cmp)(const void *, const void *));

static inline int abs(int x) { return x < 0 ? -x : x; }
static inline long labs(long x) { return x < 0 ? -x : x; }
static inline long long llabs(long long x) { return x < 0 ? -x : x; }

#endif
