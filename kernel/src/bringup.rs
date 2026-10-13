//! Boot-time helpers split out of `kernel_main`: the PCI/virtio-gpu probe, the UI text
//! engine bring-up, the browser <-> fetcher hand-off the compositor loop runs each frame, the
//! first-frame report, the boot splash and the heap smoke test.

use crate::desktop::Desktop;
use crate::fb::Canvas;
use crate::interrupts;
use crate::{
    boot, fetch, icons, io, klog, logd, pci, serial_println, text, trace, virtio, virtio_gpu,
};
use bootloader_api::info::FrameBufferInfo;

/// Enumerate PCI bus 0 and bring up the virtio-gpu driver when the device is present (the VBE /
/// GOP framebuffer stays the display either way; the log tells what a machine has).
pub fn probe_pci(phys_offset: Option<u64>) {
    // Enumerate the PCI bus — groundwork for the virtio-gpu driver: locate the
    // device and, when present, enable bus mastering so a later DMA-capable
    // driver can use it. QEMU captures the log via `-serial file:...`.
    klog!(Info, "Kitsune boot: enumerating PCI bus 0");
    pci::for_each(|d| {
        klog!(
            Info,
            "  pci {:02x}:{:02x}.{}  {:04x}:{:04x}",
            d.bus,
            d.slot,
            d.func,
            d.vendor,
            d.device
        );
    });
    match pci::find_virtio_gpu() {
        Some(gpu) => {
            gpu.enable_bus_master();
            klog!(
                Info,
                "virtio-gpu @ slot {} func {} bar0={:#010x}",
                gpu.slot,
                gpu.func,
                gpu.bar(0)
            );
            match virtio::discover(&gpu) {
                Some(caps) => {
                    klog!(
                        Info,
                        "  common bar{} off={:#x} len={}",
                        caps.common.bar,
                        caps.common.offset,
                        caps.common.length
                    );
                    klog!(
                        Info,
                        "  notify bar{} off={:#x} mul={}",
                        caps.notify.bar,
                        caps.notify.offset,
                        caps.notify_off_mul
                    );
                    klog!(
                        Info,
                        "  isr    bar{} off={:#x}   device bar{} off={:#x}",
                        caps.isr.bar,
                        caps.isr.offset,
                        caps.device.bar,
                        caps.device.offset
                    );
                }
                None => klog!(Info, "  virtio caps: none (transitional device?)"),
            }

            // Bring up the full virtio-gpu driver: negotiate, set up the control
            // queue (DMA), flip DRIVER_OK, and query the display geometry — which
            // exercises the entire command path end to end.
            if let (Some(off), Some(caps)) = (phys_offset, virtio::discover(&gpu)) {
                match virtio_gpu::GpuDevice::init(&gpu, caps, off) {
                    Some(mut dev) => {
                        klog!(Info, "virtio-gpu: control queue up, DRIVER_OK");
                        match dev.get_display_info() {
                            Some((w, h)) => {
                                klog!(Info, "virtio-gpu display 0: {}x{}", w, h)
                            }
                            None => klog!(Warn, "virtio-gpu: get_display_info failed"),
                        }
                        // Exercise the 2D command path (no scanout swap — the VBE
                        // display stays live). The accelerated scanout is the next step.
                        dev.verify_2d();
                    }
                    None => klog!(Warn, "virtio-gpu: init failed"),
                }
            }
        }
        None => klog!(Info, "virtio-gpu: absent — using the VBE framebuffer"),
    }
}

/// Parse the embedded fonts and rasterise the faces the chrome draws on every frame (the rest of
/// the glyph cache fills on first use).
pub fn init_text_engine(tsc_khz: u64) {
    // UI text engine: parse the embedded fonts and rasterise the faces the chrome
    // draws on every frame (the rest of the glyph cache fills on first use).
    {
        let (us, st) = text::init(tsc_khz);
        klog!(
            Info,
            "ui text: {} glyphs, {} KiB atlas in {} us",
            st.glyphs,
            st.arena_bytes / 1024,
            us
        );
        if trace::ON {
            serial_println!(
                "[trace] ui: text atlas {} glyphs {} bytes {} strips built in {} us",
                st.glyphs,
                st.arena_bytes,
                st.strips,
                us
            );
        }
    }
}

