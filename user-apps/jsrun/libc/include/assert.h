/* Kitsune jsrun libc shim: <assert.h>. A failed assertion prints where it failed and ends the process. */
#ifndef KJS_ASSERT_H
#define KJS_ASSERT_H
_Noreturn void __kjs_assert_fail(const char *expr, const char *file, int line, const char *func);
#endif
#undef assert
#ifdef NDEBUG
#define assert(x) ((void)0)
#else
#define assert(x) ((void)((x) || (__kjs_assert_fail(#x, __FILE__, __LINE__, __func__), 0)))
#endif
#ifndef static_assert
#define static_assert _Static_assert
#endif
