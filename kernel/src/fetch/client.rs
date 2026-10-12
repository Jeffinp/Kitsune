//! The compositor side: starting the fetcher, posting a request and taking its result.

use super::*;

/// Hand the network owner to the fetcher (call once, before spawning [`worker`]).
pub fn init(net: Netd) {
    // SAFETY: called once from `kernel_main` before the fetcher is spawned, so no other thread can
    // touch NET yet.
    unsafe {
        *NET.get() = Some(net);
    }
}

/// Record that there is no network interface: [`try_post`] then answers every request with
/// [`FailReason::Network`] right away (the browser shows the error instead of "Carregando").
pub fn init_offline() {
    OFFLINE.store(true, Ordering::Release);
}

/// True when no fetch is in flight and the worker is alive, so a navigation
/// request can be posted.
pub fn is_idle() -> bool {
    STATE.load(Ordering::Acquire) == IDLE && !worker_dead()
}

/// True once the worker thread has died. It never comes back: every later
/// navigation has to fail instead of waiting for a thread that no longer exists.
pub fn worker_dead() -> bool {
    let tid = TID.load(Ordering::Acquire);
    STATE.load(Ordering::Acquire) == WORKER_DEAD
        || (tid != usize::MAX && crate::sched::is_dead(tid))
}

/// Wake the worker from its idle block (a ping request is waiting for it).
pub fn wake_worker() {
    let tid = TID.load(Ordering::Acquire);
    if tid != usize::MAX {
        crate::sched::wake(tid);
    }
}

/// Queue a fetch for `url` if the worker is idle. Returns `true` if accepted.
/// `insecure_host` (usually empty) is the one host the user allowed to proceed
/// despite a certificate error, for this session. `body` makes it a POST (urlencoded form data).
pub fn try_post(url: &[u8], insecure_host: &[u8], body: Option<&[u8]>) -> bool {
    post(url, insecure_host, KIND_PAGE, 0, body)
}

/// Queue the download of one picture for the page being shown; it is decoded and scaled to
/// `fit_w` pixels wide on the worker. Collect the answer with [`take_image_result`].
pub fn try_post_image(url: &[u8], insecure_host: &[u8], fit_w: usize) -> bool {
    post(url, insecure_host, KIND_IMAGE, fit_w, None)
}

/// Queue the download of one style sheet of the page being shown; collect it with
/// [`take_sheet_result`].
pub fn try_post_sheet(url: &[u8], insecure_host: &[u8]) -> bool {
    post(url, insecure_host, KIND_SHEET, 0, None)
}

pub(super) fn post(
    url: &[u8],
    insecure_host: &[u8],
    kind: u8,
    fit_w: usize,
    body: Option<&[u8]>,
) -> bool {
    if worker_dead() {
        return false;
    }
    // The slot is shared with the apps: whoever wins this exchange owns the request.
    if STATE
        .compare_exchange(IDLE, CLAIMED, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        return false;
    }
    WHO.store(WHO_BROWSER, Ordering::Release);
    if OFFLINE.load(Ordering::Acquire) {
        if kind == KIND_SHEET {
            // SAFETY: this client holds the slot (CLAIMED, won above) and no worker exists (OFFLINE);
            // published by the Release store of DONE.
            unsafe {
                *SHEET_RESULT.get() = Some(None);
            }
            REQ_KIND.store(KIND_SHEET, Ordering::Relaxed);
            STATE.store(DONE, Ordering::Release);
            return true;
        }
        if kind == KIND_IMAGE {
            // SAFETY: this client holds the slot (CLAIMED, won above) and no worker exists (OFFLINE);
            // published by the Release store of DONE.
            unsafe {
                *IMG_RESULT.get() = Some(Err(ImgFail::Failed));
            }
            REQ_KIND.store(KIND_IMAGE, Ordering::Relaxed);
            STATE.store(DONE, Ordering::Release);
            return true;
        }
        REQ_KIND.store(KIND_PAGE, Ordering::Relaxed);
        // SAFETY: this client holds the slot (CLAIMED, won above) and no worker exists (OFFLINE), so
        // nothing else touches RESULT; the Release store of DONE below publishes it to `take_result`.
        unsafe {
            *RESULT.get() = Some(Err(FailReason::Network));
        }
        STATE.store(DONE, Ordering::Release);
        return true;
    }
    let n = url.len().min(URL_CAP);
    let m = insecure_host.len().min(INSECURE_CAP);
    // SAFETY: this client holds the slot (CLAIMED, won above), so the worker does not touch
    // REQ_URL/REQ_LEN/REQ_INSECURE* and no other poster can; the Release store of REQUESTED below
    // publishes them.
    unsafe {
        let buf = &mut *REQ_URL.get();
        buf[..n].copy_from_slice(&url[..n]);
        *REQ_LEN.get() = n;
        let ib = &mut *REQ_INSECURE.get();
        ib[..m].copy_from_slice(&insecure_host[..m]);
        *REQ_INSECURE_LEN.get() = m;
        *REQ_BODY.get() = body.map(<[u8]>::to_vec);
    }
    REQ_KIND.store(kind, Ordering::Relaxed);
    REQ_FIT_W.store(fit_w, Ordering::Relaxed);
    STATE.store(REQUESTED, Ordering::Release);
    let tid = TID.load(Ordering::Acquire);
    if tid != usize::MAX {
        crate::sched::wake(tid);
    }
    true
}

