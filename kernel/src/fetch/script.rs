//! Network requests of a page's scripts (`fetch`, `XMLHttpRequest`, `sendBeacon`).
//!
//! The browser takes an `F` message from a page's script process, asks the ad blocker, and queues the
//! request here ([`post_script_fetch`]); the fetcher thread serves the queue in order, after the
//! browser's own requests (page, sheets, scripts, pictures) and the shell's jobs, one request at a
//! time (`serve.rs`). The answer goes **straight to the script process** through its message queue
//! (`userproc::page_try_post`): the head as JSON, the body in pieces of up to [`PIECE`] bytes as
//! binary messages, then the end. When the process's queue is full the fetcher waits, which is the
//! back-pressure of a stream nobody reads. An abort (`AbortController`, `xhr.abort()`, the page
//! going away) stops the transfer and closes its connection.
//!
//! Each request follows the same-origin rules (`kitsune_core::cors`: `Origin`, pre-flight,
//! `Access-Control-Allow-*`), the cookie rules of the jar (`cookies.rs`, with third-party cookies
//! blocked when the page's shield is on) and the redirect rules (at most [`MAX_HOPS`], no `https` page
//! reading `http`). See `docs/design/tls-browser.md` §13.

use super::*;
use crate::allocator::SpinLock;
use alloc::collections::VecDeque;
use kitsune_core::web::script::FetchRequest;

mod serve;

/// Most requests waiting.
const MAX_QUEUED: usize = 256;
/// Largest body piece sent in one message.
pub const PIECE: usize = 256 * 1024;
/// Most redirects followed.
pub const MAX_HOPS: u32 = 20;

/// A request of a page's script, with what the fetcher needs to police it.
pub struct ScriptFetch {
    /// The script process that asked (the answer goes to it).
    pub pid: u32,
    pub req: FetchRequest,
    /// The page's address (its origin, `Referer`, the site for cookies).
    pub page_url: String,
    /// The page's ad blocker is on: no third-party cookies.
    pub shield_on: bool,
    /// The host the person allowed past a certificate error (empty: none).
    pub insecure_host: Vec<u8>,
}

static QUEUE: SpinLock<VecDeque<ScriptFetch>> = SpinLock::new(VecDeque::new());
/// `(pid, id)` of requests to abort; `id == u32::MAX` aborts everything of `pid`.
static ABORTS: SpinLock<Vec<(u32, u32)>> = SpinLock::new(Vec::new());

/// Queue a script's request; `false` when there is no network or the queue is full.
pub fn post_script_fetch(f: ScriptFetch) -> bool {
    if OFFLINE.load(Ordering::Acquire) || worker_dead() {
        return false;
    }
    {
        let mut q = QUEUE.lock();
        if q.len() >= MAX_QUEUED {
            return false;
        }
        q.push_back(f);
    }
    wake_worker();
    true
}

/// Abort request `id` of process `pid` (waiting or in progress).
pub fn abort_script_fetch(pid: u32, id: u32) {
    let removed = {
        let mut q = QUEUE.lock();
        let n = q.len();
        q.retain(|f| !(f.pid == pid && f.req.id == id));
        n != q.len()
    };
    if !removed {
        push_abort(pid, id);
    }
}

/// The script process `pid` is gone (or its page changed): drop its requests.
pub fn forget_script_fetches(pid: u32) {
    QUEUE.lock().retain(|f| f.pid != pid);
    push_abort(pid, u32::MAX);
}

fn push_abort(pid: u32, id: u32) {
    let mut a = ABORTS.lock();
    if a.len() >= 64 {
        a.remove(0);
    }
    a.push((pid, id));
}

/// Was request `id` of `pid` aborted? (Also true when the process is gone.)
pub(super) fn aborted(pid: u32, id: u32) -> bool {
    ABORTS
        .lock()
        .iter()
        .any(|&(p, i)| p == pid && (i == id || i == u32::MAX))
        || crate::userproc::end(pid) != crate::userproc::End::Running
}

/// Forget the abort marks of a request that ended.
fn clear_aborts(pid: u32, id: u32) {
    ABORTS.lock().retain(|&(p, i)| !(p == pid && i == id));
}

/// A script request is waiting (part of the worker's block condition).
pub(super) fn script_pending() -> bool {
    !QUEUE.lock().is_empty()
}

/// No network owner: answer nothing and drop the waiting requests (the pages see no answer; the
/// browser refuses new ones while offline).
pub(super) fn drop_scripts() {
    QUEUE.lock().clear();
}

/// Serve the next script request (on the worker).
pub(super) fn serve_script(net: &mut netstack::Net) {
    let Some(job) = QUEUE.lock().pop_front() else {
        return;
    };
    let (pid, id) = (job.pid, job.req.id);
    if !aborted(pid, id) {
        serve::run(net, job);
    }
    clear_aborts(pid, id);
}
