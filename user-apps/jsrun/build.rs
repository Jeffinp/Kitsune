//! Compiles the C half of `jsrun` (QuickJS, musl's libm subset, the libc shim and the glue) with clang
//! for bare-metal x86_64 and hands the objects to the linker.
//!
//! The C code may use SSE2 (a ring 3 thread's SSE state is saved and restored by the scheduler), so
//! doubles are hardware ones; the Rust side is soft-float, which is why no float crosses between the
//! two (see `libc/libc.c`). `CLANG` picks the compiler (default `clang`).

use std::path::{Path, PathBuf};
use std::process::{Child, Command};

/// QuickJS 2026-06-04 (bellard/quickjs), untouched.
const QUICKJS: &[&str] = &["quickjs", "libregexp", "libunicode", "cutils", "dtoa"];

/// The part of musl's `src/math` that QuickJS's `Math` and number code reach, with what it uses.
const MUSL: &str = "acos acosh asin asinh atan atan2 atanh cbrt copysign cos cosh exp exp_data \
    expm1 fabs floor fmax fmin fmod hypot log log_data log10 log1p log2 log2_data lrint pow pow_data \
    rint round scalbn sin sinh tan tanh trunc __cos __sin __tan __rem_pio2 __rem_pio2_large __expo2 \
    __math_divzero __math_invalid __math_oflow __math_uflow __math_xflow";

fn main() {
    let root = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let out = PathBuf::from(std::env::var("OUT_DIR").unwrap());
    let clang = std::env::var("CLANG").unwrap_or_else(|_| "clang".into());
    println!("cargo:rerun-if-env-changed=CLANG");
    for dir in ["libc", "src", "vendor"] {
        println!("cargo:rerun-if-changed={}", root.join(dir).display());
    }

    let mut jobs: Vec<(PathBuf, Vec<String>)> = Vec::new();
    for name in QUICKJS {
        jobs.push((root.join(format!("vendor/quickjs/{name}.c")), Vec::new()));
    }
    for name in MUSL.split_whitespace() {
        // musl's sources are not warning-clean under clang (they are vendored as they are) and expect
        // musl's internal <features.h> (`hidden`, `weak_alias`) to come first.
        jobs.push((
            root.join(format!("vendor/musl-math/{name}.c")),
            vec!["-w".into(), "-include".into(), "features.h".into()],
        ));
    }
    for file in ["libc/libc.c", "libc/printf.c", "src/glue.c"] {
        jobs.push((root.join(file), vec!["-Wall".into()]));
    }

    let common = flags(&root);
    let parallel = std::thread::available_parallelism().map_or(4, |n| n.get());
    let mut running: Vec<(Child, PathBuf)> = Vec::new();
    let mut objects = Vec::new();
    for (src, extra) in jobs {
        let stem = src.file_stem().unwrap().to_string_lossy().into_owned();
        let dir = src
            .parent()
            .unwrap()
            .file_name()
            .unwrap()
            .to_string_lossy()
            .into_owned();
        let obj = out.join(format!("{dir}-{stem}.o"));
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
            .unwrap_or_else(|e| panic!("cannot run `{clang}` (needed to build jsrun): {e}"));
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
        "-DCONFIG_VERSION=\"2026-06-04\"",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    f.push("-isystem".into());
    f.push(root.join("libc/include").display().to_string());
    f.push("-I".into());
    f.push(root.join("vendor/quickjs").display().to_string());
    f
}

fn wait((mut child, src): (Child, PathBuf)) {
    let status = child.wait().expect("wait for clang");
    assert!(status.success(), "clang failed on {}", src.display());
}
