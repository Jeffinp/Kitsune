//! `ls [FOLDER]`: lists a folder (the one the program was started in by default), one name per line,
//! folders with a trailing `/`.
#![no_std]
#![no_main]

use kitsune_user::abi::KIND_DIR;
use kitsune_user::fs::{self, Env};
use kitsune_user::*;

entry!(main);

fn main() -> i32 {
    let env = Env::load();
    let dir = env.args().next().unwrap_or(".");
    let mut name = [0u8; 256];
    let mut i = 0;
    loop {
        match fs::read_dir(dir, i, &mut name) {
            Ok(Some((kind, n))) => {
                let text = core::str::from_utf8(n).unwrap_or("?");
                if kind == KIND_DIR {
                    println!("{text}/");
                } else {
                    println!("{text}");
                }
                i += 1;
            }
            Ok(None) => return 0,
            Err(e) => {
                println!("ls: {dir}: {}", fs::error_text(e));
                return 1;
            }
        }
    }
}