/// If a fetch has finished, return its result and reset to idle. Yields `None`
/// while nothing is ready.
pub fn take_result() -> Option<FetchResult> {
    reap_abandoned();
    // Only the browser's own request is the browser's to take (an app's belongs to `appd`).
    if WHO.load(Ordering::Acquire) != WHO_BROWSER {
        return None;
    }
    let state = STATE.load(Ordering::Acquire);
    let is_page = REQ_KIND.load(Ordering::Relaxed) == KIND_PAGE;
    if matches!(state, REQUESTED | RUNNING) && worker_dead() {
        // The worker died with a request in flight: it will never answer.
        STATE.store(WORKER_DEAD, Ordering::Release);
        return is_page.then_some(Err(FailReason::WorkerDied));
    }
    if state != DONE || !is_page {
        return None;
    }
    // SAFETY: STATE == DONE (Acquire) means the worker finished writing RESULT and will not touch it
    // until the compositor sets IDLE, which happens after this take (compositor is the only caller).
    let r = unsafe { (*RESULT.get()).take() };
    STATE.store(IDLE, Ordering::Release);
    Some(r.unwrap_or(Err(FailReason::Network)))
}

/// If a picture request has finished, return its outcome and reset to idle.
pub fn take_image_result() -> Option<Result<ImgLoaded, ImgFail>> {
    if WHO.load(Ordering::Acquire) != WHO_BROWSER {
        return None;
    }
    let state = STATE.load(Ordering::Acquire);
    if REQ_KIND.load(Ordering::Relaxed) != KIND_IMAGE {
        return None;
    }
    if matches!(state, REQUESTED | RUNNING) && worker_dead() {
        STATE.store(WORKER_DEAD, Ordering::Release);
        return Some(Err(ImgFail::Failed));
    }
    if state != DONE {
        return None;
    }
    // SAFETY: STATE == DONE (Acquire): the worker finished writing IMG_RESULT and will not touch it
    // until the compositor sets IDLE below (the compositor is the only caller).
    let r = unsafe { (*IMG_RESULT.get()).take() };
    STATE.store(IDLE, Ordering::Release);
    Some(r.unwrap_or(Err(ImgFail::Failed)))
}

/// If a style sheet request has finished, return its body (`None` = it failed) and reset to idle.
pub fn take_sheet_result() -> Option<Option<Vec<u8>>> {
    if WHO.load(Ordering::Acquire) != WHO_BROWSER {
        return None;
    }
    let state = STATE.load(Ordering::Acquire);
    if REQ_KIND.load(Ordering::Relaxed) != KIND_SHEET {
        return None;
    }
    if matches!(state, REQUESTED | RUNNING) && worker_dead() {
        STATE.store(WORKER_DEAD, Ordering::Release);
        return Some(None);
    }
    if state != DONE {
        return None;
    }
    // SAFETY: STATE == DONE (Acquire): the worker finished writing SHEET_RESULT and will not touch it
    // until the compositor sets IDLE below (the compositor is the only caller).
    let r = unsafe { (*SHEET_RESULT.get()).take() };
    STATE.store(IDLE, Ordering::Release);
    Some(r.flatten())
}
