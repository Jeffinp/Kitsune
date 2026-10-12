//! Boot-time micro-benchmarks (`perf-trace`): heap, disk, password hashing, apps and drawing primitives.

use super::*;

/// Boot-time micro-benchmark of the kernel heap (`perf-trace` only): cost of a
/// small allocation on a clean heap, then of a large one when the free list is
/// fragmented into thousands of holes (the first-fit scan is O(holes), and so is
/// the address-sorted insertion on `dealloc`). Leaves the heap as it found it.
pub fn bench_alloc(khz: u64) {
    if !ON {
        return;
    }
    use alloc::alloc::{Layout, alloc, dealloc};
    use alloc::vec::Vec;
    let small = Layout::from_size_align(64, 8).unwrap();
    let big = Layout::from_size_align(4096, 8).unwrap();
    let avg = |total: u64, n: u64| total / n.max(1);

    // 1. clean heap: 1000 small allocs, then free them.
    let mut ptrs: Vec<*mut u8> = Vec::with_capacity(4000);
    let t0 = io::rdtsc();
    for _ in 0..1000 {
        // SAFETY: `small` has non-zero size and a power-of-two align, as `alloc` requires.
        // NOTE: the result is not null-checked; harmless at boot with an almost empty 64 MiB heap.
        ptrs.push(unsafe { alloc(small) });
    }
    let a_clean = avg(io::rdtsc() - t0, 1000);
    let t0 = io::rdtsc();
    for p in ptrs.drain(..) {
        // SAFETY: `p` came from `alloc(small)` above and is freed once (drained), with the same layout.
        unsafe { dealloc(p, small) };
    }
    let f_clean = avg(io::rdtsc() - t0, 1000);

    // 2. fragment: 4000 small blocks, free every other one => 2000 holes.
    for _ in 0..4000 {
        // SAFETY: `small` has non-zero size and a power-of-two align (see the first loop).
        ptrs.push(unsafe { alloc(small) });
    }
    let t0 = io::rdtsc();
    let mut kept: Vec<*mut u8> = Vec::with_capacity(2000);
    for (i, p) in ptrs.drain(..).enumerate() {
        if i % 2 == 0 {
            // SAFETY: `p` came from `alloc(small)` (drained from `ptrs`) and is freed once, same layout.
            unsafe { dealloc(p, small) };
        } else {
            kept.push(p);
        }
    }
    let f_frag = avg(io::rdtsc() - t0, 2000);
    // 3. big allocations must skip all 2000 holes; their frees insert at the tail.
    let mut bigs: Vec<*mut u8> = Vec::with_capacity(100);
    let t0 = io::rdtsc();
    for _ in 0..100 {
        // SAFETY: `big` has non-zero size and a power-of-two align.
        bigs.push(unsafe { alloc(big) });
    }
    let a_frag = avg(io::rdtsc() - t0, 100);
    let t0 = io::rdtsc();
    for p in bigs.drain(..) {
        // SAFETY: `p` came from `alloc(big)` (drained from `bigs`) and is freed once, same layout.
        unsafe { dealloc(p, big) };
    }
    let f_big = avg(io::rdtsc() - t0, 100);
    for p in kept.drain(..) {
        // SAFETY: `p` is a `small` block kept from the loop above; freed once, same layout.
        unsafe { dealloc(p, small) };
    }
    crate::serial_println!(
        "[trace] bench alloc: clean 64B alloc {} cyc / free {} cyc | 2000 holes: 4KiB alloc {} cyc / free {} cyc, 64B free(in list) {} cyc | khz {}",
        a_clean,
        f_clean,
        a_frag,
        f_big,
        f_frag,
        khz
    );
}

