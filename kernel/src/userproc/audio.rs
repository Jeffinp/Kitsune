//! Sound for ring 3 programs (calls 60–65): streams of the kernel mixer (`crate::audio`), owned by
//! the process that opened them. The contract is `kitsune_userabi::audio`.
//!
//! Writes never block (they take what fits, `EAGAIN` when nothing does); `audio_wait` is the one
//! call that blocks, and it is restarted on wake like `page_recv` (the deadline is kept in the
//! waiter entry, so a restart does not extend it). Any process may play; the limits are the
//! mixer's (4 streams per process, 256 KiB queued per stream, 16 streams in all). The system
//! volume and mute are what the user hears from everything, and any program of the session may set
//! them, like the volume keys of a keyboard.

use super::*;
use crate::audio::{self as snd, Waiter};
use abi::errno::{EAGAIN, EBADF, ENODEV};

/// Calls 60–65.
pub(super) fn audio_call(r: &mut Regs, rsp: u64, slot: usize) -> u64 {
    let number = r.rax;
    let (a, b, c) = (r.rdi, r.rsi, r.rdx);
    let Some(pid) = PROCS.lock().iter().find(|p| p.slot == slot).map(|p| p.pid) else {
        r.rax = EINVAL as u64;
        return rsp;
    };
    if !snd::present() {
        r.rax = ENODEV as u64;
        return rsp;
    }
    // Stream ids are 31-bit; anything else names no stream.
    let id = u32::try_from(a).unwrap_or(u32::MAX);
    let res = match number {
        abi::SYS_AUDIO_OPEN => open(pid, a, b, c),
        abi::SYS_AUDIO_WRITE => write(slot, pid, id, b, c),
        abi::SYS_AUDIO_STATUS => status(slot, pid, id, b),
        abi::SYS_AUDIO_CLOSE => {
            let r = snd::with(|s| {
                s.waiters.retain(|w| !(w.owner == pid && w.id == id));
                s.mixer.close(pid, id, b != 0)
            });
            snd::kick();
            match r {
                Some(Ok(())) => 0,
                Some(Err(e)) => snd::errno(e),
                None => ENODEV,
            }
        }
        abi::SYS_AUDIO_CTL => ctl(pid, id, b, c),
        _ => return wait(r, rsp, slot, pid, id),
    };
    r.rax = res as u64;
    rsp
}

fn open(pid: u32, rate: u64, channels: u64, format: u64) -> i64 {
    if format != abi::AUDIO_S16LE {
        return EINVAL;
    }
    let (Ok(rate), Ok(channels)) = (u32::try_from(rate), u32::try_from(channels)) else {
        return EINVAL;
    };
    match snd::with(|s| s.mixer.open(pid, rate, channels)) {
        Some(Ok(id)) => {
            crate::klog!(
                Info,
                "audio: pid {pid} opened stream {id} ({rate} Hz, {channels} ch)"
            );
            snd::kick();
            i64::from(id)
        }
        Some(Err(e)) => snd::errno(e),
        None => ENODEV,
    }
}

fn write(slot: usize, pid: u32, id: u32, ptr: u64, len: u64) -> i64 {
    let len = (len as usize).min(abi::MAX_AUDIO_WRITE);
    // Copy only what fits: the queue's room, checked first.
    let room = match snd::with(|s| s.mixer.stream(pid, id).map(|st| st.free_bytes())) {
        Some(Ok(r)) => r,
        Some(Err(e)) => return snd::errno(e),
        None => return ENODEV,
    };
    let n = len.min(room);
    if n == 0 {
        return if len == 0 { 0 } else { EAGAIN };
    }
    let mut buf = alloc::vec![0u8; n];
    {
        let procs = PROCS.lock();
        let Some(asp) = procs
            .iter()
            .find(|p| p.slot == slot)
            .and_then(|p| p.aspace.as_ref())
        else {
            return EINVAL;
        };
        if asp.copy_in(ptr, &mut buf).is_err() {
            return EFAULT;
        }
    }
    let r = snd::with(|s| s.mixer.write(pid, id, &buf));
    snd::kick();
    match r {
        Some(Ok(n)) => n as i64,
        Some(Err(e)) => snd::errno(e),
        None => ENODEV,
    }
}

