//! Background page fetcher.
//!
//! HTTP(S) requests run on a dedicated kernel thread ([`worker`]) instead of
//! inline in the compositor loop, so the UI keeps rendering during the slow
//! software TLS handshake. The compositor and worker communicate through a tiny
//! atomic state machine plus a few static mailboxes:
//!
//! ```text
//! IDLE --try_post--> REQUESTED --worker--> RUNNING --worker--> DONE --take_result--> IDLE
//! ```
//!
//! If the worker thread dies (panic, CPU fault: see `sched::kill_current`) the
//! request in flight is answered with [`FailReason::WorkerDied`] by
//! [`take_result`], the machine moves to `WORKER_DEAD`, and from then on
//! [`is_idle`] is false for good, so nothing posts to the dead thread. The NIC
//! stays with the dead worker's `Net` (nobody else may touch hardware it may have
//! left half-programmed), so the machine goes silent on the wire: no ARP or ping
//! answers either.
//! The main loop turns later navigations into the same error via [`worker_dead`].
//!
//! # Two kinds of client
//!
//! The browser (compositor thread) and WASM apps (`net_http_get`, `appd` thread) share
//! the one request slot. Whoever wins the `IDLE -> CLAIMED` compare-exchange owns the
//! request until it is back to `IDLE` (`WHO` says which kind), so a result is only ever
//! taken by the client that posted it. An app's request is **policed twice**: before it
//! is posted (`appnet::authorize`, by the caller) and on the worker, which authorizes
//! every redirect hop again, refuses a destination whose *resolved* address is not
//! public, never allows the "continue despite the certificate" override and keeps TLS
//! verification on (docs/SECURITY-MODEL.md). An app that gives up (timeout) abandons the
//! slot: the worker drops the late result, and whoever sees it first (`reap_abandoned`)
//! frees it.
//!
//! The worker thread is also the network owner: the NIC lives inside the `Net` it
//! holds (a `nic::Port`, moved in by [`init`]), so nothing else can reach the
//! hardware. Between fetches it wakes every few ticks to answer ARP and ping
//! (`Net::respond_idle`); the compositor never touches the NIC.

mod apps;
mod jobs;

pub use apps::app_get;
use apps::reap_abandoned;
pub use jobs::{JobError, NetJob, NetJobResult, run_job};
use jobs::{job_pending, serve_job};

use crate::netd::Netd;
use crate::sync::RacyCell;
use crate::{netstack, serial_println};
use alloc::string::String;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, AtomicU8, AtomicU64, AtomicUsize, Ordering};
use kitsune_core::appmanifest::NetPerm;
use kitsune_core::browser::{Conn, FailReason, MAX_RESPONSE_BYTES};
use kitsune_core::web::imgcache::{self, ImgFail, Loaded as ImgLoaded};

const IDLE: u8 = 0;
const REQUESTED: u8 = 1;
const RUNNING: u8 = 2;
const DONE: u8 = 3;
/// The worker thread died; terminal (nothing leaves this state).
const WORKER_DEAD: u8 = 4;
/// A client won the slot and is filling in the request (or freeing it).
const CLAIMED: u8 = 5;

/// Who owns the request in flight.
const WHO_BROWSER: u8 = 0;
const WHO_APP: u8 = 1;
/// The app gave up; the worker's late result is to be dropped.
const WHO_ABANDONED: u8 = 2;

static WHO: AtomicU8 = AtomicU8::new(WHO_BROWSER);
/// `appd`'s scheduler slot while it waits for an app's request (the worker wakes it).
static APP_TID: AtomicUsize = AtomicUsize::new(usize::MAX);

/// What the worker enforces on an app's request (the poster fills it before `REQUESTED`).
#[derive(Clone)]
struct AppPolicy {
    perm: NetPerm,
    hosts: Vec<String>,
}
static APP_POLICY: RacyCell<Option<AppPolicy>> = RacyCell::new(None);

const URL_CAP: usize = 512;

