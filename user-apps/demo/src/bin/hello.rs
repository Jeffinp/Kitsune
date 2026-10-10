//! Prints its process id and a countdown, sleeping between lines, then exits with status 0.
#![no_std]
#![no_main]

use kitsune_user::*;

entry!(main);

fn main() -> i32 {
    println!("hello from ring 3, pid {}", getpid());
    for i in (1..=3).rev() {
        println!("  {i}...");
        sleep_ms(200);
    }
    println!("up for {} ms, bye", uptime_ms());
    0
}
