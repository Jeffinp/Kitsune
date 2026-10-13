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

mod client;
mod load;
mod thread;
pub use client::*;
use load::*;
pub use thread::*;

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
/// One script of a page (the body comes back as bytes, in the same slot as a sheet's).
const KIND_SCRIPT: u8 = 3;
/// Largest script taken (bytes, headers included).
const SCRIPT_CAP: usize = MAX_RESPONSE_BYTES;
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
