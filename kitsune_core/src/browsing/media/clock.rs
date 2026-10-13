//! The player's clock. With sound the **sound is the master**: the time is the presentation time of
//! the sample the device is playing now, read from the mixer's "played" counter of the player's
//! stream (see `hw::audio::Stream::played`), so the picture follows what is heard, pauses with it and
//! waits with it when the sound runs dry. Without sound (no device, or a video without a sound
//! track) the system clock runs instead, stopped and started by the player.

/// Output rate of the decoder (and of the device): 48 kHz.
pub const RATE: i64 = 48_000;

/// Nanoseconds of `frames` at [`RATE`].
pub fn frames_ns(frames: i64) -> i64 {
    // Split to keep the product in range for any realistic frame count.
    (frames / RATE) * 1_000_000_000 + (frames % RATE) * 1_000_000_000 / RATE
}

/// Frames at [`RATE`] in `ns` (rounded down).
pub fn ns_frames(ns: i64) -> i64 {
    (ns / 1_000_000_000) * RATE + (ns % 1_000_000_000) * RATE / 1_000_000_000
}

/// One stream of the sound device, as the player uses it. Frames are stereo s16 at 48 kHz.
pub trait AudioSink {
    /// Queue little-endian samples (whole frames); returns the frames taken (the rest did not fit).
    fn write(&mut self, bytes: &[u8]) -> usize;
    /// Frames of this stream the device has played (its clock).
    fn played(&self) -> u64;
    /// Frames the mixer took from the queue so far (played, or on their way to the speaker).
    fn consumed(&self) -> u64;
    /// Frames waiting in the queue.
    fn queued(&self) -> u64;
    fn set_paused(&mut self, paused: bool);
    /// Drop what is queued.
    fn flush(&mut self);
    /// Volume 0–100.
    fn set_volume(&mut self, volume: u8);
}

/// Where the clock is anchored.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Anchor {
    /// Frozen at `base`.
    Still,
    /// `base` when the stream's consumed counter was `frame`; runs with the stream's played counter.
    Audio { frame: u64 },
    /// `base` at system time `ms`.
    System { ms: u64 },
}

/// The media time of a player, in nanoseconds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Clock {
    base: i64,
    anchor: Anchor,
}

impl Default for Clock {
    fn default() -> Self {
        Clock::at(0)
    }
}

impl Clock {
    /// A clock stopped at `t`.
    pub fn at(t: i64) -> Clock {
        Clock {
            base: t,
            anchor: Anchor::Still,
        }
    }

    /// The time now: `played` is the stream's played counter (sound clock), `now_ms` the system
    /// time.
    pub fn now(&self, now_ms: u64, played: u64) -> i64 {
        match self.anchor {
            Anchor::Still => self.base,
            Anchor::Audio { frame } => {
                self.base + frames_ns(played.saturating_sub(frame).min(i64::MAX as u64) as i64)
            }
            Anchor::System { ms } => {
                self.base + (now_ms.saturating_sub(ms).min(1 << 40) as i64) * 1_000_000
            }
        }
    }

    /// The sound clock: the sample consumed as frame `frame` of the stream has time `t`.
    pub fn follow_audio(&mut self, t: i64, frame: u64) {
        self.base = t;
        self.anchor = Anchor::Audio { frame };
    }

    /// Run on the system clock from `t` at `now_ms`.
    pub fn start_system(&mut self, t: i64, now_ms: u64) {
        self.base = t;
        self.anchor = Anchor::System { ms: now_ms };
    }

    /// Stop at `t`.
    pub fn freeze(&mut self, t: i64) {
        self.base = t;
        self.anchor = Anchor::Still;
    }

    /// Does it follow the sound?
    pub fn on_audio(&self) -> bool {
        matches!(self.anchor, Anchor::Audio { .. })
    }

    /// Is it running on the system clock?
    pub fn on_system(&self) -> bool {
        matches!(self.anchor, Anchor::System { .. })
    }
}
