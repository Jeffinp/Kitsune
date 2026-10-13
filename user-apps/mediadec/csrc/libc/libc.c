/* Kitsune mediadec: the small C library under libvpx and libopus.
 *
 * Strings, a minimal printf family (libvpx formats its error texts), setjmp/longjmp (libvpx's error
 * path), abort, memcpy and memset (below). memmove/memcmp come from Rust's compiler_builtins, malloc and friends
 * from src/bin/mediadec/heap.rs, output and process end from src/bin/mediadec/sys.rs (`kmd_*`, only
 * integers and pointers: the Rust side is soft-float). */
#include <setjmp.h>
#include <stdarg.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#include <emmintrin.h>

extern long kmd_write(int fd, const void *buf, size_t len);
_Noreturn extern void kmd_exit(int code);

int errno;

struct kmd_file {
    int fd;
};
static struct kmd_file out_file = {1}, err_file = {2};
FILE *const stdout = &out_file;
FILE *const stderr = &err_file;

/* ---- memcpy and memset ----
 *
 * compiler_builtins' versions (weak symbols, so these take their place for the whole program, Rust
 * included) are `rep movs`/`rep stos`, which an emulator runs one element per step: under QEMU TCG
 * memcpy was a sixth of the decoder's time (libvpx copies many small blocks and structures). These
 * move 64, 16, 8 and 4 bytes per step with plain loads and stores. `no_builtin` keeps the compiler
 * from turning the loops back into calls to themselves. */

typedef uint64_t __attribute__((aligned(1), may_alias)) u64_unaligned;
typedef uint32_t __attribute__((aligned(1), may_alias)) u32_unaligned;

__attribute__((no_builtin("memcpy"))) void *memcpy(void *restrict dst, const void *restrict src,
                                                    size_t n)
{
    unsigned char *d = dst;
    const unsigned char *s = src;
    for (; n >= 64; n -= 64, d += 64, s += 64) {
        __m128i a = _mm_loadu_si128((const __m128i *)s);
        __m128i b = _mm_loadu_si128((const __m128i *)(s + 16));
        __m128i c = _mm_loadu_si128((const __m128i *)(s + 32));
        __m128i e = _mm_loadu_si128((const __m128i *)(s + 48));
        _mm_storeu_si128((__m128i *)d, a);
        _mm_storeu_si128((__m128i *)(d + 16), b);
        _mm_storeu_si128((__m128i *)(d + 32), c);
        _mm_storeu_si128((__m128i *)(d + 48), e);
    }
    for (; n >= 16; n -= 16, d += 16, s += 16)
        _mm_storeu_si128((__m128i *)d, _mm_loadu_si128((const __m128i *)s));
    if (n >= 8) {
        *(u64_unaligned *)d = *(const u64_unaligned *)s;
        d += 8, s += 8, n -= 8;
    }
    if (n >= 4) {
        *(u32_unaligned *)d = *(const u32_unaligned *)s;
        d += 4, s += 4, n -= 4;
    }
    while (n--)
        *d++ = *s++;
    return dst;
}

__attribute__((no_builtin("memset"))) void *memset(void *dst, int c, size_t n)
{
    unsigned char *d = dst;
    const __m128i v = _mm_set1_epi8((char)c);
    for (; n >= 16; n -= 16, d += 16)
        _mm_storeu_si128((__m128i *)d, v);
    if (n >= 8) {
        *(u64_unaligned *)d = (uint64_t)_mm_cvtsi128_si64(v);
        d += 8, n -= 8;
    }
    while (n--)
        *d++ = (unsigned char)c;
    return dst;
}

/* ---- strings ---- */

size_t strlen(const char *s)
{
    const char *p = s;
    while (*p)
        p++;
    return (size_t)(p - s);
}

int strcmp(const char *a, const char *b)
{
    while (*a && *a == *b) {
        a++;
        b++;
    }
    return (unsigned char)*a - (unsigned char)*b;
}

int strncmp(const char *a, const char *b, size_t n)
{
    for (; n; n--, a++, b++) {
        if (*a != *b || !*a)
            return (unsigned char)*a - (unsigned char)*b;
    }
    return 0;
}

char *strncpy(char *restrict d, const char *restrict s, size_t n)
{
    size_t i = 0;
    for (; i < n && s[i]; i++)
        d[i] = s[i];
    for (; i < n; i++)
        d[i] = 0;
    return d;
}

/* ---- sorting ---- */

static void swap_bytes(unsigned char *a, unsigned char *b, size_t size)
{
    for (size_t i = 0; i < size; i++) {
        unsigned char t = a[i];
        a[i] = b[i];
        b[i] = t;
    }
}

/* Shell sort (gaps 3x+1): libvpx sorts the few tiles of a frame with it. Not stable, as qsort need
 * not be. */
void qsort(void *base, size_t n, size_t size, int (*cmp)(const void *, const void *))
{
    unsigned char *b = base;
    size_t gap = 1;
    while (gap < n / 3)
        gap = gap * 3 + 1;
    for (; gap > 0; gap /= 3) {
        for (size_t i = gap; i < n; i++) {
            for (size_t j = i; j >= gap && cmp(b + (j - gap) * size, b + j * size) > 0; j -= gap)
                swap_bytes(b + (j - gap) * size, b + j * size, size);
        }
    }
}

/* ---- formatting ---- */

typedef struct {
    char *buf;
    size_t cap, len;
} sink;

