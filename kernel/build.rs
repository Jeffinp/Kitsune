//! Build script for the kernel's embedded WebAssembly programs.
//!
//! Everything is turned into binary `.wasm` the kernel embeds with `include_bytes!`:
//!   * `demo.wasm` — a tiny console smoke-test, assembled from inline WAT on the
//!     host (the `wat` crate), so the bare-metal kernel needs no text parser.
//!   * `apps/<name>.wasm` — the bundled app packages: real Rust crates compiled to
//!     `wasm32-unknown-unknown` (`../wasm-apps/<name>`, each carrying its own
//!     manifest section), installed into `/apps` on first boot. This is the
//!     "compile source to wasm and equip the OS" model: a genuine compiled language
//!     becomes a native Kitsune app, no foreign OS, no emulation.
//!   * `app.wasm` — the optional legacy windowed app (DOOM, or the C demo when the
//!     wasi-sdk is available; empty otherwise).

use std::path::{Path, PathBuf};
use std::process::Command;

fn main() {
    let out = PathBuf::from(std::env::var("OUT_DIR").unwrap());

    // 1) Console demo: imports host.log(ptr,len), stores a greeting in its own
    //    linear memory, and logs it from `main`. Exercises module decode, memory
    //    export, host import resolution, and a guest→host call.
    let msg = "Hello from WASM - .wasm running native on Kitsune";
    let console = format!(
        r#"(module
  (import "host" "log" (func $log (param i32 i32)))
  (memory (export "mem") 1)
  (data (i32.const 0) "{msg}")
  (func (export "main")
    (call $log (i32.const 0) (i32.const {len}))))"#,
        msg = msg,
        len = msg.len(),
    );
    emit_wat(&out, "demo.wasm", &console);

    // 2) The legacy windowed app (`app.wasm`), only for the opt-in builds:
    //    - DOOM=1 + WASI_SDK_PATH -> compile DOOM (doomgeneric) to wasm32-wasi and
    //      embed the IWAD. The full "run open-source C software natively" path.
    //    - WASI_SDK_PATH only      -> a freestanding C demo (cdemo).
    //    - neither                 -> no legacy app (empty `app.wasm`).
    println!("cargo:rerun-if-env-changed=WASI_SDK_PATH");
    println!("cargo:rerun-if-env-changed=DOOM");
    let doom = std::env::var("DOOM").map(|v| v == "1").unwrap_or(false);
    let sdk = std::env::var("WASI_SDK_PATH")
        .ok()
        .filter(|s| !s.is_empty());
    match (doom, sdk) {
        (true, Some(sdk)) => build_doom(&out, &sdk),
        (false, Some(sdk)) => {
            build_c_app(&out, "cdemo", &sdk);
            write_empty_wad(&out);
        }
        _ => {
            std::fs::write(out.join("app.wasm"), []).expect("write empty app.wasm");
            write_empty_wad(&out);
        }
    }

    // 3) The bundled app packages (Rust -> wasm32, one workspace-detached crate
    //    each under `wasm-apps/`), installed into `/apps` on first boot. A crate
    //    that is not there is skipped, so the list can grow without breaking
    //    older checkouts.
    let apps_out = out.join("apps");
    std::fs::create_dir_all(&apps_out).expect("create apps dir");
    let mut bundled = String::new();
    for name in BUNDLED_APPS {
        let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("wasm-apps")
            .join(name);
        if !dir.join("Cargo.toml").exists() {
            continue;
        }
        build_wasm_app(&out, name, &apps_out.join(format!("{name}.wasm")));
        bundled.push_str(&format!(
            "    include_bytes!(concat!(env!(\"OUT_DIR\"), \"/apps/{name}.wasm\")),\n"
        ));
    }
    let generated = format!(
        "/// Packages bundled in the image (see `build.rs`); installed into `/apps` on first boot.\n\
         pub(crate) static BUNDLED: &[&[u8]] = &[\n{bundled}];\n\
         /// The app the dock's WASM icon launches when there is no legacy build.\n\
         pub(crate) const DEFAULT_APP: &str = \"snake\";\n"
    );
    std::fs::write(out.join("apps_gen.rs"), generated).expect("write apps_gen.rs");
    // The SDK is a path dependency of the apps: rebuild when it changes.
    println!("cargo:rerun-if-changed=../wasm-apps/sdk/src");
    println!("cargo:rerun-if-changed=../wasm-apps/sdk/Cargo.toml");

    // 4) The ring 3 demo programs (`../user-apps/demo`), ELF executables for `x86_64-unknown-none`.
    build_user_programs(&out);

    println!("cargo:rerun-if-changed=build.rs");
}

