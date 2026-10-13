//! Web Workers: a page's `new Worker(url)` runs in a process of its own (`jsrun --worker`, started by
//! the browser), and the browser carries the messages between the two. The page's side is
//! `user-apps/jsrun/src/bin/jsrun/workers.js`, the worker's `workerscope.js`; the values travel as the
//! text of `msgcodec.js`, which the browser never reads.
//!
//! | From | Message | Meaning |
//! |---|---|---|
//! | page | `Knew` LF id LF url LF name LF code | start worker `id` with this script (the page fetched it) |
//! | page | `Kmsg` LF id LF data | `worker.postMessage` |
//! | page | `Kend` LF id | `worker.terminate()` |
//! | browser to worker | `winit` (`url`, `name`, `code`), `wmsg` (`data`) | the script, a message |
//! | worker | `M` data, `E` text, `Z` | `postMessage`, the script failed, `close()` |
//! | browser to page | `worker` (`id`, `ev`: `message` + `data`, `error` + `message`, `end`) | what the worker did |
//!
//! At most [`MAX_WORKERS`] run for a page (the page refuses more itself; the browser too).

use super::*;

/// Most workers one page runs at a time.
pub const MAX_WORKERS: usize = 2;
/// Largest worker script, in bytes.
pub const MAX_WORKER_SCRIPT: usize = 1024 * 1024;

/// What a page asks about its workers (`K`).
#[derive(Debug, PartialEq, Eq)]
pub enum WorkerOp<'a> {
    New {
        id: u32,
        url: &'a str,
        name: &'a str,
        code: &'a str,
    },
    Post {
        id: u32,
        data: &'a str,
    },
    End(u32),
}

/// Read a `K` message (`m` without the kind byte).
pub(super) fn parse_worker_op(m: &[u8]) -> Option<WorkerOp<'_>> {
    let text = core::str::from_utf8(m).ok()?;
    let (op, rest) = text.split_once('\n').unwrap_or((text, ""));
    let (id, rest) = rest.split_once('\n').unwrap_or((rest, ""));
    let id: u32 = id.trim().parse().ok().filter(|&i| i > 0)?;
    match op {
        "new" => {
            let (url, rest) = rest.split_once('\n')?;
            let (name, code) = rest.split_once('\n')?;
            (url.len() <= 2048 && name.len() <= 256 && code.len() <= MAX_WORKER_SCRIPT).then_some(
                WorkerOp::New {
                    id,
                    url,
                    name,
                    code,
                },
            )
        }
        "msg" => Some(WorkerOp::Post { id, data: rest }),
        "end" => rest.is_empty().then_some(WorkerOp::End(id)),
        _ => None,
    }
}

/// The worker's script (`winit`).
pub fn worker_init_message(url: &str, name: &str, code: &str) -> Vec<u8> {
    let mut out = String::with_capacity(64 + url.len() + code.len() * 11 / 10);
    out.push_str("{\"t\":\"winit\",\"url\":");
    json_str(&mut out, url);
    out.push_str(",\"name\":");
    json_str(&mut out, name);
    out.push_str(",\"code\":");
    json_str(&mut out, code);
    out.push('}');
    out.into_bytes()
}

/// A message from the page for the worker (`wmsg`).
pub fn worker_post_message(data: &str) -> Vec<u8> {
    let mut out = String::from("{\"t\":\"wmsg\",\"data\":");
    json_str(&mut out, data);
    out.push('}');
    out.into_bytes()
}

/// What worker `id` did, for the page.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WorkerEvent<'a> {
    Message(&'a str),
    Error(&'a str),
    End,
}

/// The `worker` message for the page.
pub fn worker_event_message(id: u32, ev: WorkerEvent) -> Vec<u8> {
    let mut out = format!("{{\"t\":\"worker\",\"id\":{id},\"ev\":");
    match ev {
        WorkerEvent::Message(data) => {
            out.push_str("\"message\",\"data\":");
            json_str(&mut out, data);
        }
        WorkerEvent::Error(text) => {
            out.push_str("\"error\",\"message\":");
            json_str(&mut out, text);
        }
        WorkerEvent::End => out.push_str("\"end\""),
    }
    out.push('}');
    out.into_bytes()
}