/// Hand the browser's pending requests to the background fetcher and pick up its results.
/// Sets `redraw` when the page needs repainting on the next frame.
pub fn pump_browser(desk: &mut Desktop, redraw: &mut bool) {
    // Browser navigation: hand any pending request to the background fetcher
    // (non-blocking — the worker thread does the slow fetch while we keep
    // rendering the "Carregando" state), and pick up a finished result.
    if fetch::is_idle() {
        let mut url = [0u8; 512];
        if let Some(len) = desk.browser_take_request(&mut url) {
            let mut host = [0u8; 96];
            let hlen = desk.browser_insecure_host(&mut host);
            let body = desk.browser_post_body();
            fetch::try_post(&url[..len], &host[..hlen], body.as_deref());
        } else if let Some(len) = desk.browser_next_sheet(&mut url) {
            // The page's style sheets before its pictures: they decide what the page looks like.
            let mut host = [0u8; 96];
            let hlen = desk.browser_insecure_host_active(&mut host);
            if !fetch::try_post_sheet(&url[..len], &host[..hlen]) {
                desk.browser_sheet_done(None);
                *redraw = true;
            }
        } else if let Some(len) = desk.browser_next_script(&mut url) {
            // Then its scripts: they run once all are in.
            let mut host = [0u8; 96];
            let hlen = desk.browser_insecure_host_active(&mut host);
            if !fetch::try_post_script(&url[..len], &host[..hlen]) {
                desk.browser_script_done(None);
            }
        } else if let Some((len, fit_w)) = desk.browser_next_image(&mut url) {
            // Nothing else is waiting: fetch the next picture of the page.
            let mut host = [0u8; 96];
            let hlen = desk.browser_insecure_host_active(&mut host);
            if !fetch::try_post_image(&url[..len], &host[..hlen], fit_w) {
                desk.browser_image_done(Err(kitsune_core::web::imgcache::ImgFail::Failed));
                *redraw = true;
            }
        }
    } else if fetch::worker_dead() {
        // The fetcher thread died: fail the navigation now instead of leaving the
        // browser on "Carregando" forever.
        let mut url = [0u8; 512];
        if desk.browser_take_request(&mut url).is_some() {
            desk.browser_fail(kitsune_core::browser::FailReason::WorkerDied);
            *redraw = true;
        }
    }
    if desk.browser_poll_internal() {
        *redraw = true;
    }
    if let Some(css) = fetch::take_sheet_result() {
        desk.browser_sheet_done(css);
        *redraw = true;
    }
    if let Some(js) = fetch::take_script_result() {
        desk.browser_script_done(js);
    }
    if desk.browser_pump_js() {
        *redraw = true;
    }
    if let Some(res) = fetch::take_image_result() {
        desk.browser_image_done(res);
        *redraw = true;
    }
    if let Some(result) = fetch::take_result() {
        match result {
            Ok(page) => desk.browser_load(&page.data, page.conn, page.truncated, page.cert),
            Err(reason) => desk.browser_fail(reason),
        }
        *redraw = true;
    }
}

/// Memory figures after the first composed frame, then ask `logd` to persist the boot log.
pub fn report_first_frame() {
    trace::mark("first desktop frame composed + blitted");
    let ts = text::stats();
    klog::log_quiet(
        klog::Level::Info,
        format_args!(
            "ui memory: glyph atlas {} KiB ({} glyphs), icon cache {} KiB",
            ts.arena_bytes / 1024,
            ts.glyphs,
            icons::bytes() / 1024
        ),
    );
    if trace::ON {
        serial_println!(
            "[trace] ui: memory after the first frame: glyph atlas {} bytes ({} glyphs), icon cache {} bytes",
            ts.arena_bytes,
            ts.glyphs,
            icons::bytes()
        );
    }
    klog::log_quiet(
        klog::Level::Info,
        format_args!("first desktop frame composed + blitted"),
    );
    logd::request_boot_flush();
}

