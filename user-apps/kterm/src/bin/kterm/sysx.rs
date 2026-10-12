//! `SysInfo` for the terminal: what a program can learn and do through system calls. Waiting
//! (`sleep`, a running child) keeps reading the window's events, so Ctrl+C stops the command and
//! the window stays alive.

use crate::child;
use alloc::collections::VecDeque;
use alloc::string::String;
use core::cell::{Cell, RefCell};
use kitsune_core::hw::rtc::DateTime as Civil;
use kitsune_core::shell::sys::{DateTime, MemInfo, ProgramCtx, ProgramPoll, SpawnFail, SysInfo};
use kitsune_user::abi::{EV_CLOSE, EV_KEY, Event};
use kitsune_user::{local_time_secs, uptime_ms, win};

/// The window's events, read while a command runs.
pub struct Pump {
    /// Events that are not for the command (keys, resizes...), kept for the main loop.
    queue: RefCell<VecDeque<Event>>,
    cancel: Cell<bool>,
    closed: Cell<bool>,
}

impl Pump {
    pub fn new() -> Pump {
        Pump {
            queue: RefCell::new(VecDeque::new()),
            cancel: Cell::new(false),
            closed: Cell::new(false),
        }
    }

    /// Read the next event (waiting up to `timeout_ms`) and sort it: Ctrl+C and the close request
    /// stop the command, the rest is kept.
    fn pump(&self, timeout_ms: u64) {
        if let Ok(Some(ev)) = win::next_event(timeout_ms) {
            match ev.kind {
                EV_CLOSE => {
                    self.closed.set(true);
                    self.cancel.set(true);
                }
                EV_KEY if ev.b & 2 != 0 && ev.b & 4 == 0 && (ev.a == 99 || ev.a == 67) => {
                    self.cancel.set(true);
                }
                _ => self.queue.borrow_mut().push_back(ev),
            }
        }
    }

    /// Start of a command: a Ctrl+C meant for an earlier one is forgotten.
    pub fn arm(&self) {
        self.cancel.set(false);
    }

    pub fn next_queued(&self) -> Option<Event> {
        self.queue.borrow_mut().pop_front()
    }

    pub fn closed(&self) -> bool {
        self.closed.get()
    }
}

pub struct UserSys<'a> {
    pub pump: &'a Pump,
}

impl SysInfo for UserSys<'_> {
    fn now(&self) -> DateTime {
        let d = Civil::from_epoch(local_time_secs());
        DateTime {
            year: d.date.y,
            month: d.date.m,
            day: d.date.d,
            hour: d.time.h,
            minute: d.time.m,
            second: d.time.s,
        }
    }

    fn uptime_ms(&self) -> u64 {
        uptime_ms()
    }

    fn mem(&self) -> MemInfo {
        MemInfo::default()
    }

    fn interrupted(&self) -> bool {
        self.pump.pump(0);
        self.pump.cancel.get()
    }

    fn spawn_program(
        &mut self,
        name: &str,
        image: Option<&[u8]>,
        ctx: &ProgramCtx,
    ) -> Result<u32, SpawnFail> {
        child::spawn(name, image, ctx)
    }

    fn poll_program(&mut self, pid: u32) -> ProgramPoll {
        child::poll(pid)
    }

    fn kill_program(&mut self, pid: u32) {
        child::kill(pid);
    }

    fn sleep_ms(&mut self, ms: u64) {
        let end = uptime_ms().saturating_add(ms);
        while !self.pump.cancel.get() {
            let left = end.saturating_sub(uptime_ms());
            if left == 0 {
                break;
            }
            self.pump.pump(left.min(50));
        }
    }

    fn hostname(&self) -> String {
        String::from("kitsune")
    }
}