fn status(slot: usize, pid: u32, id: u32, out: u64) -> i64 {
    let st = match snd::with(|s| s.mixer.status(pid, id)) {
        Some(Ok(st)) => st,
        Some(Err(e)) => return snd::errno(e),
        None => return ENODEV,
    };
    let rec = abi::AudioStatus {
        queued_bytes: st.queued_bytes.min(u64::from(u32::MAX)) as u32,
        free_bytes: st.free_bytes.min(u64::from(u32::MAX)) as u32,
        played: st.played,
        consumed: st.consumed,
        written: st.written,
        underruns: st.underruns,
        rate: st.rate,
        channels: u32::from(st.channels),
        flags: abi::status_flags(st.volume, st.paused, st.draining),
    };
    let procs = PROCS.lock();
    match procs
        .iter()
        .find(|p| p.slot == slot)
        .and_then(|p| p.aspace.as_ref())
    {
        Some(asp) => match asp.poke(out, &rec.to_bytes()) {
            Ok(()) => 0,
            Err(_) => EFAULT,
        },
        None => EINVAL,
    }
}

fn ctl(pid: u32, id: u32, op: u64, value: u64) -> i64 {
    let v = u32::try_from(value).unwrap_or(u32::MAX);
    let r = snd::with(|s| -> Result<i64, kitsune_core::hw::audio::AudioErr> {
        let m = &mut s.mixer;
        Ok(match op {
            abi::CTL_SET_VOLUME => m.set_volume(pid, id, v).map(|()| 0)?,
            abi::CTL_GET_VOLUME => i64::from(m.stream(pid, id)?.volume()),
            abi::CTL_PAUSE => m.set_paused(pid, id, value != 0).map(|()| 0)?,
            abi::CTL_FLUSH => m.flush(pid, id).map(|()| 0)?,
            abi::CTL_SET_MASTER => m.set_master(v).map(|()| 0)?,
            abi::CTL_GET_MASTER => i64::from(m.master()),
            abi::CTL_SET_MUTE => {
                m.set_muted(value != 0);
                0
            }
            abi::CTL_GET_MUTE => i64::from(m.muted()),
            abi::CTL_DEVICE_RATE => i64::from(m.rate()),
            _ => return Ok(EINVAL),
        })
    });
    if op == abi::CTL_SET_MUTE {
        snd::set_muted(value != 0); // the codec's own mute too
    }
    snd::kick();
    match r {
        Some(Ok(v)) => v,
        Some(Err(e)) => snd::errno(e),
        None => ENODEV,
    }
}

/// What `audio_wait` decided.
enum WaitStep {
    Done(i64),
    Block(u64),
}

/// `audio_wait(id, want, timeout_ms)`: 1 when the condition holds, 0 at the timeout.
fn wait(r: &mut Regs, rsp: u64, slot: usize, pid: u32, id: u32) -> u64 {
    let number = r.rax;
    let (want, timeout) = (r.rsi, r.rdx);
    let now = crate::interrupts::ticks();
    let step = snd::with(|s| {
        let pos = s
            .waiters
            .iter()
            .position(|w| w.slot == slot && w.owner == pid && w.id == id);
        if s.mixer.stream(pid, id).is_err() {
            if let Some(i) = pos {
                s.waiters.swap_remove(i);
            }
            return WaitStep::Done(EBADF);
        }
        if s.mixer.ready(pid, id, want) {
            if let Some(i) = pos {
                s.waiters.swap_remove(i);
            }
            return WaitStep::Done(1);
        }
        let deadline = match pos {
            Some(i) => s.waiters[i].deadline,
            None => {
                let d = if timeout == u64::MAX {
                    crate::sched::FOREVER
                } else {
                    now.saturating_add(
                        (timeout.min(3_600_000) * u64::from(crate::interrupts::TIMER_HZ))
                            .div_ceil(1000),
                    )
                };
                s.waiters.push(Waiter {
                    owner: pid,
                    id,
                    slot,
                    want,
                    deadline: d,
                });
                d
            }
        };
        if timeout == 0 || now >= deadline {
            s.waiters
                .retain(|w| !(w.slot == slot && w.owner == pid && w.id == id));
            return WaitStep::Done(0);
        }
        WaitStep::Block(deadline)
    })
    .unwrap_or(WaitStep::Done(ENODEV));
    match step {
        WaitStep::Done(v) => {
            r.rax = v as u64;
            rsp
        }
        WaitStep::Block(deadline) => {
            snd::kick();
            // Restart this call when `audiod` wakes us (condition met) or at the deadline.
            r.rax = number;
            r.rip -= 2;
            crate::sched::set_wake(slot, deadline);
            crate::sched::yield_switch(rsp)
        }
    }
}
