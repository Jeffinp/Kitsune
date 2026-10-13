//! WebM (Matroska) demuxer: the container YouTube serves its VP9 and Opus streams in.
//!
//! * **Incremental.** Bytes go in with [`Demuxer::push`] in pieces of any size (a DASH init segment,
//!   then clusters as they download); [`Demuxer::poll`] hands out what is complete: first
//!   [`Event::Header`] (the [`Info`] and [`Track`]s are then readable), then one [`Event::Packet`]
//!   per frame, in file order. `Ok(None)` means "push more bytes".
//! * **What is read.** The EBML header (`DocType` `webm` or `matroska`), `Segment` (known or unknown
//!   size), `Info` (time scale, duration), `Tracks` (number, type, codec, codec private data, video
//!   size, audio rate and channels, codec delay, seek pre-roll), `Cues` (optional, for seeking),
//!   `Cluster` (known or unknown size) with `Timecode`, `SimpleBlock` and `BlockGroup` (`Block`,
//!   `BlockDuration`, `ReferenceBlock`, `DiscardPadding`). Everything else (`SeekHead`, `Tags`,
//!   `Chapters`, `Void`, `CRC-32`, attachments...) is skipped without being buffered.
//! * **Not supported.** Laced blocks (several frames in one block: WebM video never does it and
//!   Opus in WebM does not either) are dropped and counted in [`Demuxer::skipped`]; tracks with
//!   content encodings (compression, encryption) are flagged ([`Track::encoded`]) for the caller
//!   to refuse.
//! * **Seeking by cluster.** [`Demuxer::cluster_for`] finds, in the cues, the cluster to start
//!   from for a time; after [`Demuxer::seek`] the next bytes pushed are read as starting at that
//!   offset of the file (a cluster), with the header already known.
//! * **Hostile input.** No panics and no unbounded memory: an element that must be held whole (a
//!   block, the track list) has a size limit ([`MAX_BLOCK`], [`MAX_HEADER`]), skipped elements are
//!   consumed as they stream past, at most [`MAX_TRACKS`] tracks and [`MAX_CUES`] cue points are
//!   kept, nesting is fixed (segment, cluster, block group) and children that overrun their
//!   parent are errors. Timestamps saturate. The fuzz target `webm_demux` drives it.
//!
//! Timestamps are in nanoseconds: `(cluster time + block offset) * TimecodeScale`.

use alloc::string::String;
use alloc::vec::Vec;
use core::fmt;

mod demux;
mod ebml;
mod header;
mod ids;

pub use demux::Demuxer;
pub use header::OpusHead;

/// The largest element held whole: a block (one compressed frame) or a block group.
pub const MAX_BLOCK: u64 = 16 * 1024 * 1024;
/// The largest `Info`, `Tracks` or EBML header.
pub const MAX_HEADER: u64 = 1024 * 1024;
/// The largest `Cues` element read (a larger one is skipped: seeking then has no index).
pub const MAX_CUES_BYTES: u64 = 4 * 1024 * 1024;
/// Most tracks kept from one `Tracks` element.
pub const MAX_TRACKS: usize = 16;
/// Most cue points kept.
pub const MAX_CUES: usize = 65_536;
/// Most bytes waiting in the demuxer (pushed and not yet consumed).
pub const MAX_PENDING: usize = 64 * 1024 * 1024;

/// Why the stream was rejected. After an error the demuxer stays failed until [`Demuxer::reset`]
/// or [`Demuxer::seek`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    /// Not EBML, or a `DocType` other than `webm`/`matroska`.
    NotWebm,
    /// A malformed structure (the text says which).
    Invalid(&'static str),
    /// An element or the pending data is larger than the limits above.
    TooLarge,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::NotWebm => f.write_str("not a WebM/Matroska file"),
            Error::Invalid(why) => write!(f, "invalid WebM: {why}"),
            Error::TooLarge => f.write_str("WebM element too large"),
        }
    }
}

