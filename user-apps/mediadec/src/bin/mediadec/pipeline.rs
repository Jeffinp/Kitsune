//! From WebM bytes to pictures and PCM: the demuxer of `kitsune_core::webm`, the first VP9 and the
//! first Opus track, and the audio trimming Matroska asks for (the Opus pre-skip at the start,
//! `DiscardPadding` at the end).
//!
//! Shared by `mediadec` and the host checker (`tools/media/mediacheck`).

use super::ffi::{OPUS_MAX_SAMPLES, Opus, Picture, Vp9};
use alloc::vec;
use alloc::vec::Vec;
use kitsune_core::webm::{self, Codec, Demuxer, Event, OpusHead, TrackKind};

/// What [`Media::poll`] hands out.
pub enum Out<'a> {
    /// The tracks are known ([`Media::summary`]).
    Header,
    /// A picture to show at `ts_ns`.
    Picture { ts_ns: i64, picture: Picture<'a> },
    /// Stereo 48 kHz s16 samples, interleaved, starting at `ts_ns` (may be empty after trimming).
    Audio { ts_ns: i64, pcm: &'a [i16] },
    /// A packet that did not decode (the stream goes on).
    DecodeError { audio: bool, code: i32 },
}

/// Why the media cannot be read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fatal {
    Demux(webm::Error),
    /// The C decoder could not be created (memory).
    NoDecoder,
}

/// The tracks this pipeline plays.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Summary {
    pub duration_ns: u64,
    pub video_track: Option<u64>,
    pub width: u32,
    pub height: u32,
    pub audio_track: Option<u64>,
    /// Channels of the Opus stream (the output is always stereo).
    pub channels: u8,
    pub pre_skip: u16,
    /// Tracks present but not decodable here (other codecs, encrypted).
    pub ignored: u32,
}

struct Audio {
    dec: Opus,
    /// Samples still to drop from the start (pre-skip, or the pre-roll after a seek).
    skip: usize,
}

/// The pipeline.
pub struct Media {
    demux: Demuxer,
    summary: Summary,
    vp9: Option<Vp9>,
    audio: Option<Audio>,
    pcm: Vec<i16>,
    /// After a seek: drop output before this time.
    drop_before_ns: i64,
}

impl Default for Media {
    fn default() -> Self {
        Self::new()
    }
}

impl Media {
    pub fn new() -> Media {
        Media {
            demux: Demuxer::new(),
            summary: Summary::default(),
            vp9: None,
            audio: None,
            pcm: vec![0; OPUS_MAX_SAMPLES * 2],
            drop_before_ns: i64::MIN,
        }
    }

    /// Hand in more bytes of the file (any size; [`webm::MAX_PENDING`] at most waiting).
    pub fn push(&mut self, bytes: &[u8]) -> Result<(), Fatal> {
        self.demux.push(bytes).map_err(Fatal::Demux)
    }

    pub fn summary(&self) -> Summary {
        self.summary
    }

    pub fn demuxer(&self) -> &Demuxer {
        &self.demux
    }

    /// libvpx's text for the last video error.
    pub fn video_error(&self) -> &str {
        self.vp9.as_ref().map_or("", |v| v.error())
    }

    /// Prepare to continue from `time_ns`: returns the file offset of the cluster to feed from
    /// (from the cues), and from then on output before `time_ns` is dropped. `None` without cues.
    pub fn seek(&mut self, time_ns: u64) -> Option<u64> {
        let offset = self.demux.cluster_for(time_ns)?;
        self.restart_at(offset, time_ns.min(i64::MAX as u64) as i64);
        Some(offset)
    }

    /// The bytes pushed next start at file offset `offset` (a cluster); output before
    /// `drop_before_ns` is dropped. The decoders start over.
    pub fn restart_at(&mut self, offset: u64, drop_before_ns: i64) {
        self.demux.seek(offset);
        self.drop_before_ns = drop_before_ns;
        if let Some(a) = self.audio.as_mut() {
            a.dec.reset();
            a.skip = 0;
        }
    }

