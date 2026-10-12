//! File, window and child-process calls: validated here, carried out by `crate::userfs`.

use super::*;

pub(super) fn copy_path(asp: &AddressSpace, cwd: &str, ptr: u64, len: u64) -> Result<Vec<u8>, i64> {
    let len = usize::try_from(len).map_err(|_| abi::errno::EINVAL)?;
    if len == 0 {
        return Err(abi::errno::EINVAL);
    }
    if len > abi::MAX_PATH {
        return Err(abi::errno::ENAMETOOLONG);
    }
    let mut raw = alloc::vec![0u8; len];
    asp.copy_in(ptr, &mut raw).map_err(|_| abi::errno::EFAULT)?;
    let text = core::str::from_utf8(&raw).map_err(|_| abi::errno::EINVAL)?;
    abi::resolve_string(cwd, text)
        .map(String::into_bytes)
        .ok_or(abi::errno::ENAMETOOLONG)
}

/// Calls 32–42. The ones that touch the disk are only *queued* here (this runs with interrupts off,
/// where a disk transfer must not happen and a held lock could never be waited for): the process
/// blocks, the `procfsd` thread does the work in thread context and the call is restarted to pick
/// the result up, the same way the WebAssembly host's calls wait.
pub(super) fn file_call(r: &mut Regs, rsp: u64, slot: usize) -> u64 {
    let number = r.rax;
    let (a, b, c, d, e) = (r.rdi, r.rsi, r.rdx, r.r10, r.r8);
    let mut procs = PROCS.lock();
    let Some(p) = procs.iter_mut().find(|p| p.slot == slot) else {
        r.rax = EINVAL as u64;
        return rsp;
    };
    // A request already in flight (this is the restarted call): wait, or take its result.
    match core::mem::replace(&mut p.fs, FsState::Idle) {
        FsState::Done(v) => {
            r.rax = v as u64;
            return rsp;
        }
        busy @ (FsState::Pending(_) | FsState::Running) => {
            p.fs = busy;
            drop(procs);
            return block_restart(r, rsp, slot, number);
        }
        FsState::Idle => {}
    }
    let Some(asp) = p.aspace.as_ref() else {
        r.rax = EINVAL as u64;
        return rsp;
    };
    let cwd = p.cwd.as_str();
    let req = match number {
        abi::SYS_GETARGS => {
            let n = p.args.len().min(b as usize);
            r.rax = match asp.poke(a, &p.args[..n]) {
                Ok(()) => p.args.len() as u64,
                Err(_) => EFAULT as u64,
            };
            return rsp;
        }
        abi::SYS_CLOSE => {
            r.rax = close_fd(p, a as i64) as u64;
            return rsp;
        }
        abi::SYS_READ if a == 0 => {
            // Standard input: what was piped in; no disk involved.
            let len = (c as usize).min(abi::MAX_IO);
            let have = p.stdin.len().saturating_sub(p.stdin_pos);
            let n = len.min(have);
            r.rax = match asp.poke(b, &p.stdin[p.stdin_pos..p.stdin_pos + n]) {
                Ok(()) => {
                    p.stdin_pos += n;
                    n as u64
                }
                Err(_) => EFAULT as u64,
            };
            return rsp;
        }
        abi::SYS_WRITE if a == 1 || a == 2 => {
            drop(procs);
            r.rax = write_out(slot, b, c) as u64;
            return rsp;
        }
        abi::SYS_PROC_POLL => {
            drop(procs);
            r.rax = child_poll(slot, a, b, c) as u64;
            return rsp;
        }
        abi::SYS_PROC_KILL => {
            drop(procs);
            r.rax = child_kill(slot, a) as u64;
            return rsp;
        }
        abi::SYS_SPAWN => {
            // (req_ptr, req_len)
            let len = b as usize;
            if !(abi::SPAWN_HEAD..=abi::MAX_SPAWN).contains(&len) {
                Err(abi::errno::EINVAL)
            } else {
                let mut raw = alloc::vec![0u8; len];
                asp.copy_in(a, &mut raw)
                    .map(|()| FsReq::Spawn { req: raw })
                    .map_err(|_| abi::errno::EFAULT)
            }
        }
        abi::SYS_WIN_EVENT => {
            // (ptr, timeout_ms): one input event, or wait for one up to the timeout.
            let now = crate::interrupts::ticks();
            if let Some(ev) = p.events.pop_front() {
                p.event_deadline = None;
                r.rax = match asp.poke(a, &ev.to_bytes()) {
                    Ok(()) => 1,
                    Err(_) => EFAULT as u64,
                };
                return rsp;
            }
            let deadline = match p.event_deadline {
                Some(d) => d,
                None => {
                    let ticks = if b == u64::MAX {
                        crate::sched::FOREVER
                    } else {
                        now.saturating_add(
                            (b.min(3_600_000) * u64::from(crate::interrupts::TIMER_HZ))
                                .div_ceil(1000),
                        )
                    };
                    p.event_deadline = Some(ticks);
                    ticks
                }
            };
            if b == 0 || now >= deadline {
                p.event_deadline = None;
                r.rax = 0;
                return rsp;
            }
            drop(procs);
            // Restart this call when woken (an event arrived) or when the deadline passes.
            r.rax = number;
            r.rip -= 2;
            crate::sched::set_wake(slot, deadline);
            return crate::sched::yield_switch(rsp);
        }
        abi::SYS_WIN_OPEN => {
            // (w, h, title_ptr, title_len)
            let (w, h) = (a as i64, b as i64);
            if w < 1 || h < 1 || w > i64::from(abi::WIN_MAX_W) || h > i64::from(abi::WIN_MAX_H) {
                Err(abi::errno::EINVAL)
            } else if p.win != 0 {
                Err(abi::errno::EEXIST)
            } else {
                copy_title(asp, c, d).map(|title| FsReq::WinOpen {
                    w: w as i32,
                    h: h as i32,
                    title,
                })
            }
        }
        abi::SYS_WIN_PRESENT => {
            let (w, h) = (b as i64, c as i64);
            if p.win == 0 {
                Err(abi::errno::EBADF)
            } else if w < 1
                || h < 1
                || w > i64::from(abi::WIN_MAX_W)
                || h > i64::from(abi::WIN_MAX_H)
            {
                Err(abi::errno::EINVAL)
            } else {
                Ok(FsReq::WinPresent {
                    ptr: a,
                    w: w as i32,
                    h: h as i32,
                })
            }
        }
        abi::SYS_WIN_TITLE => {
            if p.win == 0 {
                Err(abi::errno::EBADF)
            } else {
                copy_title(asp, a, b).map(|title| FsReq::WinTitle { title })
            }
        }
        abi::SYS_OPEN => copy_path(asp, cwd, a, b).map(|path| FsReq::Open { path, flags: c }),
        abi::SYS_READ => Ok(FsReq::Read {
            fd: a as i64,
            ptr: b,
            len: (c as usize).min(abi::MAX_IO),
        }),
        abi::SYS_WRITE => Ok(FsReq::Write {
            fd: a as i64,
            ptr: b,
            len: (c as usize).min(abi::MAX_IO),
        }),
        abi::SYS_SEEK => Ok(FsReq::Seek {
            fd: a as i64,
            off: b as i64,
            whence: c,
        }),
        abi::SYS_STAT => copy_path(asp, cwd, a, b).map(|path| FsReq::Stat { path, out: c }),
        abi::SYS_READDIR => copy_path(asp, cwd, a, b).map(|path| FsReq::ReadDir {
            path,
            index: c,
            out: d,
            cap: e as usize,
        }),
        abi::SYS_MKDIR => copy_path(asp, cwd, a, b).map(|path| FsReq::Mkdir { path }),
        abi::SYS_REMOVE => copy_path(asp, cwd, a, b).map(|path| FsReq::Remove { path }),
        // SYS_RENAME: (from, from_len, to, to_len)
        _ => copy_path(asp, cwd, a, b)
            .and_then(|from| copy_path(asp, cwd, c, d).map(|to| FsReq::Rename { from, to })),
    };
    match req {
        Ok(req) => {
            p.fs = FsState::Pending(req);
            drop(procs);
            crate::userfs::wake();
            crate::userfs::hand_off();
            block_restart(r, rsp, slot, number)
        }
        Err(e) => {
            r.rax = e as u64;
            rsp
        }
    }
}

