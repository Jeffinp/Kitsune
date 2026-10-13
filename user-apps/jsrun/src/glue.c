/* Kitsune jsrun: the QuickJS side of the program.
 *
 * Rust (src/bin/jsrun.rs) reads the script and calls `kjs_run`; this file makes the runtime and the
 * context, adds `console`, `print` and `performance.now`, evaluates the script (global code with
 * top-level `await` allowed), runs the job queue (promises) until it is empty and reports what was not
 * caught. Most of QuickJS's API is `static inline` in quickjs.h, which is why this layer is C and the
 * boundary with Rust is one call with pointers and integers. The browser's script process (`--page`)
 * is in page.c and shares the helpers declared in glue.h.
 */
#include <stdint.h>
#include <string.h>
#include <stdio.h>
#include "glue.h"

#define countof(x) (sizeof(x) / sizeof((x)[0]))

extern long kjs_write(int fd, const void *buf, size_t len);
extern uint64_t kjs_uptime_ms(void);

/* How much the scripts may use. The process stack is 4 MiB (kernel `BIG_STACK`): QuickJS checks against
   3.75 MiB and the rest is for C code below its checks. The heap may grow to 64 MiB (kernel `BIG_HEAP`). */
#ifndef JS_STACK_LIMIT
#define JS_STACK_LIMIT (3840 * 1024)
#endif
#define JS_MEMORY_LIMIT (48u * 1024 * 1024)

void kjs_write_all(int fd, const char *p, size_t n)
{
    while (n) {
        long w = kjs_write(fd, p, n);
        if (w <= 0)
            return;
        p += w;
        n -= (size_t)w;
    }
}

/* One line of `console.log` is gathered here and written with a single call. */
typedef struct {
    char *buf;
    size_t len, cap;
    char small[256];
} Line;

static void line_put(JSContext *ctx, Line *l, const char *s, size_t n)
{
    if (l->len + n > l->cap) {
        size_t cap = (l->len + n) * 2;
        char *b = js_malloc(ctx, cap);
        if (!b)
            return;
        memcpy(b, l->buf, l->len);
        if (l->buf != l->small)
            js_free(ctx, l->buf);
        l->buf = b;
        l->cap = cap;
    }
    memcpy(l->buf + l->len, s, n);
    l->len += n;
}

/* A value as `console.log` shows it: strings as they are, plain objects and arrays as JSON (close to
   what other engines print), everything else (errors, functions, numbers...) as its string. */
static void put_value(JSContext *ctx, Line *l, JSValueConst v)
{
    JSValue shown = JS_UNDEFINED;
    if (JS_IsObject(v) && !JS_IsFunction(ctx, v) && !JS_IsError(ctx, v)) {
        shown = JS_JSONStringify(ctx, v, JS_UNDEFINED, JS_UNDEFINED);
        if (JS_IsException(shown)) {
            JS_FreeValue(ctx, JS_GetException(ctx)); /* a cycle or a BigInt inside: fall back */
            shown = JS_UNDEFINED;
        }
    }
    size_t n;
    const char *s = JS_ToCStringLen(ctx, &n, JS_IsUndefined(shown) ? v : shown);
    if (s) {
        line_put(ctx, l, s, n);
        JS_FreeCString(ctx, s);
    } else {
        JS_FreeValue(ctx, JS_GetException(ctx));
        line_put(ctx, l, "?", 1);
    }
    JS_FreeValue(ctx, shown);
}

/* console.log / info / debug (magic 1) and console.error / warn (magic 2): the arguments separated by
   spaces, then a newline. */
static JSValue js_console(JSContext *ctx, JSValueConst this_val, int argc, JSValueConst *argv,
                          int magic)
{
    (void)this_val;
    Line l = {0};
    l.buf = l.small;
    l.cap = sizeof(l.small);
    for (int i = 0; i < argc; i++) {
        if (i)
            line_put(ctx, &l, " ", 1);
        put_value(ctx, &l, argv[i]);
    }
    line_put(ctx, &l, "\n", 1);
    kjs_write_all(magic, l.buf, l.len);
    if (l.buf != l.small)
        js_free(ctx, l.buf);
    return JS_UNDEFINED;
}

