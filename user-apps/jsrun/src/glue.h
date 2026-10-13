/* Kitsune jsrun: what glue.c (the engine, console, error reports) shares with page.c (the browser's
 * script process). */
#ifndef KJS_GLUE_H
#define KJS_GLUE_H
#include <stddef.h>
#include "quickjs.h"

/* A runtime with the limits and a context with the globals (`console`, `print`, `performance`). */
JSContext *kjs_new_context(void);
/* Write all of `p` to fd 1 or 2. */
void kjs_write_all(int fd, const char *p, size_t n);
/* "<what> <error>" and its stack on the error output. */
void kjs_report(JSContext *ctx, const char *what, JSValueConst err);
void kjs_report_exception(JSContext *ctx, const char *what);
/* Run the promise jobs until none is left; returns how many threw. */
int kjs_drain_jobs(JSRuntime *rt);
/* Report the promises rejected without a handler (other than `except`); returns how many. */
int kjs_report_rejections(JSContext *ctx, JSValueConst except);

#endif