/// The ring 3 programs that are embedded: the crate under `user-apps/` and the binaries
/// (`src/bin/<name>.rs`) taken from it.
const USER_CRATES: &[(&str, &[&str])] = &[
    ("demo", &["hello", "spin", "crash", "probe", "mem"]),
    // The WebAssembly host that runs an app inside a process (see `src/wasm/remote.rs`).
    ("wasmhost", &["wasmhost"]),
];

/// Build every crate of [`USER_CRATES`] for bare metal with the user linker script and write
/// `out/user_gen.rs`, which lists the ELF images for `include!`.
fn build_user_programs(out: &Path) {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("user-apps");
    let linker = root.join("linker.ld");
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
    let mut list = String::new();
    std::fs::create_dir_all(out.join("user")).expect("create user dir");
    for (krate, names) in USER_CRATES {
        let manifest = root.join(krate).join("Cargo.toml");
        let target_dir = out.join("userapp").join(krate);
        let status = Command::new(&cargo)
            .env_remove("RUSTC_WORKSPACE_WRAPPER")
            .env_remove("RUSTC_WRAPPER")
            .env_remove("CARGO_ENCODED_RUSTFLAGS")
            .env_remove("CARGO_BUILD_RUSTFLAGS")
            .env(
                "RUSTFLAGS",
                format!(
                    "-C link-arg=-T{} -C relocation-model=static -C link-arg=-zmax-page-size=4096 -C link-arg=-znorelro",
                    linker.display()
                ),
            )
            .args(["build", "--release", "--target", "x86_64-unknown-none"])
            .arg("--manifest-path")
            .arg(&manifest)
            .arg("--target-dir")
            .arg(&target_dir)
            .status()
            .unwrap_or_else(|e| panic!("failed to launch cargo for the user programs: {e}"));
        assert!(
            status.success(),
            "the user program `{krate}` failed to build"
        );
        for name in *names {
            let elf = target_dir.join("x86_64-unknown-none/release").join(name);
            let dest = out.join("user").join(format!("{name}.elf"));
            std::fs::copy(&elf, &dest).unwrap_or_else(|e| panic!("copy {}: {e}", elf.display()));
            list.push_str(&format!(
                "    (\"{name}\", include_bytes!(concat!(env!(\"OUT_DIR\"), \"/user/{name}.elf\"))),\n"
            ));
        }
        println!("cargo:rerun-if-changed=../user-apps/{krate}/src");
        println!("cargo:rerun-if-changed=../user-apps/{krate}/Cargo.toml");
    }
    let generated = format!(
        "/// Ring 3 programs embedded in the image (see `build.rs`): name and ELF bytes.\n\
         pub(crate) static USER_PROGRAMS: &[(&str, &[u8])] = &[\n{list}];\n"
    );
    std::fs::write(out.join("user_gen.rs"), generated).expect("write user_gen.rs");
    println!("cargo:rerun-if-changed=../user-apps/sdk/src");
    println!("cargo:rerun-if-changed=../user-apps/linker.ld");
    // The host speaks the wire contract defined in the core.
    println!("cargo:rerun-if-changed=../kitsune_core/src/platform/appabi");
}

/// Assemble `wat` text and write the resulting binary module to `out/name`.
fn emit_wat(out: &Path, name: &str, wat: &str) {
    let wasm = wat::parse_str(wat).unwrap_or_else(|e| panic!("{name} WAT failed: {e}"));
    std::fs::write(out.join(name), &wasm).unwrap_or_else(|e| panic!("write {name}: {e}"));
}

/// The Rust app crates under `wasm-apps/` that ship in the image.
const BUNDLED_APPS: &[&str] = &["clock", "notes", "paint", "snake"];

