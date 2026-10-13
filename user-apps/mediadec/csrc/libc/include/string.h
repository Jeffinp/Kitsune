/* Kitsune mediadec libc shim: <string.h>. memcpy, memmove, memset and memcmp come from Rust's
 * compiler_builtins; the rest is in libc.c. */
#ifndef KMD_STRING_H
#define KMD_STRING_H
#include <stddef.h>

void *memcpy(void *restrict d, const void *restrict s, size_t n);
void *memmove(void *d, const void *s, size_t n);
void *memset(void *d, int c, size_t n);
int memcmp(const void *a, const void *b, size_t n);
size_t strlen(const char *s);
int strcmp(const char *a, const char *b);
int strncmp(const char *a, const char *b, size_t n);
char *strncpy(char *restrict d, const char *restrict s, size_t n);

#endif