static void put(sink *s, char c)
{
    if (s->len + 1 < s->cap)
        s->buf[s->len] = c;
    s->len++;
}

static void put_uint(sink *s, unsigned long long v, unsigned base, int neg)
{
    char tmp[24];
    int n = 0;
    do {
        unsigned d = (unsigned)(v % base);
        tmp[n++] = (char)(d < 10 ? '0' + d : 'a' + d - 10);
        v /= base;
    } while (v);
    if (neg)
        put(s, '-');
    while (n)
        put(s, tmp[--n]);
}

int vsnprintf(char *restrict buf, size_t cap, const char *restrict fmt, va_list ap)
{
    sink s = {buf, cap, 0};
    for (; *fmt; fmt++) {
        if (*fmt != '%') {
            put(&s, *fmt);
            continue;
        }
        fmt++;
        while (*fmt == '-' || *fmt == '0' || (*fmt >= '1' && *fmt <= '9') || *fmt == '.')
            fmt++; /* width and precision are ignored */
        int longs = 0;
        while (*fmt == 'l' || *fmt == 'z' || *fmt == 'h') {
            if (*fmt != 'h')
                longs++;
            fmt++;
        }
        switch (*fmt) {
        case 'd':
        case 'i': {
            long long v = longs ? va_arg(ap, long long) : va_arg(ap, int);
            put_uint(&s, v < 0 ? 0ull - (unsigned long long)v : (unsigned long long)v, 10, v < 0);
            break;
        }
        case 'u':
        case 'x': {
            unsigned long long v = longs ? va_arg(ap, unsigned long long) : va_arg(ap, unsigned);
            put_uint(&s, v, *fmt == 'x' ? 16 : 10, 0);
            break;
        }
        case 'p':
            put(&s, '0');
            put(&s, 'x');
            put_uint(&s, (uintptr_t)va_arg(ap, void *), 16, 0);
            break;
        case 's': {
            const char *str = va_arg(ap, const char *);
            for (str = str ? str : "(null)"; *str; str++)
                put(&s, *str);
            break;
        }
        case 'c':
            put(&s, (char)va_arg(ap, int));
            break;
        case '%':
            put(&s, '%');
            break;
        case 0:
            fmt--;
            break;
        default:
            put(&s, '?');
            break;
        }
    }
    if (cap)
        buf[s.len < cap ? s.len : cap - 1] = 0;
    return (int)s.len;
}

int snprintf(char *restrict buf, size_t cap, const char *restrict fmt, ...)
{
    va_list ap;
    va_start(ap, fmt);
    int n = vsnprintf(buf, cap, fmt, ap);
    va_end(ap);
    return n;
}

static int vfprintf_fd(int fd, const char *fmt, va_list ap)
{
    char line[512];
    int n = vsnprintf(line, sizeof line, fmt, ap);
    size_t len = (size_t)n < sizeof line ? (size_t)n : sizeof line - 1;
    kmd_write(fd, line, len);
    return n;
}

int fprintf(FILE *restrict f, const char *restrict fmt, ...)
{
    va_list ap;
    va_start(ap, fmt);
    int n = vfprintf_fd(f ? f->fd : 2, fmt, ap);
    va_end(ap);
    return n;
}

int printf(const char *restrict fmt, ...)
{
    va_list ap;
    va_start(ap, fmt);
    int n = vfprintf_fd(1, fmt, ap);
    va_end(ap);
    return n;
}

/* ---- process ---- */

_Noreturn void abort(void)
{
    static const char msg[] = "mediadec: abort\n";
    kmd_write(2, msg, sizeof msg - 1);
    kmd_exit(134);
}

_Noreturn void kmd_assert_fail(const char *expr, const char *file, int line)
{
    fprintf(stderr, "mediadec: assertion failed: %s (%s:%d)\n", expr, file, line);
    kmd_exit(134);
}

/* ---- setjmp/longjmp (x86-64 System V; the layout of musl's) ----
 * jmp_buf: rbx, rbp, r12, r13, r14, r15, the stack pointer after return, the return address. */
__asm__(".text\n"
        ".globl setjmp\n"
        ".type setjmp,@function\n"
        "setjmp:\n"
        "  mov %rbx, 0(%rdi)\n"
        "  mov %rbp, 8(%rdi)\n"
        "  mov %r12, 16(%rdi)\n"
        "  mov %r13, 24(%rdi)\n"
        "  mov %r14, 32(%rdi)\n"
        "  mov %r15, 40(%rdi)\n"
        "  lea 8(%rsp), %rdx\n"
        "  mov %rdx, 48(%rdi)\n"
        "  mov (%rsp), %rdx\n"
        "  mov %rdx, 56(%rdi)\n"
        "  xor %eax, %eax\n"
        "  ret\n"
        ".size setjmp, .-setjmp\n"
        ".globl longjmp\n"
        ".type longjmp,@function\n"
        "longjmp:\n"
        "  xor %eax, %eax\n"
        "  cmp $1, %esi\n"
        "  adc %esi, %eax\n" /* eax = val ? val : 1 */
        "  mov 0(%rdi), %rbx\n"
        "  mov 8(%rdi), %rbp\n"
        "  mov 16(%rdi), %r12\n"
        "  mov 24(%rdi), %r13\n"
        "  mov 32(%rdi), %r14\n"
        "  mov 40(%rdi), %r15\n"
        "  mov 48(%rdi), %rsp\n"
        "  jmp *56(%rdi)\n"
        ".size longjmp, .-longjmp\n");
