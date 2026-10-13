//! `run PROGRAM [ARGS...]`: start a ring 3 program and wait for it, showing what it writes. The program
//! gets the arguments, the folder the shell is in and what was piped into `run`. `play`, `tone`, `beep`
//! and `volume` are short names for the `play` program (sound, see `docs/design/audio.md`).

use super::*;
use crate::apps::shell::sys::{ProgramCtx, ProgramEnd, SpawnFail};

/// How often the output of a running program is collected, in milliseconds.
const POLL_MS: u64 = 25;

pub(super) fn run(cx: &mut CmdCtx<'_>) -> i32 {
    // No options of its own: everything after `run` is the program and what it is given (`-x` too).
    let operands: Vec<String> = cx.args[1..].to_vec();
    if operands.is_empty() {
        cx.error(t!("sh.run.usage"));
        return 2;
    }
    run_program(cx, operands, false)
}

/// `play ARQUIVO.wav [-v VOL] [--clock]`: the `play` program.
pub(super) fn play(cx: &mut CmdCtx<'_>) -> i32 {
    if cx.args.len() < 2 {
        cx.error(t!("sh.play.usage"));
        return 2;
    }
    let mut argv = alloc::vec![String::from("play")];
    argv.extend_from_slice(&cx.args[1..]);
    run_program(cx, argv, true)
}

/// `tone FREQ[,FREQ...] [MS] [-v VOL]`: sine tones (several frequencies play together).
pub(super) fn tone(cx: &mut CmdCtx<'_>) -> i32 {
    let Some(freqs) = cx.args.get(1).cloned() else {
        cx.error(t!("sh.tone.usage"));
        return 2;
    };
    let mut rest: Vec<String> = cx.args[2..].to_vec();
    let ms = match rest.first() {
        Some(m) if m.chars().all(|c| c.is_ascii_digit()) => rest.remove(0),
        _ => String::from("1000"),
    };
    let mut argv = alloc::vec![String::from("play"), String::from("--tone"), freqs, ms];
    argv.extend(rest);
    run_program(cx, argv, true)
}

/// `beep`: a short tone.
pub(super) fn beep(cx: &mut CmdCtx<'_>) -> i32 {
    let argv = ["play", "--tone", "880", "150"].map(String::from).to_vec();
    run_program(cx, argv, true)
}

/// `volume [N|mute|unmute]`: show or set the system volume.
pub(super) fn volume(cx: &mut CmdCtx<'_>) -> i32 {
    let mut argv = alloc::vec![String::from("play"), String::from("--volume")];
    argv.extend_from_slice(&cx.args[1..cx.args.len().min(2)]);
    run_program(cx, argv, true)
}

/// Start `operands[0]` (an embedded program, or an ELF file unless `embedded`) with the rest as its
/// arguments, and wait for it while showing what it writes.
fn run_program(cx: &mut CmdCtx<'_>, operands: Vec<String>, embedded: bool) -> i32 {
    let target = operands[0].clone();
    // A path (or anything that names a file that exists) is an ELF on the disk; otherwise the name
    // of a program the system carries.
    let from_disk = !embedded && matches!(cx.fs.stat(&target), Ok(s) if s.kind == Kind::File);
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
