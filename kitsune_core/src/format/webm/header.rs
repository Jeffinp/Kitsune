//! The elements read whole from memory: the EBML header, `Info`, `Tracks`, `Cues`, and the
//! `OpusHead` of an Opus track.

use super::ebml::{self, Children};
use super::ids::*;
use super::*;

/// Check the EBML header's `DocType`.
pub(super) fn ebml_header(data: &[u8]) -> Result<(), Error> {
    for child in Children::new(data) {
        let (id, body) = child?;
        if id == DOC_TYPE {
            return match ebml::string(body) {
                b"webm" | b"matroska" => Ok(()),
                _ => Err(Error::NotWebm),
            };
        }
    }
    // No DocType: the default is "matroska".
    Ok(())
}

pub(super) fn info(data: &[u8]) -> Result<Info, Error> {
    let mut info = Info::default();
    let mut duration = None;
    for child in Children::new(data) {
        let (id, body) = child?;
        match id {
            TIMECODE_SCALE => {
                let v = ebml::uint(body)?;
                if v == 0 {
                    return Err(Error::Invalid("TimecodeScale of zero"));
                }
                info.timescale_ns = v;
            }
            DURATION => duration = Some(ebml::float(body)?),
            _ => {}
        }
    }
    // Duration is in timescale units, as a float.
    info.duration_ns = duration.and_then(|d| {
        let ns = d * info.timescale_ns as f64;
        (ns.is_finite() && ns >= 0.0).then_some(ns as u64)
    });
    Ok(info)
}

pub(super) fn tracks(data: &[u8]) -> Result<Vec<Track>, Error> {
    let mut out: Vec<Track> = Vec::new();
    for child in Children::new(data) {
        let (id, body) = child?;
        if id != TRACK_ENTRY {
            continue;
        }
        if out.len() >= MAX_TRACKS {
            return Err(Error::TooLarge);
        }
        let t = track(body)?;
        if t.number == 0 || out.iter().any(|o| o.number == t.number) {
            return Err(Error::Invalid("track number zero or repeated"));
        }
        out.push(t);
    }
    Ok(out)
}

fn track(data: &[u8]) -> Result<Track, Error> {
    let mut t = Track {
        number: 0,
        uid: 0,
        kind: TrackKind::Other(0),
        codec: Codec::Other,
        codec_id: String::new(),
        codec_private: Vec::new(),
        default_duration_ns: None,
        codec_delay_ns: 0,
        seek_preroll_ns: 0,
        width: 0,
        height: 0,
        sample_rate: 8000,
        channels: 1,
        encoded: false,
    };
    for child in Children::new(data) {
        let (id, body) = child?;
        match id {
            TRACK_NUMBER => t.number = ebml::uint(body)?,
            TRACK_UID => t.uid = ebml::uint(body)?,
            TRACK_TYPE => {
                t.kind = match ebml::uint(body)? {
                    1 => TrackKind::Video,
                    2 => TrackKind::Audio,
                    n => TrackKind::Other(n.min(255) as u8),
                }
            }
            CODEC_ID => {
                let s = ebml::string(body);
                t.codec = Codec::from_id(s);
                t.codec_id = String::from_utf8_lossy(&s[..s.len().min(64)]).into_owned();
            }
            CODEC_PRIVATE => t.codec_private = body.to_vec(),
            DEFAULT_DURATION => t.default_duration_ns = Some(ebml::uint(body)?),
            CODEC_DELAY => t.codec_delay_ns = ebml::uint(body)?,
            SEEK_PREROLL => t.seek_preroll_ns = ebml::uint(body)?,
            CONTENT_ENCODINGS => t.encoded = true,
            VIDEO => {
                for v in Children::new(body) {
                    let (id, body) = v?;
                    let n = || -> Result<u32, Error> {
                        Ok(ebml::uint(body)?.min(u64::from(u32::MAX)) as u32)
                    };
                    match id {
                        PIXEL_WIDTH => t.width = n()?,
                        PIXEL_HEIGHT => t.height = n()?,
                        _ => {}
                    }
                }
            }
            AUDIO => {
                for a in Children::new(body) {
                    let (id, body) = a?;
                    match id {
                        SAMPLING_FREQUENCY => {
                            let f = ebml::float(body)?;
                            t.sample_rate = if (1.0..=1e7).contains(&f) {
                                (f + 0.5) as u32
                            } else {
                                return Err(Error::Invalid("bad sampling frequency"));
                            };
                        }
                        CHANNELS => t.channels = ebml::uint(body)?.clamp(1, 255) as u8,
                        _ => {}
                    }
                }
            }
            _ => {}
        }
    }
    Ok(t)
}

/// Read the cue points of `Cues`; `segment_start` turns their relative positions into file
/// offsets. Points past [`MAX_CUES`] are dropped.
pub(super) fn cues(
    data: &[u8],
    segment_start: u64,
    timescale_ns: u64,
    out: &mut Vec<CuePoint>,
) -> Result<(), Error> {
    out.clear();
    for child in Children::new(data) {
        let (id, body) = child?;
        if id != CUE_POINT {
            continue;
        }
        let mut time = None;
        for c in Children::new(body) {
            let (id, body) = c?;
            match id {
                CUE_TIME => time = Some(ebml::uint(body)?),
                CUE_TRACK_POSITIONS => {
                    let (mut track, mut pos) = (None, None);
                    for p in Children::new(body) {
                        let (id, body) = p?;
                        match id {
                            CUE_TRACK => track = Some(ebml::uint(body)?),
                            CUE_CLUSTER_POSITION => pos = Some(ebml::uint(body)?),
                            _ => {}
                        }
                    }
                    if let (Some(time), Some(track), Some(pos)) = (time, track, pos)
                        && out.len() < MAX_CUES
                    {
                        out.push(CuePoint {
                            time_ns: time.saturating_mul(timescale_ns),
                            track,
                            cluster_pos: segment_start.saturating_add(pos),
                        });
                    }
                }
                _ => {}
            }
        }
    }
    out.sort_by_key(|c| (c.time_ns, c.cluster_pos));
    Ok(())
}

/// The `OpusHead` an Opus track keeps in its codec private data (RFC 7845 §5.1).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OpusHead {
    pub channels: u8,
    /// Samples (at 48 kHz) to drop from the start of the decoded stream.
    pub pre_skip: u16,
    /// The rate of the original input (informative; Opus always decodes at 48 kHz here).
    pub input_rate: u32,
    /// Output gain in Q7.8 dB.
    pub gain: i16,
    /// Channel mapping family (0: mono or stereo, the only one a plain decoder handles).
    pub mapping_family: u8,
}

impl OpusHead {
    /// Parse the 19-byte head (or longer, with a mapping table). `None` when it is not one.
    pub fn parse(b: &[u8]) -> Option<OpusHead> {
        if b.len() < 19 || &b[..8] != b"OpusHead" || b[8] >> 4 != 0 {
            return None;
        }
        let head = OpusHead {
            channels: b[9],
            pre_skip: u16::from_le_bytes([b[10], b[11]]),
            input_rate: u32::from_le_bytes([b[12], b[13], b[14], b[15]]),
            gain: i16::from_le_bytes([b[16], b[17]]),
            mapping_family: b[18],
        };
        (head.channels > 0).then_some(head)
    }
}
