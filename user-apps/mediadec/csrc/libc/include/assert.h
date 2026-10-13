/* Kitsune mediadec libc shim: <assert.h>. The codecs are built with NDEBUG, as their release builds
 * are; without it a failed assertion ends the process (never the system). */
#undef assert
#ifdef NDEBUG
#define assert(x) ((void)0)
#else
_Noreturn void kmd_assert_fail(const char *expr, const char *file, int line);
#define assert(x) ((x) ? (void)0 : kmd_assert_fail(#x, __FILE__, __LINE__))
#endif
#ifndef static_assert
#define static_assert _Static_assert
#endif
