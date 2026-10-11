//! Native WebAssembly app platform.
//!
//! WebAssembly is Kitsune's native application format: portable programs compiled
//! to `.wasm` run *inside* the OS through this interpreter ([`wasmi`]) — no
//! foreign OS, no binary emulation, and sandboxed by construction (a guest can
//! only touch its own linear memory and the host functions we explicitly grant).
//!
//! This module is the host side of two ABIs (`docs/design/apps.md`):
//!
//! * **v1** (`host.*`): `log`, `fill_rect`, `draw_text`, `blit`, `time_ms` plus the
//!   WASI subset in [`wasi`]. Continuous `render` loop; snake, plasma, DOOM.
//! * **v2** (`osj.*`, [`abi2`]): events, windows, files, clipboard, network.
//!
//! [`manager`] runs any number of apps at once (one `Store` each, one `appd`
//! thread scheduling them round-robin) and is what the desktop talks to.
//!
//! Drawing coordinates are relative to the guest's own surface origin: the host
//! translates them by `(ox, oy)` and clips every primitive to `(cw, ch)`, so a
//! guest paints from `(0,0)` and the kernel places it inside a window's content
//! box. Console-only guests leave the surface unset, making the drawing
//! syscalls no-ops.

use crate::fb::{Canvas, Color};
use crate::serial_print;
use bootloader_api::info::FrameBufferInfo;
use wasmi::{
    Caller, Config, Engine, Extern, Linker, Module, Store, StoreLimits, StoreLimitsBuilder,
};

pub(crate) mod abi2;
pub(crate) mod appfs_backend;
pub(crate) mod manager;
pub(crate) mod remote;
mod wasi;

pub(crate) use manager::*;

// ---- guest resource limits ----
//
// A guest is untrusted code running inside the kernel: without limits a loop or
// a runaway `memory.grow` takes the whole machine down with it. Every guest gets
//   * fuel (one unit per wasm instruction) per call into it,
//   * a hard cap on its linear memory, and
//   * caps on the work the host does on its behalf (see `wasi.rs`, `host_blit`).

/// Fuel for one call into a legacy (manifest-less) guest. About 20 million wasm
/// instructions: DOOM's steady-state frame is 4.5-9 M (see
/// docs/audit/adr-isolamento.md), so this leaves ~2x headroom, yet a guest stuck
/// in a loop is stopped within a few frames' worth of CPU. Packaged apps get
/// `fuel_frame` from their manifest (never above this).
pub(crate) const FRAME_FUEL: u64 = 20_000_000;
/// Fuel for legacy module start-up: `_initialize` and the first `render`, where C
/// apps do their one-time setup (DOOM's `doomgeneric_Create` costs ~39 M).
pub(crate) const INIT_FUEL: u64 = 256_000_000;
/// Cap on a guest's table size (DOOM's indirect-call table has a few thousand).
const TABLE_LIMIT: usize = 100_000;
/// Longest string (bytes) a guest may pass to `host.log` / `host.draw_text`.
const MAX_TEXT: i32 = 4096;
/// Largest image (pixels) one `host.blit` will copy: more than any window here.
const MAX_BLIT_PIXELS: i64 = 1 << 20;
/// Memory cap of the console demo (it needs one page).
const DEMO_MEM: usize = 1 << 20;

/// The limits applied to a guest `Store`: `mem_bytes` of linear memory (a hostile
/// `memory.grow` past it fails, returning -1 to the guest as the spec says).
pub(crate) fn guest_limits(mem_bytes: usize) -> StoreLimits {
    StoreLimitsBuilder::new()
        .memory_size(mem_bytes)
        .table_elements(TABLE_LIMIT)
        .instances(1)
        .memories(1)
        .tables(1)
        .build()
}

/// An engine with fuel metering switched on (off by default in wasmi).
pub(crate) fn guest_engine() -> Engine {
    let mut cfg = Config::default();
    cfg.consume_fuel(true);
    Engine::new(&cfg)
}

/// The console demo (boot smoke-test), assembled from WAT at build time (see
/// `build.rs`).
static DEMO_WASM: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/demo.wasm"));
/// The legacy windowed app (`DOOM=1` / `WASI_SDK_PATH` builds); empty otherwise.
pub(crate) static LEGACY_APP: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/app.wasm"));
/// The DOOM IWAD, embedded so the WASI file layer ([`wasi`]) can serve it to a
/// wasm guest. Empty unless the kernel was built in DOOM mode (`build.rs` writes
/// the real `doom1.wad` to `OUT_DIR` then, otherwise an empty placeholder).
pub(super) static WAD: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/doom1.wad"));

