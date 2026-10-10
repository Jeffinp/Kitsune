//! Writes to address 0: the process must die with a page fault and the system must carry on.
#![no_std]
#![no_main]

use kitsune_user::*;

entry!(main);

fn main() -> i32 {
    println!("crash: about to write to a null pointer");
    // SAFETY: deliberately not: this is the test that the kernel contains the fault.
    unsafe { core::ptr::write_volatile(core::hint::black_box(0usize) as *mut u8, 1) };
    println!("crash: not reached");
    0
}
