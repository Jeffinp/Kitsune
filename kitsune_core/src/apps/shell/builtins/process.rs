//! `run`: start a ring 3 program and wait for it, showing what it writes.

use super::*;
use crate::apps::shell::sys::{ProgramEnd, SpawnFail};

/// How often the output of a running program is collected, in milliseconds.
const POLL_MS: u64 = 25;

pub(super) fn run(cx: &mut CmdCtx<'_>) -> i32 {
    let Some(o) = parse_opts(cx, "", "", None) else {
        return 2;
    };
    let Some(target) = o.operands.first().cloned() else {
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
    let pid = match cx.sys.spawn_program(&name, image.as_deref()) {
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