/// Boot-time micro-benchmark of the disk (`perf-trace` only): reads and write-backs of the same bytes
/// (so the contents never change) in 1-sector and 128-sector commands, and `FLUSH CACHE`.
pub fn bench_ata(khz: u64) {
    if !ON {
        return;
    }
    use kitsune_core::blockdev::BlockDevice;
    let Some(mut d) = crate::ata::AtaDisk::open() else {
        return;
    };
    if d.sector_count() < 1024 {
        return;
    }
    let mut one = [0u8; 512];
    let mut many = alloc::vec![0u8; 512 * 128];
    let avg = |total: u64, n: u64| total / n.max(1);
    let t0 = io::rdtsc();
    for i in 0..16 {
        let _ = d.read_sectors(200 + i, &mut one);
    }
    let r1 = avg(io::rdtsc() - t0, 16);
    let t0 = io::rdtsc();
    let _ = d.read_sectors(300, &mut many);
    let r128 = io::rdtsc() - t0;
    let t0 = io::rdtsc();
    for i in 0..16 {
        let _ = d.read_sectors(200 + i, &mut one);
        let _ = d.write_sectors(200 + i, &one);
    }
    let w1 = avg(io::rdtsc() - t0, 16).saturating_sub(r1);
    let t0 = io::rdtsc();
    let _ = d.write_sectors(300, &many);
    let w128 = io::rdtsc() - t0;
    let t0 = io::rdtsc();
    for _ in 0..4 {
        let _ = d.flush();
    }
    let fl = avg(io::rdtsc() - t0, 4);
    crate::serial_println!(
        "[trace] bench ata: read 1 sector {} cyc | read 128 sectors {} cyc ({}/sector) | write 1 sector {} cyc | write 128 sectors {} cyc ({}/sector) | flush {} cyc | khz {}",
        r1,
        r128,
        r128 / 128,
        w1,
        w128,
        w128 / 128,
        fl,
        khz
    );
}

/// Cost of the account password hash (`perf-trace` only): what logging in or creating an account pays.
pub fn bench_password(khz: u64) {
    if !ON {
        return;
    }
    let t0 = io::rdtsc();
    let k = kitsune_core::security::password::pbkdf2(b"correct horse", b"0123456789abcdef", 20_000);
    let cyc = io::rdtsc() - t0;
    crate::serial_println!(
        "[trace] bench password: pbkdf2 20000 iterations {} Mcyc ({} ms at {} kHz) key[0]={}",
        cyc / 1_000_000,
        cyc / khz.max(1),
        khz,
        k[0]
    );
}

/// Cost of reading the bundled app packages (`perf-trace` only): manifest and icon decode, and the
/// launcher-size resize, per package: what the app catalog pays at every boot.
pub fn bench_apps(khz: u64) {
    if !ON {
        return;
    }
    for pkg in crate::wasm::BUNDLED {
        let t0 = io::rdtsc();
        let parsed = kitsune_core::appmanifest::parse_package(pkg);
        let parse = io::rdtsc() - t0;
        let Ok(p) = parsed else { continue };
        let t0 = io::rdtsc();
        let ok = p
            .icon
            .as_ref()
            .and_then(kitsune_core::appinstall::launcher_icon)
            .is_some();
        let icon = io::rdtsc() - t0;
        crate::serial_println!(
            "[trace] bench app {}: package {} B, parse+icon decode {} kcyc, launcher resize {} kcyc (icon {}) | khz {}",
            p.manifest.id,
            pkg.len(),
            parse / 1000,
            icon / 1000,
            ok,
            khz
        );
    }
}

/// Boot-time micro-benchmark of the drawing primitives on the back buffer
/// (`perf-trace` only). Prints cycles and cycles/pixel of the best of 4 runs,
/// which separates "this primitive is slow" from "it is called too often".
pub fn bench_prims(back: &mut [u8], info: bootloader_api::info::FrameBufferInfo, khz: u64) {
    if !ON {
        return;
    }
    use crate::fb::{Canvas, Color};
    let best = |f: &mut dyn FnMut()| -> u64 {
        let mut b = u64::MAX;
        for _ in 0..4 {
            let t0 = io::rdtsc();
            f();
            b = b.min(io::rdtsc().wrapping_sub(t0));
        }
        b
    };
    let (w, h) = (512usize, 320usize);
    let px = (w * h) as u64;
    let mut c = Canvas::new(back, info);
    let col = Color::rgb(0x20, 0x40, 0x80);
    let s = "The quick brown fox jumps over the lazy dog 0123";

    let fill = best(&mut || c.fill_rect(100, 100, w, h, col));
    let rr = best(&mut || c.fill_round_rect(100, 100, w, h, 12, col));
    let al = best(&mut || c.fill_round_rect_alpha(100, 100, w, h, 12, col, 28));
    let al_small = best(&mut || c.fill_round_rect_alpha(100, 100, 60, 30, 8, col, 28));
    let txt = best(&mut || crate::text::legacy::draw_text(&mut c, 100, 100, s, col, 2));
    crate::serial_println!(
        "[trace] bench ({}x{} px, bpp {}): fill_rect {} cyc ({}/px) | round_rect {} ({}/px) | alpha {} ({}/px) | alpha 60x30 {} ({}/px) | text {} chars {} cyc ({}/char) | khz {}",
        w,
        h,
        info.bytes_per_pixel,
        fill,
        fill / px,
        rr,
        rr / px,
        al,
        al / px,
        al_small,
        al_small / (60 * 30),
        s.len(),
        txt,
        txt / s.len() as u64,
        khz
    );
    bench_ui(&mut c, best_fn(), khz);
}

