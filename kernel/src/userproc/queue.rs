//! The file-system request queue and window events the compositor loop services for processes.

use super::*;

/// Take a queued file request: (pid, request, who it acts as). `None` when nothing is waiting.
pub(crate) fn fs_take() -> Option<(u32, FsReq, Option<Cred>)> {
    let mut procs = PROCS.lock();
    for p in procs.iter_mut() {
        if end_of(p) != End::Running {
            continue;
        }
        if matches!(p.fs, FsState::Pending(_))
            && let FsState::Pending(req) = core::mem::replace(&mut p.fs, FsState::Running)
        {
            return Some((p.pid, req, p.cred.clone()));
        }
    }
    None
}

/// Is a request waiting (what the daemon blocks on)?
pub(crate) fn fs_pending() -> bool {
    PROCS
        .lock()
        .iter()
        .any(|p| matches!(p.fs, FsState::Pending(_)) && end_of(p) == End::Running)
}

/// A copy of descriptor `fd` of process `pid`.
pub(crate) fn fs_fd(pid: u32, fd: i64) -> Option<Fd> {
    let i = usize::try_from(fd - abi::FIRST_FD).ok()?;
    PROCS
        .lock()
        .iter()
        .find(|p| p.pid == pid)?
        .fds
        .get(i)?
        .clone()
}

/// Replace descriptor `fd` of `pid` (after a read, write or seek moved it).
pub(crate) fn fs_set_fd(pid: u32, fd: i64, new: Fd) {
    let Ok(i) = usize::try_from(fd - abi::FIRST_FD) else {
        return;
    };
    if let Some(slot) = PROCS
        .lock()
        .iter_mut()
        .find(|p| p.pid == pid)
        .and_then(|p| p.fds.get_mut(i))
    {
        *slot = Some(new);
    }
}

/// Give `pid` the new descriptor `fd`: its number, or `EMFILE`.
pub(crate) fn fs_new_fd(pid: u32, fd: Fd) -> i64 {
    let mut procs = PROCS.lock();
    let Some(p) = procs.iter_mut().find(|p| p.pid == pid) else {
        return abi::errno::EBADF;
    };
    if let Some(i) = p.fds.iter().position(Option::is_none) {
        p.fds[i] = Some(fd);
        return abi::FIRST_FD + i as i64;
    }
    if p.fds.len() >= abi::MAX_FDS {
        return abi::errno::EMFILE;
    }
    p.fds.push(Some(fd));
    abi::FIRST_FD + p.fds.len() as i64 - 1
}

/// Finish the request of `pid` with `value` and let the process go on.
pub(crate) fn fs_done(pid: u32, value: i64) {
    let slot = {
        let mut procs = PROCS.lock();
        let Some(p) = procs.iter_mut().find(|p| p.pid == pid) else {
            return;
        };
        p.fs = FsState::Done(value);
        p.slot
    };
    crate::sched::wake(slot);
    crate::sched::hand_off(slot);
}

// ---- the window of a process ----

/// Queue an input event for the window of process `pid` and wake it if it waits (pointer motion
/// replaces the one before it).
pub(crate) fn win_push(pid: u32, ev: abi::Event) {
    let slot = {
        let mut procs = PROCS.lock();
        let Some(p) = procs.iter_mut().find(|p| p.pid == pid) else {
            return;
        };
        if ev.kind == abi::EV_POINTER
            && let Some(last) = p.events.back_mut()
            && last.kind == abi::EV_POINTER
            && last.c == ev.c
        {
            *last = ev;
        } else if ev.kind == abi::EV_RESIZE
            && let Some(last) = p.events.back_mut()
            && last.kind == abi::EV_RESIZE
        {
            *last = ev;
        } else if p.events.len() < 256 {
            p.events.push_back(ev);
        }
        p.slot
    };
    crate::sched::wake(slot);
}

/// How process `pid` ended (`Running` while it lives; a process no longer in the table exited).
pub(crate) fn end(pid: u32) -> End {
    PROCS
        .lock()
        .iter()
        .find(|p| p.pid == pid)
        .map_or(End::Exited(0), end_of)
}

/// The program's name and the window it owns (`0` = none).
pub(crate) fn win_of(pid: u32) -> Option<(String, u32)> {
    let procs = PROCS.lock();
    let p = procs.iter().find(|p| p.pid == pid)?;
    Some((p.name.clone(), p.win))
}

/// Remember the window `handle` of process `pid`.
pub(crate) fn set_win(pid: u32, handle: u32) {
    if let Some(p) = PROCS.lock().iter_mut().find(|p| p.pid == pid) {
        p.win = handle;
    }
}
