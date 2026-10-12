/* Kitsune jsrun: the small C library under QuickJS and musl's libm.
 *
 * Everything a freestanding QuickJS needs that the compiler does not provide: strings, the clock and
 * calendar, process end, a few math functions musl's subset lacks, and stubs for the threading calls of
 * `Atomics.wait` (a jsrun process has one thread). Formatted output is in printf.c; memcpy, memmove,
 * memset and memcmp come from Rust's compiler_builtins; malloc and friends from the Rust side
 * (src/bin/jsrun/heap.rs). Calls into Rust (`kjs_*`) take and return only integers and pointers: the
 * Rust target is soft-float, so no float may cross that boundary.
 */
#include <stdint.h>
#include <stdlib.h>
#include <string.h>
#include <stdio.h>
#include <errno.h>
#include <time.h>
#include <sys/time.h>
#include <pthread.h>
#include <math.h>

/* Provided by src/bin/jsrun/sys.rs. */
extern long kjs_write(int fd, const void *buf, size_t len);
extern int64_t kjs_utc_ms(void);
extern int64_t kjs_tz_offset_secs(void);
extern uint64_t kjs_uptime_ms(void);
_Noreturn extern void kjs_exit(int code);

int errno;

/* ---- strings ---- */

size_t strlen(const char *s)
{
    const char *p = s;
    while (*p)
        p++;
    return p - s;
}

