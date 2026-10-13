//! Sound for ring 3 programs: calls 60–65 and the record `audio_status` writes.
//!
//! A program opens a **stream** (`audio_open`: rate, channels, sample format) and writes signed
//! 16-bit little-endian samples to it; the kernel mixes every open stream (its own included),
//! converts each to the device rate and plays the sum. Writes never block: `audio_write` takes the
//! whole frames that fit in the stream's queue (at most [`QUEUE_BYTES`]) and returns how many bytes
//! that was, `EAGAIN` when not even one frame fits. To wait for room without spinning there is
//! `audio_wait`, which blocks until the queue has the room asked for, until everything written has
//! been played (`want = 0`), or until the timeout. A player that runs an event loop (the browser's
//! script process) writes what fits and comes back later; a simple one (`play`) alternates
//! `audio_write` and `audio_wait`.
//!
//! **The clock.** [`AudioStatus::played`] counts the stream's frames (at its own rate) that the
//! device has actually played, not merely taken: it is what a video player shows the picture
//! against. It keeps counting through mute and stops while the stream is paused or starved.
//!
//! Limits: [`MAX_STREAMS_PER_PROCESS`] streams per process, [`QUEUE_BYTES`] queued per stream,
//! [`MAX_AUDIO_WRITE`] per write. Without a sound device `audio_open` returns `ENODEV`. The
//! streams of a process close when it ends.

/// `audio_open(rate, channels, format)`: a new stream; returns its id (`>= 0`). `EINVAL` for a rate
/// outside [`MIN_RATE`]..=[`MAX_RATE`], channels other than 1 or 2, or a format other than
/// [`AUDIO_S16LE`]; `EMFILE` past the stream limit; `ENODEV` without a sound device.
pub const SYS_AUDIO_OPEN: u64 = 60;
/// `audio_write(id, ptr, len)`: queue samples; returns the bytes taken (whole frames), `EAGAIN` when
/// the queue is full.
pub const SYS_AUDIO_WRITE: u64 = 61;
/// `audio_status(id, out_ptr)`: writes an [`AudioStatus`] ([`AUDIO_STATUS_LEN`] bytes); returns 0.
pub const SYS_AUDIO_STATUS: u64 = 62;
/// `audio_close(id, drain)`: `drain = 0` stops at once; `1` plays what is queued first (the id is
/// gone either way).
pub const SYS_AUDIO_CLOSE: u64 = 63;
/// `audio_ctl(id, op, value)`: volume, pause and the system volume, see the `CTL_*` operations.
pub const SYS_AUDIO_CTL: u64 = 64;
/// `audio_wait(id, want, timeout_ms)`: block until the queue has `want` free bytes (`want = 0`:
/// until everything written has been played). Returns 1 when that holds, 0 at the timeout (`0`
/// only looks, `u64::MAX` waits without limit).
pub const SYS_AUDIO_WAIT: u64 = 65;

/// Signed 16-bit little-endian samples, interleaved when stereo. The only format the kernel mixes
/// (it has no floating point); [`AUDIO_F32LE`] is reserved and refused.
pub const AUDIO_S16LE: u64 = 1;
pub const AUDIO_F32LE: u64 = 2;

/// Rates a stream may have.
pub const MIN_RATE: u32 = 8_000;
pub const MAX_RATE: u32 = 96_000;
/// Streams one process may have open.
pub const MAX_STREAMS_PER_PROCESS: usize = 4;
/// Most bytes queued per stream.
pub const QUEUE_BYTES: usize = 256 * 1024;
/// Most bytes one `audio_write` looks at.
pub const MAX_AUDIO_WRITE: usize = 64 * 1024;
/// Volumes are percentages.
pub const VOLUME_MAX: u64 = 100;

