//! Running an app in a **ring 3 process**: the kernel side of the `wasmhost` program.
//!
//! The interpreter (`wasmi`), the app's linear memory and its fuel live in a user process
//! (`user-apps/wasmhost`); a bug in the interpreter or in the guest can crash that process and
//! nothing else. What needs authority stays here: `HostState` (the surface the app draws on, its
//! files, network, clipboard) and the `osj.*` implementation itself, which is the very same
//! function table the in-kernel runtime uses (`abi2::call`) running on a different [`Guest`]: the
//! process's page tables instead of a `wasmi` `Caller`.
//!
//! The `appd` thread drives a job at a time. It posts the job, then waits for the process to either
//! ask for a host call (served here, reading and writing the guest's memory through the process's
//! page tables, one page at a time) or report the job done. The wire format is
//! `kitsune_core::appabi::remote`; the blocking protocol is in `crate::userproc`.

use super::abi2::{self, Guest};
use super::{AppFault, Halt, HostState, Why};
use crate::userproc::{self, HostEvent};
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::cell::Cell;
use core::ops::Range;
use core::sync::atomic::{AtomicU64, Ordering};
use kitsune_core::appabi::remote::{
    CALL_EXIT, CALL_FAULT, DONE_EXIT, DONE_FAULT, DONE_LOAD_FAILED, DONE_OK, DONE_OUT_OF_FUEL,
    DONE_TRAP, Done, EXPORT_NAMES, HOST_FNS, HostCall, JOB_CALL, JOB_INIT, Job,
};
use kitsune_core::tk;

/// Whether v2 apps run in a process (when the image has the host and a process can be started).
/// When it is off, or starting one fails, they run in the kernel as before.
/// (Building with `KITSUNE_APPS_LOCAL=1` in the environment forces the kernel runtime, for A/B timing.)
const RING3_APPS: bool = option_env!("KITSUNE_APPS_LOCAL").is_none();

pub(crate) fn enabled() -> bool {
    RING3_APPS && userproc::embedded("wasmhost").is_some()
}

/// Cost counters for the `perf-trace` report: jobs, cycles inside jobs, host calls, cycles serving them.
pub(super) static STATS: [AtomicU64; 6] = [const { AtomicU64::new(0) }; 6];
/// The same for guests that run in the kernel: calls and cycles.
pub(super) static LOCAL_STATS: [AtomicU64; 4] = [const { AtomicU64::new(0) }; 4];
/// Jobs longer than this are one-off work (a directory scan, an image) and are kept out of the steady average.
pub(super) const STEADY_MAX: u64 = 20_000_000;

/// Calls and cycles per host function id.
pub(super) static BY_ID: [[AtomicU64; 2]; 32] =
    [const { [AtomicU64::new(0), AtomicU64::new(0)] }; 32];

/// One line for the `perf-trace` report.
pub(crate) fn log_stats() {
    let g = |a: &AtomicU64| a.load(Ordering::Relaxed);
    let (lc, lcy) = (g(&LOCAL_STATS[0]), g(&LOCAL_STATS[1]));
    let (j, jcy, h, hcy) = (g(&STATS[0]), g(&STATS[1]), g(&STATS[2]), g(&STATS[3]));
    crate::serial_println!(
        "[trace] wasm: in kernel {lc} calls, {} kcyc/call ({} steady); in process {j} jobs, {} kcyc/job \
         ({} steady), {h} host calls ({} per job), {} kcyc/call served",
        lcy / lc.max(1) / 1000,
        g(&LOCAL_STATS[3]) / g(&LOCAL_STATS[2]).max(1) / 1000,
        jcy / j.max(1) / 1000,
        g(&STATS[5]) / g(&STATS[4]).max(1) / 1000,
        h / j.max(1),
        hcy / h.max(1) / 1000
    );
    for (id, f) in HOST_FNS.iter().enumerate() {
        let (n, cy) = (g(&BY_ID[id][0]), g(&BY_ID[id][1]));
        if let Some(avg) = cy.checked_div(n) {
            crate::serial_println!(
                "[trace]   osj.{}: {n} calls, {} kcyc/call",
                f.name,
                avg / 1000
            );
        }
    }
}

/// Ticks without a word from the process before it is declared hung (20 s).
const HUNG_TICKS: u64 = 5_000;
/// Heap the host needs besides the guest's memory and the module: interpreter, stacks, buffers.
const HOST_SLACK: u64 = 6 * 1024 * 1024;

