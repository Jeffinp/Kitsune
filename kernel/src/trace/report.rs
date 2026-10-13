//! The per-second report: reads and resets the counters and prints the `[trace]` lines.

use super::*;

/// Reset-and-read helper for the atomics.
pub(super) fn take(a: &AtomicU64) -> u64 {
    a.swap(0, Relaxed)
}

pub(super) fn us(cycles: u64, khz: u64) -> u64 {
    cycles * 1000 / khz.max(1)
}

/// Print and reset the per-second statistics (call once per wall-clock second
/// from the compositor, outside any timed region).
pub fn report(khz: u64, ticks: u64) {
    if !ON {
        return;
    }
    let st = stats();
    crate::serial_println!(
        "[trace] t={}ticks loops={} hltwakes={}",
        ticks,
        st.loops,
        st.hlt_wakes
    );
    crate::netd::log_stats();
    crate::wasm::remote::log_stats();
    for p in 0..PATHS {
        if st.path_n[p] > 0 {
            crate::serial_println!(
                "[trace]   path {:<6} n={:<4} avg={}us max={}us cpu={}us",
                PATH_NAMES[p],
                st.path_n[p],
                us(st.path_sum[p] / st.path_n[p], khz),
                us(st.path_max[p], khz),
                us(st.path_cpu[p] / st.path_n[p], khz)
            );
        }
    }
    if st.stage_sum.iter().any(|&v| v > 0) {
        crate::serial_println!(
            "[trace]   stage sum/s: {}={}us {}={}us {}={}us {}={}us  (max {}us {}us {}us {}us)",
            STAGE_NAMES[0],
            us(st.stage_sum[0], khz),
            STAGE_NAMES[1],
            us(st.stage_sum[1], khz),
            STAGE_NAMES[2],
            us(st.stage_sum[2], khz),
            STAGE_NAMES[3],
            us(st.stage_sum[3], khz),
            us(st.stage_max[0], khz),
            us(st.stage_max[1], khz),
            us(st.stage_max[2], khz),
            us(st.stage_max[3], khz),
        );
    }
    if st.lat_n > 0 {
        crate::serial_println!(
            "[trace]   input n={} irq->pickup avg={}us max={}us  irq->frame avg={}us max={}us",
            st.lat_n,
            us(st.lat_sum / st.lat_n, khz),
            us(st.lat_max, khz),
            us(st.e2e_sum / st.lat_n, khz),
            us(st.e2e_max, khz)
        );
    }
    if PRIM_N.iter().any(|n| n.load(Relaxed) > 0) {
        crate::serial_println!(
            "[trace]   prim {}={}us/{} {}={}us/{} {}={}us/{} {}={}us/{} {}={}us/{} {}={}us/{}",
            PRIM_NAMES[0],
            us(take(&PRIM_CYC[0]), khz),
            take(&PRIM_N[0]),
            PRIM_NAMES[1],
            us(take(&PRIM_CYC[1]), khz),
            take(&PRIM_N[1]),
            PRIM_NAMES[2],
            us(take(&PRIM_CYC[2]), khz),
            take(&PRIM_N[2]),
            PRIM_NAMES[3],
            us(take(&PRIM_CYC[3]), khz),
            take(&PRIM_N[3]),
            PRIM_NAMES[4],
            us(take(&PRIM_CYC[4]), khz),
            take(&PRIM_N[4]),
            PRIM_NAMES[5],
            us(take(&PRIM_CYC[5]), khz),
            take(&PRIM_N[5]),
        );
    }
    let (vu, vc) = (take(&VRAM_UP), take(&VRAM_CHG));
    if vu > 0 {
        crate::serial_println!("[trace]   vram upload={}B changed={}B", vu, vc);
    }
    // Exact heap occupancy (the HUD only shows whole percent of 64 MiB): lets a
    // window open/close soak prove nothing leaks.
    let used = crate::HEAP_SIZE - crate::ALLOCATOR.free_bytes().min(crate::HEAP_SIZE);
    crate::serial_println!("[trace]   heap used={}B", used);
    let an = take(&ALLOC_N);
    let fnn = take(&FREE_N);
    if an + fnn > 0 {
        crate::serial_println!(
            "[trace]   alloc n={} bytes={} avg={}cyc max={}cyc scanned/alloc={} | free n={} avg={}cyc",
            an,
            take(&ALLOC_BYTES),
            take(&ALLOC_CYC) / an.max(1),
            take(&ALLOC_MAX),
            take(&ALLOC_SCANNED) / an.max(1),
            fnn,
            take(&FREE_CYC) / fnn.max(1)
        );
    }
    let isr_n = take(&ISR_N);
    super::pcs::report_pcs();
    let idle = take(&TICK_IDLE);
    let busy = take(&TICK_BUSY);
    crate::serial_println!(
        "[trace]   timer isr n={} avg={}cyc max={}cyc | cpu samples idle={} busy={} (comp={} fetch={} wasm={})",
        isr_n,
        take(&ISR_CYC) / isr_n.max(1),
        take(&ISR_MAX),
        idle,
        busy,
        take(&TICK_BUSY_THR[0]),
        take(&TICK_BUSY_THR[1]),
        take(&TICK_BUSY_THR[2])
    );
    let (wn, wc, rn, rc) = (
        take(&ATA_W_N),
        take(&ATA_W_CYC),
        take(&ATA_R_N),
        take(&ATA_R_CYC),
    );
    if wn + rn > 0 {
        crate::serial_println!(
            "[trace]   ata write n={} avg={}us | read n={} avg={}us",
            wn,
            us(wc / wn.max(1), khz),
            rn,
            us(rc / rn.max(1), khz)
        );
    }
    *st = Stats {
        path_n: [0; PATHS],
        path_sum: [0; PATHS],
        path_cpu: [0; PATHS],
        path_max: [0; PATHS],
        stage_sum: [0; STAGES],
        stage_max: [0; STAGES],
        loops: 0,
        hlt_wakes: 0,
        lat_n: 0,
        lat_sum: 0,
        lat_max: 0,
        e2e_sum: 0,
        e2e_max: 0,
    };
}