/* performance.now(): milliseconds since boot (the kernel's clock has millisecond resolution). */
static JSValue js_perf_now(JSContext *ctx, JSValueConst this_val, int argc, JSValueConst *argv)
{
    (void)this_val;
    (void)argc;
    (void)argv;
    return JS_NewFloat64(ctx, (double)kjs_uptime_ms());
}

static const JSCFunctionListEntry console_funcs[] = {
    JS_CFUNC_MAGIC_DEF("log", 1, js_console, 1),
    JS_CFUNC_MAGIC_DEF("info", 1, js_console, 1),
    JS_CFUNC_MAGIC_DEF("debug", 1, js_console, 1),
    JS_CFUNC_MAGIC_DEF("error", 1, js_console, 2),
    JS_CFUNC_MAGIC_DEF("warn", 1, js_console, 2),
};

static const JSCFunctionListEntry perf_funcs[] = {
    JS_CFUNC_DEF("now", 0, js_perf_now),
};

static void add_globals(JSContext *ctx)
{
    JSValue global = JS_GetGlobalObject(ctx);
    JSValue console = JS_NewObject(ctx);
    JS_SetPropertyFunctionList(ctx, console, console_funcs, countof(console_funcs));
    JS_SetPropertyStr(ctx, global, "console", console);
    JS_SetPropertyStr(ctx, global, "print",
                      JS_NewCFunctionMagic(ctx, js_console, "print", 1, JS_CFUNC_generic_magic, 1));
    JSValue perf = JS_NewObject(ctx);
    JS_SetPropertyFunctionList(ctx, perf, perf_funcs, countof(perf_funcs));
    JS_SetPropertyStr(ctx, global, "performance", perf);
    JS_FreeValue(ctx, global);
    /* QuickJS stops runaway recursion with InternalError("stack overflow"), as SpiderMonkey does; V8
       and the pages written for it expect a RangeError. Making InternalError a kind of RangeError lets
       `e instanceof RangeError` hold while the name and message stay QuickJS's. */
    static const char compat[] =
        "Object.setPrototypeOf(InternalError.prototype, RangeError.prototype);"
        "Object.setPrototypeOf(InternalError, RangeError);";
    JS_FreeValue(ctx, JS_Eval(ctx, compat, sizeof(compat) - 1, "<jsrun>", JS_EVAL_TYPE_GLOBAL));
}

/* "Uncaught <error>" and, for an Error, its stack, on the error output. */
void kjs_report(JSContext *ctx, const char *what, JSValueConst err)
{
    /* Gathered into one write: the kernel logs each write of a process as its own lines. */
    Line l = {0};
    l.buf = l.small;
    l.cap = sizeof(l.small);
    line_put(ctx, &l, what, strlen(what));
    line_put(ctx, &l, " ", 1);
    const char *s = JS_ToCString(ctx, err);
    line_put(ctx, &l, s ? s : "?", s ? strlen(s) : 1);
    line_put(ctx, &l, "\n", 1);
    if (s)
        JS_FreeCString(ctx, s);
    if (JS_IsError(ctx, err)) {
        JSValue stack = JS_GetPropertyStr(ctx, err, "stack");
        if (JS_IsString(stack)) {
            const char *st = JS_ToCString(ctx, stack);
            if (st) {
                line_put(ctx, &l, st, strlen(st));
                JS_FreeCString(ctx, st);
            }
        }
        JS_FreeValue(ctx, stack);
    }
    kjs_write_all(2, l.buf, l.len);
    if (l.buf != l.small)
        js_free(ctx, l.buf);
}

void kjs_report_exception(JSContext *ctx, const char *what)
{
    JSValue e = JS_GetException(ctx);
    kjs_report(ctx, what, e);
    JS_FreeValue(ctx, e);
}

/* Promises rejected with nobody to handle them, reported when the queue is empty (a handler may
   still be attached later in the same run). */
#define MAX_REJECTED 16
static struct {
    JSValue promise, reason;
} rejected[MAX_REJECTED];
static int n_rejected;