/// `SYS_PROC_POLL`: move up to `cap` bytes of child `pid`'s output to the caller and say how it stands.
/// The child counts as over only once its output is all taken; then it may be reaped.
pub(super) fn child_poll(slot: usize, pid: u64, ptr: u64, cap: u64) -> i64 {
    let cap = (cap as usize).min(abi::MAX_IO);
    let mut procs = PROCS.lock();
    let Some(me) = procs.iter().find(|p| p.slot == slot).map(|p| p.pid) else {
        return abi::errno::EINVAL;
    };
    let Some(c) = procs
        .iter_mut()
        .find(|p| u64::from(p.pid) == pid && p.parent == me)
    else {
        return abi::errno::ESRCH;
    };
    let n = c.out.len().min(cap);
    let chunk: Vec<u8> = c.out.drain(..n).collect();
    let state = match end_of(c) {
        _ if !c.out.is_empty() => abi::ChildState::Running,
        End::Running => abi::ChildState::Running,
        End::Exited(code) => abi::ChildState::Exited(code as i16),
        _ => abi::ChildState::Killed,
    };
    if state != abi::ChildState::Running {
        c.keep = false;
    }
    let ok = procs
        .iter()
        .find(|p| p.pid == me)
        .and_then(|p| p.aspace.as_ref())
        .is_some_and(|a| a.poke(ptr, &chunk).is_ok());
    if ok {
        abi::pack_poll(n, state)
    } else {
        abi::errno::EFAULT
    }
}

