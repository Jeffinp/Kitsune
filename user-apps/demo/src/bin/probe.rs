//! Tries things ring 3 must not be able to do, and says what happened (each in its own process via
//! the first byte of the pid parity is not used: it runs one probe chosen by uptime and reports).
#![no_std]
#![no_main]

use kitsune_user::*;

entry!(main);

fn main() -> i32 {
    // 1. Bad pointers to the kernel's own write call come back as errors, not faults.
    let r = unsafe { syscall3(1, 1, 0xFFFF_8000_0000_0000, 16) };
    println!("write from a kernel address -> {r}");
    let r = unsafe { syscall3(1, 1, 0, 16) };
    println!("write from address 0 -> {r}");
    let r = unsafe { syscall3(1, 9, 0x400000, 1) };
    println!("write to fd 9 -> {r}");
    let r = unsafe { syscall3(999, 0, 0, 0) };
    println!("unknown call 999 -> {r}");
    // 2. A privileged instruction must kill us with #GP.
    println!("probe: executing cli (privileged)...");
    unsafe { core::arch::asm!("cli") };
    println!("probe: not reached");
    0
}
