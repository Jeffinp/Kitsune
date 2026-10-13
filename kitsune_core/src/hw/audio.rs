//! Device-independent sound: the kernel mixer and its parts.
//!
//! The kernel driver (`kernel/src/ac97.rs`) asks the [`Mixer`] for blocks of stereo signed 16-bit
//! frames at [`DEVICE_RATE`] and copies them into the DMA buffers; everything that decides what those
//! frames are lives here, so it is tested on the host:
//!
//! * [`ring`]: the queue of one stream (the caller's samples, mono or stereo, as written);
//! * [`stream`]: one stream: queue, linear-interpolation resampler from its rate to the device's,
//!   volume, pause, and the map from device time to the stream's own frames (what `audio_status`
//!   reports as *played*, the clock a video player follows);
//! * [`mixer`]: the streams together: open/close with limits per owner, saturating sum, master volume
//!   and mute;
//! * [`tone`]: a sine generator (integer only: the kernel has no floating point) for `beep`/`tone`.
//!
//! Integer arithmetic throughout: the kernel target is soft-float, and the mixer runs every few
//! milliseconds.

pub mod mixer;
pub mod ring;
pub mod stream;
pub mod tone;

pub use mixer::Mixer;
pub use ring::SampleRing;
pub use stream::{Stream, StreamStatus};
pub use tone::Tone;

/// The rate the mixer produces (the AC'97 default and what every codec takes).
pub const DEVICE_RATE: u32 = 48_000;
/// Rates a stream may have (any integer in between works; 8, 11.025, 16, 22.05, 32, 44.1 and 48 kHz
/// are the usual ones).
pub const MIN_RATE: u32 = 8_000;
pub const MAX_RATE: u32 = 96_000;
/// Streams open at once, in total and per owner (a process, or the kernel).
pub const MAX_STREAMS: usize = 16;
pub const MAX_STREAMS_PER_OWNER: usize = 4;
/// Most bytes a stream may have waiting (in the caller's format): about 1.4 s of 44.1 kHz stereo.
pub const QUEUE_BYTES: usize = 256 * 1024;
/// Volumes are percentages.
pub const VOLUME_MAX: u8 = 100;

/// One output frame: left and right.
pub type Frame = [i16; 2];

/// Why a mixer operation was refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AudioErr {
    /// A rate outside [`MIN_RATE`]..=[`MAX_RATE`].
    BadRate,
    /// Not one or two channels.
    BadChannels,
    /// [`MAX_STREAMS`] or [`MAX_STREAMS_PER_OWNER`] reached.
    TooMany,
    /// No such stream, or it belongs to someone else.
    NoStream,
    /// The stream's queue has no room for one more frame.
    Full,
    /// A volume above [`VOLUME_MAX`].
    BadValue,
}

/// A percentage as a Q16 gain (100 % = 65536).
pub fn gain_q16(volume: u8) -> i64 {
    i64::from(volume.min(VOLUME_MAX)) * 65536 / i64::from(VOLUME_MAX)
}

#[cfg(test)]
mod tests;
