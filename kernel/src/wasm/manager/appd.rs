//! `appd`: the thread that runs every app, one slice at a time (round-robin), plus the
//! scheduling decisions it makes under the slot lock.

use super::*;

/// Is slot `s` ready to run a slice at `now`?
fn ready(s: &Slot, now: u64) -> bool {
    if s.closing || s.restart {
        return false;
    }
    match s.state {
        State::Starting => true,
        State::Running => {
            if !s.events.is_empty() || s.want_redraw {
                return true;
            }
            if (s.req_w, s.req_h) != (s.cur_w, s.cur_h)
                && now.saturating_sub(s.req_at) >= RESIZE_SETTLE_TICKS
            {
                return true;
            }
            if s.v1 {
                return now >= s.last_frame + FRAME_TICKS;
            }
            tick_due(s, now)
        }
        _ => false,
    }
}

fn tick_due(s: &Slot, now: u64) -> bool {
    let ms = s.manifest.tick_ms as u64;
    ms != 0 && now >= s.last_tick + ms.div_ceil(4)
}

/// Earliest tick at which some slot becomes ready on its own (`FOREVER` if none).
fn next_deadline() -> u64 {
    with(|slots| {
        let mut d = crate::sched::FOREVER;
        for s in slots.iter().flatten() {
            if s.closing || s.restart || s.state != State::Running {
                continue;
            }
            if s.v1 {
                d = d.min(s.last_frame + FRAME_TICKS);
            } else if s.manifest.tick_ms != 0 {
                d = d.min(s.last_tick + (s.manifest.tick_ms as u64).div_ceil(4));
            }
            if (s.req_w, s.req_h) != (s.cur_w, s.cur_h) {
                d = d.min(s.req_at + RESIZE_SETTLE_TICKS);
            }
        }
        d
    })
}

fn any_ready() -> bool {
    if rts().iter().flatten().any(|rt| rt.is_dead()) {
        return true;
    }
    let now = crate::interrupts::ticks();
    with(|slots| {
        slots
            .iter()
            .flatten()
            .any(|s| ready(s, now) || s.closing || s.restart)
    })
}

/// `appd` entry point: the single owner of every wasm runtime.
pub extern "C" fn worker() -> ! {
    TID.store(crate::sched::current(), Ordering::Release);
    let mut start = 0usize;
    loop {
        housekeeping();
        let mut ran = false;
        for k in 0..MAX_APPS {
            if run_slice((start + k) % MAX_APPS) {
                ran = true;
            }
        }
        start = (start + 1) % MAX_APPS;
        if !ran {
            let deadline = next_deadline();
            crate::sched::block(deadline, || !any_ready());
        }
    }
}

/// Drop the runtime of closing/restarting apps (frees their memory at once).
fn housekeeping() {
    // A process that died while its app was idle (killed from Tarefas, a fault): end the app.
    for i in 0..MAX_APPS {
        if rts()[i].as_ref().is_some_and(|rt| rt.is_dead()) {
            finish(i, State::Crashed, Why::new(tk!("apps.why.proc_died")));
        }
    }
    for i in 0..MAX_APPS {
        let act = with(|slots| match slots[i].as_mut() {
            Some(s) if s.closing && !s.rt_dropped => 1,
            Some(s) if s.restart && !s.closing => {
                s.restart = false;
                2
            }
            _ => 0,
        });
        match act {
            1 => {
                rts()[i] = None;
                with(|slots| {
                    if let Some(s) = slots[i].as_mut() {
                        s.rt_dropped = true;
                    }
                });
            }
            2 => {
                rts()[i] = None;
                with(|slots| {
                    if let Some(s) = slots[i].as_mut() {
                        s.state = State::Starting;
                        s.reason = Why::default();
                        s.events.clear();
                        s.want_redraw = true;
                        s.ready = false;
                        s.rt_dropped = true;
                        s.cur_w = 0;
                        s.cur_h = 0;
                        s.title.clear();
                    }
                });
            }
            _ => {}
        }
    }
}

/// End the app in slot `i`: drop its runtime now, remember why.
fn finish(i: usize, state: State, reason: Why) {
    rts()[i] = None;
    with(|slots| {
        if let Some(s) = slots[i].as_mut() {
            serial_println!(
                "apps: `{}` {}: {}",
                s.manifest.id,
                if state == State::Crashed {
                    "crashed"
                } else {
                    "exited"
                },
                reason.log()
            );
            s.state = state;
            s.reason = reason;
            s.rt_dropped = true;
            s.events.clear();
            s.want_redraw = false;
        }
    });
}

fn run_slice(i: usize) -> bool {
    let now = crate::interrupts::ticks();
    let job = with(|slots| {
        let s = slots[i].as_mut()?;
        if !ready(s, now) {
            return None;
        }
        let starting = s.state == State::Starting;
        let mut events = Vec::new();
        for _ in 0..SLICE_EVENTS {
            match s.events.pop_front() {
                Some(e) => events.push(e),
                None => break,
            }
        }
        let resize = ((s.req_w, s.req_h) != (s.cur_w, s.cur_h)
            && (starting || now.saturating_sub(s.req_at) >= RESIZE_SETTLE_TICKS))
            .then_some((s.req_w, s.req_h));
        let tick = (!starting && !s.v1 && tick_due(s, now)).then(|| {
            let dt = (now.saturating_sub(s.last_tick) * 4).min(i32::MAX as u64) as i32;
            s.last_tick = now;
            dt
        });
        let frame = s.v1 && now >= s.last_frame + FRAME_TICKS;
        let redraw = core::mem::take(&mut s.want_redraw);
        Some(Job {
            starting,
            v1: s.v1,
            quotas: s.quotas,
            events,
            redraw,
            resize,
            tick,
            frame,
        })
    });
    let Some(job) = job else {
        return false;
    };
    let t0 = io::rdtsc();

    // ---- start-up ----
    if job.starting {
        match build_runtime(i) {
            Ok(rt) => {
                // The window may have been closed while the module was loading (the slot
                // can even be gone already): then the new runtime is dropped right away.
                let alive = with(|slots| match slots[i].as_mut() {
                    Some(s) if !s.closing => {
                        s.state = if s.visible {
                            State::Running
                        } else {
                            State::Suspended
                        };
                        s.rt_dropped = false;
                        s.started_tick = now;
                        s.last_tick = now;
                        true
                    }
                    _ => false,
                });
                if !alive {
                    return true;
                }
                rts()[i] = Some(rt);
            }
            Err(why) => {
                finish(i, State::Crashed, why);
                return true;
            }
        }
    }

    // ---- surfaces ----
    if let Some((w, h)) = job.resize
        && !resize_surfaces(i, w, h)
    {
        finish(
            i,
            State::Crashed,
            Why::new(tk!("apps.why.no_window_memory")),
        );
        return true;
    }

    // ---- run the guest ----
    let outcome = run_guest(i, &job);
    let spent = io::rdtsc().wrapping_sub(t0);
    match outcome {
        Ok(rendered) => {
            if rendered {
                publish(i);
            }
            account(i, spent, now);
            sync_title(i);
        }
        Err(halt) => {
            account(i, spent, now);
            let state = if halt.exited {
                State::Exited
            } else {
                State::Crashed
            };
            finish(i, state, halt.why);
        }
    }
    true
}