static void track_rejection(JSContext *ctx, JSValueConst promise, JSValueConst reason,
                            JS_BOOL is_handled, void *opaque)
{
    (void)opaque;
    if (!is_handled) {
        if (n_rejected < MAX_REJECTED) {
            rejected[n_rejected].promise = JS_DupValue(ctx, promise);
            rejected[n_rejected].reason = JS_DupValue(ctx, reason);
            n_rejected++;
        }
        return;
    }
    for (int i = 0; i < n_rejected; i++) {
        if (JS_VALUE_GET_PTR(rejected[i].promise) == JS_VALUE_GET_PTR(promise)) {
            JS_FreeValue(ctx, rejected[i].promise);
            JS_FreeValue(ctx, rejected[i].reason);
            rejected[i] = rejected[--n_rejected];
            return;
        }
    }
}

/* Run the jobs until none is left; returns how many threw. */
int kjs_drain_jobs(JSRuntime *rt)
{
    int failed = 0;
    for (;;) {
        JSContext *jctx;
        int r = JS_ExecutePendingJob(rt, &jctx);
        if (r == 0)
            break;
        if (r < 0) {
            kjs_report_exception(jctx, "Uncaught");
            failed++;
        }
    }
    return failed;
}

/* Report the promises still rejected without a handler (except `except`, the script's own) and
   forget them; returns how many were reported. */
int kjs_report_rejections(JSContext *ctx, JSValueConst except)
{
    int n = 0;
    for (int i = 0; i < n_rejected; i++) {
        if (JS_VALUE_GET_PTR(rejected[i].promise) != JS_VALUE_GET_PTR(except)) {
            kjs_report(ctx, "Uncaught (in promise)", rejected[i].reason);
            n++;
        }
        JS_FreeValue(ctx, rejected[i].promise);
        JS_FreeValue(ctx, rejected[i].reason);
    }
    n_rejected = 0;
    return n;
}

/* A runtime with the limits and a context with the globals; NULL (after saying why) on failure. */
JSContext *kjs_new_context(void)
{
    JSRuntime *rt = JS_NewRuntime();
    if (!rt) {
        fputs("jsrun: cannot create the JavaScript runtime\n", stderr);
        return NULL;
    }
    JS_SetMaxStackSize(rt, JS_STACK_LIMIT);
    JS_SetMemoryLimit(rt, JS_MEMORY_LIMIT);
    JS_SetCanBlock(rt, 0);
    JS_SetHostPromiseRejectionTracker(rt, track_rejection, NULL);
    JSContext *ctx = JS_NewContext(rt);
    if (!ctx) {
        fputs("jsrun: cannot create the JavaScript context\n", stderr);
        JS_FreeRuntime(rt);
        return NULL;
    }
    add_globals(ctx);
    return ctx;
}

/* Evaluate `src` (`len` bytes, with a NUL at src[len]) named `name`. Returns the process status: 0, or
   1 when something was thrown and not caught, 2 when the engine could not start. */
int kjs_run(const char *src, size_t len, const char *name)
{
    JSContext *ctx = kjs_new_context();
    if (!ctx)
        return 2;
    JSRuntime *rt = JS_GetRuntime(ctx);

    int status = 0;
    JSValue result = JS_Eval(ctx, src, len, name, JS_EVAL_TYPE_GLOBAL | JS_EVAL_FLAG_ASYNC);
    if (JS_IsException(result)) {
        kjs_report_exception(ctx, "Uncaught");
        status = 1;
    } else {
        if (kjs_drain_jobs(rt))
            status = 1;
        /* The script's own promise: a rejection is the script's uncaught error. */
        if (JS_PromiseState(ctx, result) == JS_PROMISE_REJECTED) {
            JSValue reason = JS_PromiseResult(ctx, result);
            kjs_report(ctx, "Uncaught", reason);
            JS_FreeValue(ctx, reason);
            status = 1;
        }
    }
    if (kjs_report_rejections(ctx, result))
        status = 1;
    JS_FreeValue(ctx, result);
    /* No JS_FreeContext/JS_FreeRuntime: the process ends next and the kernel takes back all of its
       memory at once, which is faster than freeing every object one by one. */
    return status;
}
