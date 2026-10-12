/* Kitsune jsrun: formatted output (printf family) for QuickJS.
 *
 * Conversions: d i u x X o c s p % and f e g (E G), with the flags - 0 + space #, width and precision
 * (also `*`), and the lengths hh h l ll z j t. Floating point goes through QuickJS's own exact
 * dtoa (dtoa.c), so `%f`/`%e`/`%g` print the correctly rounded digits; `%e` writes the exponent the way
 * JavaScript does (`1.5e+2`, no zero padding). QuickJS formats numbers for scripts itself; this is for
 * its error messages, Date strings and debug dumps.
 */
#include <stdint.h>
#include <stdarg.h>
#include <stdio.h>
#include <string.h>
#include "dtoa.h"

extern long kjs_write(int fd, const void *buf, size_t len);

struct KjsFile {
    int fd;
};
static struct KjsFile files[2] = {{1}, {2}};
FILE *const stdout = &files[0];
FILE *const stderr = &files[1];

/* Where formatted text goes: a caller's buffer (snprintf), or a staging buffer flushed to an fd. */
typedef struct {
    char *buf;
    size_t cap; /* bytes of `buf` usable for text (the NUL is extra) */
    size_t pos; /* bytes kept in `buf` */
    size_t total; /* bytes produced, kept or not */
    int fd; /* -1: string; otherwise flush to this fd when full */
} Out;

static void out_flush(Out *o)
{
    if (o->fd >= 0 && o->pos) {
        const char *p = o->buf;
        size_t n = o->pos;
        while (n) {
            long w = kjs_write(o->fd, p, n);
            if (w <= 0)
                break;
            p += w;
            n -= (size_t)w;
        }
        o->pos = 0;
    }
}

static void out_put(Out *o, const char *s, size_t n)
{
    o->total += n;
    while (n) {
        if (o->pos == o->cap) {
            if (o->fd < 0)
                return;
            out_flush(o);
        }
        size_t k = o->cap - o->pos;
        if (k > n)
            k = n;
        memcpy(o->buf + o->pos, s, k);
        o->pos += k;
        s += k;
        n -= k;
    }
}

static void out_pad(Out *o, char c, int n)
{
    char tmp[32];
    memset(tmp, c, sizeof(tmp));
    while (n > 0) {
        int k = n > (int)sizeof(tmp) ? (int)sizeof(tmp) : n;
        out_put(o, tmp, (size_t)k);
        n -= k;
    }
}

enum { F_LEFT = 1, F_ZERO = 2, F_PLUS = 4, F_SPACE = 8, F_ALT = 16 };

/* Write `prefix` (sign or 0x), then `body`, padded to `width` as the flags say. */
static void out_field(Out *o, int flags, int width, const char *prefix, const char *body, int blen)
{
    int plen = (int)strlen(prefix);
    int pad = width - plen - blen;
    if (!(flags & F_LEFT) && !(flags & F_ZERO))
        out_pad(o, ' ', pad);
    out_put(o, prefix, (size_t)plen);
    if (!(flags & F_LEFT) && (flags & F_ZERO))
        out_pad(o, '0', pad);
    out_put(o, body, (size_t)blen);
    if (flags & F_LEFT)
        out_pad(o, ' ', pad);
}

static void fmt_int(Out *o, uint64_t v, int neg, int base, int upper, int flags, int width, int prec)
{
    char tmp[72];
    const char *digits = upper ? "0123456789ABCDEF" : "0123456789abcdef";
    int n = 0;
    while (v) {
        tmp[sizeof(tmp) - 1 - n++] = digits[v % (unsigned)base];
        v /= (unsigned)base;
    }
    if (prec < 0)
        prec = 1;
    else
        flags &= ~F_ZERO;
    while (n < prec && n < 64)
        tmp[sizeof(tmp) - 1 - n++] = '0';
    const char *prefix = neg ? "-" : (flags & F_PLUS) ? "+" : (flags & F_SPACE) ? " " : "";
    if ((flags & F_ALT) && base == 16 && n)
        prefix = upper ? "0X" : "0x";
    out_field(o, flags, width, prefix, tmp + sizeof(tmp) - n, n);
}