include!(concat!(env!("OUT_DIR"), "/apps_gen.rs"));

/// A guest fault the host raises as a trap: the app dies, nothing else does.
#[derive(Debug)]
pub(crate) struct AppFault(pub &'static str);

impl core::fmt::Display for AppFault {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        // The log is English; the window asks the catalog again in the language of the moment.
        f.write_str(kitsune_core::i18n::tr_in(
            kitsune_core::i18n::Lang::En,
            self.0,
        ))
    }
}

/// Why an app stopped: a catalog key (`apps.why.*`) and the one value it names (an exit code,
/// the text of a load error, always `{x}`). The window shows [`Why::text`] in the language in
/// effect when it is drawn; the serial log gets the English [`Why::log`].
#[derive(Clone, Debug, Default)]
pub(crate) struct Why {
    key: &'static str,
    arg: alloc::string::String,
}

impl Why {
    pub(crate) fn new(key: &'static str) -> Why {
        Why {
            key,
            arg: alloc::string::String::new(),
        }
    }

    pub(crate) fn with(key: &'static str, arg: impl Into<alloc::string::String>) -> Why {
        Why {
            key,
            arg: arg.into(),
        }
    }

    fn render(&self, l: kitsune_core::i18n::Lang) -> alloc::string::String {
        kitsune_core::i18n::tr_fmt_in(
            l,
            self.key,
            &[("x", kitsune_core::i18n::Arg::Str(self.arg.as_str()))],
        )
    }

    /// Is this the "no process could be started" reason (the app then runs in the kernel)?
    pub(crate) fn is_no_process(&self) -> bool {
        self.key == kitsune_core::tk!("apps.why.no_process")
    }

    /// In the language in effect.
    pub(crate) fn text(&self) -> alloc::string::String {
        self.render(kitsune_core::i18n::lang())
    }

    /// In English, for the log.
    pub(crate) fn log(&self) -> alloc::string::String {
        self.render(kitsune_core::i18n::Lang::En)
    }
}

/// How a guest call ended badly: the reason for the window, and whether the guest left by itself
/// (`exit`) rather than crashed.
#[derive(Debug)]
pub(crate) struct Halt {
    pub(crate) why: Why,
    pub(crate) exited: bool,
}

impl From<wasmi::Error> for Halt {
    fn from(e: wasmi::Error) -> Halt {
        Halt {
            exited: e.i32_exit_status().is_some(),
            why: describe(&e),
        }
    }
}

impl core::fmt::Display for Why {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(&self.log())
    }
}

impl wasmi::errors::HostError for AppFault {}

/// Per-instance host state: the surface a guest's drawing syscalls target plus
/// the translation/clip that places that surface inside a window, and (for
/// packaged apps) the v2 state.
///
/// A raw pointer (not a borrow) because the guest calls back into these host
/// functions from inside `wasmi`, outliving any normal borrow. The `HostState`
/// of an app lives in its `Store`, which only the `appd` worker thread touches;
/// the console demo runs once on the boot thread before the scheduler starts.
/// Either way one thread at a time owns it.
pub(crate) struct HostState {
    fb: *mut u8,
    fb_len: usize,
    info: Option<FrameBufferInfo>,
    ox: i32,
    oy: i32,
    cw: i32,
    ch: i32,
    /// WASI: the single open WAD file handle (DOOM keeps the IWAD open for the
    /// whole session). `-1` when closed. `wad_pos` is the read cursor.
    wad_fd: i32,
    wad_pos: usize,
    /// Memory/table/instance caps, enforced by the `Store`'s resource limiter.
    limits: StoreLimits,
    /// ABI v2 state (`None` for the console demo).
    pub(crate) v2: Option<alloc::boxed::Box<abi2::V2>>,
}

impl HostState {
    /// Console-only state: the drawing syscalls become no-ops.
    fn console() -> Self {
        Self::with_limits(guest_limits(DEMO_MEM))
    }

    pub(crate) fn with_limits(limits: StoreLimits) -> Self {
        Self {
            fb: core::ptr::null_mut(),
            fb_len: 0,
            info: None,
            ox: 0,
            oy: 0,
            cw: 0,
            ch: 0,
            wad_fd: -1,
            wad_pos: 0,
            limits,
            v2: None,
        }
    }

