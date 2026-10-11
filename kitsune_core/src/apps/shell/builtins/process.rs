//! `run PROGRAM [ARGS...]`: start a ring 3 program and wait for it, showing what it writes. The program
//! gets the arguments, the folder the shell is in and what was piped into `run`.

use super::*;
use crate::apps::shell::sys::{ProgramCtx, ProgramEnd, SpawnFail};

/// How often the output of a running program is collected, in milliseconds.
const POLL_MS: u64 = 25;

pub(super) fn run(cx: &mut CmdCtx<'_>) -> i32 {
    // No options of its own: everything after `run` is the program and what it is given (`-x` too).
    let operands: Vec<String> = cx.args[1..].to_vec();
    let Some(target) = operands.first().cloned() else {
        cx.error(t!("sh.run.usage"));
        return 2;
    };
    // A path (or anything that names a file that exists) is an ELF on the disk; otherwise the name
    // of a program the system carries.
    let from_disk = matches!(cx.fs.stat(&target), Ok(s) if s.kind == Kind::File);
    let image = if from_disk {
        match cx.fs.read(&target) {
            Ok(d) => Some(d),
            Err(e) => {
                fs_err(cx, &target, e);
                return 1;
            }
        }
    } else {
        None
    };
    let name = basename(&target).trim_end_matches(".elf").to_string();
    let ctx = ProgramCtx {
        args: operands[1..].to_vec(),
        cwd: cx.fs.cwd(),
        stdin: cx.stdin.to_vec(),
    };
    let pid = match cx.sys.spawn_program(&name, image.as_deref(), &ctx) {
        Ok(p) => p,
        Err(e) => {
            let msg = if e == SpawnFail::Unknown {
                t!("sh.run.unknown_named", name = target.as_str())
            } else {
                String::from(e.message())
            };
            cx.error(&msg);
            return 1;
        }
    };
    let mut interrupted = false;
    loop {
        let poll = cx.sys.poll_program(pid);
        cx.out(&poll.output);
        if let Some(end) = poll.end {
            return match end {
                ProgramEnd::Exited(0) => 0,
                ProgramEnd::Exited(c) => {
                    cx.error(&t!("sh.run.exit_status", status = c));
                    c.clamp(1, 255)
                }
                ProgramEnd::Killed => {
                    if !interrupted {
                        cx.error(t!("sh.run.killed"));
                    }
                    if interrupted { 130 } else { 139 }
                }
            };
        }
        if !interrupted && cx.sys.interrupted() {
            interrupted = true;
            cx.sys.kill_program(pid);
        }
        cx.sys.sleep_ms(POLL_MS);
        if cx.full() {
            cx.sys.kill_program(pid);
            return 1;
        }
    }
}