/// Compile the workspace-detached `../wasm-apps/<crate>` to
/// `wasm32-unknown-unknown` (release) and copy the resulting module to `dest`. Uses a dedicated target dir so the nested build never
/// contends with the outer one's lock.
fn build_wasm_app(out: &Path, crate_name: &str, dest: &Path) {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let app_dir = root.join("..").join("wasm-apps").join(crate_name);
    let manifest = app_dir.join("Cargo.toml");
    let target_dir = out.join("wasmapp");

    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
    let status = Command::new(cargo)
        // Build the app as a plain release, isolated from how the kernel itself
        // is being compiled. Without this, running `cargo clippy` on the kernel
        // leaks its clippy wrapper + `-D warnings` into this nested build and the
        // app's own lints would fail the parent lint.
        .env_remove("RUSTC_WORKSPACE_WRAPPER")
        .env_remove("RUSTC_WRAPPER")
        .env_remove("RUSTFLAGS")
        .env_remove("CARGO_ENCODED_RUSTFLAGS")
        .env_remove("CARGO_BUILD_RUSTFLAGS")
        .args(["build", "--release", "--target", "wasm32-unknown-unknown"])
        .arg("--manifest-path")
        .arg(&manifest)
        .arg("--target-dir")
        .arg(&target_dir)
        .status()
        .unwrap_or_else(|e| panic!("failed to launch cargo for {crate_name}: {e}"));
    assert!(status.success(), "wasm app `{crate_name}` failed to build");

    let wasm = target_dir
        .join("wasm32-unknown-unknown/release")
        .join(format!("{crate_name}.wasm"));
    std::fs::copy(&wasm, dest).unwrap_or_else(|e| panic!("copy {}: {e}", wasm.display()));

    println!("cargo:rerun-if-changed={}", app_dir.join("src").display());
    println!("cargo:rerun-if-changed={}", manifest.display());
}

/// Compile a freestanding C app `../wasm-apps/<name>/<name>.c` to
/// `wasm32-unknown-unknown` with the wasi-sdk clang at `wasi_sdk`, exporting our
/// app entry points and importing the host ABI. Writes `out/app.wasm`. This is
/// the same toolchain path a ported C game uses.
fn build_c_app(out: &Path, name: &str, wasi_sdk: &str) {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let src = root
        .join("..")
        .join("wasm-apps")
        .join(name)
        .join(format!("{name}.c"));
    let clang = PathBuf::from(wasi_sdk).join("bin/clang");
    let app = out.join("app.wasm");

    let status = Command::new(&clang)
        .args([
            "--target=wasm32-unknown-unknown",
            "-O2",
            "-nostdlib",
            "-Wl,--no-entry",
            "-Wl,--export=render",
            "-Wl,--export=on_key",
            "-Wl,--export-memory",
            "-Wl,--allow-undefined",
            "-o",
        ])
        .arg(&app)
        .arg(&src)
        .status()
        .unwrap_or_else(|e| panic!("failed to launch clang ({}): {e}", clang.display()));
    assert!(status.success(), "C app `{name}` failed to build");

    println!("cargo:rerun-if-changed={}", src.display());
}

/// Ensure `out/doom1.wad` exists so the kernel's `include_bytes!` always works.
/// Empty when not building DOOM (no IWAD to embed).
fn write_empty_wad(out: &Path) {
    let wad = out.join("doom1.wad");
    if !wad.exists() {
        std::fs::write(&wad, []).expect("write empty doom1.wad");
    }
}

/// Build DOOM (doomgeneric) to wasm32-wasi via `tools/build-doom.sh` (which uses
/// the wasi-sdk clang and clones the GPL upstream if needed), then embed the
/// resulting module as `app.wasm` and the IWAD as `doom1.wad`. The IWAD must be
/// present at `wasm-apps/doom/doom1.wad` (fetched separately — not redistributed).
fn build_doom(out: &Path, wasi_sdk: &str) {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..");
    let script = root.join("tools/build-doom.sh");
    let status = Command::new("bash")
        .arg(&script)
        .env("WASI_SDK_PATH", wasi_sdk)
        .status()
        .unwrap_or_else(|e| panic!("failed to run build-doom.sh: {e}"));
    assert!(status.success(), "DOOM wasm build failed");

    let doom_wasm = root.join("wasm-apps/doom/doom.wasm");
    let iwad = root.join("wasm-apps/doom/doom1.wad");
    assert!(
        iwad.exists(),
        "missing IWAD at {} (fetch doom1.wad there first)",
        iwad.display()
    );
    std::fs::copy(&doom_wasm, out.join("app.wasm")).expect("copy doom.wasm");
    std::fs::copy(&iwad, out.join("doom1.wad")).expect("copy doom1.wad");

    println!("cargo:rerun-if-changed={}", doom_wasm.display());
    println!("cargo:rerun-if-changed={}", iwad.display());
}
