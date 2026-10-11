#![no_std]
#![no_main]
#![warn(clippy::undocumented_unsafe_blocks)]
#![feature(abi_x86_interrupt)]

extern crate alloc;

mod allocator;
mod ata;
mod boot;
mod bringup;
mod clock;
mod crash;
mod desktop;
mod fb;
mod fetch;
mod font;
mod gdt;
mod glyphs;
mod icons;
mod interrupts;
mod io;
mod klog;
mod logd;
mod ne2000;
mod netd;
mod netstack;
mod nic;
mod notify;
mod pci;
mod perf;
mod power;
mod ps2;
mod rng;
mod rtc;
mod sched;
mod serial;
mod settings;
mod storage;
mod sync;
mod sysinfo;
mod text;
mod theme;
mod tlsv;
mod trace;
mod userproc;
mod uvm;
mod virtio;
mod virtio_gpu;
mod virtio_net;
mod virtio_rng;
mod vm;
mod wasm;

use bootloader_api::config::{BootloaderConfig, Mapping};
use bootloader_api::{BootInfo, entry_point};
use core::panic::PanicInfo;
use desktop::{Compositor, Desktop, FrameIn, Screen};
use fb::Canvas;
use kitsune_core::Time;
use ps2::Event;

// Ask the bootloader to map all physical memory at a fixed offset. This gives
// the kernel a `physical_memory_offset` so it can translate between virtual and
// physical addresses — required for DMA: the virtio-gpu device reads its
// descriptor rings and buffers by physical address.
static BOOT_CONFIG: BootloaderConfig = {
    let mut c = BootloaderConfig::new_default();
    c.mappings.physical_memory = Some(Mapping::Dynamic);
    // The default 80 KiB boot stack (which the compositor thread runs on) has
    // only ~11 KiB in use but no canary; the recursive HTML/CSS layout and the
    // TLS stack are the deepest users. 512 KiB keeps a wide margin; the
    // bootloader still adds a guard page below it.
    c.kernel_stack_size = 512 * 1024;
    c
};

entry_point!(kernel_main, config = &BOOT_CONFIG);

// Render buffers sized for up to 1920x1080x4. BACK is the compositing target;
// BG caches the static wallpaper so it is never recomputed per frame.
const MAX_BYTES: usize = 1920 * 1080 * 4;

// 64-byte aligned so the framebuffer fast paths can reinterpret rows as `[u32]`
// (one 32-bit store per pixel, vectorized) without an unaligned cast.
#[repr(C, align(64))]
struct AlignedBuf([u8; MAX_BYTES]);

use sync::RacyCell;

static BACK: RacyCell<AlignedBuf> = RacyCell::new(AlignedBuf([0; MAX_BYTES]));
static BG: RacyCell<AlignedBuf> = RacyCell::new(AlignedBuf([0; MAX_BYTES]));
// Cached "everything except the animating window(s)" layer, composed once per
// animation so each frame only redraws the small damaged region.
static STATIC: RacyCell<AlignedBuf> = RacyCell::new(AlignedBuf([0; MAX_BYTES]));

// Kernel heap backing the global allocator. Sized for the heaviest user: a
// native WASM app's linear memory (DOOM grows its wasm memory to ~16-20 MiB for
// its zone + the IWAD it reads in), well above what the TLS 1.3 handshake or the
// smoltcp socket buffers need. Zero-initialized BSS — no on-disk image cost.
const HEAP_SIZE: usize = 64 * 1024 * 1024;
static HEAP: RacyCell<[u8; HEAP_SIZE]> = RacyCell::new([0; HEAP_SIZE]);

#[global_allocator]
static ALLOCATOR: allocator::LockedHeap = allocator::LockedHeap::new();

