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
#[cfg(feature = "heap")]
mod heap;
pub mod win;
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
    pub const TIME: u64 = 7;
    pub const LANG: u64 = 8;
    pub const UNIX_MS: u64 = 9;
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

/// Seconds since 1970-01-01 on the wall clock, already in the time zone the user chose (`0` when the
/// system has no clock).
pub fn local_time_secs() -> i64 {
    // SAFETY: no arguments.
    unsafe { syscall3(nr::TIME, 0, 0, 0) }
}

/// Milliseconds since 1970-01-01 in UTC (SNTP-corrected when the network gave the time; `0` when the
/// system has no clock). This is what `Date.now()` is; [`local_time_secs`] minus it is the time zone.
pub fn unix_time_ms() -> i64 {
    // SAFETY: no arguments.
    unsafe { syscall3(nr::UNIX_MS, 0, 0, 0) }
}

/// The language of the interface (`0` Portuguese, `1` English), as the user chose it.
pub fn language() -> u8 {
    // SAFETY: no arguments.
    unsafe { syscall3(nr::LANG, 0, 0, 0) as u8 }
}

/// Grow the heap to at least `end` (a byte address, `0` = just ask). Returns the end of the heap
/// after the call; the heap starts at [`HEAP_BASE`] and can reach the process's limit (8 MiB for most
/// programs, more for a few the kernel names). It never shrinks.
pub fn brk(end: u64) -> u64 {
    // SAFETY: one plain argument.
    unsafe { syscall3(nr::BRK, end, 0, 0) as u64 }
}

/// Where the heap begins.
pub const HEAP_BASE: u64 = 0x1000_0000;

/// The next message from the browser, for a page's script process ([`abi::SYS_PAGE_RECV`]): waits up
/// to `timeout_ms` (`u64::MAX`: no limit). Returns its length in `buf`, `0` when none came, or `-errno`.
pub fn page_recv(buf: &mut [u8], timeout_ms: u64) -> i64 {
    // SAFETY: the pointer and length describe `buf`, which the kernel may write.
    unsafe {
        syscall3(
            abi::SYS_PAGE_RECV,
            buf.as_mut_ptr() as u64,
            buf.len() as u64,
            timeout_ms,
        )
    }
}

/// Send a message to the browser ([`abi::SYS_PAGE_SEND`]). Returns `0`, or `-errno` (`EAGAIN`: the
/// browser has not taken the earlier ones yet).
pub fn page_send(data: &[u8]) -> i64 {
    // SAFETY: the pointer and length describe `data`.
    unsafe {
        syscall3(
            abi::SYS_PAGE_SEND,
            data.as_ptr() as u64,
            data.len() as u64,
            0,
        )
    }
}

/// Fill `buf` from the kernel's random generator ([`abi::SYS_GETRANDOM`], at most
/// [`abi::MAX_RANDOM`] bytes). Returns the bytes filled, or `-errno`.
pub fn getrandom(buf: &mut [u8]) -> i64 {
    // SAFETY: the pointer and length describe `buf`, which the kernel may write.
    unsafe {
        syscall3(
            abi::SYS_GETRANDOM,
            buf.as_mut_ptr() as u64,
            buf.len() as u64,
            0,
        )
    }
}

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
///
/// `_start` is the ELF entry point, not a function: the kernel enters it with the stack pointer
/// 16-byte aligned (as Linux does), while compiled code expects it 8 bytes off at a function's first
/// instruction (where a `call` left its return address). So `_start` is a few instructions that
/// realign and *call* the real entry; otherwise every frame is misaligned and the first aligned SSE
/// access to the stack (`movaps`, common in C built with SSE) faults.
#[macro_export]
macro_rules! entry {
    ($main:path) => {
        extern "C" fn __kitsune_entry() -> ! {
            let code: i32 = $main();
            $crate::exit(code)
        }

        #[unsafe(naked)]
        #[unsafe(no_mangle)]
        pub extern "C" fn _start() -> ! {
            core::arch::naked_asm!(
                "and rsp, -16",
                "call {entry}",
                "ud2",
                entry = sym __kitsune_entry,
            )
        }
    };
}

#[cfg(not(test))]
#[panic_handler]
fn panic(info: &core::panic::PanicInfo<'_>) -> ! {
    _print(format_args!("panic: {info}\n"));
    exit(101)
}
