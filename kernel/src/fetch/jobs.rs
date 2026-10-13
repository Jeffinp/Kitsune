//! Network jobs for the shell (`nslookup`, `curl`, `wget`, `ping` by name): the blocking mailbox
//! the terminal's thread uses, served by the fetcher worker.

use super::*;

// ---- network jobs for the shell (`nslookup`, `curl`, `wget`, `ping` by name) ----
//
// The browser's mailbox above belongs to the compositor. The terminal's commands run on
// their own thread (`shelld`) and may wait for the network, so they get a second, blocking
// mailbox served by the same worker (the only party that may touch the NIC):
//
// ```text
// IDLE --claim--> CLAIMED --post--> REQUESTED --worker--> RUNNING --worker--> DONE --take--> IDLE
//                                       |  (caller gave up before the worker started: back to IDLE)
//                                       +-- RUNNING --caller gave up--> ABANDONED --worker--> IDLE
// ```
//
// An abandoned job still runs to its own end (a fetch cannot be interrupted half way), but its
// result is dropped, and nobody can post another job until the worker has returned the mailbox
// to IDLE.

const J_IDLE: u8 = 0;
const J_CLAIMED: u8 = 1;
const J_REQUESTED: u8 = 2;
const J_RUNNING: u8 = 3;
const J_DONE: u8 = 4;
const J_ABANDONED: u8 = 5;

static JSTATE: AtomicU8 = AtomicU8::new(J_IDLE);
static JOB: RacyCell<Option<NetJob>> = RacyCell::new(None);
static JRESULT: RacyCell<Option<NetJobResult>> = RacyCell::new(None);

/// What the shell asks the network owner to do.
pub enum NetJob {
    /// Resolve a host name (or a dotted-quad literal) to an IPv4 address.
    Resolve(alloc::string::String),
    /// GET a `http://` or `https://` URL, following redirects.
    Get(alloc::string::String),
}

/// What came back.
pub enum NetJobResult {
    Addr(Option<[u8; 4]>),
    Page(Loaded),
    Failed(FailReason),
}

/// Why [`run_job`] gave up without an answer.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum JobError {
    /// No NIC, or the worker thread died.
    NoNetwork,
    /// Another job (or a page load) kept the mailbox busy for too long.
    Busy,
    /// The caller's `cancel` turned true.
    Cancelled,
    /// No answer within [`JOB_TIMEOUT_MS`].
    Timeout,
}

/// Longest [`run_job`] waits for the worker (a TLS page load is the slow case).
pub const JOB_TIMEOUT_MS: u64 = 90_000;
/// Longest [`run_job`] waits for the mailbox to be free.
const JOB_CLAIM_MS: u64 = 10_000;

fn ticks_ms(ms: u64) -> u64 {
    ms * u64::from(crate::interrupts::TIMER_HZ) / 1000
}

/// Run `job` on the network owner and wait for the answer. Blocking: call it from a worker
/// thread (the compositor would freeze). `cancel` is polled about 20 times a second; when it
/// turns true the call returns at once.
pub fn run_job(job: NetJob, cancel: impl Fn() -> bool) -> Result<NetJobResult, JobError> {
    use crate::interrupts::ticks;
    if OFFLINE.load(Ordering::Acquire) || worker_dead() {
        return Err(JobError::NoNetwork);
    }
    let claim_limit = ticks() + ticks_ms(JOB_CLAIM_MS);
    while JSTATE
        .compare_exchange(J_IDLE, J_CLAIMED, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        if cancel() {
            return Err(JobError::Cancelled);
        }
        if ticks() >= claim_limit {
            return Err(JobError::Busy);
        }
        crate::sched::block(ticks() + ticks_ms(50), || true);
    }
    // SAFETY: JSTATE is CLAIMED, which only the winner of the compare-exchange above holds: the
    // worker reads JOB only after seeing REQUESTED (Acquire), stored below after this write.
    unsafe {
        *JOB.get() = Some(job);
    }
    JSTATE.store(J_REQUESTED, Ordering::Release);
    wake_worker();
    let limit = ticks() + ticks_ms(JOB_TIMEOUT_MS);
    loop {
        if JSTATE.load(Ordering::Acquire) == J_DONE {
            // SAFETY: DONE (Acquire) pairs with the worker's Release store after it wrote JRESULT, and
            // the worker does not touch it again until we set IDLE below.
            let r = unsafe { (*JRESULT.get()).take() };
            JSTATE.store(J_IDLE, Ordering::Release);
            return Ok(r.unwrap_or(NetJobResult::Failed(FailReason::Network)));
        }
        let gave_up = if worker_dead() {
            Some(JobError::NoNetwork)
        } else if cancel() {
            Some(JobError::Cancelled)
        } else if ticks() >= limit {
            Some(JobError::Timeout)
        } else {
            None
        };
        if let Some(why) = gave_up {
            // Not started yet: withdraw it. Running: let it finish but drop the answer.
            if JSTATE
                .compare_exchange(J_REQUESTED, J_IDLE, Ordering::AcqRel, Ordering::Acquire)
                .is_err()
                && JSTATE
                    .compare_exchange(J_RUNNING, J_ABANDONED, Ordering::AcqRel, Ordering::Acquire)
                    .is_err()
                && JSTATE.load(Ordering::Acquire) == J_DONE
            {
                continue; // it finished just now: take the answer
            }
            return Err(why);
        }
        crate::sched::block(ticks() + ticks_ms(50), || {
            JSTATE.load(Ordering::Acquire) != J_DONE
        });
    }
}

/// One shell job, on the worker thread.
pub(super) fn serve_job(net: &mut netstack::Net) {
    if JSTATE
        .compare_exchange(J_REQUESTED, J_RUNNING, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        return;
    }
    // SAFETY: REQUESTED -> RUNNING was won by this thread, so the poster finished writing JOB
    // (Release before REQUESTED) and will not touch it again.
    let job = unsafe { (*JOB.get()).take() };
    let result = match job {
        Some(NetJob::Resolve(host)) => {
            serial_println!("shell: resolve {}", host);
            NetJobResult::Addr(net.resolve(&host).and_then(|ip| match ip {
                smoltcp::wire::IpAddress::Ipv4(a) => Some(a.octets()),
                #[allow(unreachable_patterns)]
                _ => None,
            }))
        }
        Some(NetJob::Get(url)) => {
            match fetch_url(
                net,
                url.as_bytes(),
                b"",
                MAX_RESPONSE_BYTES,
                None,
                None,
                cookies::Use::None,
            ) {
                Ok(p) => NetJobResult::Page(p),
                Err(e) => NetJobResult::Failed(e),
            }
        }
        None => NetJobResult::Failed(FailReason::Network),
    };
    // SAFETY: still RUNNING (or ABANDONED): the caller reads JRESULT only after DONE, stored below.
    unsafe {
        *JRESULT.get() = Some(result);
    }
    if JSTATE
        .compare_exchange(J_RUNNING, J_DONE, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        // The caller gave up (ABANDONED): drop the answer and free the mailbox.
        // SAFETY: nobody waits for this result any more.
        unsafe {
            *JRESULT.get() = None;
        }
        JSTATE.store(J_IDLE, Ordering::Release);
    }
}

/// A shell job is waiting for the worker (part of its block condition).
pub(super) fn job_pending() -> bool {
    JSTATE.load(Ordering::Acquire) == J_REQUESTED
}
