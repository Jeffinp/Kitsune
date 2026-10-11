//! `AppManager`: many WebAssembly apps at once.
//!
//! # Threads and ownership
//!
//! * The **compositor thread** (desktop) opens, resizes, feeds input to, draws and
//!   closes apps through the functions at the bottom of this file. It only touches
//!   the [`Slot`] table, never a wasm `Store`.
//! * The **`appd` thread** ([`worker`]) is the only owner of the wasm runtimes
//!   (`RT`): it instantiates, runs and drops every `Store`. One thread, not one per
//!   app, because the kernel has 8 thread slots and a single owner needs no locks
//!   between apps.
//! * The `Slot` table is shared; the kernel is single-core, so every access runs with
//!   interrupts masked ([`with`]) and is short. Surfaces are the exception: they are
//!   copied outside the lock under the `reading` handshake (see [`blit`]).
//!
//! # Scheduling
//!
//! `appd` loops over the slots round-robin. A slot is *ready* when it has input, a
//! redraw request, a size change, a tick due, or (legacy v1 apps) a frame due. Each
//! ready app runs **one slice** per pass: at most [`SLICE_EVENTS`] event calls and one
//! `render`, each call with its own fuel budget (`fuel_frame` from the manifest). No
//! app runs two slices before every other ready app ran one. With nothing ready,
//! `appd` blocks in `sched::block` until the nearest deadline or a `wake` (input,
//! open, close): an idle platform costs no CPU.
//!
//! # Failure
//!
//! Any failed guest call (out of fuel, trap, invalid pointer, `exit`) ends **that
//! app only**: its runtime (and with it its memory and open files) is dropped on the
//! spot and the slot keeps the reason, which the window shows.

use super::remote::{self, RemoteRt};
use super::{
    AppFault, FRAME_FUEL, Halt, HostState, INIT_FUEL, Why, abi2, describe, guest_engine,
    guest_limits, install_all,
};
use crate::sync::RacyCell;
use crate::{io, serial_println};
use alloc::boxed::Box;
use alloc::collections::VecDeque;
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;
use bootloader_api::info::FrameBufferInfo;
use core::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use kitsune_core::appfs::Sandbox;
use kitsune_core::appmanifest::{Manifest, Quotas};
use kitsune_core::{t, tk};
use wasmi::{Instance, Linker, Memory, Module, Store, Val};

/// Handle of a running app (never reused).
pub type AppId = u32;

/// Most apps alive at once.
pub const MAX_APPS: usize = 8;
const EV_CAP: usize = 64;
/// Event calls per slice.
const SLICE_EVENTS: usize = 8;
/// Legacy v1 apps render every 4 ticks (16 ms).
const FRAME_TICKS: u64 = 4;
/// Window of the CPU percentage (1 s at 250 Hz).
const CPU_WINDOW_TICKS: u64 = 250;
/// A resize is applied once the requested size has been stable this many ticks.
const RESIZE_SETTLE_TICKS: u64 = 3;
/// Most pixels one surface may have (the biggest window is 1280x800 content).
const MAX_SURFACE_PIXELS: usize = 1280 * 800;

/// Life-cycle state of an app.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    Starting,
    Running,
    /// Window minimized: no render, no ticks, input dropped.
    Suspended,
    /// Left by itself (`exit` / `proc_exit`).
    Exited,
    Crashed,
}

#[derive(Clone, Copy)]
enum Ev {
    Key(i32, i32),
    Text(i32),
    Pointer(i32, i32, i32),
    Close,
}

struct Slot {
    id: AppId,
    manifest: Manifest,
    quotas: Quotas,
    v1: bool,
    wasm: Arc<[u8]>,
    state: State,
    reason: Why,
    events: VecDeque<Ev>,
    want_redraw: bool,
    visible: bool,
    /// Content size the window wants, and when that changed.
    req_w: i32,
    req_h: i32,
    req_at: u64,
    /// Content size of the surfaces (and of what the guest knows).
    cur_w: i32,
    cur_h: i32,
    title: String,
    surf: [Vec<u8>; 2],
    front: usize,
    ready: bool,
    /// Surface index the compositor is copying right now.
    reading: Option<usize>,
    closing: bool,
    restart: bool,
    rt_dropped: bool,
    // ---- accounting ----
    cpu_total: u64,
    cpu_win: u64,
    cpu_win_start: u64,
    cpu_pct: u8,
    fuel_total: u64,
    mem_kib: u32,
    mem_peak_kib: u32,
    calls: u64,
    frames: u64,
    started_tick: u64,
    last_frame: u64,
    last_tick: u64,
}

