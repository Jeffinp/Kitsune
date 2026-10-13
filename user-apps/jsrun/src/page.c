/* Kitsune jsrun --page: the script process of a browser page.
 *
 * The browser starts this process for a page with scripts and talks to it with messages (system calls
 * 56 and 57, `kjs_page_recv` / `kjs_page_send` on the Rust side). A message from the browser is JSON
 * text, or binary body bytes of a network answer (first byte DATA_TAG, then the request id as 4 bytes
 * little endian: kitsune_core::web::script::data_message), handed to the DOM code as an ArrayBuffer. The DOM, the events and the timers
 * are JavaScript (src/bin/jsrun/dom.js, evaluated first); this file is the loop around it:
 *
 *   wait for a message (or until the next timer) -> hand it to the DOM code -> run the tasks that are
 *   due, each with its own time budget, and the promise jobs after each -> send the document if it
 *   changed -> wait again.
 *
 * A task that runs longer than TASK_MS is interrupted (QuickJS's interrupt handler: an uncatchable
 * error), reported, and the timer that started it is cancelled; the process and the page go on. The
 * process is preempted like any other, so even a stuck task never stalls the desktop. Loading a script
 * (compiling it and running its top level) gets LOAD_MS instead: QuickJS compiles every function of a
 * file up front, and a big bundle (YouTube's player, 1.9 MB) takes ~0.5 s native, ~10 s under QEMU
 * without KVM. The interrupt handler stays the same, so a script stuck in a loop still stops.
 */
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include "glue.h"

extern long kjs_page_recv(void *buf, size_t cap, uint64_t timeout_ms);
extern long kjs_page_send(const void *buf, size_t len);
extern void kjs_sleep_ms(uint64_t ms);
extern uint64_t kjs_uptime_ms(void);
extern long kjs_getrandom(void *buf, size_t len);

#define MAX_MSG (4u * 1024 * 1024) /* kitsune_userabi::MAX_PAGE_MSG */
#define TASK_MS 3000               /* the longest one task may run */
#define LOAD_MS 60000              /* the longest one script may take to load (compile + top level) */
#define BATCH_MS 40                /* tasks run back to back before the document is sent */
#define ENOSYS_ (-38)
#define EAGAIN_ (-11)
#define DATA_TAG 1     /* a binary message: body bytes of a network answer */
#define DRAIN_MAX 64   /* messages taken back to back before the tasks run */
#define DRAIN_MS 20
#ifndef SLOW_SCRIPT_MS
#define SLOW_SCRIPT_MS 250 /* a script that runs this long is logged */
#endif

static uint64_t deadline;
static int stopped;

static int interrupt(JSRuntime *rt, void *opaque)
{
    (void)rt;
    (void)opaque;
    if (kjs_uptime_ms() > deadline) {
        stopped = 1;
        return 1;
    }
    return 0;
}

/* A new task: a fresh budget. */
static void begin(void)
{
    deadline = kjs_uptime_ms() + TASK_MS;
    stopped = 0;
}

/* __kjs_native.send(string): one message to the browser (waits while the browser's queue is full). */
static JSValue js_send(JSContext *ctx, JSValueConst this_val, int argc, JSValueConst *argv)
{
    (void)this_val;
    if (argc < 1)
        return JS_NewInt32(ctx, -22);
    size_t n;
    const char *s = JS_ToCStringLen(ctx, &n, argv[0]);
    if (!s)
        return JS_EXCEPTION;
    long r = -22;
    for (int tries = 0; tries < 400; tries++) {
        r = kjs_page_send(s, n);
        if (r != EAGAIN_)
            break;
        kjs_sleep_ms(5);
    }
    JS_FreeCString(ctx, s);
    return JS_NewInt32(ctx, (int32_t)r);
}

/* __kjs_native.sendBytes(head, buffer): one message to the browser made of the string `head` (UTF-8)
   followed by the bytes of the ArrayBuffer `buffer` (a request body). */
static JSValue js_send_bytes(JSContext *ctx, JSValueConst this_val, int argc, JSValueConst *argv)
{
    (void)this_val;
    if (argc < 1)
        return JS_NewInt32(ctx, -22);
    size_t hn, bn = 0;
    uint8_t *bytes = NULL;
    if (argc > 1 && !JS_IsUndefined(argv[1]) && !JS_IsNull(argv[1])) {
        bytes = JS_GetArrayBuffer(ctx, &bn, argv[1]);
        if (!bytes)
            return JS_EXCEPTION;
    }
    const char *h = JS_ToCStringLen(ctx, &hn, argv[0]);
    if (!h)
        return JS_EXCEPTION;
    long r = -7; /* E2BIG */
    if (hn + bn <= MAX_MSG) {
        char *m = malloc(hn + bn + 1);
        if (!m) {
            JS_FreeCString(ctx, h);
            return JS_ThrowOutOfMemory(ctx);
        }
        memcpy(m, h, hn);
        if (bn)
            memcpy(m + hn, bytes, bn);
        for (int tries = 0; tries < 400; tries++) {
            r = kjs_page_send(m, hn + bn);
            if (r != EAGAIN_)
                break;
            kjs_sleep_ms(5);
        }
        free(m);
    }
    JS_FreeCString(ctx, h);
    return JS_NewInt32(ctx, (int32_t)r);
}

