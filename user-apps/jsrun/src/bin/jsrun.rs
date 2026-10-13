//! `jsrun`: runs JavaScript in a ring 3 process with QuickJS.
//!
//! ```text
//! run jsrun script.js        the file (relative paths start at the terminal's folder)
//! run jsrun -e 'CODE'        the code given
//! run jsrun --selftest       the built-in test of the engine, with a small benchmark
//! echo 'CODE' | run jsrun    standard input
//! jsrun --page               the script process of a browser page (started by the browser)
//! jsrun --worker             a page's Web Worker (started by the browser)
//! ```
//!
//! `console.log` (and `print`) write to the output; an uncaught exception prints its message and stack
//! and the status is 1. The engine and its C library are compiled by build.rs; this side reads the
//! script, gives C its heap (`heap.rs`) and its system calls (`sys.rs`).
#![no_std]
#![no_main]

extern crate alloc;

#[path = "jsrun/heap.rs"]
mod heap;
#[path = "jsrun/sys.rs"]
mod sys;

use alloc::vec::Vec;
use kitsune_user::fs::{self, Env, File};
use kitsune_user::*;

entry!(main);

/// The largest script read, in bytes.
const MAX_SCRIPT: usize = 16 * 1024 * 1024;

/// The engine's own test, with a NUL after it (QuickJS wants one).
const SELFTEST: &str = concat!(include_str!("jsrun/selftest.js"), "\0");
/// The DOM, events and timers of page mode, its network (`fetch`, XHR...), the user's input, layout
/// measures, observers, document structure, storage, history, `Intl`, the rest of the window and the
/// media elements (`<video>`, `<audio>`, Media Source Extensions): JavaScript run before the page's
/// scripts, in this order (each file builds on the ones before),
/// NUL-ended.
const DOM_JS: &str = concat!(
    include_str!("jsrun/dom.js"),
    "\n",
    include_str!("jsrun/net.js"),
    "\n",
    include_str!("jsrun/forms.js"),
    "\n",
    include_str!("jsrun/events.js"),
    "\n",
    include_str!("jsrun/layoutapi.js"),
    "\n",
    include_str!("jsrun/observers.js"),
    "\n",
    include_str!("jsrun/structure.js"),
    "\n",
    include_str!("jsrun/storage.js"),
    "\n",
    include_str!("jsrun/history.js"),
    "\n",
    include_str!("jsrun/intl.js"),
    "\n",
    include_str!("jsrun/webapis.js"),
    "\n",
    include_str!("jsrun/scripts.js"),
    "\n",
    include_str!("jsrun/msgcodec.js"),
    "\n",
    include_str!("jsrun/workers.js"),
    "\n",
    include_str!("jsrun/media.js"),
    "\n",
    include_str!("jsrun/compat.js"),
    "\0"
);
/// A worker's prelude: the timers and the task loop of dom.js (its document stays empty and is
/// hidden), the network, `Intl`, the message codec and the worker's global scope.
const WORKER_JS: &str = concat!(
    include_str!("jsrun/dom.js"),
    "\n",
    include_str!("jsrun/net.js"),
    "\n",
    include_str!("jsrun/intl.js"),
    "\n",
    include_str!("jsrun/msgcodec.js"),
    "\n",
    include_str!("jsrun/workerscope.js"),
    "\0"
);

unsafe extern "C" {
    /// `glue.c`: evaluate `len` bytes at `src` (NUL at `src[len]`) named `name` (NUL-terminated);
    /// returns the process status.
    fn kjs_run(src: *const u8, len: usize, name: *const u8) -> i32;
    /// `page.c`: the browser page loop with the DOM code at `src` (`len` bytes, NUL at `src[len]`);
    /// returns only when it cannot start.
    fn kjs_page(src: *const u8, len: usize) -> i32;
}

const USAGE: &str = "usage: jsrun FILE.js | jsrun -e CODE | jsrun --selftest | ... | jsrun\n";

fn main() -> i32 {
    let env = Env::load();
    let mut args = env.args();
    let (mut source, name): (Vec<u8>, &str) = match args.next() {
        Some("--page") => {
            // SAFETY: `DOM_JS` is static, `len` bytes followed by a NUL.
            return unsafe { kjs_page(DOM_JS.as_ptr(), DOM_JS.len() - 1) };
        }
        Some("--worker") => {
            // SAFETY: `WORKER_JS` is static, `len` bytes followed by a NUL.
            return unsafe { kjs_page(WORKER_JS.as_ptr(), WORKER_JS.len() - 1) };
        }
        Some("--selftest") => {
            return run(SELFTEST.as_bytes(), SELFTEST.len() - 1, "selftest.js");
        }
        Some("-h" | "--help") => {
            print!("{USAGE}");
            return 0;
        }
        Some("-e") => match args.next() {
            Some(code) => (Vec::from(code.as_bytes()), "<eval>"),
            None => {
                print!("{USAGE}");
                return 2;
            }
        },
        Some(path) => match read_file(path) {
            Ok(v) => (v, path),
            Err(msg) => {
                println!("jsrun: {path}: {msg}");
                return 2;
            }
        },
        None => match read_stdin() {
            Ok(v) => (v, "<stdin>"),
            Err(msg) => {
                println!("jsrun: <stdin>: {msg}");
                return 2;
            }
        },
    };
    let len = source.len();
    source.push(0);
    run(&source, len, name)
}

fn run(src: &[u8], len: usize, name: &str) -> i32 {
    let mut cname = Vec::with_capacity(name.len() + 1);
    cname.extend_from_slice(name.as_bytes());
    cname.push(0);
    debug_assert!(src.len() > len && src[len] == 0);
    // SAFETY: `src` holds `len` bytes and a NUL at `src[len]`; `cname` is NUL-terminated; both live
    // through the call, which does not keep them.
    unsafe { kjs_run(src.as_ptr(), len, cname.as_ptr()) }
}

fn read_file(path: &str) -> Result<Vec<u8>, &'static str> {
    let mut f = File::open(path).map_err(fs::error_text)?;
    read_all(|buf| f.read(buf).map_err(fs::error_text))
}

fn read_stdin() -> Result<Vec<u8>, &'static str> {
    read_all(|buf| fs::read_stdin(buf).map_err(fs::error_text))
}

/// Read until the end in 64 KiB pieces (the most one call moves).
fn read_all(
    mut read: impl FnMut(&mut [u8]) -> Result<usize, &'static str>,
) -> Result<Vec<u8>, &'static str> {
    let mut out = Vec::new();
    loop {
        let at = out.len();
        if at >= MAX_SCRIPT {
            return Err("script too large (16 MiB at most)");
        }
        out.resize(at + 64 * 1024, 0);
        let n = read(&mut out[at..])?;
        out.truncate(at + n);
        if n == 0 {
            return Ok(out);
        }
    }
}