const NONE_SLOT: Option<Slot> = None;
static SLOTS: RacyCell<[Option<Slot>; MAX_APPS]> = RacyCell::new([NONE_SLOT; MAX_APPS]);
static NEXT_ID: AtomicU64 = AtomicU64::new(1);

/// Run `f` with exclusive access to the slot table (single core, IRQs masked).
fn with<R>(f: impl FnOnce(&mut [Option<Slot>; MAX_APPS]) -> R) -> R {
    x86_64::instructions::interrupts::without_interrupts(|| {
        // SAFETY: single core and interrupts masked for the closure, which never re-enters `with`,
        // so this is the only live reference to the table.
        f(unsafe { &mut *SLOTS.get() })
    })
}

fn slot_of(slots: &mut [Option<Slot>; MAX_APPS], id: AppId) -> Option<&mut Slot> {
    slots.iter_mut().flatten().find(|s| s.id == id)
}

// ---------------------------------------------------------------- runtimes (appd only)

/// Where the guest executes. (Boxed once, inside `Runtime`; the size gap is not worth a second box.)
#[allow(clippy::large_enum_variant)]
enum Guest {
    /// `wasmi` inside the kernel.
    Local {
        store: Store<HostState>,
        instance: Instance,
        memory: Option<Memory>,
    },
    /// `wasmi` in a ring 3 process (see `remote`).
    Remote(RemoteRt),
}

struct Runtime {
    guest: Guest,
    first_render: bool,
    /// Fuel consumed and calls made since the last `account`.
    fuel_spent: u64,
    calls: u64,
    /// Button state of the last pointer event delivered (v1 apps see only presses).
    last_buttons: i32,
    /// The guest asked for a redraw while rendering.
    redraw_again: bool,
}

static RT: RacyCell<[Option<Box<Runtime>>; MAX_APPS]> = RacyCell::new([const { None }; MAX_APPS]);

fn rts() -> &'static mut [Option<Box<Runtime>>; MAX_APPS] {
    // SAFETY: RT is only touched by the `appd` worker thread (build, run, drop); no reference outlives
    // one loop iteration there.
    // NOTE: not guaranteed by the type (safe fn returning `&'static mut`).
    unsafe { &mut *RT.get() }
}

static TID: AtomicUsize = AtomicUsize::new(usize::MAX);
static TSC_KHZ: AtomicU64 = AtomicU64::new(1_000_000);
/// The real framebuffer layout, captured at boot to derive the offscreen one.
static FB_INFO: RacyCell<Option<FrameBufferInfo>> = RacyCell::new(None);

/// Capture the framebuffer layout and TSC rate (once, before `appd` spawns).
pub fn init(info: FrameBufferInfo, tsc_khz: u64) {
    // SAFETY: called once, before `appd` is spawned (`kernel_main`), so nothing can read FB_INFO yet.
    unsafe {
        *FB_INFO.get() = Some(info);
    }
    TSC_KHZ.store(tsc_khz.max(1), Ordering::Relaxed);
}

fn fb_info() -> Option<FrameBufferInfo> {
    // SAFETY: written once in `init`, before the readers exist; only read afterwards.
    unsafe { *FB_INFO.get() }
}

fn surface_info(w: i32, h: i32) -> Option<FrameBufferInfo> {
    let mut oi = fb_info()?;
    oi.width = w.max(1) as usize;
    oi.height = h.max(1) as usize;
    oi.stride = w.max(1) as usize;
    oi.byte_len = oi.stride * oi.height * oi.bytes_per_pixel;
    Some(oi)
}

