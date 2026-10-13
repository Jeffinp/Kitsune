//! Sound: the mixer, the device behind it and the `audiod` thread that keeps the device fed.
//!
//! The state (a `kitsune_core::hw::audio::Mixer` and the [`Ac97`] driver) lives behind one
//! [`SpinLock`] taken with interrupts off: by the system calls of ring 3 processes
//! (`userproc/audio.rs`), by the kernel's own streams ([`beep`]) and by `audiod`. `audiod` wakes
//! every tick (4 ms) while something plays: it reads the device position (the clock of every
//! stream), mixes blocks until ~128 ms are queued (one block per lock hold, so interrupts are never
//! off for long), wakes the processes waiting in `audio_wait`, and closes the streams of processes
//! that ended. With nothing open it sleeps until a stream is opened.
//!
//! Without a device (`audio: no device` in the log) every call answers `ENODEV` and the thread is
//! not started; the rest of the system does not notice.

use crate::ac97::Ac97;
use crate::allocator::SpinLock;
use crate::klog;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use kitsune_core::hw::audio::{AudioErr, Mixer, Tone};

/// The owner id of the kernel's own streams (process ids start at 5000).
pub const KERNEL: u32 = 0;

/// A process blocked in `audio_wait`.
pub struct Waiter {
    pub owner: u32,
    pub id: u32,
    pub slot: usize,
    /// Free bytes wanted, or 0 = until drained.
    pub want: u64,
    /// Tick at which it gives up.
    pub deadline: u64,
}

pub struct State {
    pub mixer: Mixer,
    dev: Ac97,
    pub waiters: Vec<Waiter>,
    /// Kernel-side tones still being generated (`beep`): stream id and the generator.
    tones: Vec<(u32, Tone)>,
}

static AUDIO: SpinLock<Option<State>> = SpinLock::new(None);
/// The `audiod` thread's slot (`usize::MAX` before it starts).
static AUDIOD: AtomicUsize = AtomicUsize::new(usize::MAX);
/// Set by [`kick`]; `audiod` does not go to sleep while it is set (a kick that lands between its
/// last look and its sleep is not lost).
static KICKED: AtomicBool = AtomicBool::new(false);

/// Probe the sound device and, if there is one, start `audiod`. Call once at boot, with interrupts
/// on and the scheduler running.
pub fn init(phys_offset: Option<u64>) {
    let dev = phys_offset.and_then(Ac97::probe);
    let Some(dev) = dev else {
        klog!(Info, "audio: no device");
        return;
    };
    *AUDIO.lock() = Some(State {
        mixer: Mixer::new(),
        dev,
        waiters: Vec::new(),
        tones: Vec::new(),
    });
    x86_64::instructions::interrupts::without_interrupts(|| {
        crate::sched::spawn("audiod", worker);
    });
}

/// Is there a sound device.
pub fn present() -> bool {
    AUDIO.lock().is_some()
}

/// Run `f` on the state; `None` without a device.
pub fn with<R>(f: impl FnOnce(&mut State) -> R) -> Option<R> {
    AUDIO.lock().as_mut().map(f)
}

/// Make `audiod` look now (a stream was opened or written).
pub fn kick() {
    KICKED.store(true, Ordering::Release);
    let slot = AUDIOD.load(Ordering::Relaxed);
    if slot != usize::MAX {
        crate::sched::wake(slot);
    }
}

/// Close every stream of a process that ended.
pub fn close_owner(pid: u32) {
    with(|s| {
        if s.mixer.close_owner(pid) > 0 {
            klog!(Info, "audio: closed the streams of pid {pid}");
        }
        s.waiters.retain(|w| w.owner != pid);
    });
}

/// The system volume (0–100) and mute, for the Settings and the panel.
pub fn master() -> Option<(u8, bool)> {
    with(|s| (s.mixer.master(), s.mixer.muted()))
}

pub fn set_master(volume: u8) {
    with(|s| {
        let _ = s.mixer.set_master(u32::from(volume));
    });
}

pub fn set_muted(m: bool) {
    with(|s| {
        s.mixer.set_muted(m);
        s.dev.set_codec_mute(m);
    });
}