static void fmt_float(Out *o, double d, char conv, int flags, int width, int prec)
{
    char tmp[512];
    JSDTOATempMem mem;
    const char *prefix = "";
    int len;
    if (prec < 0)
        prec = 6;
    if (prec > JS_DTOA_MAX_DIGITS - 1)
        prec = JS_DTOA_MAX_DIGITS - 1;
    if (__builtin_signbit(d) && d == d) {
        d = -d;
        prefix = "-";
    } else if (flags & F_PLUS) {
        prefix = "+";
    } else if (flags & F_SPACE) {
        prefix = " ";
    }
    if (d != d || d - d != 0) { /* NaN or infinity (dtoa would spell them the JavaScript way) */
        int upper = conv == 'F' || conv == 'E' || conv == 'G';
        const char *s = d != d ? (upper ? "NAN" : "nan") : (upper ? "INF" : "inf");
        out_field(o, flags & ~F_ZERO, width, prefix, s, 3);
        return;
    }
    /* A huge value in %f would not fit the buffer: show it in %e instead. */
    if ((conv | 32) == 'f' && js_dtoa_max_len(d, 10, prec, JS_DTOA_FORMAT_FRAC) >= (int)sizeof(tmp))
        conv = 'e';
    switch (conv | 32) {
    case 'f':
        len = js_dtoa(tmp, d, 10, prec, JS_DTOA_FORMAT_FRAC, &mem);
        break;
    case 'e':
        len = js_dtoa(tmp, d, 10, prec + 1, JS_DTOA_FORMAT_FIXED | JS_DTOA_EXP_ENABLED, &mem);
        break;
    default: /* g */
        len = js_dtoa(tmp, d, 10, prec ? prec : 1, JS_DTOA_FORMAT_FIXED, &mem);
        if (!(flags & F_ALT) && memchr(tmp, '.', (size_t)len) && !memchr(tmp, 'e', (size_t)len)) {
            while (len > 1 && tmp[len - 1] == '0')
                len--;
            if (tmp[len - 1] == '.')
                len--;
        }
        break;
    }
    out_field(o, flags, width, prefix, tmp, len);
}

static void fmt_core(Out *o, const char *f, va_list ap)
{
    for (; *f; f++) {
        if (*f != '%') {
            const char *s = f;
            while (f[1] && f[1] != '%')
                f++;
            out_put(o, s, (size_t)(f - s + 1));
            continue;
        }
        f++;
        int flags = 0, width = 0, prec = -1, lng = 0;
        for (;; f++) {
            if (*f == '-') flags |= F_LEFT;
            else if (*f == '0') flags |= F_ZERO;
            else if (*f == '+') flags |= F_PLUS;
            else if (*f == ' ') flags |= F_SPACE;
            else if (*f == '#') flags |= F_ALT;
            else break;
        }
        if (*f == '*') {
            width = va_arg(ap, int);
            if (width < 0) {
                flags |= F_LEFT;
                width = -width;
            }
            f++;
        } else {
            while (*f >= '0' && *f <= '9')
                width = width * 10 + (*f++ - '0');
        }
        if (*f == '.') {
            f++;
            prec = 0;
            if (*f == '*') {
                prec = va_arg(ap, int);
                f++;
            } else {
                while (*f >= '0' && *f <= '9')
                    prec = prec * 10 + (*f++ - '0');
            }
        }
        for (;; f++) {
            if (*f == 'l' || *f == 'z' || *f == 'j' || *f == 't') lng = 1; /* all 64-bit on x86_64 */
            else if (*f == 'h') ;
            else break;
        }
        if (flags & F_LEFT)
            flags &= ~F_ZERO;
        switch (*f) {
        case 'd':
        case 'i': {
            int64_t v = lng ? va_arg(ap, int64_t) : va_arg(ap, int);
            fmt_int(o, v < 0 ? -(uint64_t)v : (uint64_t)v, v < 0, 10, 0, flags, width, prec);
            break;
        }
        case 'u':
        case 'x':
        case 'X':
        case 'o': {
            uint64_t v = lng ? va_arg(ap, uint64_t) : va_arg(ap, unsigned);
            int base = *f == 'u' ? 10 : *f == 'o' ? 8 : 16;
            fmt_int(o, v, 0, base, *f == 'X', flags & ~(F_PLUS | F_SPACE), width, prec);
            break;
        }
        case 'p':
            fmt_int(o, (uintptr_t)va_arg(ap, void *), 0, 16, 0, F_ALT | (flags & F_LEFT), width, -1);
            break;
        case 'c': {
            char c = (char)va_arg(ap, int);
            out_field(o, flags & ~F_ZERO, width, "", &c, 1);
            break;
        }
        case 's': {
            const char *s = va_arg(ap, const char *);
            if (!s)
                s = "(null)";
            int n = (int)(prec >= 0 ? strnlen(s, (size_t)prec) : strlen(s));
            out_field(o, flags & ~F_ZERO, width, "", s, n);
            break;
        }
        case 'f':
        case 'F':
        case 'e':
        case 'E':
        case 'g':
        case 'G':
            fmt_float(o, va_arg(ap, double), *f, flags, width, prec);
            break;
        case '%':
            out_put(o, "%", 1);
            break;
        case 0:
            return;
        default: /* unknown conversion: show it as written */
            out_put(o, "%", 1);
            out_put(o, f, 1);
            break;
        }
    }
}