/// An app running in a process.
pub(super) struct RemoteRt {
    pid: u32,
    /// What `osj.*` reads and writes: the surface, the v2 state (files, net, clipboard).
    pub(super) state: HostState,
    /// Size of the guest's memory after the last job, in 64 KiB pages.
    pub(super) mem_pages: u64,
    /// Error of a host call that must end the job (fault, out of fuel, `exit`).
    pending: Option<wasmi::Error>,
}

impl Drop for RemoteRt {
    fn drop(&mut self) {
        userproc::kill(self.pid);
    }
}

/// A refusal to start: what the window shows.
pub(super) struct StartError(pub Why);

/// Start `wasm` in a new process and instantiate it. `state` is the app's host state.
pub(super) fn start(
    name: &str,
    wasm: &Arc<[u8]>,
    state: HostState,
    mem_bytes: usize,
    fuel: u64,
) -> Result<RemoteRt, StartError> {
    let heap = mem_bytes as u64 + 4 * wasm.len() as u64 + HOST_SLACK;
    let pid = match userproc::spawn_host(&alloc::format!("app:{name}"), wasm.clone(), heap) {
        Ok(p) => p,
        Err(e) => {
            crate::serial_println!("apps: no process for `{name}`: {e:?}");
            return Err(StartError(Why::new(tk!("apps.why.no_process"))));
        }
    };
    let mut rt = RemoteRt {
        pid,
        state,
        mem_pages: 0,
        pending: None,
    };
    let job = Job {
        win: ((rt.state.cw as u64) << 32) | u64::from(rt.state.ch as u32),
        lang: u64::from(kitsune_core::i18n::lang().index()),
        kind: JOB_INIT,
        fuel,
        mem_bytes: mem_bytes as u64,
        ..Job::default()
    };
    match rt.run(job) {
        Ok(_) => Ok(rt),
        // `rt` drops on return and kills the process.
        Err(e) => Err(StartError(Halt::from(e).why)),
    }
}

enum RunError {
    /// Instantiation or import checks failed (the process's words).
    Load(String),
    /// The guest or a host call ended the job.
    Trap(wasmi::Error),
    /// The process died or hung.
    Died,
}

impl From<RunError> for Halt {
    fn from(e: RunError) -> Halt {
        match e {
            RunError::Trap(e) => Halt::from(e),
            RunError::Load(m) => Halt {
                why: Why::with(tk!("apps.why.start_failed"), m),
                exited: false,
            },
            RunError::Died => Halt {
                why: Why::new(tk!("apps.why.proc_died")),
                exited: false,
            },
        }
    }
}

impl RemoteRt {
    /// Is the process still running?
    pub(super) fn alive(&self) -> bool {
        userproc::alive(self.pid)
    }

    /// Call export `name` with `args`. Returns the fuel the job used. A missing export is fine.
    pub(super) fn call(&mut self, name: &str, args: &[i32], fuel: u64) -> Result<u64, Halt> {
        let Some(export) = EXPORT_NAMES.iter().position(|n| *n == name) else {
            return Ok(0);
        };
        let mut a = [0i32; 4];
        for (slot, v) in a.iter_mut().zip(args) {
            *slot = *v;
        }
        let job = Job {
            win: ((self.state.cw as u64) << 32) | u64::from(self.state.ch as u32),
            lang: u64::from(kitsune_core::i18n::lang().index()),
            kind: JOB_CALL,
            export: export as u32,
            args: a,
            fuel,
            ..Job::default()
        };
        self.run(job).map_err(Halt::from)
    }

    /// Post `job` and serve the process until it reports the job done. Returns the fuel spent.
    fn run(&mut self, job: Job) -> Result<u64, RunError> {
        let pid = self.pid;
        let budget = job.fuel;
        let mut charged = 0u64;
        self.pending = None;
        if !userproc::host_post(pid, job) {
            return Err(RunError::Died);
        }
        let t0 = crate::io::rdtsc();
        let mut last = crate::interrupts::ticks();
        loop {
            match userproc::host_poll(pid) {
                HostEvent::Done(d, msg) => {
                    STATS[0].fetch_add(1, Ordering::Relaxed);
                    let took = crate::io::rdtsc().wrapping_sub(t0);
                    STATS[1].fetch_add(took, Ordering::Relaxed);
                    if took < STEADY_MAX {
                        STATS[4].fetch_add(1, Ordering::Relaxed);
                        STATS[5].fetch_add(took, Ordering::Relaxed);
                    }
                    return self.finish(d, msg, charged);
                }
                HostEvent::Call(calls) => {
                    self.serve(&calls, budget, &mut charged);
                    last = crate::interrupts::ticks();
                }
                HostEvent::Dead => return Err(RunError::Died),
                HostEvent::Nothing => {
                    if crate::interrupts::ticks().saturating_sub(last) > HUNG_TICKS {
                        crate::serial_println!("apps: process {pid} stopped answering, killed");
                        return Err(RunError::Died);
                    }
                    crate::sched::block(last + HUNG_TICKS + 1, || !userproc::host_ready(pid));
                }
            }
        }
    }

