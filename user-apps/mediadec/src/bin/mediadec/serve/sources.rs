//! Source buffers: each has its own demuxer and the track buffers its bytes fill (one video, one
//! sound track at most: what this player decodes).

use super::*;
use kitsune_core::media::ranges::Ranges;
use kitsune_core::media::track::opus_packet_ns;
use kitsune_core::webm::{Codec, OpusHead, TrackKind};

/// The sound track's numbers from its header.
#[derive(Clone, Copy, Debug, Default)]
pub(super) struct AudioInfo {
    pub channels: u8,
    pub pre_skip: u16,
    pub gain: i16,
}

/// One source buffer.
pub(super) struct Source {
    pub demux: Demuxer,
    /// `timestampOffset`, added to every frame's time.
    pub ts_off: i64,
    /// Track number and coded frames of the video track.
    pub video: Option<(u64, TrackBuffer)>,
    pub audio: Option<(u64, TrackBuffer, AudioInfo)>,
    pub width: u32,
    pub height: u32,
    pub duration_ns: u64,
    /// The bytes are broken: nothing more is taken until a reset.
    pub broken: bool,
}

impl Source {
    pub fn new() -> Source {
        Source {
            demux: Demuxer::new(),
            ts_off: 0,
            video: None,
            audio: None,
            width: 0,
            height: 0,
            duration_ns: 0,
            broken: false,
        }
    }

    pub fn bytes(&self) -> usize {
        self.video.as_ref().map_or(0, |v| v.1.bytes()) + self.audio.as_ref().map_or(0, |a| a.1.bytes())
    }

    /// What this source holds: the time all its tracks have.
    pub fn buffered(&self) -> Ranges {
        match (&self.video, &self.audio) {
            (Some(v), Some(a)) => v.1.buffered().intersect(&a.1.buffered()),
            (Some(v), None) => v.1.buffered(),
            (None, Some(a)) => a.1.buffered(),
            (None, None) => Ranges::new(),
        }
    }

    pub fn remove(&mut self, a: i64, b: i64) {
        if let Some(v) = self.video.as_mut() {
            v.1.remove(a, b);
        }
        if let Some(x) = self.audio.as_mut() {
            x.1.remove(a, b);
        }
    }

    pub fn evict_before(&mut self, t: i64) {
        if let Some(v) = self.video.as_mut() {
            v.1.evict_before(t);
        }
        if let Some(x) = self.audio.as_mut() {
            x.1.evict_before(t);
        }
    }

    /// The next bytes start at file offset `offset` (`u64::MAX`: a new stream from its EBML
    /// header). What was half read is dropped; the coded frames stay.
    pub fn reset(&mut self, offset: u64) {
        if offset == u64::MAX {
            self.demux.reset();
        } else {
            self.demux.seek(offset);
        }
        self.broken = false;
    }

    /// A header came: pick the tracks this player decodes. Returns whether any is new.
    fn header(&mut self) -> bool {
        let mut new = false;
        self.duration_ns = self.demux.info().duration_ns.unwrap_or(0);
        let (mut vnum, mut anum) = (None, None);
        for t in self.demux.tracks() {
            match (t.kind, t.codec) {
                (TrackKind::Video, Codec::Vp9) if vnum.is_none() && !t.encoded => {
                    vnum = Some(t.number);
                    self.width = t.width;
                    self.height = t.height;
                }
                (TrackKind::Audio, Codec::Opus) if anum.is_none() && !t.encoded => {
                    let delay = (t.codec_delay_ns.saturating_mul(48_000) / 1_000_000_000) as u16;
                    let info = match OpusHead::parse(&t.codec_private) {
                        Some(h) if h.mapping_family == 0 && h.channels <= 2 => AudioInfo {
                            channels: h.channels,
                            pre_skip: h.pre_skip,
                            gain: h.gain,
                        },
                        None if t.channels <= 2 => AudioInfo {
                            channels: t.channels,
                            pre_skip: delay,
                            gain: 0,
                        },
                        _ => continue,
                    };
                    anum = Some((t.number, info));
                }
                _ => {}
            }
        }
        if let Some(n) = vnum {
            match self.video.as_mut() {
                // A new init segment for the same track (another quality): the frames stay.
                Some(v) => v.0 = n,
                None => {
                    self.video = Some((n, TrackBuffer::new(true)));
                    new = true;
                }
            }
        }
        if let Some((n, info)) = anum {
            match self.audio.as_mut() {
                Some(a) => {
                    a.0 = n;
                    a.2 = info;
                }
                None => {
                    self.audio = Some((n, TrackBuffer::new(false), info));
                    new = true;
                }
            }
        }
        new
    }
}