/// Shortest the splash plays, so the mark is seen and the fades finish (ms).
const SPLASH_MIN_MS: u64 = 1500;
/// How long the bar takes to fill the last fifth once the boot work is done (ms).
const SPLASH_FINISH_MS: u64 = 300;

/// Plays the boot splash until the desktop is ready. `step` is the boot work, run once between two
/// frames; it says whether more is left. The bar follows the clock but waits at 80 % while the
/// work is not done, then fills; the splash ends when the work is done and at least
/// [`SPLASH_MIN_MS`] have passed (it used to last five seconds whatever the machine did).
pub fn run_splash(
    framebuffer: &mut bootloader_api::info::FrameBuffer,
    back: &mut [u8],
    info: FrameBufferInfo,
    n: usize,
    cache: &mut [u8],
    step: &mut dyn FnMut() -> bool,
) {
    // The background is the same in every frame: draw it once into `cache` and copy it.
    let accent = crate::theme::accent();
    {
        let mut c = Canvas::new(&mut cache[..n], info);
        boot::draw_splash_background(&mut c);
    }
    let ms = || interrupts::ticks() * 1000 / u64::from(interrupts::TIMER_HZ);
    let mut more = true;
    let start = ms();
    let mut done_at = 0u64;
    let (mut frames, mut draw_cyc, mut blit_cyc) = (0u64, 0u64, 0u64);
    loop {
        let elapsed = ms() - start;
        let base = (elapsed as f32 / SPLASH_MIN_MS as f32).min(1.0);
        let p = if more {
            base.min(0.8)
        } else if done_at <= SPLASH_MIN_MS {
            base
        } else {
            // Late: the bar was held at 80 %; fill the rest now.
            (0.8 + 0.2 * (elapsed - done_at) as f32 / SPLASH_FINISH_MS as f32).min(1.0)
        };
        let t0 = trace::t();
        back[..n].copy_from_slice(&cache[..n]);
        {
            let mut c = Canvas::new(back, info);
            boot::draw_splash(&mut c, p, accent);
        }
        let t1 = trace::t();
        fb_full_blit(framebuffer.buffer_mut(), back, n);
        let t2 = trace::t();
        if trace::ON {
            frames += 1;
            draw_cyc += t1 - t0;
            blit_cyc += t2 - t1;
        }
        if !more && elapsed >= SPLASH_MIN_MS && p >= 1.0 {
            break;
        }
        if more {
            more = step();
            if !more {
                done_at = ms() - start;
            }
        } else {
            io::delay_cycles(20_000_000);
        }
    }
    if trace::ON {
        // Raw TSC cycles (the TSC rate is printed on the "TSC calibrated" line).
        serial_println!(
            "[trace] splash: {} frames, draw avg {} cyc, blit avg {} cyc, delay_cycles 20000000 per frame",
            frames,
            draw_cyc / frames.max(1),
            blit_cyc / frames.max(1)
        );
    }
}

/// Upload the whole back buffer to the framebuffer (timed as `Stage::Blit`).
fn fb_full_blit(fb: &mut [u8], back: &[u8], n: usize) {
    if trace::ON {
        trace::vram_upload(n as u64, trace::count_diff(&fb[..n], &back[..n]));
    }
    let t0 = trace::t();
    fb[..n].copy_from_slice(back);
    trace::stage(trace::Stage::Blit, t0);
}

/// Exercises the heap (alloc, grow, free) once at boot. A broken allocator
/// would fault or hang instead of silently corrupting later.
pub fn heap_smoke_test() {
    use alloc::vec::Vec;
    let mut v: Vec<u32> = Vec::new();
    for i in 0..1024 {
        v.push(i);
    }
    let sum: u32 = v.iter().sum();
    core::hint::black_box(sum);
    drop(v);

    // Fragmentation check: churn many small allocations, free them, then demand
    // a block larger than any single freed chunk. This only succeeds if the
    // allocator coalesced the freed regions back together.
    let mut chunks: Vec<Vec<u8>> = Vec::new();
    for _ in 0..256 {
        chunks.push(alloc::vec![0u8; 1024]);
    }
    drop(chunks); // frees ~256 KiB in scattered chunks
    let big: Vec<u8> = alloc::vec![7u8; 200 * 1024];
    core::hint::black_box(big.len());
}
