//! Builds the C of `mediadec` (the list in `user-apps/mediadec/csources.rs`) with the host's clang
//! into a static library. `CLANG` picks the compiler.

use std::path::PathBuf;
use std::process::{Child, Command};

include!("../../../user-apps/mediadec/csources.rs");

fn main() {
    let here = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let root = here.join("../../../user-apps/mediadec");
    let out = PathBuf::from(std::env::var("OUT_DIR").unwrap());
    let clang = std::env::var("CLANG").unwrap_or_else(|_| "clang".into());
    println!("cargo:rerun-if-env-changed=CLANG");
    for d in ["csrc", "vendor", "csources.rs"] {
        println!("cargo:rerun-if-changed={}", root.join(d).display());
    }
    let parallel = std::thread::available_parallelism().map_or(4, |n| n.get());
    let mut running: Vec<(Child, PathBuf)> = Vec::new();
    let mut objects = Vec::new();
    for (i, (src, extra)) in c_jobs(&root).into_iter().enumerate() {
        let obj = out.join(format!(
            "{i}-{}.o",
            src.file_stem().unwrap().to_string_lossy()
        ));
        if running.len() >= parallel {
            wait(running.remove(0));
        }
        let child = Command::new(&clang)
            .args(["-O2", "-msse2", "-fPIC", "-fno-math-errno", "-fwrapv", "-g"])
            .args(&extra)
            .arg("-c")
            .arg(&src)
            .arg("-o")
            .arg(&obj)
            .spawn()
            .expect("run clang");
        running.push((child, src));
        objects.push(obj);
    }
    for job in running {
        wait(job);
    }
    let lib = out.join("libmediac.a");
    let _ = std::fs::remove_file(&lib);
    let status = Command::new("ar")
        .arg("crs")
        .arg(&lib)
        .args(&objects)
        .status()
        .expect("run ar");
    assert!(status.success());
    println!("cargo:rustc-link-search=native={}", out.display());
    println!("cargo:rustc-link-lib=static=mediac");
}

fn wait((mut child, src): (Child, PathBuf)) {
    let status = child.wait().expect("wait for clang");
    assert!(status.success(), "clang failed on {}", src.display());
}
