//! `cat FILE...`: writes the files to the output; with no argument, copies its standard input.
#![no_std]
#![no_main]

use kitsune_user::fs::{self, Env, File};
use kitsune_user::*;

entry!(main);

fn main() -> i32 {
    let env = Env::load();
    let mut buf = [0u8; 4096];
    let mut status = 0;
    let mut any = false;
    for name in env.args() {
        any = true;
        match File::open(name) {
            Ok(mut f) => loop {
                match f.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => {
                        write(1, &buf[..n]);
                    }
                    Err(e) => {
                        println!("cat: {name}: {}", fs::error_text(e));
                        status = 1;
                        break;
                    }
                }
            },
            Err(e) => {
                println!("cat: {name}: {}", fs::error_text(e));
                status = 1;
            }
        }
    }
    if !any {
        while let Ok(n) = fs::read_stdin(&mut buf) {
            if n == 0 {
                break;
            }
            write(1, &buf[..n]);
        }
    }
    status
}