    /// The next output; `Ok(None)` when more bytes are needed.
    pub fn poll(&mut self) -> Result<Option<Out<'_>>, Fatal> {
        loop {
            let ev = match self.demux.poll() {
                Ok(Some(ev)) => ev,
                Ok(None) => return Ok(None),
                Err(e) => return Err(Fatal::Demux(e)),
            };
            let p = match ev {
                Event::Header => {
                    self.header()?;
                    return Ok(Some(Out::Header));
                }
                Event::Packet(p) => p,
            };
            if Some(p.track) == self.summary.video_track {
                let Some(v) = self.vp9.as_mut() else { continue };
                if let Err(code) = v.decode(&p.data) {
                    return Ok(Some(Out::DecodeError { audio: false, code }));
                }
                if p.timestamp_ns < self.drop_before_ns {
                    continue;
                }
                if let Some(picture) = self.vp9.as_ref().and_then(|v| v.picture()) {
                    return Ok(Some(Out::Picture {
                        ts_ns: p.timestamp_ns,
                        picture,
                    }));
                }
            } else if Some(p.track) == self.summary.audio_track {
                let Some(a) = self.audio.as_mut() else {
                    continue;
                };
                let n = match a.dec.decode(&p.data, &mut self.pcm) {
                    Ok(n) => n,
                    Err(code) => return Ok(Some(Out::DecodeError { audio: true, code })),
                };
                let head = a.skip.min(n);
                a.skip -= head;
                // DiscardPadding: nanoseconds at 48 kHz, rounded.
                let tail = if p.discard_padding_ns > 0 {
                    ((p.discard_padding_ns as u64).saturating_mul(48_000) + 500_000_000)
                        / 1_000_000_000
                } else {
                    0
                };
                let end = n.saturating_sub(tail.min(n as u64) as usize).max(head);
                if p.timestamp_ns < self.drop_before_ns {
                    continue;
                }
                let ts = p
                    .timestamp_ns
                    .saturating_add((head as i64) * 1_000_000_000 / 48_000);
                return Ok(Some(Out::Audio {
                    ts_ns: ts,
                    pcm: &self.pcm[head * 2..end * 2],
                }));
            }
        }
    }

    /// Pick the tracks of a new header and make their decoders.
    fn header(&mut self) -> Result<(), Fatal> {
        let mut s = Summary {
            duration_ns: self.demux.info().duration_ns.unwrap_or(0),
            ..Summary::default()
        };
        let mut audio_head = None;
        for t in self.demux.tracks() {
            match (t.kind, t.codec) {
                (TrackKind::Video, Codec::Vp9) if s.video_track.is_none() && !t.encoded => {
                    s.video_track = Some(t.number);
                    s.width = t.width;
                    s.height = t.height;
                }
                (TrackKind::Audio, Codec::Opus) if s.audio_track.is_none() && !t.encoded => {
                    // Mapping family 0 only (mono or stereo): the plain decoder.
                    let head = OpusHead::parse(&t.codec_private);
                    let delay = (t.codec_delay_ns.saturating_mul(48_000) / 1_000_000_000) as u16;
                    match head {
                        Some(h) if h.mapping_family == 0 && h.channels <= 2 => {
                            s.audio_track = Some(t.number);
                            s.channels = h.channels;
                            s.pre_skip = h.pre_skip;
                            audio_head = Some(h);
                        }
                        None if t.channels <= 2 => {
                            s.audio_track = Some(t.number);
                            s.channels = t.channels;
                            s.pre_skip = delay;
                        }
                        _ => s.ignored += 1,
                    }
                }
                (TrackKind::Video | TrackKind::Audio, _) => s.ignored += 1,
                _ => {}
            }
        }
        self.vp9 = None;
        self.audio = None;
        if s.video_track.is_some() {
            self.vp9 = Some(Vp9::new().ok_or(Fatal::NoDecoder)?);
        }
        if s.audio_track.is_some() {
            let mut dec = Opus::new().ok_or(Fatal::NoDecoder)?;
            if let Some(h) = audio_head {
                dec.set_gain(h.gain);
            }
            self.audio = Some(Audio {
                dec,
                skip: usize::from(s.pre_skip),
            });
        }
        self.summary = s;
        Ok(())
    }
}
