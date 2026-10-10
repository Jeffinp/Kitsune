//! The builtin commands.
//!
//! Every command reads its arguments from [`CmdCtx::args`], writes with
//! [`CmdCtx::print`]/[`CmdCtx::error`] and returns an exit status (0 success,
//! 1 failure, 2 usage error). Commands that depend on the system (`date`,
//! `uptime`, `free`, `df`, `ps`, `kill`, `ping`, `sleep`, `clear`) go through
//! [`super::sys::SysInfo`].

use super::exec::{BuiltinFn, Builtins, CmdCtx, Registry};
use super::fs::{FsErr, Kind, basename, join};
use super::regex::Regex;
use crate::i18n::{self, Arg, dec};
use crate::{t, tk, tp};
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

mod accounts;
mod archive;
mod conditions;
mod files;
mod misc;
mod process;
mod session;
mod system;
mod text;
#[cfg(test)]
pub(crate) use accounts::apply_mode;
use accounts::*;
use archive::*;
use conditions::*;
use files::*;
use misc::*;
use process::*;
use session::*;
use system::*;
use text::*;

/// Register every standard builtin into `r`.
pub fn register_all(r: &mut Registry) {
    let table: [(&str, &'static str, BuiltinFn); 61] = [
        ("help", tk!("sh.help.help"), help),
        ("whoami", tk!("sh.whoami.help"), whoami),
        ("id", tk!("sh.id.help"), id),
        ("groups", tk!("sh.groups.help"), groups),
        ("users", tk!("sh.users.help"), users),
        ("chmod", tk!("sh.chmod.help"), chmod),
        ("chown", tk!("sh.chown.help"), chown),
        ("chgrp", tk!("sh.chgrp.help"), chgrp),
        ("ls", tk!("sh.ls.help"), ls),
        ("cd", tk!("sh.cd.help"), cd),
        ("pwd", tk!("sh.pwd.help"), pwd),
        ("cat", tk!("sh.cat.help"), cat),
        ("echo", tk!("sh.echo.help"), echo),
        ("mkdir", tk!("sh.mkdir.help"), mkdir),
        ("rm", tk!("sh.rm.help"), rm),
        ("rmdir", tk!("sh.rmdir.help"), rmdir),
        ("mv", tk!("sh.mv.help"), mv),
        ("cp", tk!("sh.cp.help"), cp),
        ("touch", tk!("sh.touch.help"), touch),
        ("head", tk!("sh.head.help"), head),
        ("tail", tk!("sh.tail.help"), tail),
        ("wc", tk!("sh.wc.help"), wc),
        ("grep", tk!("sh.grep.help"), grep),
        ("sort", tk!("sh.sort.help"), sort),
        ("uniq", tk!("sh.uniq.help"), uniq),
        ("tee", tk!("sh.tee.help"), tee),
        ("clear", tk!("sh.clear.help"), clear),
        ("env", tk!("sh.env.help"), env),
        ("export", tk!("sh.export.help"), export),
        ("unset", tk!("sh.unset.help"), unset),
        ("history", tk!("sh.history.help"), history),
        ("alias", tk!("sh.alias.help"), alias),
        ("unalias", tk!("sh.unalias.help"), unalias),
        ("which", tk!("sh.which.help"), which),
        ("date", tk!("sh.date.help"), date),
        ("uptime", tk!("sh.uptime.help"), uptime),
        ("free", tk!("sh.free.help"), free),
        ("df", tk!("sh.df.help"), df),
        ("ps", tk!("sh.ps.help"), ps),
        ("kill", tk!("sh.kill.help"), kill),
        ("ping", tk!("sh.ping.help"), ping),
        ("true", tk!("sh.true.help"), true_cmd),
        ("false", tk!("sh.false.help"), false_cmd),
        ("test", tk!("sh.test.help"), test),
        ("[", tk!("sh.bracket.help"), bracket),
        ("sleep", tk!("sh.sleep.help"), sleep),
        ("seq", tk!("sh.seq.help"), seq),
        ("basename", tk!("sh.basename.help"), basename_cmd),
        ("dirname", tk!("sh.dirname.help"), dirname_cmd),
        ("stat", tk!("sh.stat.help"), stat),
        ("rev", tk!("sh.rev.help"), rev),
        ("tr", tk!("sh.tr.help"), tr),
        ("cut", tk!("sh.cut.help"), cut),
        ("nl", tk!("sh.nl.help"), nl),
        ("yes", tk!("sh.yes.help"), yes),
        ("gzip", tk!("sh.gzip.help"), gzip),
        ("gunzip", tk!("sh.gunzip.help"), gunzip),
        ("zip", tk!("sh.zip.help"), zip),
        ("unzip", tk!("sh.unzip.help"), unzip),
        ("iconv", tk!("sh.iconv.help"), iconv),
        ("run", tk!("sh.run.help"), run),
    ];
    for (n, h, f) in table {
        r.register(n, h, f);
    }
}

// ---- helpers ---------------------------------------------------------------

/// Parsed option letters and operands.
pub(super) struct Opts {
    pub(super) flags: Vec<char>,
    pub(super) values: Vec<(char, String)>,
    pub(super) operands: Vec<String>,
}

impl Opts {
    pub(super) fn has(&self, c: char) -> bool {
        self.flags.contains(&c)
    }

    pub(super) fn value(&self, c: char) -> Option<&str> {
        self.values
            .iter()
            .rev()
            .find(|(k, _)| *k == c)
            .map(|(_, v)| v.as_str())
    }
}

/// Parse `-abc`, `-n 5`, `-n5`, `--`. `valued` lists letters taking a value;
/// `numeric` maps `-5` to that letter's value. Unknown letters are an error.
pub(super) fn parse_opts(
    cx: &mut CmdCtx<'_>,
    known: &str,
    valued: &str,
    numeric: Option<char>,
) -> Option<Opts> {
    let mut o = Opts {
        flags: Vec::new(),
        values: Vec::new(),
        operands: Vec::new(),
    };
    let args: Vec<String> = cx.args[1..].to_vec();
    let mut i = 0;
    let mut done = false;
    while i < args.len() {
        let a = &args[i];
        i += 1;
        if done || !a.starts_with('-') || a.len() == 1 {
            o.operands.push(a.clone());
            continue;
        }
        if a == "--" {
            done = true;
            continue;
        }
        let body = &a[1..];
        if let Some(nc) = numeric
            && body.chars().all(|c| c.is_ascii_digit())
        {
            o.values.push((nc, body.to_string()));
            continue;
        }
        let chars = body.char_indices();
        for (idx, c) in chars {
            if valued.contains(c) {
                let rest = &body[idx + c.len_utf8()..];
                if !rest.is_empty() {
                    o.values.push((c, rest.to_string()));
                } else if i < args.len() {
                    o.values.push((c, args[i].clone()));
                    i += 1;
                } else {
                    cx.error(&t!("sh.err.option_needs_arg", c = c.to_string().as_str()));
                    return None;
                }
                break;
            } else if known.contains(c) {
                o.flags.push(c);
            } else {
                cx.error(&t!("sh.err.invalid_option", c = c.to_string().as_str()));
                return None;
            }
        }
    }
    Some(o)
}

pub(super) fn fs_err(cx: &mut CmdCtx<'_>, path: &str, e: FsErr) {
    let msg = format!("{path}: {}", e.message());
    cx.error(&msg);
}

/// Read each operand (or stdin when there are none or for `-`).
fn inputs(cx: &mut CmdCtx<'_>, files: &[String]) -> (Vec<(String, Vec<u8>)>, bool) {
    let mut ok = true;
    let mut out = Vec::new();
    if files.is_empty() {
        out.push((String::new(), cx.stdin.to_vec()));
        return (out, ok);
    }
    for f in files {
        if f == "-" {
            out.push((String::from("-"), cx.stdin.to_vec()));
            continue;
        }
        match cx.fs.read(f) {
            Ok(d) => out.push((f.clone(), d)),
            Err(e) => {
                fs_err(cx, f, e);
                ok = false;
            }
        }
    }
    (out, ok)
}

fn lines_of(data: &[u8]) -> Vec<String> {
    let text = String::from_utf8_lossy(data);
    let mut v: Vec<String> = text.split('\n').map(String::from).collect();
    if v.last().is_some_and(String::is_empty) {
        v.pop();
    }
    v
}

fn parse_num(s: &str) -> Option<i64> {
    s.trim().parse::<i64>().ok()
}

// ---- commands -----------------------------------------------------------------

fn help(cx: &mut CmdCtx<'_>) -> i32 {
    if cx.args.len() > 1 {
        let mut st = 0;
        for name in &cx.args[1..] {
            match cx.registry.help_of(name) {
                Some(h) => cx.println(h),
                None => {
                    cx.error(&t!("sh.help.none", name = name.as_str()));
                    st = 1;
                }
            }
        }
        return st;
    }
    let list = cx.registry.list();
    cx.println(t!("sh.help.header"));
    for (_, h) in list {
        cx.print("  ");
        cx.println(h);
    }
    0
}

// ---- test / [ -----------------------------------------------------------------

struct TestEval<'a, 'b> {
    t: &'a [String],
    i: usize,
    cx: &'a mut CmdCtx<'b>,
}