pub(crate) fn wake_worker() {
    let tid = TID.load(Ordering::Acquire);
    if tid != usize::MAX {
        crate::sched::wake(tid);
    }
}

// ---------------------------------------------------------------- launch / close (compositor)

/// Why an app could not be started.
#[derive(Debug, PartialEq, Eq)]
pub enum LaunchError {
    /// All [`MAX_APPS`] slots are in use.
    Full,
}

/// Start an app from its package bytes. `w`/`h` are the window's content size.
/// The module is instantiated by `appd`; failures show up as `Crashed` in the window.
pub fn launch(
    wasm: Vec<u8>,
    manifest: Manifest,
    v1: bool,
    w: i32,
    h: i32,
) -> Result<AppId, LaunchError> {
    let quotas = manifest.granted();
    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed) as AppId;
    let now = crate::interrupts::ticks();
    let (w, h) = (w.clamp(1, 1280), h.clamp(1, 800));
    let slot = Slot {
        id,
        manifest,
        quotas,
        v1,
        wasm: Arc::from(wasm),
        state: State::Starting,
        reason: Why::default(),
        events: VecDeque::new(),
        want_redraw: true,
        visible: true,
        req_w: w,
        req_h: h,
        req_at: now,
        cur_w: 0,
        cur_h: 0,
        title: String::new(),
        surf: [Vec::new(), Vec::new()],
        front: 0,
        ready: false,
        reading: None,
        closing: false,
        restart: false,
        rt_dropped: true,
        cpu_total: 0,
        cpu_win: 0,
        cpu_win_start: now,
        cpu_pct: 0,
        fuel_total: 0,
        mem_kib: 0,
        mem_peak_kib: 0,
        calls: 0,
        frames: 0,
        started_tick: now,
        last_frame: 0,
        last_tick: now,
    };
    let ok = with(|slots| match slots.iter_mut().find(|s| s.is_none()) {
        Some(free) => {
            *free = Some(slot);
            true
        }
        None => false,
    });
    if !ok {
        return Err(LaunchError::Full);
    }
    wake_worker();
    Ok(id)
}

/// The window was asked to close: tell the app (`on_close`) and mark it for removal.
pub fn request_close(id: AppId) {
    with(|slots| {
        if let Some(s) = slot_of(slots, id)
            && matches!(s.state, State::Running | State::Suspended)
        {
            s.events.push_back(Ev::Close);
        }
    });
    wake_worker();
}

/// The window is gone: drop the app (runtime first, by `appd`; the slot on `reap`).
pub fn close(id: AppId) {
    with(|slots| {
        if let Some(s) = slot_of(slots, id) {
            s.closing = true;
            s.events.clear();
        }
    });
    wake_worker();
}

/// Restart an app: new `Store` from the same package (Tarefas, `R`).
pub fn restart(id: AppId) {
    with(|slots| {
        if let Some(s) = slot_of(slots, id)
            && !s.closing
        {
            s.restart = true;
        }
    });
    wake_worker();
}

/// Free the slots of apps whose runtime `appd` has dropped. Call every frame.
pub fn reap() {
    with(|slots| {
        for s in slots.iter_mut() {
            let done = matches!(s, Some(x) if x.closing && x.rt_dropped && x.reading.is_none());
            if done {
                *s = None; // surfaces are freed here, on the compositor thread
            }
        }
    });
}

/// Window geometry and visibility, once per frame: the content size the surface
/// should have, and whether the window is on screen (minimized -> suspended).
/// Returns the app's title if the guest set one.
pub fn sync_window(id: AppId, w: i32, h: i32, visible: bool) -> Option<String> {
    let now = crate::interrupts::ticks();
    let mut wake = false;
    let title = with(|slots| {
        let s = slot_of(slots, id)?;
        let (w, h) = (w.clamp(1, 1280), h.clamp(1, 800));
        if (w, h) != (s.req_w, s.req_h) {
            s.req_w = w;
            s.req_h = h;
            s.req_at = now;
            wake = true;
        }
        if visible != s.visible {
            s.visible = visible;
            match (visible, s.state) {
                (false, State::Running) => s.state = State::Suspended,
                (true, State::Suspended) => {
                    s.state = State::Running;
                    s.want_redraw = true;
                    wake = true;
                }
                _ => {}
            }
        }
        (!s.title.is_empty()).then(|| s.title.clone())
    });
    if wake {
        wake_worker();
    }
    title
}