pub(super) fn best_fn() -> impl Fn(&mut dyn FnMut()) -> u64 {
    |f: &mut dyn FnMut()| -> u64 {
        let mut b = u64::MAX;
        for _ in 0..4 {
            let t0 = io::rdtsc();
            f();
            b = b.min(io::rdtsc().wrapping_sub(t0));
        }
        b
    }
}

/// The UI toolkit's primitives (anti-aliased shapes, shadows, text, surfaces,
/// blur, scaling), best of 4, in cycles and cycles per pixel.
pub(super) fn bench_ui(
    c: &mut crate::fb::Canvas,
    best: impl Fn(&mut dyn FnMut()) -> u64,
    khz: u64,
) {
    use crate::fb::{Color, Corner, Shadow};
    use crate::text::{self, Weight};
    use kitsune_core::Rect;
    use kitsune_core::raster::{self, Paint, Surface};
    let col = Color::rgb(0x20, 0x40, 0x80);
    let col2 = Color::rgb(0xF0, 0xF4, 0xFF);
    let rect = Rect::new(100, 100, 512, 320);
    let px = (rect.w * rect.h) as u64;
    let rr_solid = best(&mut || c.fill_rrect(rect, 12, Corner::Circle, col, 256));
    let rr_alpha = best(&mut || c.fill_rrect(rect, 12, Corner::Circle, col, 180));
    let rr_grad = best(&mut || c.fill_rrect_vgrad(rect, 12, Corner::Circle, col2, col, 256));
    let stroke = best(&mut || c.stroke_rrect(rect, 12, Corner::Circle, col2, 120));
    let body = Rect::new(150, 120, 600, 400);
    let hole = Rect::new(150, 132, 600, 376);
    let sh_area = ((body.w + 40) * (body.h + 40) - hole.w * hole.h) as u64;
    let shadow = best(&mut || {
        c.draw_shadow(
            body,
            Shadow {
                blur: 20,
                dy: 14,
                alpha: 70,
            },
            hole,
        );
        c.draw_shadow(
            body,
            Shadow {
                blur: 6,
                dy: 4,
                alpha: 64,
            },
            hole,
        );
    });
    let s = "The quick brown fox jumps over the lazy dog 0123";
    let t13 = best(&mut || {
        text::draw(c, 100, 100, s, 13, Weight::Regular, col);
    });
    let masks = crate::fb::masks();
    let mut icon = Surface::new(128, 128);
    icon.fill_rrect(
        0,
        0,
        128,
        128,
        29,
        raster::Corner::Squircle,
        Paint::Vertical(raster::rgb(0x60A5FA), raster::rgb(0x2563EB)),
        masks,
    );
    let scaled = icon.resized(56, 56);
    let blit = best(&mut || c.blit_surface(&scaled, 300, 300, 256));
    let resize = best(&mut || {
        core::hint::black_box(icon.resized(56, 56));
    });
    let mut bd = Surface::new(360, 80);
    bd.fill(raster::rgb(0x305080));
    let blur = best(&mut || bd.blur(8, 3));
    let mut buf = alloc::vec::Vec::new();
    let region = Rect::new(100, 100, 360, 80);
    let read = best(&mut || c.read_region(region, &mut buf));
    let write = best(&mut || c.write_region(region, &buf));
    let lut = raster::glow_lut();
    let glow = best(&mut || c.glow(400, 300, 300, col2, 40, &lut));
    let masks_t0 = io::rdtsc();
    core::hint::black_box(raster::CornerMasks::with_max_radius(48));
    let masks_build = io::rdtsc().wrapping_sub(masks_t0);
    crate::serial_println!(
        "[trace] bench ui ({}x{}): rrect solid {} ({}/px) | rrect a180 {} ({}/px) | rrect vgrad {} ({}/px) | stroke {} | shadow x2 {} ({}/px of ring) | text 48ch@13 {} ({}/ch) | blit 56x56 {} ({}/px) | resize 128->56 {} | blur 360x80 r8x3 {} ({}/px) | read 360x80 {} | write {} | glow r300 {} | masks(48) build {} | khz {}",
        rect.w,
        rect.h,
        rr_solid,
        rr_solid / px,
        rr_alpha,
        rr_alpha / px,
        rr_grad,
        rr_grad / px,
        stroke,
        shadow,
        shadow / sh_area.max(1),
        t13,
        t13 / s.len() as u64,
        blit,
        blit / (56 * 56),
        resize,
        blur,
        blur / (360 * 80),
        read,
        write,
        glow,
        masks_build,
        khz
    );
}
