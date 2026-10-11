//! Fuzz target: what a ring 3 program hands the kernel for files and arguments.
//!
//! * `resolve` on arbitrary working directories and paths never panics, and what it returns is an
//!   absolute path with no `.`/`..`/empty component, at most `MAX_PATH` bytes, that resolving again
//!   leaves unchanged;
//! * `pack_args` / `unpack_args` round-trip whatever fits, and `unpack_args` on arbitrary bytes
//!   never panics and yields only what is in the blob.
#![no_main]

use kitsune_userabi::{MAX_PATH, pack_args, resolve_string, unpack_args};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    // Arbitrary blob.
    let (cwd, args) = unpack_args(data);
    assert!(cwd.len() <= data.len());
    let total: usize = args.map(str::len).sum();
    assert!(total <= data.len());

    // Paths: split the input at its first NUL into (cwd, path).
    let at = data.iter().position(|&b| b == 0).unwrap_or(data.len() / 2);
    let (a, b) = data.split_at(at);
    let (Ok(cwd), Ok(path)) = (std::str::from_utf8(a), std::str::from_utf8(b.get(1..).unwrap_or(&[]))) else {
        return;
    };
    if let Some(p) = resolve_string(cwd, path) {
        assert!(p.starts_with('/') && p.len() <= MAX_PATH);
        assert!(p == "/" || (!p.ends_with('/') && !p.contains("//")));
        assert!(!p.split('/').any(|c| c == "." || c == ".."));
        // Idempotent: a normalised path resolves to itself, whatever the working directory.
        assert_eq!(resolve_string("/x", &p).as_deref(), Some(p.as_str()));
    }

    // Arguments made of the two parts round-trip.
    let items = vec![path.to_string()];
    if let Some(blob) = pack_args(cwd, &items) {
        let (c, it) = unpack_args(&blob);
        assert_eq!(c, cwd);
        assert_eq!(it.collect::<Vec<_>>(), [path]);
    }
});