    /// Point the drawing syscalls at a surface of `w` x `h` pixels.
    pub(crate) fn set_surface(&mut self, fb: *mut u8, len: usize, info: FrameBufferInfo) {
        self.fb = fb;
        self.fb_len = len;
        self.cw = info.width as i32;
        self.ch = info.height as i32;
        self.ox = 0;
        self.oy = 0;
        self.info = Some(info);
    }

    /// Detach the surface (the guest is not running).
    pub(crate) fn clear_surface(&mut self) {
        self.info = None;
        self.fb = core::ptr::null_mut();
        self.fb_len = 0;
    }

    /// Build a `Canvas` over the surface, or `None` for console-only guests.
    fn canvas(&self) -> Option<Canvas<'static>> {
        let info = self.info?;
        // SAFETY: only the thread that owns this `Store` (the `appd` worker, or the boot thread for
        // the console demo) calls this; the pointer/length come from a live surface slice that outlives
        // this guest call (set right before it, cleared right after).
        let buf = unsafe { core::slice::from_raw_parts_mut(self.fb, self.fb_len) };
        Some(Canvas::new(buf, info))
    }
}

/// Unpack a packed `0xRRGGBB` integer into a framebuffer `Color`.
fn rgb(packed: i32) -> Color {
    let p = packed as u32;
    Color::rgb((p >> 16) as u8, (p >> 8) as u8, p as u8)
}

/// `host.fill_rect`: fill a guest rectangle, translated by the surface origin
/// and clipped to the surface box.
fn host_fill(st: &HostState, x: i32, y: i32, w: i32, h: i32, color: i32) {
    let Some(mut c) = st.canvas() else { return };
    let x0 = (st.ox as i64 + x as i64).max(st.ox as i64);
    let y0 = (st.oy as i64 + y as i64).max(st.oy as i64);
    let x1 = (st.ox as i64 + x as i64 + w as i64).min((st.ox + st.cw) as i64);
    let y1 = (st.oy as i64 + y as i64 + h as i64).min((st.oy + st.ch) as i64);
    if x1 > x0 && y1 > y0 {
        c.fill_rect(
            x0.max(0) as usize,
            y0.max(0) as usize,
            (x1 - x0) as usize,
            (y1 - y0) as usize,
            rgb(color),
        );
    }
}

/// `host.draw_text`: draw guest text translated by the surface origin and
/// clipped to the content box, glyph by glyph, so a guest can never paint text
/// past its window onto the desktop or another window (sandbox containment).
fn host_text(st: &HostState, s: &str, x: i32, y: i32, color: i32, scale: i32) {
    let Some(mut c) = st.canvas() else { return };
    let scale = scale.clamp(1, 16) as usize;
    let cw = crate::text::guest::cell_w(scale) as i64;
    let gh = (8 * scale) as i64; // glyph cell height
    let (bx, by, bw, bh) = (st.ox as i64, st.oy as i64, st.cw as i64, st.ch as i64);
    let py = st.oy as i64 + y as i64;
    // Vertical clip: drop the whole line unless it fits inside the box.
    if py < by || py + gh > by + bh {
        return;
    }
    let col = rgb(color);
    let mut px = st.ox as i64 + x as i64;
    for ch in s.chars() {
        if px >= bx + bw {
            break; // past the right edge — nothing more is visible
        }
        // Only draw glyphs wholly inside the box horizontally.
        if px >= bx && px + cw <= bx + bw {
            crate::text::guest::draw_char(&mut c, px as usize, py as usize, ch, col, scale);
        }
        px += cw;
    }
}

