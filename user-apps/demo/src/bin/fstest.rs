//! Exercises the file calls and prints `ok` / `FAIL` for each step (the QEMU scenario reads them):
//! create, write, read back, append, seek, stat, rename, folders, listing, remove, and the refusals
//! (a missing file, a folder opened as a file, a file the user may not change, a bad descriptor).
#![no_std]
#![no_main]

use kitsune_user::abi::{self, errno};
use kitsune_user::fs::{self, Env, File};
use kitsune_user::*;

entry!(main);

static mut FAILS: u32 = 0;

fn check(what: &str, good: bool) {
    let t = uptime_ms();
    if good {
        println!("ok   {what} ({t} ms)");
    } else {
        println!("FAIL {what} ({t} ms)");
        // SAFETY: single-threaded program.
        unsafe { FAILS += 1 };
    }
}

fn main() -> i32 {
    let env = Env::load();
    println!("cwd {} args {}", env.cwd(), env.args().count());

    // Create, write, read back.
    {
        let mut f = File::create("fstest.tmp").expect("create");
        check("write", f.write_all(b"hello, files\n").is_ok());
    }
    let mut buf = [0u8; 64];
    {
        let mut f = File::open("fstest.tmp").expect("open");
        let n = f.read(&mut buf).unwrap_or(0);
        check("read back", &buf[..n] == b"hello, files\n");
        check("end of file", f.read(&mut buf) == Ok(0));
        check("seek to 7", f.seek(7, abi::SEEK_SET) == Ok(7));
        let n = f.read(&mut buf).unwrap_or(0);
        check("read after seek", &buf[..n] == b"files\n");
        check("seek from end", f.seek(-6, abi::SEEK_END) == Ok(7));
        check(
            "writing a read-only file is refused",
            f.write(b"x") == Err(errno::EBADF),
        );
    }
    // Append, then stat.
    {
        let mut f = File::append("fstest.tmp").expect("append");
        check("append", f.write_all(b"more\n").is_ok());
    }
    match fs::stat("fstest.tmp") {
        Ok(s) => {
            check("stat size", s.size == 18);
            check("stat kind", s.kind == abi::KIND_FILE);
        }
        Err(_) => check("stat", false),
    }
    // Folders and listing.
    check("mkdir", fs::mkdir("fstest.dir").is_ok());
    check(
        "mkdir twice is refused",
        fs::mkdir("fstest.dir") == Err(errno::EEXIST),
    );
    check(
        "rename into the folder",
        fs::rename("fstest.tmp", "fstest.dir/moved.txt").is_ok(),
    );
    let mut name = [0u8; 256];
    match fs::read_dir("fstest.dir", 0, &mut name) {
        Ok(Some((_, n))) => check("listing", n == b"moved.txt"),
        _ => check("listing", false),
    }
    check(
        "end of listing",
        matches!(fs::read_dir("fstest.dir", 1, &mut name), Ok(None)),
    );
    // Refusals.
    check(
        "missing file",
        File::open("no-such-file").err() == Some(errno::ENOENT),
    );
    check(
        "a folder is not a file",
        File::open("fstest.dir").err() == Some(errno::EISDIR),
    );
    check("close a bad descriptor", {
        // SAFETY: no pointers.
        unsafe { syscall3(abi::SYS_CLOSE, 99, 0, 0) == errno::EBADF }
    });
    // The user's rights: the account database belongs to the system.
    let denied = File::create("/etc/accounts");
    println!(
        "write /etc/accounts: {}",
        match &denied {
            Ok(_) => "allowed",
            Err(e) => fs::error_text(*e),
        }
    );
    check(
        "system files are protected",
        denied.err() == Some(errno::EACCES),
    );
    // Clean up (to the trash).
    check("remove file", fs::remove("fstest.dir/moved.txt").is_ok());
    check("remove folder", fs::remove("fstest.dir").is_ok());
    // SAFETY: single-threaded program.
    let fails = unsafe { FAILS };
    println!("{}", if fails == 0 { "all ok" } else { "FAILED" });
    fails as i32
}
