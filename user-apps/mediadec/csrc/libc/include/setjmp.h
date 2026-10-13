/* Kitsune mediadec libc shim: <setjmp.h>. libvpx reports a corrupt frame by longjmp-ing out of the
 * decoder; the implementation (x86-64 System V, the callee-saved registers, the stack and the return
 * address) is in libc.c. */
#ifndef KMD_SETJMP_H
#define KMD_SETJMP_H

typedef unsigned long jmp_buf[8];

__attribute__((returns_twice)) int setjmp(jmp_buf env);
_Noreturn void longjmp(jmp_buf env, int val);

#endif
