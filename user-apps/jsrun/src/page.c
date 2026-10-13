/* Kitsune jsrun --page: the script process of a browser page.
 *
 * The browser starts this process for a page with scripts and talks to it with messages (system calls
 * 56 and 57, `kjs_page_recv` / `kjs_page_send` on the Rust side). The DOM, the events and the timers
 * are JavaScript (src/bin/jsrun/dom.js, evaluated first); this file is the loop around it:
 *
 *   wait for a message (or until the next timer) -> hand it to the DOM code -> run the tasks that are
 *   due, each with its own time budget, and the promise jobs after each -> send the document if it
 *   changed -> wait again.
 *
 * A task that runs longer than TASK_MS is interrupted (QuickJS's interrupt handler: an uncatchable
 * error), reported, and the timer that started it is cancelled; the process and the page go on. The
 * process is preempted like any other, so even a stuck task never stalls the desktop.
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

#define MAX_MSG (4u * 1024 * 1024) /* kitsune_userabi::MAX_PAGE_MSG */
#define TASK_MS 3000               /* the longest one task may run */
#define BATCH_MS 40                /* tasks run back to back before the document is sent */
#define ENOSYS_ (-38)
#define EAGAIN_ (-11)

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

/* __kjs_native.evalScript(code, name): run a page script as global code. An error it throws is
   reported here (the page goes on, as in a browser) and the result is false; a task stopped for
   running too long is passed on to the loop. */
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
    JSValue r = JS_Eval(ctx, code, n, name ? name : "<script>", JS_EVAL_TYPE_GLOBAL);
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
    JS_CFUNC_DEF("evalScript", 2, js_eval_script),
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
    if (JS_IsUndefined(on_message) || JS_IsUndefined(run_one) || JS_IsUndefined(next_delay) ||
        JS_IsUndefined(flush))
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
        if (n > 0) {
            JSValue msg = JS_NewStringLen(ctx, buf, (size_t)n);
            call(ctx, on_message, interrupted, 1, (JSValueConst *)&msg, NULL);
            JS_FreeValue(ctx, msg);
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