/* __kjs_native.random(n): an ArrayBuffer of n random bytes from the kernel (crypto.getRandomValues). */
static JSValue js_random(JSContext *ctx, JSValueConst this_val, int argc, JSValueConst *argv)
{
    (void)this_val;
    int32_t n = 0;
    if (argc < 1 || JS_ToInt32(ctx, &n, argv[0]))
        return JS_EXCEPTION;
    if (n < 0 || n > 65536)
        return JS_ThrowRangeError(ctx, "random: 0 to 65536 bytes");
    uint8_t *b = malloc((size_t)n + 1);
    if (!b)
        return JS_ThrowOutOfMemory(ctx);
    long r = n ? kjs_getrandom(b, (size_t)n) : 0;
    JSValue v = r == n ? JS_NewArrayBufferCopy(ctx, b, (size_t)n) : JS_ThrowInternalError(ctx, "no random bytes");
    free(b);
    return v;
}

/* __kjs_native.evalScript(code, name, load): run a page script as global code. An error it throws is
   reported here (the page goes on, as in a browser) and the result is false; a task stopped for
   running too long is passed on to the loop. `load` (a script of the page loading, not a timer's
   string) gives it LOAD_MS from now; the task's own budget comes back after it. */
static JSValue js_eval_script(JSContext *ctx, JSValueConst this_val, int argc, JSValueConst *argv)
{
    (void)this_val;
    if (argc < 2)
        return JS_FALSE;
    size_t n;
    const char *code = JS_ToCStringLen(ctx, &n, argv[0]);
    if (!code)
        return JS_EXCEPTION;
    const char *name = JS_ToCString(ctx, argv[1]);
    uint64_t t0 = kjs_uptime_ms();
    uint64_t saved = deadline;
    int load = argc > 2 && JS_ToBool(ctx, argv[2]) > 0;
    if (load && t0 + LOAD_MS > deadline)
        deadline = t0 + LOAD_MS;
    JSValue r = JS_Eval(ctx, code, n, name ? name : "<script>", JS_EVAL_TYPE_GLOBAL);
    uint64_t took = kjs_uptime_ms() - t0;
    if (load && !stopped)
        deadline = saved > kjs_uptime_ms() ? saved : kjs_uptime_ms() + TASK_MS / 10;
    /* A slow script is worth a line in the log (a page's big bundles: parse and first run). */
    if (took >= SLOW_SCRIPT_MS)
        fprintf(stderr, "jsrun: %.120s (%zu bytes) ran in %llu ms\n", name ? name : "<script>", n,
                (unsigned long long)took);
    JS_FreeCString(ctx, code);
    if (name)
        JS_FreeCString(ctx, name);
    if (JS_IsException(r)) {
        if (stopped)
            return JS_EXCEPTION;
        kjs_report_exception(ctx, "Uncaught");
        return JS_FALSE;
    }
    JS_FreeValue(ctx, r);
    return JS_TRUE;
}

static const JSCFunctionListEntry native_funcs[] = {
    JS_CFUNC_DEF("send", 1, js_send),
    JS_CFUNC_DEF("evalScript", 3, js_eval_script),
    JS_CFUNC_DEF("sendBytes", 2, js_send_bytes),
    JS_CFUNC_DEF("random", 1, js_random),
};

/* Call `fn` with a fresh budget; an exception is reported and, when the task was stopped for running
   too long, the DOM code is told (`interrupted`). Returns the result as an integer in *out. */
static int call(JSContext *ctx, JSValueConst fn, JSValueConst interrupted, int argc, JSValueConst *argv,
                int64_t *out)
{
    begin();
    JSValue r = JS_Call(ctx, fn, JS_UNDEFINED, argc, argv);
    int bad = JS_IsException(r);
    if (bad) {
        kjs_report_exception(ctx, "Uncaught");
    } else if (out) {
        if (JS_ToInt64(ctx, out, r))
            JS_FreeValue(ctx, JS_GetException(ctx));
    }
    JS_FreeValue(ctx, r);
    kjs_drain_jobs(JS_GetRuntime(ctx));
    if (stopped && !JS_IsUndefined(interrupted)) {
        begin();
        JSValue x = JS_Call(ctx, interrupted, JS_UNDEFINED, 0, NULL);
        if (JS_IsException(x))
            kjs_report_exception(ctx, "Uncaught");
        JS_FreeValue(ctx, x);
    }
    return bad ? -1 : 0;
}