int vsnprintf(char *restrict buf, size_t n, const char *restrict fmt, va_list ap)
{
    Out o = {buf, n ? n - 1 : 0, 0, 0, -1};
    fmt_core(&o, fmt, ap);
    if (n)
        buf[o.pos] = 0;
    return (int)o.total;
}

int snprintf(char *restrict buf, size_t n, const char *restrict fmt, ...)
{
    va_list ap;
    va_start(ap, fmt);
    int r = vsnprintf(buf, n, fmt, ap);
    va_end(ap);
    return r;
}

int vsprintf(char *restrict buf, const char *restrict fmt, va_list ap)
{
    return vsnprintf(buf, (size_t)-1 / 2, fmt, ap);
}

int sprintf(char *restrict buf, const char *restrict fmt, ...)
{
    va_list ap;
    va_start(ap, fmt);
    int r = vsprintf(buf, fmt, ap);
    va_end(ap);
    return r;
}

int vfprintf(FILE *restrict f, const char *restrict fmt, va_list ap)
{
    char stage[512];
    Out o = {stage, sizeof(stage), 0, 0, f->fd};
    fmt_core(&o, fmt, ap);
    out_flush(&o);
    return (int)o.total;
}

int fprintf(FILE *restrict f, const char *restrict fmt, ...)
{
    va_list ap;
    va_start(ap, fmt);
    int r = vfprintf(f, fmt, ap);
    va_end(ap);
    return r;
}

int vprintf(const char *restrict fmt, va_list ap) { return vfprintf(stdout, fmt, ap); }

int printf(const char *restrict fmt, ...)
{
    va_list ap;
    va_start(ap, fmt);
    int r = vfprintf(stdout, fmt, ap);
    va_end(ap);
    return r;
}

size_t fwrite(const void *restrict p, size_t size, size_t n, FILE *restrict f)
{
    size_t total = size * n, done = 0;
    while (done < total) {
        long w = kjs_write(f->fd, (const char *)p + done, total - done);
        if (w <= 0)
            break;
        done += (size_t)w;
    }
    return size ? done / size : 0;
}

int fputs(const char *restrict s, FILE *restrict f)
{
    size_t n = strlen(s);
    return fwrite(s, 1, n, f) == n ? 0 : EOF;
}

int fputc(int c, FILE *f)
{
    char ch = (char)c;
    return fwrite(&ch, 1, 1, f) == 1 ? (unsigned char)c : EOF;
}

int putc(int c, FILE *f) { return fputc(c, f); }
int putchar(int c) { return fputc(c, stdout); }

int puts(const char *s)
{
    if (fputs(s, stdout) == EOF)
        return EOF;
    return fputc('\n', stdout) == EOF ? EOF : 0;
}

int fflush(FILE *f)
{
    (void)f;
    return 0;
}
