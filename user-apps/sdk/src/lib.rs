//! Kitsune user SDK: the ring 3 side of the system-call interface.
//!
//! ```ignore
//! #![no_std]
//! #![no_main]
//! use kitsune_user::*;
//!
//! entry!(main);
//! fn main() -> i32 {
//!     println!("hello from pid {}", getpid());
//!     0
//! }
//! ```
//!
//! The calls are those of `kernel/src/userproc.rs`. A negative return is an error (`-errno`).

#![no_std]

use core::fmt;

pub mod fs;
pub use kitsune_userabi as abi;

/// System call numbers.
mod nr {
    pub const EXIT: u64 = 0;
    pub const WRITE: u64 = 1;
    pub const YIELD: u64 = 2;
    pub const GETPID: u64 = 3;
    pub const SLEEP_MS: u64 = 4;
    pub const UPTIME_MS: u64 = 5;
    pub const BRK: u64 = 6;
}

/// A raw three-argument system call.
///
/// # Safety
/// `n` must be a defined call and the arguments must satisfy what it expects (pointers that are
/// valid for the length given); the kernel checks them, but a wrong call is still your bug.
#[inline]
pub unsafe fn syscall3(n: u64, a: u64, b: u64, c: u64) -> i64 {
    let ret: i64;
    // SAFETY: `int 0x80` enters the kernel, which preserves every register but `rax`.
    unsafe {
        core::arch::asm!(
            "int 0x80",
            inlateout("rax") n as i64 => ret,
            in("rdi") a,
            in("rsi") b,
            in("rdx") c,
            options(nostack),
        );
    }
    ret
}

/// A raw five-argument system call (`rdi`, `rsi`, `rdx`, `r10`, `r8`).
///
/// # Safety
/// As [`syscall3`].
#[inline]
pub unsafe fn syscall5(n: u64, a: u64, b: u64, c: u64, d: u64, e: u64) -> i64 {
    let ret: i64;
    // SAFETY: `int 0x80` enters the kernel, which preserves every register but `rax`.
    unsafe {
        core::arch::asm!(
            "int 0x80",
            inlateout("rax") n as i64 => ret,
            in("rdi") a,
            in("rsi") b,
            in("rdx") c,
            in("r10") d,
            in("r8") e,
            options(nostack),
        );
    }
    ret
}

/// End the process with `code`.
pub fn exit(code: i32) -> ! {
    // SAFETY: `exit` takes only a status and never returns.
    unsafe { syscall3(nr::EXIT, code as u64, 0, 0) };
    loop {
        core::hint::spin_loop();
    }
}

/// Write bytes to the process's output (`fd` 1 or 2). Returns how many were taken, or `-errno`.
pub fn write(fd: u64, data: &[u8]) -> i64 {
    // SAFETY: the pointer and length describe `data`.
    unsafe { syscall3(nr::WRITE, fd, data.as_ptr() as u64, data.len() as u64) }
}

/// Give the CPU to another thread now.
pub fn yield_now() {
    // SAFETY: no arguments.
    unsafe { syscall3(nr::YIELD, 0, 0, 0) };
}

/// The process id.
pub fn getpid() -> u32 {
    // SAFETY: no arguments.
    unsafe { syscall3(nr::GETPID, 0, 0, 0) as u32 }
}

/// Sleep for about `ms` milliseconds (up to a minute).
pub fn sleep_ms(ms: u64) {
    // SAFETY: one plain argument.
    unsafe { syscall3(nr::SLEEP_MS, ms, 0, 0) };
}

/// Milliseconds since the system booted.
pub fn uptime_ms() -> u64 {
    // SAFETY: no arguments.
    unsafe { syscall3(nr::UPTIME_MS, 0, 0, 0) as u64 }
}

/// Grow the heap to at least `end` (a byte address, `0` = just ask). Returns the end of the heap
/// after the call; the heap starts at [`HEAP_BASE`] and can reach 8 MiB. It never shrinks.
pub fn brk(end: u64) -> u64 {
    // SAFETY: one plain argument.
    unsafe { syscall3(nr::BRK, end, 0, 0) as u64 }
}

/// Where the heap begins.
pub const HEAP_BASE: u64 = 0x1000_0000;

/// `fmt::Write` over `write(1, ...)`.
pub struct Stdout;

impl fmt::Write for Stdout {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        let mut rest = s.as_bytes();
        while !rest.is_empty() {
            let n = write(1, &rest[..rest.len().min(1024)]);
            if n <= 0 {
                return Err(fmt::Error);
            }
            rest = &rest[n as usize..];
        }
        Ok(())
    }
}

/// Collects a whole `print!` into one buffer and writes it with a single call (a formatted line is
/// many small pieces; one call per piece would be many system calls and many log lines).
struct Line {
    buf: [u8; 512],
    len: usize,
}

impl fmt::Write for Line {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        let mut rest = s.as_bytes();
        while !rest.is_empty() {
            if self.len == self.buf.len() {
                self.flush();
            }
            let n = rest.len().min(self.buf.len() - self.len);
            self.buf[self.len..self.len + n].copy_from_slice(&rest[..n]);
            self.len += n;
            rest = &rest[n..];
        }
        Ok(())
    }
}

impl Line {
    fn flush(&mut self) {
        let mut rest = &self.buf[..self.len];
        while !rest.is_empty() {
            let n = write(1, rest);
            if n <= 0 {
                break;
            }
            rest = &rest[n as usize..];
        }
        self.len = 0;
    }
}

#[doc(hidden)]
pub fn _print(args: fmt::Arguments<'_>) {
    let mut line = Line {
        buf: [0; 512],
        len: 0,
    };
    let _ = fmt::Write::write_fmt(&mut line, args);
    line.flush();
}

/// Print to the process output.
#[macro_export]
macro_rules! print {
    ($($arg:tt)*) => { $crate::_print(format_args!($($arg)*)) };
}

/// Print a line to the process output.
#[macro_export]
macro_rules! println {
    () => { $crate::print!("\n") };
    ($($arg:tt)*) => { $crate::print!("{}\n", format_args!($($arg)*)) };
}

/// Declare the program's entry: `entry!(main)` with `fn main() -> i32` defines `_start`, which
/// calls it and exits with its value.
#[macro_export]
macro_rules! entry {
    ($main:path) => {
        #[unsafe(no_mangle)]
        pub extern "C" fn _start() -> ! {
            let code: i32 = $main();
            $crate::exit(code)
        }
    };
}

#[cfg(not(test))]
#[panic_handler]
fn panic(info: &core::panic::PanicInfo<'_>) -> ! {
    _print(format_args!("panic: {info}\n"));
    exit(101)
}
