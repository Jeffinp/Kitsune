//! Web Workers of a page: each runs in a process of its own (`jsrun --worker`, ring 3, the same link
//! as the page's process), started when the page asks (`K new`, with the script the page fetched),
//! and the browser carries the messages both ways. A worker's network requests go through the same
//! ad blocker and fetcher as the page's. They end with the page (dropping the `JsProc`), with
//! `terminate()` or `close()`. The messages are `kitsune_core::web::script::worker`.

use super::script::JsProc;
use crate::desktop::*;
use kitsune_core::web::script::{
    FromScript, MAX_WORKERS, WorkerEvent, WorkerOp, parse_message, worker_event_message,
    worker_init_message, worker_post_message,
};

/// A worker's process.
pub(crate) struct WorkerProc {
    /// The page's number for it.
    pub id: u32,
    pub pid: u32,
}

impl Drop for WorkerProc {
    fn drop(&mut self) {
        crate::fetch::forget_script_fetches(self.pid);
        crate::userproc::kill(self.pid);
    }
}

/// Most messages taken from one worker per frame.
const MAX_PER_FRAME: usize = 32;

/// What the page asked about its workers.
pub(super) fn on_op(js: &mut JsProc, op: WorkerOp) {
    match op {
        WorkerOp::New {
            id,
            url,
            name,
            code,
        } => {
            if js.workers.len() >= MAX_WORKERS || js.workers.iter().any(|w| w.id == id) {
                js.post(worker_event_message(
                    id,
                    WorkerEvent::Error("too many workers"),
                ));
                return;
            }
            let pid = match crate::userproc::spawn_worker(crate::desktop::services::accounts::cred())
            {
                Ok(pid) => pid,
                Err(e) => {
                    crate::klog!(Warn, "browser: no process for a worker ({:?})", e);
                    js.post(worker_event_message(
                        id,
                        WorkerEvent::Error("no process for the worker"),
                    ));
                    return;
                }
            };
            if !crate::userproc::page_post(pid, worker_init_message(url, name, code)) {
                crate::userproc::kill(pid);
                js.post(worker_event_message(
                    id,
                    WorkerEvent::Error("the worker's script is too large"),
                ));
                return;
            }
            crate::klog!(
                Info,
                "browser: worker {} of pid {} is pid {} ({})",
                id,
                js.pid,
                pid,
                url
            );
            js.workers.push(WorkerProc { id, pid });
        }
        WorkerOp::Post { id, data } => {
            if let Some(w) = js.workers.iter().find(|w| w.id == id) {
                crate::userproc::page_post(w.pid, worker_post_message(data));
            }
        }
        WorkerOp::End(id) => js.workers.retain(|w| w.id != id),
    }
}

/// Every frame: what the workers of tab `t` sent (messages for the page, network requests), and the
/// ones that ended.
pub(super) fn pump(t: &mut TabData) {
    let Some(js) = t.js.as_mut() else { return };
    if js.workers.is_empty() {
        return;
    }
    let page = js.pid;
    let mut ended: Vec<u32> = Vec::new();
    for w in &js.workers {
        for _ in 0..MAX_PER_FRAME {
            let Some(m) = crate::userproc::page_take(w.pid) else {
                break;
            };
            match parse_message(&m) {
                FromScript::ToPage(data) => {
                    crate::userproc::page_post(
                        page,
                        worker_event_message(w.id, WorkerEvent::Message(data)),
                    );
                }
                FromScript::WorkerFailed(text) => {
                    crate::userproc::page_post(
                        page,
                        worker_event_message(w.id, WorkerEvent::Error(text)),
                    );
                }
                FromScript::WorkerClose => ended.push(w.id),
                FromScript::Fetch(req) => {
                    super::jsnet::on_fetch(&mut t.shield, &t.browser, w.pid, req)
                }
                FromScript::Abort(rid) => crate::fetch::abort_script_fetch(w.pid, rid),
                FromScript::SetCookie(text) => super::jsnet::on_set_cookie(&t.browser, w.pid, text),
                _ => {}
            }
        }
        if crate::userproc::end(w.pid) != crate::userproc::End::Running && !ended.contains(&w.id) {
            crate::klog!(Info, "browser: worker {} (pid {}) ended", w.id, w.pid);
            ended.push(w.id);
        }
    }
    for id in ended {
        js.workers.retain(|w| w.id != id);
        crate::userproc::page_post(page, worker_event_message(id, WorkerEvent::End));
    }
}
