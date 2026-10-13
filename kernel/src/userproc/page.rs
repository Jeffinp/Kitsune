//! The link between the browser and the script process of a page (calls 56–57): two queues of
//! messages, the browser's side served from the compositor loop (`page_post`, `page_take`), the
//! process's side by `page_recv` / `page_send`. The kernel only carries bytes; what they mean is
//! `kitsune_core::web::script`.

use super::*;

/// Most bytes waiting in either direction; a sender past it gets `EAGAIN` (process) or `false`
/// (browser).
const MAX_QUEUED: usize = 8 * 1024 * 1024;
/// Most messages waiting for the process (clicks beyond it are dropped).
const MAX_INBOX: usize = 256;

/// The two queues of a page's script process.
#[derive(Default)]
pub(super) struct PageLink {
    inbox: VecDeque<Vec<u8>>,
    inbox_bytes: usize,
    outbox: VecDeque<Vec<u8>>,
    outbox_bytes: usize,
    /// When a `page_recv` that waits gives up (ticks).
    deadline: Option<u64>,
}

/// Start the JavaScript engine as the script process of a page (`jsrun --page`), acting as `cred`.
/// Returns its pid; the browser then talks to it with [`page_post`] and [`page_take`].
pub fn spawn_page(cred: Option<Cred>) -> Result<u32, SpawnError> {
    let image = embedded("jsrun").ok_or(SpawnError::NoHost)?;
    let args = abi::pack_args("/", &[String::from("--page")]).unwrap_or_default();
    spawn_elf(
        "jsrun",
        image,
        Launch {
            args,
            cwd: String::from("/"),
            cred,
            page: true,
            ..Launch::default()
        },
    )
}

/// Start the media decoder as a message server (`mediadec --serve`), acting as `cred`: the same two
/// queues as a page's script process, so a player talks to it with [`page_post`] and [`page_take`]
/// (WebM bytes in, RGBA pictures and PCM out; the protocol is in `docs/design/midia.md`). Returns
/// its pid. Nothing starts it yet: it is the decoding half of the browser's future `<video>`.
#[allow(dead_code)] // the caller (the browser's player) is the next step
pub fn spawn_media(cred: Option<Cred>) -> Result<u32, SpawnError> {
    let image = embedded("mediadec").ok_or(SpawnError::NoHost)?;
    let args = abi::pack_args("/", &[String::from("--serve")]).unwrap_or_default();
    spawn_elf(
        "mediadec",
        image,
        Launch {
            args,
            cwd: String::from("/"),
            cred,
            page: true,
            ..Launch::default()
        },
    )
}

/// Why [`page_try_post`] did not queue a message.
pub enum PostError {
    /// The process is gone, or is not a page process.
    Gone,
    /// Its queue is full for now; the message is given back.
    Full(Vec<u8>),
}

/// Queue `msg` for the script process `pid` and wake it. `false` when it is gone, is not a page
/// process, or has too much waiting already.
pub fn page_post(pid: u32, msg: Vec<u8>) -> bool {
    page_try_post(pid, msg).is_ok()
}

/// Like [`page_post`], but says whether to try again later (the fetcher waits on a full queue: a
/// stream the page reads slowly).
pub fn page_try_post(pid: u32, msg: Vec<u8>) -> Result<(), PostError> {
    if msg.len() > abi::MAX_PAGE_MSG {
        return Err(PostError::Gone);
    }
    let slot = {
        let mut procs = PROCS.lock();
        let Some(p) = procs.iter_mut().find(|p| p.pid == pid) else {
            return Err(PostError::Gone);
        };
        if end_of(p) != End::Running {
            return Err(PostError::Gone);
        }
        let Some(link) = p.page.as_mut() else {
            return Err(PostError::Gone);
        };
        if link.inbox.len() >= MAX_INBOX || link.inbox_bytes + msg.len() > MAX_QUEUED {
            return Err(PostError::Full(msg));
        }
        link.inbox_bytes += msg.len();
        link.inbox.push_back(msg);
        p.slot
    };
    crate::sched::wake(slot);
    Ok(())
}

/// The next message the script process `pid` sent, if any.
pub fn page_take(pid: u32) -> Option<Vec<u8>> {
    let mut procs = PROCS.lock();
    let link = procs.iter_mut().find(|p| p.pid == pid)?.page.as_mut()?;
    let m = link.outbox.pop_front()?;
    link.outbox_bytes -= m.len();
    Some(m)
}

/// Calls 56 (`page_recv`) and 57 (`page_send`).
pub(super) fn page_call(r: &mut Regs, rsp: u64, slot: usize) -> u64 {
    let number = r.rax;
    let (a, b, c) = (r.rdi, r.rsi, r.rdx);
    let mut procs = PROCS.lock();
    let Some(p) = procs.iter_mut().find(|p| p.slot == slot) else {
        r.rax = EINVAL as u64;
        return rsp;
    };
    let (Some(asp), Some(link)) = (p.aspace.as_ref(), p.page.as_mut()) else {
        r.rax = ENOSYS as u64;
        return rsp;
    };
    if number == abi::SYS_PAGE_SEND {
        // (ptr, len)
        let len = b as usize;
        r.rax = if len > abi::MAX_PAGE_MSG {
            abi::errno::EINVAL as u64
        } else if link.outbox_bytes + len > MAX_QUEUED {
            abi::errno::EAGAIN as u64
        } else {
            let mut buf = alloc::vec![0u8; len];
            match asp.copy_in(a, &mut buf) {
                Ok(()) => {
                    link.outbox_bytes += len;
                    link.outbox.push_back(buf);
                    0
                }
                Err(_) => EFAULT as u64,
            }
        };
        return rsp;
    }
    // page_recv(ptr, cap, timeout_ms): one message, or wait for one up to the timeout.
    let now = crate::interrupts::ticks();
    if let Some(m) = link.inbox.pop_front() {
        link.inbox_bytes -= m.len();
        link.deadline = None;
        r.rax = if m.len() > b as usize {
            abi::errno::E2BIG as u64
        } else {
            match asp.poke(a, &m) {
                Ok(()) => m.len() as u64,
                Err(_) => EFAULT as u64,
            }
        };
        return rsp;
    }
    let deadline = *link.deadline.get_or_insert_with(|| {
        if c == u64::MAX {
            crate::sched::FOREVER
        } else {
            now.saturating_add(
                (c.min(3_600_000) * u64::from(crate::interrupts::TIMER_HZ)).div_ceil(1000),
            )
        }
    });
    if c == 0 || now >= deadline {
        link.deadline = None;
        r.rax = 0;
        return rsp;
    }
    drop(procs);
    // Restart this call when woken (a message arrived) or when the deadline passes.
    r.rax = number;
    r.rip -= 2;
    crate::sched::set_wake(slot, deadline);
    crate::sched::yield_switch(rsp)
}