fn kernel_main(boot_info: &'static mut BootInfo) -> ! {
    // Serial first: a text log on COM1 that survives even if the framebuffer is
    // missing, so driver bring-up is observable without the screen.
    serial::init();
    trace::mark("kernel entry");
    klog!(Info, "Kitsune boot: kernel entry");

    // Capture the physical-memory offset before `boot_info` is borrowed for the
    // framebuffer. Needed for DMA (virtio-gpu addresses memory physically).
    let phys_offset = boot_info.physical_memory_offset.into_option();
    let kernel_len = boot_info.kernel_len;
    klog!(Info, "physical_memory_offset: {:#x?}", phys_offset);
    // The page tables are reached through that mapping (thread-stack guard pages, see `vm`).
    vm::init(phys_offset);
    // Physical frames for user address spaces: every usable region the bootloader reported.
    // (A fixed array: the heap does not exist yet.)
    let mut usable = [(0u64, 0u64); 64];
    let mut n_usable = 0;
    for r in boot_info.memory_regions.iter() {
        if r.kind == bootloader_api::info::MemoryRegionKind::Usable && n_usable < usable.len() {
            usable[n_usable] = (r.start, r.end);
            n_usable += 1;
        }
    }

    let framebuffer = match boot_info.framebuffer.as_mut() {
        Some(fb) => fb,
        None => {
            serial_println!("FATAL: bootloader provided no framebuffer");
            crash::halt()
        }
    };
    let info = framebuffer.info();
    klog!(
        Info,
        "framebuffer: {}x{} stride {} px, {} bytes/px, {:?}",
        info.width,
        info.height,
        info.stride,
        info.bytes_per_pixel,
        info.pixel_format
    );
    sysinfo::capture(
        &boot_info.memory_regions,
        kernel_len,
        info.width,
        info.height,
    );
    // From here on a panic or fatal exception paints an error page on this framebuffer.
    crash::register_framebuffer(framebuffer.buffer_mut(), info);
    let n = framebuffer.buffer().len().min(MAX_BYTES);

    // The render buffers are fixed-size statics and `Canvas` indexes them with the real
    // layout, so a larger screen would panic with no message once drawing starts. Refuse
    // cleanly instead, saying what was detected.
    let fb_need = info.stride * info.height * info.bytes_per_pixel;
    if fb_need > n {
        crash::die(
            crash::Kind::Unsupported,
            "this screen resolution is not supported",
            format_args!(
                "Detected {}x{} (stride {}, {} bytes/pixel): the screen needs {} bytes, the \
                 bootloader provided a framebuffer of {} bytes and the kernel's render \
                 buffers hold at most {} bytes (1920x1080x4).\nSelect a smaller resolution \
                 in the firmware or bootloader settings (1920x1080 or lower) and reboot.",
                info.width,
                info.height,
                info.stride,
                info.bytes_per_pixel,
                fb_need,
                framebuffer.buffer().len(),
                MAX_BYTES,
            ),
            None,
        );
    }

    // Wipe the bootloader's on-screen debug log immediately, so the early init
    // (PCI scan, TSC calibration, DHCP) shows a clean screen instead of a frozen
    // wall of text until the splash takes over.
    framebuffer.buffer_mut()[..n].fill(0);
    trace::mark("framebuffer cleared");

    // SAFETY: BACK/BG/STATIC are three distinct statics, each viewed once here (`kernel_main` runs
    // once, on the boot thread) with `n <= MAX_BYTES`, their size; afterwards only the compositor
    // loop uses them, so each `&mut [u8]` is unique.
    let back: &mut [u8] = unsafe { core::slice::from_raw_parts_mut(BACK.get() as *mut u8, n) };
    // SAFETY: as for `back` (BG is a separate static).
    let bg: &mut [u8] = unsafe { core::slice::from_raw_parts_mut(BG.get() as *mut u8, n) };
    // SAFETY: as for `back` (STATIC is a separate static).
    let static_buf: &mut [u8] =
        unsafe { core::slice::from_raw_parts_mut(STATIC.get() as *mut u8, n) };

    // Initialize the kernel heap so `alloc` works, then smoke-test it.
    // SAFETY: HEAP is a dedicated 64 MiB static, valid and writable, used for nothing else; this
    // runs once, on the boot thread, before the first allocation.
    // NOTE: HEAP's type has align 1; the 8-byte alignment `init` needs comes from linker placement
    // and is only `debug_assert!`ed.
    unsafe {
        ALLOCATOR.init(HEAP.get() as usize, HEAP_SIZE);
    }
    bringup::heap_smoke_test();
    trace::mark("heap init + smoke test");
    uvm::init_frames(&usable[..n_usable]);
    uvm::harden();

    // First run through the native WebAssembly app engine: prove the OS can load
    // and execute a `.wasm` program (its native app format) end to end. Output
    // lands on serial. The graphics/input ABI (windowed apps) builds on this.
    klog!(Info, "Kitsune boot: running native WASM demo");
    wasm::run_demo();
    trace::mark("wasm demo done");

    bringup::probe_pci(phys_offset);
    trace::mark("pci scan + virtio-gpu probe done");

    // Kernel scheduler: register the boot context (the compositor) as thread 0.
    // The preemptive round-robin is in place for real future threads, but we no
    // longer spawn demo spin-workers — they consumed 2/3 of the CPU under the
    // equal-slice round-robin, starving the compositor and capping the frame
    // rate. With the GUI as the sole thread it gets the whole core.
    // Probe the IDE channels: report where the OS is installed (boot disk) and
    // whether each disk is a spinning HD or an SSD, so storage can adapt.
    ata::detect_and_log();
    trace::mark("ata detect done");

    // Own GDT + TSS (IST stack for #DF) before anything captures CS/SS.
    gdt::init();

    sched::init();

    // Configure the PS/2 controller BEFORE enabling interrupts, so the init
    // handshake (config write + mouse ACKs) is read by polling without racing
    // the IRQ handlers.
    ps2::init();

    // Real interrupts: IDT + exception handlers, PIC remap, PIT timer, and
    // IRQ-driven keyboard (IRQ1) + mouse (IRQ12).
    interrupts::init();
    trace::mark("sched + ps2 + idt/pic/pit init");

    // Calibrate the TSC against the now-running PIT so the perf HUD can report
    // frame time in real milliseconds.
    let tsc_khz = perf::calibrate_khz();
    klog!(Info, "TSC calibrated: {} kHz", tsc_khz);
    netd::set_tsc_khz(tsc_khz);
    // Entropy before the first consumer (DHCP transaction ids, TCP sequence numbers): probes
    // RDSEED/RDRAND and virtio-rng, and starts crediting the timer-interrupt samples.
    rng::init(phys_offset);
    trace::mark("tsc calibrated (25 PIT ticks)");
    trace::calibrated(tsc_khz);
    trace::bench_prims(back, info, tsc_khz);
    trace::bench_alloc(tsc_khz);
    trace::bench_ata(tsc_khz);
    trace::bench_password(tsc_khz);
    trace::bench_apps(tsc_khz);
    let mut perf = perf::Perf::new(tsc_khz);

    // Bring up the NIC (if present), lease an IP over DHCP (falling back to the
    // static address if no server answers), and announce ourselves with a
    // gratuitous ARP so the network is visible on the wire from boot. No card ->
    // skip and keep the static IP. The `Port` is then moved into the network stack
    // that the `fetcher` thread owns: from there on it is the only party that can
    // touch the hardware.
    let port = nic::probe(phys_offset);
    trace::mark("nic init done");
    // `Netd::boot` runs the DHCP exchange (falling back to the static address) and builds the stack
    // around the port. The result is handed to the `fetcher` thread, which is from then on the only
    // party that can touch the hardware (DHCP renewal, ARP/ping and fetches all run there).
    let netd = port.map(netd::Netd::boot);
    trace::mark("dhcp done");
    // Capture the RTC (UTC) once, before any other thread can touch the CMOS
    // ports: it is the local clock that SNTP then corrects for TLS date checks.
    clock::init();

    // Hand the browser's TCP/IP + TLS stack to the background fetcher and spawn
    // its worker thread, so page loads (and the slow software TLS handshake) run
    // off the compositor thread and never freeze the UI. Interrupts are disabled
    // around `spawn` because it mutates the scheduler's thread list, which the
    // timer ISR also reads.
    match netd {
        Some(netd) => {
            fetch::init(netd);
            x86_64::instructions::interrupts::without_interrupts(|| {
                sched::spawn("fetcher", fetch::worker);
            });
        }
        None => fetch::init_offline(),
    }

    // Hand the framebuffer layout to the WASM app manager and spawn `appd`, the
    // thread that runs every app (round-robin), so a heavy app (DOOM loading its
    // WAD, then running its game loop) renders off the compositor thread and never
    // freezes the UI.
    wasm::init(info, tsc_khz);
    x86_64::instructions::interrupts::without_interrupts(|| {
        sched::spawn_with_stack("appd", wasm::worker, 512 * 1024);
        // The terminal's command thread: sleep, ping and curl wait here, not in the compositor.
        sched::spawn("shelld", desktop::shell_worker);
        sched::spawn("shelld2", desktop::shell_worker2);
    });

    trace::mark("threads spawned (fetcher, appd, shelld x2)");

    // Storage service: detect/mount/migrate OJFS v3 on the filesystem disk (the desktop
    // still runs on the v2 image; see storage.rs). Runs with the scheduler up, so the
    // ATA driver can yield to the other threads between sectors.
    storage::init();
    trace::mark("storage init done");
    desktop::init_accounts();
    trace::mark("accounts init done");

    bringup::init_text_engine(tsc_khz);
    trace::mark("ui text engine ready");
    // logd: persists the system log off the compositor thread (the boot log is written
    // after the first desktop frame, see `logd`).
    x86_64::instructions::interrupts::without_interrupts(|| {
        sched::spawn("logd", logd::worker);
    });

    // Boot splash: progress tracks real elapsed time (>= 5 seconds). The brand mark is drawn
    // once up front; its cost is logged below, the TSC is already calibrated.
    let brand_t0 = io::rdtsc();
    boot::prewarm();
    let brand_cyc = io::rdtsc().wrapping_sub(brand_t0);
    klog!(
        Info,
        "boot: brand mark rendered in {} us ({} Mcycles)",
        brand_cyc.saturating_mul(1000) / tsc_khz.max(1),
        brand_cyc / 1_000_000
    );
    bringup::run_splash(&mut *framebuffer, &mut *back, info, n);
    trace::mark("splash end (artificial >= 5 s)");

    // The desktop (and its filesystem, which holds the stored settings) comes first so
    // the wallpaper and accent can honour them; with no settings file the defaults
    // paint exactly what the desktop always looked like.
    let mut desk = Desktop::new(info.width as i32, info.height as i32);
    trace::mark("Desktop::new (fs load from ATA) done");
    desk.load_settings();
    trace::mark("settings loaded");

    // Static layer painted once.
    {
        let mut c = Canvas::new(&mut *bg, info);
        desktop::paint_background(&mut c);
    }
    trace::mark("wallpaper painted");
    let mut last_sec = 0xFFu8; // force first render
    // The compositor owns the cursor sprite, the toasts and the HUD (all framebuffer-only) and
    // the damage engine; the first frame it plans repaints the whole screen over the splash.
    let mut comp = Compositor::new(info.width as i32, info.height as i32);
    // Seed from the live tick count, NOT 0: the splash ran for ~5 s with the
    // timer firing, so a 0 seed would make the first frame's delta enormous and
    // instantly complete the open animation (skipping the full-screen blit that
    // clears the splash).
    let mut last_tick = interrupts::ticks();
    // Set after a browser fetch completes so the next iteration repaints the
    // page (the fetch itself blocks, so it can't render in its own frame).
    let mut browser_redraw = false;

    // Animations run on real time: seconds per timer tick (250 Hz), independent of how
    // often the GUI thread is scheduled (the timer preempts round-robin across threads).
    const DT_PER_TICK: f32 = 1.0 / interrupts::TIMER_HZ as f32;
    let mut first_frame = true;

    loop {
        let rt = rtc::now();
        let time = Time {
            h: rt.h,
            m: rt.m,
            s: rt.s,
        };

        // Advance animation by real elapsed timer ticks.
        let tick = interrupts::ticks();
        let tick_delta = tick.saturating_sub(last_tick);
        last_tick = tick;
        let tick_changed = tick_delta > 0;
        if tick_changed {
            desk.animate(tick_delta as f32 * DT_PER_TICK);
        }

        // Drain all pending PS/2 events.
        let external = core::mem::take(&mut browser_redraw);
        let mut scene_dirty = external;
        let mut cursor_moved = false;
        let mut clock_tick = false;
        let mut got_input = false;
        trace::loop_iter();
        while let Some(event) = ps2::poll() {
            got_input = true;
            match event {
                Event::Mouse(p) => {
                    let r = desk.handle_mouse(p.dx, p.dy, p.left, p.right);
                    scene_dirty |= r.scene_dirty;
                    cursor_moved |= r.cursor_moved;
                    // The wheel goes to the window under the pointer.
                    if p.dz != 0 && desk.handle_wheel(p.dz) {
                        scene_dirty = true;
                    }
                }
                Event::Key(k) => {
                    if desk.handle_key(k.scan_code, k.extended, k.pressed, time) {
                        scene_dirty = true;
                    }
                }
            }
        }

        // A browser window whose client area is the only thing that changed.
        let client_dirty = desk.take_client_dirty();
        let input_irq_tsc = if got_input { trace::input_taken() } else { 0 };
        let mut report_due = false;
        if rt.s != last_sec {
            last_sec = rt.s;
            report_due = true;
            desk.tick_processes();
            clock_tick = true;
            perf.second_tick();
            let (fps, frame_us, max_us) = perf.stats();
            desk.sample_system(&desktop::SysInputs {
                ticks: interrupts::ticks(),
                busy: sched::busy_ticks(),
                heap_used: HEAP_SIZE - ALLOCATOR.free_bytes().min(HEAP_SIZE),
                heap_total: HEAP_SIZE,
                fps,
                frame_us,
                max_us,
                tsc_khz,
            });
        }

        // Tell the app manager each WASM window's size / visibility; reap finished apps.
        desk.wasm_sync();

        // The wallpaper or accent changed: repaint the cached background and
        // recompose everything (the settings app also asked for a full repaint).
        if desk.take_bg_repaint() {
            let mut c = Canvas::new(&mut *bg, info);
            desktop::paint_background(&mut c);
            comp.repaint_everything();
            scene_dirty = true;
        }

        // One frame: the compositor decides what to repaint from the desktop's state (see
        // `desktop::compositor`), uploads exactly that, and keeps the cursor, the toasts and the
        // HUD in the framebuffer.
        let force_full = desk.take_full_repaint();
        let extra_dirty = desk.take_extra_dirty();
        let mut scr = Screen {
            fb: framebuffer.buffer_mut(),
            back: &mut *back,
            bg: &mut *bg,
            check: &mut *static_buf,
            info,
            n,
        };
        let out = comp.frame(
            &mut desk,
            &mut scr,
            &mut perf,
            &FrameIn {
                time,
                tick,
                tick_changed,
                input: scene_dirty || force_full,
                force_full,
                extra_dirty,
                client: client_dirty,
                external,
                cursor_moved,
                clock_tick,
            },
        );
        if out.worked {
            trace::input_done(input_irq_tsc);
            if first_frame {
                first_frame = false;
                bringup::report_first_frame();
            }
        }
        if report_due {
            trace::report(tsc_khz, tick);
        }

        bringup::pump_browser(&mut desk, &mut browser_redraw);

        // Idle until the next interrupt instead of busy-spinning. The timer
        // (250 Hz) wakes us to step animations and the per-second clock; the
        // keyboard/mouse IRQs wake us immediately on input. This paces frames to
        // the tick rate and stops the compositor from burning a full core — a
        // real system halts when it has nothing to draw.
        //
        // `sched::idle` does this without the old lost-wakeup window (input that
        // lands between the last poll and the `hlt` is noticed with interrupts
        // masked) and, when a worker thread is runnable, gives it the CPU now
        // rather than halting through the rest of our slice.
        sched::idle(interrupts::input_pending);
        trace::hlt_wake();
    }
}

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    // COM1 and the screen both get the report (see `crash`); without this every
    // panic, including allocation failure, was a silent `hlt`.
    // A panic in a plain thread context kills only that thread (see `sched::kill_current`); one in the
    // compositor, in an ISR or with interrupts off, or a second one while killing, is fatal.
    crash::fault(
        crash::Kind::Panic,
        "the kernel panicked",
        format_args!("{info}"),
        None,
        x86_64::instructions::interrupts::are_enabled(),
    )
}