// ---------------------------------------------------------------- input (compositor)

fn push_ev(id: AppId, ev: Ev) {
    let queued = with(|slots| {
        let Some(s) = slot_of(slots, id) else {
            return false;
        };
        if !matches!(s.state, State::Running | State::Starting) || s.closing {
            return false;
        }
        // Coalesce pointer motion: only the newest position matters.
        if let (Ev::Pointer(_, _, b), Some(Ev::Pointer(_, _, lb))) = (ev, s.events.back())
            && b == *lb
        {
            s.events.pop_back();
        }
        if s.events.len() >= EV_CAP {
            return false;
        }
        s.events.push_back(ev);
        true
    });
    if queued {
        wake_worker();
    }
}

/// Key press: `code` per the ABI (ASCII, 10 Enter, 27 Esc, 8 BS, 127 Del, 0x100.. arrows),
/// `mods` bit0 Shift, bit1 Ctrl, bit2 Alt.
pub fn key(id: AppId, code: i32, mods: i32) {
    push_ev(id, Ev::Key(code, mods));
    // Printable keys also arrive as text (already translated by the keymap).
    if (0x20..0x7F).contains(&code) && mods & 6 == 0 {
        push_ev(id, Ev::Text(code));
    }
}

/// Pointer position (content-local) and buttons (bit0 left, bit1 right).
pub fn pointer(id: AppId, x: i32, y: i32, buttons: i32) {
    push_ev(id, Ev::Pointer(x, y, buttons));
}

// ---------------------------------------------------------------- status (Tarefas, desktop)

/// A snapshot of one app for Tarefas.
pub struct Status {
    pub id: AppId,
    pub app_id: String,
    pub state: State,
    pub cpu_pct: u8,
    pub mem_kib: u32,
}

/// Roll the CPU window if a second has passed.
fn roll_cpu(s: &mut Slot, now: u64) {
    let elapsed = now.saturating_sub(s.cpu_win_start);
    if elapsed >= CPU_WINDOW_TICKS {
        let khz = TSC_KHZ.load(Ordering::Relaxed).max(1);
        // cycles available in `elapsed` ticks (4 ms each)
        let avail = khz.saturating_mul(4).saturating_mul(elapsed).max(1);
        s.cpu_pct = (s.cpu_win.saturating_mul(100) / avail).min(100) as u8;
        s.cpu_win = 0;
        s.cpu_win_start = now;
    }
}

/// Status of every live app, in slot order.
pub fn statuses() -> Vec<Status> {
    let now = crate::interrupts::ticks();
    with(|slots| {
        slots
            .iter_mut()
            .flatten()
            .filter(|s| !s.closing)
            .map(|s| {
                roll_cpu(s, now);
                if now.saturating_sub(s.cpu_win_start) > 2 * CPU_WINDOW_TICKS {
                    s.cpu_pct = 0;
                }
                Status {
                    id: s.id,
                    app_id: s.manifest.id.clone(),
                    state: s.state,
                    cpu_pct: s.cpu_pct,
                    mem_kib: s.mem_kib,
                }
            })
            .collect()
    })
}

/// What `appd` decided to do with one slot this pass (a snapshot taken under the lock).
struct Job {
    starting: bool,
    v1: bool,
    quotas: Quotas,
    events: Vec<Ev>,
    redraw: bool,
    resize: Option<(i32, i32)>,
    tick: Option<i32>,
    frame: bool,
}

mod appd;
mod clip;
mod draw;
mod runtime;

pub use appd::worker;
pub use clip::{clip_generation, clip_get, clip_load, clip_set};
pub use draw::blit;
use runtime::{account, build_runtime, publish, resize_surfaces, run_guest, sync_title};
