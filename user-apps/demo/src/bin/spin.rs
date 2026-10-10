//! Never stops: proves the scheduler preempts ring 3 code and that such a process can be killed.
#![no_std]
#![no_main]

use kitsune_user::*;

entry!(main);

fn main() -> i32 {
    println!("spin: looping forever");
    let mut n: u64 = 0;
    loop {
        n = n.wrapping_add(1);
        core::hint::black_box(&mut n);
    }
}