/// `host.blit`: copy a `w*h` RGBA image from guest memory (at offset `off`) to
/// the surface at content-local `(dx, dy)`, translated by the surface origin and
/// clipped to the content box. The per-frame primitive for framebuffer apps
/// (a game pushes its rendered frame this way).
///
/// The copy is host work the fuel meter cannot see, so it is capped
/// ([`MAX_BLIT_PIXELS`]) and charged to the guest's fuel (one unit per 8 pixels).
fn host_blit(
    caller: &mut Caller<'_, HostState>,
    off: i32,
    w: i32,
    h: i32,
    dx: i32,
    dy: i32,
) -> Result<(), wasmi::Error> {
    if w <= 0 || h <= 0 || (w as i64) * (h as i64) > MAX_BLIT_PIXELS {
        return Ok(());
    }
    charge(caller, (w as u64) * (h as u64) / 8)?;
    let caller = &*caller;
    let st = caller.data();
    let (ox, oy, cw, ch) = (st.ox, st.oy, st.cw, st.ch);
    let Some(mut c) = st.canvas() else {
        return Ok(());
    };
    let need = (w as i64) * (h as i64) * 4;
    let Some(px) = guest_bytes(caller, off, need.min(i32::MAX as i64) as i32) else {
        return Ok(());
    };
    // Integer nearest-neighbor upscale to fill the content box starting at
    // `(dx, dy)`, aspect-preserved and centered horizontally — so a small guest
    // framebuffer (DOOM's 320×200) fills the window instead of sitting tiny in a
    // corner. `scale == 1` reproduces a plain 1:1 blit.
    let avail_w = (cw - dx).max(1);
    let avail_h = (ch - dy).max(1);
    let scale = (avail_w / w).min(avail_h / h).max(1);
    let x_off = dx + (avail_w - w * scale).max(0) / 2;
    let s = scale as usize;
    // Each source pixel becomes a scale×scale block. The image is scaled to fit
    // inside the content box, so the blocks never exceed it; `fill_rect` (which
    // clips to the framebuffer) draws each block on the fast 32-bit path.
    for row in 0..h {
        let py0 = (oy + dy + row * scale).max(0) as usize;
        for col in 0..w {
            let o = ((row * w + col) * 4) as usize;
            let color = Color::rgb(px[o], px[o + 1], px[o + 2]);
            c.fill_rect((ox + x_off + col * scale).max(0) as usize, py0, s, s, color);
        }
    }
    Ok(())
}

/// Charge `units` of fuel for host work done on the guest's behalf. Fails with
/// the `OutOfFuel` trap when the guest's budget for this call is spent.
pub(crate) fn charge(caller: &mut Caller<'_, HostState>, units: u64) -> Result<(), wasmi::Error> {
    let left = caller.get_fuel()?;
    if left < units {
        return Err(wasmi::Error::from(wasmi::TrapCode::OutOfFuel));
    }
    caller.set_fuel(left - units)
}

/// Register the v1 OS ABI (`host.*`) and the WASI subset on `linker`.
fn install_abi(linker: &mut Linker<HostState>) -> Result<(), &'static str> {
    linker
        .func_wrap(
            "host",
            "log",
            |caller: Caller<'_, HostState>, ptr: i32, len: i32| {
                if len > MAX_TEXT {
                    return;
                }
                if let Some(s) = guest_str(&caller, ptr, len) {
                    serial_print!("{}", s);
                }
            },
        )
        .map_err(|_| "link host.log")?;
    linker
        .func_wrap(
            "host",
            "fill_rect",
            |caller: Caller<'_, HostState>, x: i32, y: i32, w: i32, h: i32, color: i32| {
                host_fill(caller.data(), x, y, w, h, color);
            },
        )
        .map_err(|_| "link host.fill_rect")?;
    linker
        .func_wrap(
            "host",
            "draw_text",
            |caller: Caller<'_, HostState>,
             x: i32,
             y: i32,
             ptr: i32,
             len: i32,
             color: i32,
             scale: i32| {
                if len > MAX_TEXT {
                    return;
                }
                if let Some(s) = guest_str(&caller, ptr, len) {
                    host_text(caller.data(), s, x, y, color, scale);
                }
            },
        )
        .map_err(|_| "link host.draw_text")?;
    // host.blit(ptr, w, h, dx, dy): copy an RGBA frame from guest memory.
    linker
        .func_wrap(
            "host",
            "blit",
            |mut caller: Caller<'_, HostState>,
             off: i32,
             w: i32,
             h: i32,
             dx: i32,
             dy: i32|
             -> Result<(), wasmi::Error> { host_blit(&mut caller, off, w, h, dx, dy) },
        )
        .map_err(|_| "link host.blit")?;
    // host.time_ms(): milliseconds since boot. The PIT ticks at 250 Hz, so each
    // tick is 4 ms. Lets a guest animate or time its own logic.
    linker
        .func_wrap("host", "time_ms", |_: Caller<'_, HostState>| -> i64 {
            (crate::interrupts::ticks() * 4) as i64
        })
        .map_err(|_| "link host.time_ms")?;
    // The WASI subset (wasi_snapshot_preview1) a C/clang guest like DOOM needs.
    // Harmless for guests that don't import it.
    wasi::install(linker)?;
    Ok(())
}

