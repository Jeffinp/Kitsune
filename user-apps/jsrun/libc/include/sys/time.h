/* Kitsune jsrun libc shim: <sys/time.h>. */
#ifndef KJS_SYS_TIME_H
#define KJS_SYS_TIME_H
#include <time.h>
struct timeval {
    time_t tv_sec;
    long tv_usec;
};
int gettimeofday(struct timeval *tv, void *tz);
#endif