/// `SYS_PROC_KILL`: stop a child of the caller.
pub(super) fn child_kill(slot: usize, pid: u64) -> i64 {
    let target = {
        let procs = PROCS.lock();
        let Some(me) = procs.iter().find(|p| p.slot == slot).map(|p| p.pid) else {
            return abi::errno::EINVAL;
        };
        procs
            .iter()
            .find(|p| u64::from(p.pid) == pid && p.parent == me)
            .map(|p| p.pid)
    };
    match target {
        Some(t) if kill(t) => 0,
        Some(_) => 0, // already over
        None => abi::errno::ESRCH,
    }
}

pub(super) fn copy_title(asp: &AddressSpace, ptr: u64, len: u64) -> Result<Vec<u8>, i64> {
    let len = usize::try_from(len).map_err(|_| abi::errno::EINVAL)?;
    if len > abi::WIN_MAX_TITLE {
        return Err(abi::errno::ENAMETOOLONG);
    }
    let mut raw = alloc::vec![0u8; len];
    asp.copy_in(ptr, &mut raw).map_err(|_| abi::errno::EFAULT)?;
    // Printable ASCII only (the title bar draws it as it is).
    if raw.iter().any(|b| !(0x20..0x7F).contains(b)) {
        return Err(abi::errno::EINVAL);
    }
    Ok(raw)
}

pub(super) fn close_fd(p: &mut Proc, fd: i64) -> i64 {
    let Ok(i) = usize::try_from(fd - abi::FIRST_FD) else {
        return abi::errno::EBADF;
    };
    match p.fds.get_mut(i) {
        Some(slot @ Some(_)) => {
            *slot = None;
            0
        }
        _ => abi::errno::EBADF,
    }
}

// What the `procfsd` thread (see `userfs`) does with the processes' tables.
