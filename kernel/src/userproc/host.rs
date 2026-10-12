//! WebAssembly host processes: the call channel between the host and the module it runs.

use super::*;

/// Calls 16–20 (see [`remote`]), for a process that has a [`HostLink`].
pub(super) fn host_call(r: &mut Regs, rsp: u64, slot: usize) -> u64 {
    let (number, a, b) = (r.rax, r.rdi, r.rsi);
    let mut procs = PROCS.lock();
    let Some(p) = procs.iter_mut().find(|p| p.slot == slot) else {
        r.rax = EINVAL as u64;
        return rsp;
    };
    let (Some(asp), Some(link)) = (p.aspace.as_ref(), p.host.as_mut()) else {
        r.rax = ENOSYS as u64;
        return rsp;
    };
    let mut wake_appd = false;
    let mut block = false;
    r.rax = match r.rax {
        remote::SYS_HOST_MODULE_LEN => link.module.len() as u64,
        remote::SYS_HOST_MODULE_READ => {
            let n = (b as usize).min(link.module.len() - link.module_pos);
            match asp.poke(a, &link.module[link.module_pos..link.module_pos + n]) {
                Ok(()) => {
                    link.module_pos += n;
                    n as u64
                }
                Err(_) => EFAULT as u64,
            }
        }
        remote::SYS_HOST_BATCH => match link.call {
            CallState::Idle => {
                let n = b as usize;
                let mut raw = alloc::vec![0u8; n.min(remote::MAX_BATCH) * 72];
                if n == 0 || n > remote::MAX_BATCH || asp.copy_in(a, &mut raw).is_err() {
                    r.rax = EINVAL as u64;
                    return rsp;
                }
                let calls = raw
                    .as_chunks::<72>()
                    .0
                    .iter()
                    .map(HostCall::from_bytes)
                    .collect();
                link.call = CallState::Pending(calls);
                wake_appd = true;
                block = true;
                0
            }
            CallState::Served(v) => {
                link.call = CallState::Idle;
                v as u64
            }
            _ => {
                block = true;
                0
            }
        },
        remote::SYS_HOST_JOB_WAIT => match link.job {
            Some(job) => match asp.poke(a, &job.to_bytes()) {
                Ok(()) => {
                    link.job = None;
                    0
                }
                Err(_) => EFAULT as u64,
            },
            None => {
                block = true;
                0
            }
        },
        remote::SYS_HOST_CALL => match core::mem::replace(&mut link.call, CallState::Idle) {
            CallState::Idle => {
                let mut raw = [0u8; 72];
                if asp.copy_in(a, &mut raw).is_err() {
                    r.rax = EFAULT as u64;
                    return rsp;
                }
                link.call = CallState::Pending(alloc::vec![HostCall::from_bytes(&raw)]);
                wake_appd = true;
                block = true;
                0
            }
            CallState::Served(v) => v as u64,
            waiting => {
                link.call = waiting;
                block = true;
                0
            }
        },
        _ => {
            // SYS_HOST_JOB_DONE
            let mut raw = [0u8; 40];
            if asp.copy_in(a, &mut raw).is_err() {
                r.rax = EFAULT as u64;
                return rsp;
            }
            let done = Done::from_bytes(&raw);
            let mut msg = alloc::vec![0u8; (done.msg_len as usize).min(remote::MAX_MSG)];
            if !msg.is_empty() && asp.copy_in(done.msg_ptr, &mut msg).is_err() {
                msg.clear();
            }
            link.done = Some((done, msg));
            wake_appd = true;
            0
        }
    };
    drop(procs);
    if wake_appd {
        crate::wasm::wake_worker();
        crate::wasm::hand_off_to_worker();
    }
    if block {
        return block_restart(r, rsp, slot, number);
    }
    rsp
}

pub(super) fn host_of<T>(pid: u32, f: impl FnOnce(&mut Proc, &mut HostLink) -> T) -> Option<T> {
    let mut procs = PROCS.lock();
    let p = procs.iter_mut().find(|p| p.pid == pid)?;
    let mut link = p.host.take()?;
    let out = f(p, &mut link);
    p.host = Some(link);
    Some(out)
}

/// Is process `pid` still there and running?
pub fn alive(pid: u32) -> bool {
    PROCS
        .lock()
        .iter()
        .any(|p| p.pid == pid && end_of(p) == End::Running)
}

/// Give the host process `pid` its next job. `false` if it is gone or the previous job is unread.
pub fn host_post(pid: u32, job: Job) -> bool {
    let slot = host_of(pid, |p, l| {
        if l.job.is_some() || end_of(p) != End::Running {
            return None;
        }
        l.job = Some(job);
        Some(p.slot)
    });
    match slot.flatten() {
        Some(s) => {
            crate::sched::wake(s);
            crate::sched::hand_off(s);
            true
        }
        None => false,
    }
}

/// What the host process did since the last look (takes a finished job, marks a call as taken).
pub fn host_poll(pid: u32) -> HostEvent {
    host_of(pid, |p, l| {
        if let Some((d, m)) = l.done.take() {
            return HostEvent::Done(d, m);
        }
        if matches!(l.call, CallState::Pending(_))
            && let CallState::Pending(calls) = core::mem::replace(&mut l.call, CallState::Serving)
        {
            return HostEvent::Call(calls);
        }
        if end_of(p) != End::Running {
            return HostEvent::Dead;
        }
        HostEvent::Nothing
    })
    .unwrap_or(HostEvent::Dead)
}

/// Is there something for `appd` to look at? (the condition `appd` blocks on)
pub fn host_ready(pid: u32) -> bool {
    host_of(pid, |p, l| {
        l.done.is_some() || matches!(l.call, CallState::Pending(_)) || end_of(p) != End::Running
    })
    .unwrap_or(true)
}

/// Answer the call `appd` is serving and let the process go on.
pub fn host_reply(pid: u32, value: i64) {
    let slot = host_of(pid, |p, l| {
        l.call = CallState::Served(value);
        p.slot
    });
    if let Some(s) = slot {
        crate::sched::wake(s);
        crate::sched::hand_off(s);
    }
}

/// Bytes copied per lock hold, so interrupts are never off for long.
pub(super) const CHUNK: usize = 4096;

/// Copy `len` bytes at user address `va` of process `pid`. `None` if any page is not mapped.
pub fn host_read(pid: u32, va: u64, len: usize) -> Option<Vec<u8>> {
    let mut out = alloc::vec![0u8; len];
    let mut done = 0;
    while done < len {
        let n = (len - done).min(CHUNK);
        let at = va.checked_add(done as u64)?;
        let procs = PROCS.lock();
        let asp = procs.iter().find(|p| p.pid == pid)?.aspace.as_ref()?;
        asp.copy_in(at, &mut out[done..done + n]).ok()?;
        done += n;
    }
    Some(out)
}

/// Copy `bytes` to user address `va` of process `pid`. `false` if any page is not mapped.
pub fn host_write(pid: u32, va: u64, bytes: &[u8]) -> bool {
    for (k, chunk) in bytes.chunks(CHUNK).enumerate() {
        let Some(at) = va.checked_add((k * CHUNK) as u64) else {
            return false;
        };
        let procs = PROCS.lock();
        let Some(asp) = procs
            .iter()
            .find(|p| p.pid == pid)
            .and_then(|p| p.aspace.as_ref())
        else {
            return false;
        };
        if asp.poke(at, chunk).is_err() {
            return false;
        }
    }
    true
}

// ---- files and arguments (calls 32-42) ----
