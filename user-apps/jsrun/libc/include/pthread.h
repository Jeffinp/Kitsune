/* Kitsune jsrun libc shim: <pthread.h>. A jsrun process has one thread, so a mutex never waits and
   `Atomics.wait` cannot block (the runtime is made with `JS_SetCanBlock(rt, 0)`). */
#ifndef KJS_PTHREAD_H
#define KJS_PTHREAD_H
#include <time.h>
typedef struct { int locked; } pthread_mutex_t;
typedef struct { int unused; } pthread_cond_t;
typedef struct { int unused; } pthread_mutexattr_t;
typedef struct { int unused; } pthread_condattr_t;
#define PTHREAD_MUTEX_INITIALIZER { 0 }
#define PTHREAD_COND_INITIALIZER { 0 }
int pthread_mutex_lock(pthread_mutex_t *m);
int pthread_mutex_unlock(pthread_mutex_t *m);
int pthread_cond_init(pthread_cond_t *c, const pthread_condattr_t *a);
int pthread_cond_destroy(pthread_cond_t *c);
int pthread_cond_wait(pthread_cond_t *c, pthread_mutex_t *m);
int pthread_cond_timedwait(pthread_cond_t *c, pthread_mutex_t *m, const struct timespec *t);
int pthread_cond_signal(pthread_cond_t *c);
#endif
