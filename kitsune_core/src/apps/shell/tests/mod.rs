//! End-to-end tests of the executor and builtins against [`MemFs`] and
//! [`MockSys`].

use super::*;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use crate::i18n::Lang;
use crate::i18n::testlang::LangGuard;

/// A shell under test. The tests below assert on the English texts, so a `T` pins the language
/// of its thread to English (see [`T::in_lang`] for the Portuguese checks).
struct T {
    sh: Shell,
    fs: MemFs,
    sys: MockSys,
    _lang: LangGuard,
}

impl T {
    fn new() -> Self {
        Self::in_lang(Lang::En)
    }

    fn in_lang(l: Lang) -> Self {
        Self {
            sh: Shell::new(),
            fs: MemFs::new(),
            sys: MockSys::default(),
            _lang: LangGuard::new(l),
        }
    }

    /// Switch the language the following commands answer in.
    fn lang(&mut self, l: Lang) {
        self._lang.set(l);
    }

    fn with_files() -> Self {
        let mut t = Self::new();
        t.fs = MemFs::new()
            .with_file("/a.txt", b"alpha\nbeta\n")
            .with_file("/b.txt", b"gamma\n")
            .with_file("/c.md", b"# title\n")
            .with_dir("/d")
            .with_file("/d/x.txt", b"x1\nx2\nx3\n");
        t
    }

    fn run(&mut self, line: &str) -> RunResult {
        let mut h = Host {
            fs: &mut self.fs,
            sys: &mut self.sys,
        };
        self.sh.run_line(line, &mut h)
    }

    fn out(&mut self, line: &str) -> String {
        self.run(line).text()
    }

    fn script(&mut self, src: &str, args: &[&str]) -> RunResult {
        let args: Vec<String> = args.iter().map(|s| s.to_string()).collect();
        let mut h = Host {
            fs: &mut self.fs,
            sys: &mut self.sys,
        };
        self.sh.run_script(src, &args, &mut h)
    }
}

fn out(line: &str) -> String {
    T::with_files().out(line)
}

fn xorshift(x: &mut u64) -> u64 {
    *x ^= *x << 13;
    *x ^= *x >> 7;
    *x ^= *x << 17;
    *x
}

/// Run `f` on a thread with a small stack, as a kernel thread would have.
fn on_small_stack(kib: usize, f: impl FnOnce() + Send + 'static) {
    std::thread::Builder::new()
        .stack_size(kib * 1024)
        .spawn(f)
        .unwrap()
        .join()
        .unwrap();
}

mod accounts;
mod archive;
mod i18n;
mod net;
mod process;

mod builtins;
mod control_flow;
mod expansion_quoting;
mod limits;
mod operators;
mod robustness;
