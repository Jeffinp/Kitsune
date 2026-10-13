//! Compiles the C half of `mediadec` (libvpx's VP9 decoder, libopus in fixed point, the glue and the
//! small C library; the list is `csources.rs`) with clang for bare-metal x86_64 and hands the objects
//! to the linker.
//!
//! The C code may use SSE2 (a ring 3 thread's SSE state is saved and restored by the scheduler); the
//! Rust side is soft-float, so no float crosses between the two. `CLANG` picks the compiler.

use std::path::{Path, PathBuf};
use std::process::{Child, Command};

include!("csources.rs");

fn main() {
    let root = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let out = PathBuf::from(std::env::var("OUT_DIR").unwrap());
    let clang = std::env::var("CLANG").unwrap_or_else(|_| "clang".into());
    println!("cargo:rerun-if-env-changed=CLANG");
    for dir in ["csrc", "vendor", "csources.rs"] {
        println!("cargo:rerun-if-changed={}", root.join(dir).display());
    }
    let common = flags(&root);
    let parallel = std::thread::available_parallelism().map_or(4, |n| n.get());
    let mut running: Vec<(Child, PathBuf)> = Vec::new();
    let mut objects = Vec::new();
    for (i, (src, extra)) in c_jobs(&root, true).into_iter().enumerate() {
        let stem = src.file_stem().unwrap().to_string_lossy().into_owned();
        let obj = out.join(format!("{i:03}-{stem}.o"));
        if running.len() >= parallel {
            wait(running.remove(0));
        }
        let child = Command::new(&clang)
            .args(&common)
            .args(&extra)
            .arg("-c")
            .arg(&src)
            .arg("-o")
            .arg(&obj)
            .spawn()
            .unwrap_or_else(|e| panic!("cannot run `{clang}` (needed to build mediadec): {e}"));
        running.push((child, src));
        objects.push(obj);
    }
    for job in running {
        wait(job);
    }
    for obj in objects {
        println!("cargo:rustc-link-arg-bins={}", obj.display());
    }
}

fn flags(root: &Path) -> Vec<String> {
    let mut f: Vec<String> = [
        "--target=x86_64-unknown-none-elf",
        "-ffreestanding",
        "-nostdlibinc",
        "-O2",
        "-msse2",
        "-fno-pic",
        "-fno-pie",
        "-fno-stack-protector",
        "-fno-asynchronous-unwind-tables",
        "-fno-unwind-tables",
        "-fno-math-errno",
        "-fwrapv",
        "-ffunction-sections",
        "-fdata-sections",
        "-fno-common",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    f.push("-isystem".into());
    f.push(root.join("csrc/libc/include").display().to_string());
    f
}

fn wait((mut child, src): (Child, PathBuf)) {
    let status = child.wait().expect("wait for clang");
    assert!(status.success(), "clang failed on {}", src.display());
}
