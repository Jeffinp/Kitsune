/* Kitsune jsrun libc shim: <time.h>. The wall clock comes from the kernel (UTC milliseconds, call 9);
   the user's time zone is the difference to the local clock (call 7). See ../libc.c. */
#ifndef KJS_TIME_H
#define KJS_TIME_H
#include <stddef.h>
typedef long time_t;
typedef int clockid_t;
struct timespec {
    time_t tv_sec;
    long tv_nsec;
};
struct tm {
    int tm_sec, tm_min, tm_hour, tm_mday, tm_mon, tm_year, tm_wday, tm_yday, tm_isdst;
    long tm_gmtoff;
    const char *tm_zone;
};
#define CLOCK_REALTIME 0
#define CLOCK_MONOTONIC 1
time_t time(time_t *t);
int clock_gettime(clockid_t id, struct timespec *ts);
struct tm *gmtime_r(const time_t *t, struct tm *out);
struct tm *localtime_r(const time_t *t, struct tm *out);
struct tm *gmtime(const time_t *t);
struct tm *localtime(const time_t *t);
time_t mktime(struct tm *tm);
#endif
