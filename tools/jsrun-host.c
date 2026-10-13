/* The browser's script process (user-apps/jsrun: QuickJS, glue.c and page.c, untouched) built for the
 * host, so a host program can play the browser (tools/yt-probe.js --engine qjs). The system calls of
 * page.c become pipes: a message is a 4-byte little-endian length and the bytes, on stdin (browser to
 * page) and stdout (page to browser); console output and errors go to stderr.
 *
 *   clang -O2 -DCONFIG_VERSION='"2026-06-04"' -D_GNU_SOURCE -Iuser-apps/jsrun/vendor/quickjs \
 *     -Iuser-apps/jsrun/src tools/jsrun-host.c user-apps/jsrun/src/{glue,page}.c \
 *     user-apps/jsrun/vendor/quickjs/{quickjs,libregexp,libunicode,cutils,dtoa}.c -lm -o jsrun-host
 *   jsrun-host prelude1.js prelude2.js ...      (the files of DOM_JS in jsrun.rs, in order)
 *
 * yt-probe.js builds it by itself. Not part of the system image.
 */
#include <errno.h>
#include <poll.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>
#include <unistd.h>

int kjs_page(const char *prelude, size_t len);

static uint64_t start_ms;

uint64_t kjs_uptime_ms(void)
{
    struct timespec ts;
    clock_gettime(CLOCK_MONOTONIC, &ts);
    return (uint64_t)ts.tv_sec * 1000 + (uint64_t)ts.tv_nsec / 1000000 - start_ms;
}

long kjs_write(int fd, const void *buf, size_t len)
{
    (void)fd;
    return write(2, buf, len);
}

void kjs_sleep_ms(uint64_t ms)
{
    struct timespec ts = {(time_t)(ms / 1000), (long)(ms % 1000) * 1000000};
    nanosleep(&ts, NULL);
}

long kjs_getrandom(void *buf, size_t len)
{
    FILE *f = fopen("/dev/urandom", "rb");
    if (!f)
        return -5;
    size_t n = fread(buf, 1, len, f);
    fclose(f);
    return (long)n;
}

static int read_full(void *p, size_t n)
{
    unsigned char *b = p;
    while (n) {
        ssize_t r = read(0, b, n);
        if (r <= 0) {
            if (r < 0 && errno == EINTR)
                continue;
            return -1;
        }
        b += r;
        n -= (size_t)r;
    }
    return 0;
}

/* One message from the browser (stdin), waiting at most `timeout_ms`; -11 (EAGAIN) when none came.
   The browser closing stdin ends the process. */
long kjs_page_recv(void *buf, size_t cap, uint64_t timeout_ms)
{
    struct pollfd p = {0, POLLIN, 0};
    int t = timeout_ms > 1000000 ? -1 : (int)timeout_ms;
    int r = poll(&p, 1, t);
    if (r <= 0)
        return -11;
    unsigned char h[4];
    if (read_full(h, 4))
        exit(0);
    size_t n = (size_t)h[0] | (size_t)h[1] << 8 | (size_t)h[2] << 16 | (size_t)h[3] << 24;
    if (n > cap) {
        fprintf(stderr, "jsrun-host: message of %zu bytes is too large\n", n);
        exit(3);
    }
    if (read_full(buf, n))
        exit(0);
    return (long)n;
}

long kjs_page_send(const void *buf, size_t len)
{
    unsigned char h[4] = {(unsigned char)len, (unsigned char)(len >> 8), (unsigned char)(len >> 16),
                          (unsigned char)(len >> 24)};
    if (fwrite(h, 1, 4, stdout) != 4 || fwrite(buf, 1, len, stdout) != len)
        exit(0);
    fflush(stdout);
    return 0;
}

int main(int argc, char **argv)
{
    start_ms = 0;
    start_ms = kjs_uptime_ms();
    size_t len = 0, cap = 1 << 20;
    char *src = malloc(cap);
    for (int i = 1; i < argc && src; i++) {
        FILE *f = fopen(argv[i], "rb");
        if (!f) {
            fprintf(stderr, "jsrun-host: %s: %s\n", argv[i], strerror(errno));
            return 2;
        }
        for (;;) {
            if (len + 65536 + 2 > cap)
                src = realloc(src, cap *= 2);
            size_t n = fread(src + len, 1, 65536, f);
            len += n;
            if (n == 0)
                break;
        }
        fclose(f);
        src[len++] = '\n';
    }
    if (!src)
        return 2;
    src[len] = 0;
    return kjs_page(src, len);
}