    fn finish(&mut self, d: Done, msg: Vec<u8>, charged: u64) -> Result<u64, RunError> {
        self.mem_pages = d.mem_pages;
        let spent = d.fuel_spent.saturating_add(charged);
        if let Some(e) = self.pending.take() {
            return Err(RunError::Trap(e));
        }
        let text = String::from_utf8_lossy(&msg).into_owned();
        match d.status {
            DONE_OK => Ok(spent),
            DONE_LOAD_FAILED => Err(RunError::Load(text)),
            DONE_OUT_OF_FUEL => Err(RunError::Trap(wasmi::Error::from(
                wasmi::TrapCode::OutOfFuel,
            ))),
            DONE_EXIT => Err(RunError::Trap(wasmi::Error::i32_exit(d.code))),
            DONE_TRAP | DONE_FAULT => {
                crate::serial_println!("apps: guest trapped: {text}");
                Err(RunError::Trap(wasmi::Error::host(AppFault(tk!(
                    "apps.why.trap"
                )))))
            }
            _ => Err(RunError::Died),
        }
    }

    /// Serve the call (or the batch of calls) the process is waiting on and let it go on.
    fn serve(&mut self, calls: &[HostCall], budget: u64, charged: &mut u64) {
        let mut reply = 0;
        for hc in calls {
            let t1 = crate::io::rdtsc();
            reply = match self.serve_one(hc, budget, charged) {
                Ok(v) => v,
                Err(v) => {
                    userproc::host_reply(self.pid, v);
                    return;
                }
            };
            let spent = crate::io::rdtsc().wrapping_sub(t1);
            STATS[2].fetch_add(1, Ordering::Relaxed);
            STATS[3].fetch_add(spent, Ordering::Relaxed);
            if let Some(b) = BY_ID.get(hc.id as usize) {
                b[0].fetch_add(1, Ordering::Relaxed);
                b[1].fetch_add(spent, Ordering::Relaxed);
            }
        }
        userproc::host_reply(self.pid, if calls.len() > 1 { 0 } else { reply });
    }

    /// One host call. `Err` carries the reply that makes the process stop (the error is kept).
    fn serve_one(&mut self, hc: &HostCall, budget: u64, charged: &mut u64) -> Result<i64, i64> {
        let mut g = ProcGuest {
            pid: self.pid,
            state: &mut self.state,
            base: hc.mem_base,
            len: hc.mem_len as usize,
            charged,
            budget,
            bad: Cell::new(false),
        };
        let args = hc.args.map(|v| v as i64);
        let mut r = abi2::call(&mut g, hc.id as usize, &args);
        if g.bad.get() && r.is_ok() {
            r = Err(wasmi::Error::host(AppFault(tk!("apps.why.bad_pointer"))));
        }
        r.map_err(|e| {
            let v = if e.i32_exit_status().is_some() {
                CALL_EXIT
            } else {
                CALL_FAULT
            };
            self.pending = Some(e);
            v
        })
    }
}

/// The guest as seen from the kernel: its linear memory is `len` bytes at `base` in the process.
struct ProcGuest<'a> {
    pid: u32,
    state: &'a mut HostState,
    base: u64,
    len: usize,
    charged: &'a mut u64,
    budget: u64,
    /// A copy failed (the process's page tables do not back the range it reported).
    bad: Cell<bool>,
}

impl Guest for ProcGuest<'_> {
    fn host(&self) -> &HostState {
        self.state
    }
    fn host_mut(&mut self) -> &mut HostState {
        self.state
    }
    fn mem_size(&self) -> usize {
        self.len
    }
    fn read_mem(&self, r: Range<usize>) -> Vec<u8> {
        userproc::host_read(self.pid, self.base + r.start as u64, r.len()).unwrap_or_else(|| {
            self.bad.set(true);
            alloc::vec![0; r.len()]
        })
    }
    fn write_mem(&mut self, at: usize, bytes: &[u8]) {
        if !userproc::host_write(self.pid, self.base + at as u64, bytes) {
            self.bad.set(true);
        }
    }
    fn charge(&mut self, units: u64) -> Result<(), wasmi::Error> {
        *self.charged = self.charged.saturating_add(units);
        if *self.charged > self.budget {
            return Err(wasmi::Error::from(wasmi::TrapCode::OutOfFuel));
        }
        Ok(())
    }
}