static JSValue get(JSContext *ctx, JSValueConst obj, const char *name)
{
    JSValue v = JS_GetPropertyStr(ctx, obj, name);
    if (!JS_IsFunction(ctx, v)) {
        fprintf(stderr, "jsrun: the page code has no %s\n", name);
        JS_FreeValue(ctx, v);
        return JS_UNDEFINED;
    }
    return v;
}

/* Run the page loop with the DOM code `prelude` (`len` bytes, NUL at prelude[len]). Returns only when
   it cannot start: 2. */
int kjs_page(const char *prelude, size_t len)
{
    JSContext *ctx = kjs_new_context();
    if (!ctx)
        return 2;
    JS_SetInterruptHandler(JS_GetRuntime(ctx), interrupt, NULL);
    JSValue global = JS_GetGlobalObject(ctx);
    JSValue native = JS_NewObject(ctx);
    JS_SetPropertyFunctionList(ctx, native, native_funcs, sizeof(native_funcs) / sizeof(native_funcs[0]));
    JS_SetPropertyStr(ctx, global, "__kjs_native", native);

    begin();
    JSValue r = JS_Eval(ctx, prelude, len, "<kitsune-dom>", JS_EVAL_TYPE_GLOBAL);
    if (JS_IsException(r)) {
        kjs_report_exception(ctx, "jsrun: the page code failed:");
        return 2;
    }
    JS_FreeValue(ctx, r);
    JSValue api = JS_GetPropertyStr(ctx, global, "__kjs");
    JSAtom a1 = JS_NewAtom(ctx, "__kjs"), a2 = JS_NewAtom(ctx, "__kjs_native");
    JS_DeleteProperty(ctx, global, a1, 0);
    JS_DeleteProperty(ctx, global, a2, 0);
    JS_FreeAtom(ctx, a1);
    JS_FreeAtom(ctx, a2);
    JS_FreeValue(ctx, global);
    JSValue on_message = get(ctx, api, "onMessage");
    JSValue run_one = get(ctx, api, "runOne");
    JSValue next_delay = get(ctx, api, "nextDelay");
    JSValue flush = get(ctx, api, "flush");
    JSValue interrupted = get(ctx, api, "interrupted");
    JSValue on_data = get(ctx, api, "onData");
    if (JS_IsUndefined(on_message) || JS_IsUndefined(run_one) || JS_IsUndefined(next_delay) ||
        JS_IsUndefined(flush) || JS_IsUndefined(on_data))
        return 2;

    char *buf = malloc(MAX_MSG + 1);
    if (!buf) {
        fputs("jsrun: out of memory\n", stderr);
        return 2;
    }
    for (;;) {
        int64_t d = -1;
        call(ctx, next_delay, interrupted, 0, NULL, &d);
        long n = kjs_page_recv(buf, MAX_MSG, d < 0 ? UINT64_MAX : (uint64_t)d);
        if (n == ENOSYS_) {
            fputs("jsrun: --page is for the browser's script process\n", stderr);
            return 2;
        }
        /* Take what is waiting back to back (a download arrives as many pieces), then run tasks. */
        uint64_t t0 = kjs_uptime_ms();
        for (int got = 0; n > 0;) {
            if ((unsigned char)buf[0] == DATA_TAG && n >= 5) {
                uint32_t id = (uint32_t)(unsigned char)buf[1] | (uint32_t)(unsigned char)buf[2] << 8 |
                              (uint32_t)(unsigned char)buf[3] << 16 | (uint32_t)(unsigned char)buf[4] << 24;
                JSValue args[2] = {JS_NewUint32(ctx, id),
                                   JS_NewArrayBufferCopy(ctx, (const uint8_t *)buf + 5, (size_t)n - 5)};
                call(ctx, on_data, interrupted, 2, (JSValueConst *)args, NULL);
                JS_FreeValue(ctx, args[1]);
            } else {
                JSValue msg = JS_NewStringLen(ctx, buf, (size_t)n);
                call(ctx, on_message, interrupted, 1, (JSValueConst *)&msg, NULL);
                JS_FreeValue(ctx, msg);
            }
            if (++got >= DRAIN_MAX || kjs_uptime_ms() - t0 > DRAIN_MS)
                break;
            n = kjs_page_recv(buf, MAX_MSG, 0);
        }
        uint64_t start = kjs_uptime_ms();
        for (int k = 0; k < 10000; k++) {
            int64_t ran = 0;
            int bad = call(ctx, run_one, interrupted, 0, NULL, &ran);
            if (!bad && !ran)
                break;
            if (kjs_uptime_ms() - start > BATCH_MS)
                break;
        }
        kjs_report_rejections(ctx, JS_UNDEFINED);
        call(ctx, flush, interrupted, 0, NULL, NULL);
    }
}