size_t strnlen(const char *s, size_t max)
{
    size_t n = 0;
    while (n < max && s[n])
        n++;
    return n;
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

char *strcpy(char *restrict d, const char *restrict s)
{
    char *r = d;
    while ((*d++ = *s++))
        ;
    return r;
}

char *strcat(char *restrict d, const char *restrict s)
{
    strcpy(d + strlen(d), s);
    return d;
}

char *strchr(const char *s, int c)
{
    for (;; s++) {
        if (*s == (char)c)
            return (char *)s;
        if (!*s)
            return NULL;
    }
}

char *strrchr(const char *s, int c)
{
    const char *last = NULL;
    for (;; s++) {
        if (*s == (char)c)
            last = s;
        if (!*s)
            return (char *)last;
    }
}

char *strstr(const char *h, const char *n)
{
    size_t len = strlen(n);
    if (!len)
        return (char *)h;
    for (; *h; h++) {
        if (*h == *n && !strncmp(h, n, len))
            return (char *)h;
    }
    return NULL;
}

void *memchr(const void *s, int c, size_t n)
{
    const unsigned char *p = s;
    for (; n; n--, p++) {
        if (*p == (unsigned char)c)
            return (void *)p;
    }
    return NULL;
}

char *strdup(const char *s)
{
    size_t n = strlen(s) + 1;
    char *d = malloc(n);
    if (d)
        memcpy(d, s, n);
    return d;
}

char *strerror(int err)
{
    switch (err) {
    case ENOMEM: return "out of memory";
    case EINVAL: return "invalid argument";
    case ERANGE: return "result out of range";
    case ETIMEDOUT: return "timed out";
    default: return "error";
    }
}

int abs(int x) { return x < 0 ? -x : x; }
long labs(long x) { return x < 0 ? -x : x; }
long long llabs(long long x) { return x < 0 ? -x : x; }

char *getenv(const char *name)
{
    (void)name;
    return NULL;
}

/* ---- process end ---- */

void exit(int status) { kjs_exit(status); }

void abort(void)
{
    static const char msg[] = "jsrun: abort\n";
    kjs_write(2, msg, sizeof(msg) - 1);
    kjs_exit(134);
}

void __kjs_assert_fail(const char *expr, const char *file, int line, const char *func)
{
    fprintf(stderr, "jsrun: assertion failed: %s (%s:%d, %s)\n", expr, file, line, func);
    kjs_exit(134);
}

/* ---- clock and calendar ---- */

int gettimeofday(struct timeval *tv, void *tz)
{
    (void)tz;
    int64_t ms = kjs_utc_ms();
    tv->tv_sec = ms / 1000;
    tv->tv_usec = (ms % 1000) * 1000;
    return 0;
}

int clock_gettime(clockid_t id, struct timespec *ts)
{
    int64_t ms = id == CLOCK_MONOTONIC ? (int64_t)kjs_uptime_ms() : kjs_utc_ms();
    ts->tv_sec = ms / 1000;
    ts->tv_nsec = (ms % 1000) * 1000000;
    return 0;
}

time_t time(time_t *t)
{
    time_t now = kjs_utc_ms() / 1000;
    if (t)
        *t = now;
    return now;
}

/* Days since 1970-01-01 of a civil date (proleptic Gregorian), and back (Howard Hinnant's algorithms). */
static int64_t days_from_civil(int64_t y, int m, int d)
{
    y -= m <= 2;
    int64_t era = (y >= 0 ? y : y - 399) / 400;
    int64_t yoe = y - era * 400;
    int64_t doy = (153 * (m + (m > 2 ? -3 : 9)) + 2) / 5 + d - 1;
    int64_t doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    return era * 146097 + doe - 719468;
}

static struct tm *split_time(int64_t t, long gmtoff, struct tm *out)
{
    int64_t days = t / 86400, secs = t % 86400;
    if (secs < 0) {
        secs += 86400;
        days--;
    }
    out->tm_hour = secs / 3600;
    out->tm_min = secs / 60 % 60;
    out->tm_sec = secs % 60;
    out->tm_wday = (int)(((days + 4) % 7 + 7) % 7);
    int64_t z = days + 719468;
    int64_t era = (z >= 0 ? z : z - 146096) / 146097;
    int64_t doe = z - era * 146097;
    int64_t yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    int64_t doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    int64_t mp = (5 * doy + 2) / 153;
    int d = (int)(doy - (153 * mp + 2) / 5 + 1);
    int m = (int)(mp < 10 ? mp + 3 : mp - 9);
    int64_t y = yoe + era * 400 + (m <= 2);
    out->tm_year = (int)(y - 1900);
    out->tm_mon = m - 1;
    out->tm_mday = d;
    out->tm_yday = (int)(days - days_from_civil(y, 1, 1));
    out->tm_isdst = 0;
    out->tm_gmtoff = gmtoff;
    out->tm_zone = gmtoff ? "LOCAL" : "UTC";
    return out;
}

struct tm *gmtime_r(const time_t *t, struct tm *out) { return split_time(*t, 0, out); }

/* The zone is one fixed offset (the one the user chose in the settings, no daylight-saving rules). */
struct tm *localtime_r(const time_t *t, struct tm *out)
{
    long off = (long)kjs_tz_offset_secs();
    return split_time(*t + off, off, out);
}

struct tm *gmtime(const time_t *t)
{
    static struct tm tm;
    return gmtime_r(t, &tm);
}

struct tm *localtime(const time_t *t)
{
    static struct tm tm;
    return localtime_r(t, &tm);
}

time_t mktime(struct tm *tm)
{
    int64_t y = tm->tm_year + 1900 + tm->tm_mon / 12;
    int mon = tm->tm_mon % 12;
    if (mon < 0) {
        mon += 12;
        y--;
    }
    int64_t t = days_from_civil(y, mon + 1, 1) + tm->tm_mday - 1;
    t = t * 86400 + tm->tm_hour * 3600 + tm->tm_min * 60 + tm->tm_sec;
    t -= kjs_tz_offset_secs();
    time_t r = t;
    localtime_r(&r, tm);
    return r;
}

/* ---- threads (one per process) ---- */

int pthread_mutex_lock(pthread_mutex_t *m) { (void)m; return 0; }
int pthread_mutex_unlock(pthread_mutex_t *m) { (void)m; return 0; }
int pthread_cond_init(pthread_cond_t *c, const pthread_condattr_t *a) { (void)c; (void)a; return 0; }
int pthread_cond_destroy(pthread_cond_t *c) { (void)c; return 0; }
int pthread_cond_signal(pthread_cond_t *c) { (void)c; return 0; }
/* Nobody else could wake the waiter: report a timeout rather than hang. */
int pthread_cond_wait(pthread_cond_t *c, pthread_mutex_t *m) { (void)c; (void)m; return 0; }
int pthread_cond_timedwait(pthread_cond_t *c, pthread_mutex_t *m, const struct timespec *t)
{
    (void)c;
    (void)m;
    (void)t;
    return ETIMEDOUT;
}

/* ---- math that the musl subset does not carry ---- */

double sqrt(double x) { return __builtin_sqrt(x); } /* one `sqrtsd` (built with -fno-math-errno) */

double ceil(double x) { return -floor(-x); }

double modf(double x, double *ip)
{
    double i = trunc(x);
    *ip = i;
    if (isinf(x))
        return copysign(0.0, x);
    return copysign(x - i, x);
}