/// Register every host function a guest may import: v1 and v2.
pub(crate) fn install_all(linker: &mut Linker<HostState>) -> Result<(), &'static str> {
    install_abi(linker)?;
    abi2::install(linker)
}

/// Run the embedded console demo: decode it, wire up the ABI, call `main`.
pub fn run_demo() {
    match run(DEMO_WASM, "main", HostState::console()) {
        Ok(()) => crate::serial_println!("wasm: demo module ran OK"),
        Err(e) => crate::serial_println!("wasm: demo failed: {}", e),
    }
}

/// Instantiate `bytes`, granting it the OS ABI, and call its `entry` export.
fn run(bytes: &[u8], entry: &str, state: HostState) -> Result<(), &'static str> {
    let engine = guest_engine();
    let module = Module::new(&engine, bytes).map_err(|_| "module decode")?;
    let mut store = Store::new(&engine, state);
    store.limiter(|st| &mut st.limits);
    store.set_fuel(INIT_FUEL).map_err(|_| "set fuel")?;
    let mut linker = <Linker<HostState>>::new(&engine);
    install_all(&mut linker)?;
    let instance = linker
        .instantiate_and_start(&mut store, &module)
        .map_err(|_| "instantiate")?;
    let func = instance
        .get_typed_func::<(), ()>(&store, entry)
        .map_err(|_| "no entry export")?;
    func.call(&mut store, ()).map_err(|_| "trap in entry")?;
    Ok(())
}

/// Why a guest call failed, in words for the window (and, in English, for the log).
pub(crate) fn describe(e: &wasmi::Error) -> Why {
    use kitsune_core::tk;
    if e.as_trap_code() == Some(wasmi::TrapCode::OutOfFuel) {
        return Why::new(tk!("apps.why.out_of_fuel"));
    }
    if let Some(code) = e.i32_exit_status() {
        return Why::with(tk!("apps.why.exit_code"), alloc::format!("{code}"));
    }
    if let Some(f) = e.downcast_ref::<AppFault>() {
        return Why::new(f.0);
    }
    Why::new(match e.as_trap_code() {
        Some(wasmi::TrapCode::MemoryOutOfBounds) => tk!("apps.why.out_of_bounds"),
        Some(wasmi::TrapCode::UnreachableCodeReached) => tk!("apps.why.unreachable"),
        Some(wasmi::TrapCode::StackOverflow) => tk!("apps.why.stack_overflow"),
        Some(wasmi::TrapCode::IntegerDivisionByZero) => tk!("apps.why.div_zero"),
        Some(wasmi::TrapCode::IndirectCallToNull) => tk!("apps.why.null_call"),
        Some(wasmi::TrapCode::BadSignature) => tk!("apps.why.bad_signature"),
        Some(_) => tk!("apps.why.trap"),
        None => tk!("apps.why.guest"),
    })
}

/// The guest's exported linear memory. WAT modules here export it as `mem`;
/// Rust/clang-compiled modules export it as `memory` — accept either.
pub(crate) fn guest_mem(caller: &Caller<'_, HostState>) -> Option<wasmi::Memory> {
    for name in ["memory", "mem"] {
        if let Some(Extern::Memory(m)) = caller.get_export(name) {
            return Some(m);
        }
    }
    None
}

/// Borrow `len` raw bytes at offset `ptr` from the guest's linear memory.
/// `None` on missing memory or an out-of-bounds range.
fn guest_bytes<'a>(caller: &'a Caller<'_, HostState>, ptr: i32, len: i32) -> Option<&'a [u8]> {
    let mem = guest_mem(caller)?;
    let data = mem.data(caller);
    let (ptr, len) = (ptr as usize, len.max(0) as usize);
    data.get(ptr..ptr.saturating_add(len))
}

/// Borrow a UTF-8 string of `len` bytes at offset `ptr` from guest memory.
/// `None` on missing memory, out-of-bounds range, or non-UTF-8.
fn guest_str<'a>(caller: &'a Caller<'_, HostState>, ptr: i32, len: i32) -> Option<&'a str> {
    core::str::from_utf8(guest_bytes(caller, ptr, len)?).ok()
}
