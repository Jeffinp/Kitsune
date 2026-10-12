/* Kitsune jsrun libc shim: <errno.h>. One process, one thread: `errno` is a plain global. */
#ifndef KJS_ERRNO_H
#define KJS_ERRNO_H
extern int errno;
#define EPERM 1
#define ENOENT 2
#define EINTR 4
#define EIO 5
#define EBADF 9
#define EAGAIN 11
#define ENOMEM 12
#define EACCES 13
#define EFAULT 14
#define EBUSY 16
#define EEXIST 17
#define EINVAL 22
#define ENOSPC 28
#define EDOM 33
#define ERANGE 34
#define ENOSYS 38
#define ETIMEDOUT 110
#endif