static STATE: AtomicU8 = AtomicU8::new(IDLE);
/// Scheduler slot of the worker thread (`usize::MAX` until it has started), so
/// [`try_post`] can wake it from its idle block.
static TID: AtomicUsize = AtomicUsize::new(usize::MAX);
/// No NIC was found: every navigation fails at once with a network error instead of
/// waiting for a worker that does not exist.
static OFFLINE: AtomicBool = AtomicBool::new(false);
static NET: RacyCell<Option<Netd>> = RacyCell::new(None);
static REQ_URL: RacyCell<[u8; URL_CAP]> = RacyCell::new([0; URL_CAP]);
static REQ_LEN: RacyCell<usize> = RacyCell::new(0);
/// Body of the page request when it is a POST (written with the URL).
static REQ_BODY: RacyCell<Option<Vec<u8>>> = RacyCell::new(None);
/// Host the user explicitly allowed past a certificate error (empty = none). It
/// applies to that host only, on every hop of the navigation.
static REQ_INSECURE: RacyCell<[u8; INSECURE_CAP]> = RacyCell::new([0; INSECURE_CAP]);
static REQ_INSECURE_LEN: RacyCell<usize> = RacyCell::new(0);
const INSECURE_CAP: usize = 96;
static RESULT: RacyCell<Option<FetchResult>> = RacyCell::new(None);
/// What the current request is for: a page (`KIND_PAGE`) or one picture of a page.
static REQ_KIND: AtomicU8 = AtomicU8::new(KIND_PAGE);
const KIND_PAGE: u8 = 0;
const KIND_IMAGE: u8 = 1;
/// One style sheet of a page (the body comes back as bytes).
const KIND_SHEET: u8 = 2;
/// Column width the downloaded picture is scaled to (image requests).
static REQ_FIT_W: AtomicUsize = AtomicUsize::new(0);
/// Outcome of the last image request (decoded on the fetcher thread, so the UI never waits).
static IMG_RESULT: RacyCell<Option<Result<ImgLoaded, ImgFail>>> = RacyCell::new(None);
/// Outcome of the last style sheet request: its body, or `None` when it failed.
static SHEET_RESULT: RacyCell<Option<Option<Vec<u8>>>> = RacyCell::new(None);
/// Largest style sheet taken (bytes, headers included).
const SHEET_CAP: usize = 320 * 1024;

/// A fetched page plus what the browser needs to describe it honestly.
pub struct Loaded {
    /// Raw HTTP response (headers + body), at most `MAX_RESPONSE_BYTES`.
    pub data: Vec<u8>,
    /// How the *final* connection (after redirects) was authenticated.
    pub conn: Conn,
    /// The response was cut at the size cap.
    pub truncated: bool,
    /// Summary of the server certificate (HTTPS only), for the security popover.
    pub cert: Option<kitsune_core::browser::CertInfo>,
}

/// Outcome of one navigation: the page, or why it failed.
pub type FetchResult = Result<Loaded, FailReason>;

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

