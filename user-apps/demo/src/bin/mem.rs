//! Grows its heap with `brk`, uses it, hits the 8 MiB ceiling, then touches memory it never got.
#![no_std]
#![no_main]

use kitsune_user::*;

entry!(main);

fn main() -> i32 {
    println!("heap starts at {:#x}, end {:#x}", HEAP_BASE, brk(0));
    let end = brk(HEAP_BASE + 1024 * 1024);
    println!("asked for 1 MiB -> end {end:#x}");
    // SAFETY: the first MiB of the heap is mapped and writable now.
    unsafe {
        let p = HEAP_BASE as *mut u8;
        for i in 0..(1024 * 1024) {
            p.add(i).write_volatile((i % 251) as u8);
        }
        let mut sum = 0u64;
        for i in 0..(1024 * 1024) {
            sum += u64::from(p.add(i).read_volatile());
        }
        println!("wrote and read back 1 MiB, checksum {sum}");
    }
    let refused = brk(HEAP_BASE + 64 * 1024 * 1024);
    println!("asked for 64 MiB -> end {refused:#x} (unchanged: {})", refused == end);
    println!("touching the page after the heap...");
    // SAFETY: deliberately not: this must fault and kill only this process.
    unsafe { core::ptr::write_volatile(core::hint::black_box(end as usize) as *mut u8, 1) };
    println!("not reached");
    0
}