/// `audio_ctl` operations. The stream ones take the stream's id; the system ones ignore it.
pub const CTL_SET_VOLUME: u64 = 1;
pub const CTL_GET_VOLUME: u64 = 2;
/// `value` 1 pauses, 0 resumes.
pub const CTL_PAUSE: u64 = 3;
/// Drop what is queued (seek, stop); the clock keeps what was played.
pub const CTL_FLUSH: u64 = 4;
/// The system (master) volume and mute: what the volume control in Settings shows.
pub const CTL_SET_MASTER: u64 = 5;
pub const CTL_GET_MASTER: u64 = 6;
pub const CTL_SET_MUTE: u64 = 7;
pub const CTL_GET_MUTE: u64 = 8;
/// The rate the device plays at (48000), or `ENODEV`.
pub const CTL_DEVICE_RATE: u64 = 9;

/// [`AudioStatus::flags`] bits.
pub const STATUS_PAUSED: u32 = 1;
pub const STATUS_DRAINING: u32 = 2;

/// What [`SYS_AUDIO_STATUS`] writes: 48 bytes, little endian.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct AudioStatus {
    /// Bytes waiting in the queue, and bytes that still fit.
    pub queued_bytes: u32,
    pub free_bytes: u32,
    /// Frames (at the stream's rate) the device has played: the player's clock.
    pub played: u64,
    /// Frames the mixer has taken from the queue (played or in the device's buffers).
    pub consumed: u64,
    /// Frames ever written.
    pub written: u64,
    /// Times the stream ran dry while playing.
    pub underruns: u32,
    pub rate: u32,
    pub channels: u32,
    /// Stream volume (0–100) in the low byte; [`STATUS_PAUSED`], [`STATUS_DRAINING`] from bit 8.
    pub flags: u32,
}

/// Size of an [`AudioStatus`] on the wire.
pub const AUDIO_STATUS_LEN: usize = 48;

impl AudioStatus {
    pub fn volume(&self) -> u8 {
        self.flags as u8
    }

    pub fn paused(&self) -> bool {
        (self.flags >> 8) & STATUS_PAUSED != 0
    }

    pub fn draining(&self) -> bool {
        (self.flags >> 8) & STATUS_DRAINING != 0
    }

    /// Milliseconds played (the clock in the unit a player wants).
    pub fn played_ms(&self) -> u64 {
        self.played * 1000 / u64::from(self.rate.max(1))
    }

    pub fn to_bytes(&self) -> [u8; AUDIO_STATUS_LEN] {
        let mut b = [0u8; AUDIO_STATUS_LEN];
        b[0..4].copy_from_slice(&self.queued_bytes.to_le_bytes());
        b[4..8].copy_from_slice(&self.free_bytes.to_le_bytes());
        b[8..16].copy_from_slice(&self.played.to_le_bytes());
        b[16..24].copy_from_slice(&self.consumed.to_le_bytes());
        b[24..32].copy_from_slice(&self.written.to_le_bytes());
        b[32..36].copy_from_slice(&self.underruns.to_le_bytes());
        b[36..40].copy_from_slice(&self.rate.to_le_bytes());
        b[40..44].copy_from_slice(&self.channels.to_le_bytes());
        b[44..48].copy_from_slice(&self.flags.to_le_bytes());
        b
    }

    pub fn from_bytes(b: &[u8; AUDIO_STATUS_LEN]) -> AudioStatus {
        let w = |i: usize| u32::from_le_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]]);
        let d = |i: usize| u64::from(w(i)) | u64::from(w(i + 4)) << 32;
        AudioStatus {
            queued_bytes: w(0),
            free_bytes: w(4),
            played: d(8),
            consumed: d(16),
            written: d(24),
            underruns: w(32),
            rate: w(36),
            channels: w(40),
            flags: w(44),
        }
    }
}

/// The `flags` word from a volume and the two state bits.
pub fn status_flags(volume: u8, paused: bool, draining: bool) -> u32 {
    let mut f = u32::from(volume);
    if paused {
        f |= STATUS_PAUSED << 8;
    }
    if draining {
        f |= STATUS_DRAINING << 8;
    }
    f
}

#[cfg(test)]
mod tests;