fn post(url: &[u8], insecure_host: &[u8], kind: u8, fit_w: usize, body: Option<&[u8]>) -> bool {
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

/// Worker thread entry. Processes one queued request at a time and, while idle,
/// is *blocked* in the scheduler (it costs no time slice at all) until
/// [`try_post`] wakes it.
pub extern "C" fn worker() -> ! {
    TID.store(crate::sched::current(), Ordering::Release);
    // Confirm the time over SNTP before anything else: certificate validity dates
    // are only as good as the clock. A request posted meanwhile waits (at most a
    // few seconds); if no server answers the RTC is used and the browser says
    // "hora nao confirmada" when a date check fails.
    // SAFETY: NET is set once before this thread exists and used only by this worker.
    if let Some(netd) = unsafe { (*NET.get()).as_mut() } {
        netd.net_mut().sync_time();
        LAST_RESYNC_MS.store(crate::netd::now_ms().max(1), Ordering::Relaxed);
    }
    loop {
        if STATE.load(Ordering::Acquire) == REQUESTED {
            STATE.store(RUNNING, Ordering::Relaxed);
            // SAFETY: STATE == REQUESTED (Acquire) pairs with `try_post`'s Release, so REQ_URL/REQ_LEN are
            // complete; the compositor will not rewrite them until DONE -> IDLE.
            let url = unsafe {
                let n = *REQ_LEN.get();
                let buf = &*REQ_URL.get();
                buf[..n].to_vec()
            };
            // SAFETY: as for REQ_URL above (REQ_INSECURE* are written together with it).
            let insecure = unsafe {
                let n = *REQ_INSECURE_LEN.get();
                (&*REQ_INSECURE.get())[..n].to_vec()
            };
            // SAFETY: as for REQ_URL above (written with it, before REQUESTED).
            let body = unsafe { (*REQ_BODY.get()).take() };
            let kind = REQ_KIND.load(Ordering::Relaxed);
            let fit_w = REQ_FIT_W.load(Ordering::Relaxed);
            let app = WHO.load(Ordering::Acquire) != WHO_BROWSER;
            let policy = if app {
                // SAFETY: written by the app's poster before the Release store of REQUESTED that this
                // worker saw (Acquire) above; not touched again until the slot is back to IDLE.
                unsafe { (*APP_POLICY.get()).clone() }
            } else {
                None
            };
            // SAFETY: NET is set once, before this thread exists (`fetch::init`), and used only by this
            // worker, one request at a time, so the `&mut` is unique.
            let netd = unsafe { (*NET.get()).as_mut() };
            if kind == KIND_SHEET {
                let result = match netd {
                    Some(netd) => fetch_sheet(netd.net_mut(), &url, &insecure),
                    None => None,
                };
                // SAFETY: STATE is RUNNING; the compositor reads SHEET_RESULT only after seeing DONE
                // (Release store right after this write).
                unsafe {
                    *SHEET_RESULT.get() = Some(result);
                }
                STATE.store(DONE, Ordering::Release);
            } else if kind == KIND_IMAGE {
                let result = match netd {
                    Some(netd) => fetch_image(netd.net_mut(), &url, &insecure, fit_w),
                    None => Err(ImgFail::Failed),
                };
                // SAFETY: STATE is RUNNING; the compositor reads IMG_RESULT only after seeing DONE
                // (Release store right after this write).
                unsafe {
                    *IMG_RESULT.get() = Some(result);
                }
                STATE.store(DONE, Ordering::Release);
            } else {
                let result = match netd {
                    Some(netd) => {
                        let net = netd.net_mut();
                        // An app only reaches public addresses, checked on what the name resolved to.
                        net.set_public_only(app);
                        let r = match (app, policy.as_ref()) {
                            (false, _) => fetch_url(
                                net,
                                &url,
                                &insecure,
                                MAX_RESPONSE_BYTES,
                                None,
                                body.as_deref(),
                            ),
                            (true, Some(p)) => {
                                fetch_url(net, &url, &[], MAX_RESPONSE_BYTES, Some(p), None)
                            }
                            // An app request without a policy cannot happen; refuse rather than guess.
                            (true, None) => Err(FailReason::Network),
                        };
                        net.set_public_only(false);
                        r
                    }
                    None => Err(FailReason::Network),
                };
                if WHO.load(Ordering::Acquire) == WHO_ABANDONED {
                    // The app timed out and left: nobody wants this result.
                    drop(result);
                    WHO.store(WHO_BROWSER, Ordering::Release);
                    STATE.store(IDLE, Ordering::Release);
                } else {
                    // SAFETY: STATE is RUNNING here; the poster only touches RESULT after seeing DONE,
                    // which is stored (Release) right after this write.
                    unsafe {
                        *RESULT.get() = Some(result);
                    }
                    STATE.store(DONE, Ordering::Release);
                    if app {
                        let tid = APP_TID.load(Ordering::Acquire);
                        if tid != usize::MAX {
                            crate::sched::wake(tid);
                        }
                    }
                }
            }
        } else if job_pending() {
            // SAFETY: NET is used only by this worker thread.
            if let Some(netd) = unsafe { (*NET.get()).as_mut() } {
                serve_job(netd.net_mut());
            }
        } else {
            // Idle: service the network (DHCP timers, ARP/ping responder, ping client),
            // then leave the run queue until a request is posted (`try_post` and
            // `ping_start` wake us) or the service interval ends. The scheduler
            // re-checks the condition after announcing the block, so a request posted
            // in between is not missed.
            // Fold the entropy sample ring into the pool and reseed when due (cheap when idle).
            crate::rng::service();
            // SAFETY: as above: NET is used only by this worker thread.
            let sleep = match unsafe { (*NET.get()).as_mut() } {
                Some(netd) => netd.service(),
                None => crate::netd::IDLE_POLL_TICKS,
            };
            crate::sched::block(crate::interrupts::ticks() + sleep, || {
                STATE.load(Ordering::Acquire) != REQUESTED
                    && !crate::netd::ping_pending()
                    && !job_pending()
            });
        }
    }
}

/// Monotonic ms of the last SNTP attempt made on behalf of a page load.
static LAST_RESYNC_MS: AtomicU64 = AtomicU64::new(0);

/// Retry the SNTP sync before an HTTPS load when the clock is still
/// unconfirmed, rate-limited to one attempt per minute.
fn resync_clock_if_needed(net: &mut netstack::Net) {
    if crate::clock::confirmed() {
        return;
    }
    let now = crate::netd::now_ms();
    let last = LAST_RESYNC_MS.load(Ordering::Relaxed);
    if last != 0 && now.saturating_sub(last) < 60_000 {
        return;
    }
    LAST_RESYNC_MS.store(now.max(1), Ordering::Relaxed);
    net.sync_time();
}

/// Resolve a URL and fetch it (HTTP or HTTPS), following up to
/// [`kitsune_core::redirect::MAX_REDIRECTS`] redirects. Returns the final page,
/// or why the navigation failed. Redirect policy (scheme kept, https -> http
/// refused, loops, bad `Location` values) lives in `kitsune_core::redirect`.
fn fetch_url(
    net: &mut netstack::Net,
    url: &[u8],
    insecure_host: &[u8],
    cap: usize,
    app: Option<&AppPolicy>,
    body: Option<&[u8]>,
) -> FetchResult {
    use kitsune_core::browser::{header_value, parse_url, status_code};
    use kitsune_core::redirect::Redirects;
    let mut cur: Vec<u8> = url.to_vec();
    let mut chain: Option<Redirects> = None;
    // The form data of a POST: kept through 307/308, dropped (the next hop is a GET) otherwise.
    let mut body = body;

    loop {
        // An app's request, and each redirect hop it is sent on to, passes the same gate:
        // permission, destination filter and allow-list (`kitsune_core::appnet`).
        if let Some(p) = app
            && let Err(code) = kitsune_core::appnet::authorize(p.perm, &p.hosts, &cur)
        {
            serial_println!("fetch: app request refused ({})", code);
            return Err(FailReason::Network);
        }
        let u = parse_url(&cur).ok_or(FailReason::Network)?;
        let chain = chain.get_or_insert_with(|| Redirects::new(&u));
        let (Ok(host), Ok(path)) = (
            core::str::from_utf8(u.host()),
            core::str::from_utf8(u.path()),
        ) else {
            return Err(FailReason::Network);
        };
        serial_println!(
            "fetch: {} {}://{}{} :{}",
            if body.is_some() { "POST" } else { "GET" },
            if u.https { "https" } else { "http" },
            host,
            path,
            u.port
        );
        let (r, conn) = if u.https {
            // Certificate dates need a confirmed clock: if the boot-time SNTP did not
            // succeed, try again (at most once a minute) before the handshake.
            resync_clock_if_needed(net);
            // Validation is skipped only for the one host the user allowed.
            // (Never for an app: `insecure_host` is empty for them.)
            let allow = app.is_none()
                && !insecure_host.is_empty()
                && u.host().eq_ignore_ascii_case(insecure_host);
            net.https_get(host, path, u.port, allow, cap, body)?
        } else {
            (net.http_get(host, path, u.port, cap, body)?, Conn::Plain)
        };

        let code = status_code(&r.data).unwrap_or(0);
        if matches!(code, 301 | 302 | 303 | 307 | 308)
            && let Some(loc) = header_value(&r.data, b"location")
        {
            match chain.follow(&u, loc) {
                Ok(next) => {
                    cur = next;
                    if !matches!(code, 307 | 308) {
                        body = None;
                    }
                    serial_println!("fetch: {} redirect", code);
                    continue;
                }
                Err(e) => {
                    serial_println!("fetch: {} redirect refused: {:?}", code, e);
                    return Err(FailReason::from_redirect(e));
                }
            }
        }

        serial_println!(
            "fetch: {} bytes (status {}){}",
            r.data.len(),
            code,
            if r.truncated { " truncated" } else { "" }
        );
        return Ok(Loaded {
            data: r.data,
            conn,
            truncated: r.truncated,
            cert: r.cert,
        });
    }
}

/// Download one picture (same stack, same redirect rules; at most `MAX_IMAGE_BYTES`) and turn it
/// into column-sized pixels here, on the worker, so the compositor never decodes.
/// Download one style sheet: its decoded body when the answer is a 200, else `None`.
fn fetch_sheet(net: &mut netstack::Net, url: &[u8], insecure_host: &[u8]) -> Option<Vec<u8>> {
    let r = fetch_url(net, url, insecure_host, SHEET_CAP, None, None).ok()?;
    let name = core::str::from_utf8(url).unwrap_or("?");
    let code = kitsune_core::browser::status_code(&r.data).unwrap_or(0);
    if code != 200 {
        serial_println!("css: status {} for {}", code, name);
        return None;
    }
    let body = kitsune_core::browser::page_body_partial(&r.data, r.truncated).body;
    serial_println!("css: {} -> {} bytes", name, body.len());
    Some(body)
}

fn fetch_image(
    net: &mut netstack::Net,
    url: &[u8],
    insecure_host: &[u8],
    fit_w: usize,
) -> Result<ImgLoaded, ImgFail> {
    let r = match fetch_url(
        net,
        url,
        insecure_host,
        imgcache::MAX_IMAGE_BYTES + 8 * 1024,
        None,
        None,
    ) {
        Ok(r) => r,
        Err(_) => return Err(ImgFail::Failed),
    };
    let code = kitsune_core::browser::status_code(&r.data).unwrap_or(0);
    let name = core::str::from_utf8(url).unwrap_or("?");
    if code != 200 {
        serial_println!("img: status {} for {}", code, name);
        return Err(ImgFail::Failed);
    }
    if r.truncated {
        return Err(ImgFail::TooBig);
    }
    let body = kitsune_core::browser::page_body(&r.data);
    drop(r);
    let out = imgcache::decode_for_page(&body, fit_w);
    match &out {
        Ok(l) => serial_println!(
            "img: {} -> {}x{} (shown {}x{})",
            name,
            l.orig_w,
            l.orig_h,
            l.img.width(),
            l.img.height()
        ),
        Err(e) => serial_println!("img: {} failed: {:?}", name, e),
    }
    out
}