/// What a track carries.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TrackKind {
    Video,
    Audio,
    /// Subtitles, metadata and the rest (the Matroska track type number).
    Other(u8),
}

/// The codecs this system knows by name (from `CodecID`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Codec {
    Vp8,
    Vp9,
    Av1,
    Opus,
    Vorbis,
    Other,
}

impl Codec {
    fn from_id(id: &[u8]) -> Codec {
        match id {
            b"V_VP8" => Codec::Vp8,
            b"V_VP9" => Codec::Vp9,
            b"V_AV1" => Codec::Av1,
            b"A_OPUS" => Codec::Opus,
            b"A_VORBIS" => Codec::Vorbis,
            _ => Codec::Other,
        }
    }
}

/// One entry of `Tracks`.
#[derive(Clone, Debug, PartialEq)]
pub struct Track {
    /// The number blocks refer to (`TrackNumber`, at least 1).
    pub number: u64,
    pub uid: u64,
    pub kind: TrackKind,
    pub codec: Codec,
    /// `CodecID` as written (`V_VP9`, `A_OPUS`...).
    pub codec_id: String,
    /// `CodecPrivate` (for Opus, the `OpusHead`; see [`OpusHead::parse`]).
    pub codec_private: Vec<u8>,
    /// `DefaultDuration` of a frame, nanoseconds.
    pub default_duration_ns: Option<u64>,
    /// `CodecDelay`: nanoseconds of decoded output to drop at the start (Opus pre-skip).
    pub codec_delay_ns: u64,
    /// `SeekPreRoll`: nanoseconds to decode before the target after a seek (Opus: 80 ms).
    pub seek_preroll_ns: u64,
    /// Video: `PixelWidth` x `PixelHeight` (0 when absent or not video).
    pub width: u32,
    pub height: u32,
    /// Audio: `SamplingFrequency` in Hz, rounded (8000 when absent, as Matroska says).
    pub sample_rate: u32,
    /// Audio: `Channels` (1 when absent).
    pub channels: u8,
    /// The track has `ContentEncodings` (compressed or encrypted blocks): its packets cannot be
    /// decoded as they are.
    pub encoded: bool,
}

/// `Info`: the segment-wide facts.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Info {
    /// Nanoseconds per timestamp unit (`TimecodeScale`, 1 000 000 by default: milliseconds).
    pub timescale_ns: u64,
    /// The duration, when the file says it.
    pub duration_ns: Option<u64>,
}

impl Default for Info {
    fn default() -> Self {
        Info {
            timescale_ns: 1_000_000,
            duration_ns: None,
        }
    }
}

/// One cue point: where the cluster holding a keyframe of `track` at `time_ns` starts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CuePoint {
    pub time_ns: u64,
    pub track: u64,
    /// Absolute byte offset of the cluster in the file.
    pub cluster_pos: u64,
}

/// One compressed frame.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Packet {
    pub track: u64,
    /// Presentation time in nanoseconds (may be negative: a block before its cluster's time).
    pub timestamp_ns: i64,
    /// A frame that decodes on its own (`SimpleBlock` key flag, or a `BlockGroup` without
    /// `ReferenceBlock`).
    pub keyframe: bool,
    /// `BlockDuration` (block groups only), nanoseconds.
    pub duration_ns: Option<u64>,
    /// `DiscardPadding`: nanoseconds of decoded audio to drop at the end of this frame (the end of
    /// an Opus stream). Zero when absent.
    pub discard_padding_ns: i64,
    /// The absolute file offset of the cluster this packet came from.
    pub cluster_pos: u64,
    pub data: Vec<u8>,
}

/// What [`Demuxer::poll`] hands out.
#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    /// The `Tracks` (and, when it came first, `Info`) of a segment were read; read them with
    /// [`Demuxer::tracks`] and [`Demuxer::info`]. Comes again if a new segment begins.
    Header,
    /// A frame.
    Packet(Packet),
}

#[cfg(test)]
mod tests;
