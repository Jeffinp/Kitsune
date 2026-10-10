//! `shell` — a pure command-line engine for the terminal: parser, executor,
//! builtins and (in [`line`]) the line editor.
//!
//! Everything the engine needs from the machine arrives through two traits the
//! kernel implements: [`ShellFs`] (paths, files, directories) and [`SysInfo`]
//! (clock, memory, processes, network, sleeping). The engine itself is
//! `no_std` + `alloc`, never panics on any input, and bounds every resource
//! with [`Limits`] so a runaway script cannot hang the kernel.
//!
//! ```text
//! typed keys -> LineEditor -> Submit(line)
//!                                 |
//!                  Shell::run_line(line, &mut Host { fs, sys })
//!                                 |
//!        parse -> expand ($VAR, $(..), $((..)), globs) -> pipelines
//!                                 |
//!                       RunResult { status, output, exit, clear }
//! ```
//!
//! Limitations (documented, not bugs): no background jobs (`&` is a parse
//! error), no here-documents or descriptor redirections (`<<`, `2>`, `>&`),
//! variables are global (no `local`), commands in a `{ ...; }` group each see
//! the whole stdin instead of consuming it, globs match `*` and `?` only.

pub mod builtins;
pub mod env;
pub mod exec;
pub mod fs;
pub mod glob;
pub mod history;
pub mod line;
pub mod netcmds;
pub mod parse;
pub mod regex;
pub mod screen;
pub mod sys;
pub mod term;
#[cfg(test)]
mod tests;

pub use env::Env;
pub use exec::{BuiltinFn, Builtins, CmdCtx, Host, Limits, Registry, RunResult, Shell};
pub use fs::{FsErr, MemFs, ShellFs};
pub use history::History;
pub use line::{Completer, LineEditor, LineEvent, ShellCompleter};
pub use screen::Screen;
pub use sys::{
    HttpResponse, MockSys, NetInfo, ProgramEnd, ProgramPoll, SpawnFail, SysErr, SysInfo,
};
pub use term::{Term, TermAction};