impl Server {
    /// Bytes for source `src` (append `seq`): demux them into its track buffers at once and say
    /// what it holds now.
    pub(super) fn append(&mut self, src: usize, seq: u32, bytes: &[u8]) {
        if self.held() + bytes.len() > EVICT_BYTES {
            self.evict();
        }
        if self.held() + bytes.len() > MAX_BYTES {
            return self.report(src, seq, proto::BUF_FULL);
        }
        let s = self.source(src);
        if s.broken {
            return self.report(src, seq, proto::BUF_BROKEN);
        }
        if let Err(e) = s.demux.push(bytes) {
            s.broken = true;
            let t = text(e);
            self.error(proto::ERR_DEMUX, &t);
            return self.report(src, seq, proto::BUF_BROKEN);
        }
        loop {
            let s = &mut self.sources[src];
            let ev = match s.demux.poll() {
                Ok(Some(ev)) => ev,
                Ok(None) => break,
                Err(e) => {
                    s.broken = true;
                    let t = text(e);
                    self.error(proto::ERR_DEMUX, &t);
                    return self.report(src, seq, proto::BUF_BROKEN);
                }
            };
            match ev {
                Event::Header => {
                    s.header();
                    self.bind(src);
                    let s = &self.sources[src];
                    let h = proto::Header {
                        duration_ns: s.duration_ns,
                        width: s.width,
                        height: s.height,
                        video: s.video.is_some(),
                        audio: s.audio.is_some(),
                        channels: s.audio.as_ref().map_or(0, |a| a.2.channels),
                        src: src as u8,
                    };
                    if !h.video && !h.audio {
                        self.error(proto::ERR_CODEC, "no VP9 or Opus track");
                    }
                    self.post(proto::header(&h), KIND_OTHER);
                }
                Event::Packet(p) => {
                    let pts = p.timestamp_ns.saturating_add(s.ts_off);
                    if let Some((n, buf)) = s.video.as_mut()
                        && *n == p.track
                    {
                        let dur = p.duration_ns.map(|d| d.min(i64::MAX as u64) as i64);
                        buf.push(pts, dur, p.keyframe, 0, p.data);
                    } else if let Some((n, buf, _)) = s.audio.as_mut()
                        && *n == p.track
                    {
                        let dur = Some(opus_packet_ns(&p.data));
                        buf.push(pts, dur, true, p.discard_padding_ns, p.data);
                    }
                }
            }
        }
        self.report(src, seq, proto::BUF_OK);
    }

    /// Bind the decoders to the first source that has each track.
    fn bind(&mut self, src: usize) {
        let s = &self.sources[src];
        if self.video.is_none() && s.video.is_some() {
            match Vp9::new() {
                Some(dec) => {
                    self.video = Some(VideoDec {
                        src,
                        dec,
                        cursor: if self.target == i64::MIN {
                            Cursor::default()
                        } else {
                            Cursor::seek(self.target)
                        },
                    })
                }
                None => self.error(proto::ERR_MEMORY, "cannot create the VP9 decoder"),
            }
        }
        let s = &self.sources[src];
        if self.audio.is_none()
            && let Some((_, _, info)) = s.audio
        {
            match Opus::new() {
                Some(mut dec) => {
                    dec.set_gain(info.gain);
                    let start = self.target == i64::MIN;
                    self.audio = Some(AudioDec {
                        src,
                        dec,
                        cursor: if start {
                            Cursor::default()
                        } else {
                            Cursor::seek(self.target.saturating_sub(PRE_ROLL_NS))
                        },
                        skip: if start { usize::from(info.pre_skip) } else { 0 },
                        delay_ns: i64::from(info.pre_skip) * 1_000_000_000 / 48_000,
                    });
                }
                None => self.error(proto::ERR_MEMORY, "cannot create the Opus decoder"),
            }
        }
    }
}
