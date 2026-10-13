//! How many pixels a second the video decoder keeps up with on this machine, for
//! `navigator.mediaCapabilities.decodingInfo(...).smooth` (a player such as YouTube's takes the
//! highest quality whose answer is `smooth`).
//!
//! The decoder is software (libvpx, one thread) and the machine may be anything from real hardware
//! to an emulator 20-50 times slower, so the answer is measured with the decoder itself, once: when
//! the first page with scripts starts, a decoder process (`mediadec --serve`) decodes the small
//! clip built into it twice ([`proto::IN_BENCH`]) and says how many pixels a pass was and how long
//! the second one took (the first warms up; under an emulator it also translates the code); scaled
//! by the share of the CPU the process got meanwhile (its scheduler slot's busy ticks: the page may
//! be compiling its scripts at the same time) that gives pixels per CPU second. A playing video may
//! use [`SHARE_PERCENT`] of that (the decoder shares the CPU with the compositor, the page's scripts
//! and the sound), and a real video decodes slower than the clip ([`REAL_PERCENT`]). The pages hear
//! the budget as soon as it is known (`wire::budget_message`).

use super::*;
use crate::allocator::SpinLock;

/// The part of the CPU a playing video's decoding may take (the page's scripts are demoted while
/// it plays, `priority.rs`; the compositor and the sound take the rest).
const SHARE_PERCENT: u64 = 50;
/// How fast a real video decodes next to the built-in clip, per pixel. The clip is small and simple
/// (320x180, low bit rate): natively a real 240p decodes about as fast per pixel and a 360p about 30 %
/// slower, but under QEMU TCG, where every memory access of a bigger picture costs more, YouTube's
/// 360p decoded at ~44 % of the clip's rate (W61: the decoder at 53 % of the CPU only just kept 25
/// fps of 480x360 while the clip measured 18.6 Mpixels/s). The worse figure is used: on a fast
/// machine the budget is far above the 480p ceiling anyway.
const REAL_PERCENT: u64 = 40;
/// A measurement that takes longer than this is given up (the budget stays unknown).
const GIVE_UP_MS: u64 = 20_000;

enum Bench {
    Idle,
    Running {
        pid: u32,
        slot: usize,
        busy0: u64,
        since_ms: u64,
    },
    /// The budget in pixels a second (0: could not be measured).
    Done(u64),
}

static BENCH: SpinLock<Bench> = SpinLock::new(Bench::Idle);

/// The decoding budget for a playing video, in pixels a second, once measured.
fn decode_budget() -> Option<u64> {
    match *BENCH.lock() {
        Bench::Done(b) if b > 0 => Some(b),
        _ => None,
    }
}

/// A page's new script process `pid`: it hears the budget if it is known, else the measurement
/// starts (and the page hears it from [`pump_speed`]).
pub(crate) fn tell_budget(pid: u32) {
    match decode_budget() {
        Some(b) => {
            crate::userproc::page_post(pid, wire::budget_message(b));
        }
        None => start_speed_test(),
    }
}

/// Step the measurement; when it ends, the script processes `pids` (every page's) hear the budget.
pub(crate) fn pump_speed(pids: impl Iterator<Item = u32>) {
    if let Some(budget) = pump_speed_test() {
        for pid in pids {
            crate::userproc::page_post(pid, wire::budget_message(budget));
        }
    }
}

/// Start the measurement if it never ran.
fn start_speed_test() {
    let mut b = BENCH.lock();
    if !matches!(*b, Bench::Idle) {
        return;
    }
    *b = Bench::Done(0);
    drop(b);
    let Ok(pid) = crate::userproc::spawn_media(crate::desktop::services::accounts::cred()) else {
        crate::klog!(Warn, "media: no decoder for the speed test");
        return;
    };
    let Some(slot) = crate::userproc::slot_of(pid) else {
        crate::userproc::kill(pid);
        return;
    };
    let busy0 = crate::sched::busy_ticks().get(slot).copied().unwrap_or(0);
    crate::userproc::page_post(pid, proto::msg(proto::IN_BENCH, &[]));
    *BENCH.lock() = Bench::Running {
        pid,
        slot,
        busy0,
        since_ms: now_ms(),
    };
}

/// Take the answer of a running measurement; returns the budget the moment it is known.
fn pump_speed_test() -> Option<u64> {
    let Bench::Running {
        pid,
        slot,
        busy0,
        since_ms,
    } = *BENCH.lock()
    else {
        return None;
    };
    let mut result = None;
    while let Some(m) = crate::userproc::page_take(pid) {
        if let Some(Out::Bench {
            pictures,
            pixels,
            ms,
            total_ms,
        }) = parse_out(&m)
        {
            let busy = crate::sched::busy_ticks().get(slot).copied().unwrap_or(0);
            let cpu_ms = busy.saturating_sub(busy0) * 1000 / u64::from(crate::interrupts::TIMER_HZ);
            // The timed pass's wall time, times the share of the CPU the process got over the
            // whole run (the page may be compiling its scripts meanwhile); a run too short for the
            // ticks to see takes the wall time.
            let (ms, total) = (u64::from(ms), u64::from(total_ms));
            let took = if cpu_ms > 0 && total > 0 {
                ms * cpu_ms.min(total) / total
            } else {
                ms
            };
            let rate = pixels.saturating_mul(1000) / took.max(1);
            let budget = rate / 100 * SHARE_PERCENT / 100 * REAL_PERCENT;
            crate::klog!(
                Info,
                "media: decoder speed test: {pictures} pictures, {} kpixels in {ms} ms (CPU {cpu_ms} ms): \
                 {} kpixels/s, budget for a playing video {} kpixels/s",
                pixels / 1000,
                rate / 1000,
                budget / 1000
            );
            result = Some(budget);
            break;
        }
    }
    let gone = crate::userproc::end(pid) != crate::userproc::End::Running;
    if result.is_none() && (gone || now_ms().saturating_sub(since_ms) > GIVE_UP_MS) {
        crate::klog!(Warn, "media: the decoder speed test gave no answer");
        result = Some(0);
    }
    let budget = result?;
    crate::userproc::kill(pid);
    *BENCH.lock() = Bench::Done(budget);
    (budget > 0).then_some(budget)
}