/// Play a sine tone from the kernel (the volume feedback in Settings, `beep`): `freq` Hz for `ms`
/// milliseconds. Returns at once; `false` without a device or a free stream.
pub fn beep(freq: u32, ms: u32) -> bool {
    let ok = with(|s| {
        // One kernel tone at a time is plenty: a new one replaces the old.
        for (id, _) in s.tones.drain(..) {
            let _ = s.mixer.close(KERNEL, id, false);
        }
        match s.mixer.open(KERNEL, 48_000, 1) {
            Ok(id) => {
                s.tones.push((
                    id,
                    Tone::new(freq.clamp(20, 20_000), 48_000, ms.min(10_000), 12_000),
                ));
                true
            }
            Err(_) => false,
        }
    })
    .unwrap_or(false);
    if ok {
        kick();
    }
    ok
}

/// Feed the kernel tones into their streams; finished ones are closed with drain.
fn feed_tones(s: &mut State) {
    let mut buf = [0i16; 1024];
    let mut i = 0;
    while i < s.tones.len() {
        let (id, ref mut tone) = s.tones[i];
        let room = s
            .mixer
            .stream(KERNEL, id)
            .map_or(0, |st| st.free_bytes() / 2);
        let n = tone.fill(&mut buf[..room.min(1024)]);
        if n > 0
            && let Ok(st) = s.mixer.stream(KERNEL, id)
        {
            st.write_samples(&buf[..n]);
        }
        if tone.remaining() == 0 {
            let _ = s.mixer.close(KERNEL, id, true);
            s.tones.swap_remove(i);
        } else {
            i += 1;
        }
    }
}

/// Map a mixer refusal to an errno for the system calls.
pub fn errno(e: AudioErr) -> i64 {
    use kitsune_userabi::errno::*;
    match e {
        AudioErr::BadRate | AudioErr::BadChannels | AudioErr::BadValue => EINVAL,
        AudioErr::TooMany => EMFILE,
        AudioErr::NoStream => EBADF,
        AudioErr::Full => EAGAIN,
    }
}

/// The `audiod` thread.
extern "C" fn worker() -> ! {
    AUDIOD.store(crate::sched::current(), Ordering::Relaxed);
    let mut last_reap = 0u64;
    let mut late_seen = 0u64;
    loop {
        KICKED.store(false, Ordering::Release);
        // Where the device is (the streams' clocks), and the kernel tones.
        let running = with(|s| {
            let r = s.dev.observe(&mut s.mixer);
            feed_tones(s);
            r
        })
        .unwrap_or(false);
        // Top up the device, one block per lock hold.
        while with(|s| s.dev.fill_one(&mut s.mixer)).unwrap_or(false) {}
        // Wake the waiters whose condition holds or whose time is up.
        let now = crate::interrupts::ticks();
        let (open, audible, late) = with(|s| {
            let State { mixer, waiters, .. } = s;
            for w in waiters.iter() {
                if now >= w.deadline || mixer.ready(w.owner, w.id, w.want) {
                    crate::sched::wake(w.slot);
                }
            }
            (mixer.open_count(), mixer.has_audio(), s.dev.late)
        })
        .unwrap_or((0, false, 0));
        if late > late_seen {
            klog!(
                Warn,
                "audio: device starved {} time(s) (audiod was late)",
                late - late_seen
            );
            late_seen = late;
        }
        // Streams of processes that ended (a killed player stops at once, not at the next reap).
        if open > 0 && now.saturating_sub(last_reap) >= 25 {
            last_reap = now;
            let owners: Vec<u32> = with(|s| {
                let mut v: Vec<u32> = s.mixer.streams().map(|(o, _)| o).collect();
                v.sort_unstable();
                v.dedup();
                v
            })
            .unwrap_or_default();
            for pid in owners {
                if pid != KERNEL && !crate::userproc::is_running(pid) {
                    close_owner(pid);
                }
            }
        }
        // Every tick while sound flows; every 40 ms while streams are open but quiet (paused, or
        // waiting for data: a write kicks the thread anyway); otherwise until a stream opens.
        let now = crate::interrupts::ticks();
        let until = if running || audible {
            now + 1
        } else if open > 0 {
            now + 10
        } else {
            crate::sched::FOREVER
        };
        crate::sched::block(until, || !KICKED.load(Ordering::Acquire));
    }
}
