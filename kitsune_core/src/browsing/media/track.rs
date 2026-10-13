//! A track buffer: the coded frames of one track a source buffer holds (the "track buffer" of the
//! Media Source Extensions spec), sorted by presentation time, with what a player needs from it:
//! the buffered ranges, removal (`SourceBuffer.remove`, eviction of what was played), and where to
//! start decoding for a time ([`TrackBuffer::seek_index`]) and what comes next ([`TrackBuffer::next`]).
//!
//! VP9 in WebM has no reordering (decode order is presentation order), so one sorted list serves
//! both. A video frame depends on the frames before it back to the last keyframe: removing frames
//! also removes the ones after them up to the next keyframe, and a decoder that finds a hole in front
//! of it skips to the next keyframe.

use super::ranges::Ranges;
use alloc::collections::VecDeque;
use alloc::vec::Vec;

/// Gaps up to this long do not split the buffered ranges (a frame's worth of rounding).
pub const RANGE_SLACK_NS: i64 = 70_000_000;
/// A seek is served from a keyframe that starts at most this long after the time asked for when
/// nothing is buffered at the time itself (a segment that starts a hair late).
pub const SEEK_SLACK_NS: i64 = 500_000_000;
/// Duration given to a video frame until the next one says how long it was.
const DEFAULT_VIDEO_DUR: i64 = 33_333_333;
/// Longest gap between two video frames still taken as the first one's duration.
const MAX_VIDEO_DUR: i64 = 1_000_000_000;

/// One coded frame.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CodedFrame {
    /// Presentation time, ns.
    pub pts: i64,
    /// Duration, ns (estimated until known for video).
    pub dur: i64,
    pub key: bool,
    /// `DiscardPadding` (audio): ns of decoded audio to drop at the end.
    pub discard_ns: i64,
    /// Order of arrival in this buffer: a frame follows the one decoded before it when the frame
    /// before it in the buffer has the decoded one's `seq`.
    pub seq: u64,
    pub data: Vec<u8>,
}

/// Where a decoder of the track is.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Cursor {
    /// A seek is pending to this time: decoding starts at [`TrackBuffer::seek_index`].
    pub target: Option<i64>,
    /// The last frame handed to the decoder (`pts`, `seq`).
    pub last: Option<(i64, u64)>,
}

impl Cursor {
    /// Start over at `t` (a seek).
    pub fn seek(t: i64) -> Cursor {
        Cursor {
            target: Some(t),
            last: None,
        }
    }

    /// Frame `f` was decoded.
    pub fn advance(&mut self, f: &CodedFrame) {
        self.target = None;
        self.last = Some((f.pts, f.seq));
    }
}

/// The coded frames of one track.
#[derive(Clone, Debug, Default)]
pub struct TrackBuffer {
    frames: VecDeque<CodedFrame>,
    bytes: usize,
    seq: u64,
    video: bool,
    /// The last frame's duration is a guess.
    last_estimated: bool,
}

impl TrackBuffer {
    /// A buffer for a video track (frames depend on keyframes) or an audio one (every frame stands
    /// alone).
    pub fn new(video: bool) -> TrackBuffer {
        TrackBuffer {
            video,
            ..TrackBuffer::default()
        }
    }

    pub fn is_video(&self) -> bool {
        self.video
    }

    pub fn len(&self) -> usize {
        self.frames.len()
    }

    pub fn is_empty(&self) -> bool {
        self.frames.is_empty()
    }

    /// Bytes of coded data held.
    pub fn bytes(&self) -> usize {
        self.bytes
    }

    pub fn get(&self, i: usize) -> Option<&CodedFrame> {
        self.frames.get(i)
    }

    /// Presentation time of the first and the end of the last frame.
    pub fn span(&self) -> Option<(i64, i64)> {
        Some((
            self.frames.front()?.pts,
            self.frames.iter().map(|f| f.pts + f.dur).max()?,
        ))
    }

    /// Add a frame (`dur` `None`: not known yet). A frame that lands on time already buffered
    /// replaces what was there (the "coded frame processing" of the spec, simplified: the frames
    /// that start inside the new one's time go, and the frames depending on them).
    pub fn push(&mut self, pts: i64, dur: Option<i64>, key: bool, discard_ns: i64, data: Vec<u8>) {
        let key = key || !self.video;
        let estimated = dur.is_none_or(|d| d <= 0);
        let mut dur = match dur {
            Some(d) if d > 0 => d,
            _ if self.video => DEFAULT_VIDEO_DUR,
            _ => 20_000_000,
        };
        // The frame before this one learns its duration.
        if self.last_estimated
            && let Some(last) = self.frames.back_mut()
            && pts > last.pts
            && pts - last.pts <= MAX_VIDEO_DUR
        {
            last.dur = pts - last.pts;
        }
        if estimated && self.video {
            // Until the next frame comes, as long as the one before.
            if let Some(last) = self.frames.back()
                && pts > last.pts
            {
                dur = last.dur;
            }
        }
        let behind = self.frames.back().is_some_and(|l| pts <= l.pts);
        if behind {
            self.remove_overlap(pts, pts + dur);
        }
        let f = CodedFrame {
            pts,
            dur,
            key,
            discard_ns,
            seq: self.seq,
            data,
        };
        self.seq += 1;
        self.bytes += f.data.len();
        let at = self.frames.partition_point(|x| x.pts < pts);
        let at_end = at == self.frames.len();
        self.frames.insert(at, f);
        if at_end {
            self.last_estimated = estimated && self.video;
        }
    }

    /// Frames starting in `[a, b)` go (a new frame covers that time), with their dependents.
    fn remove_overlap(&mut self, a: i64, b: i64) {
        let i = self.frames.partition_point(|x| x.pts < a);
        let j = self.frames.partition_point(|x| x.pts < b);
        if i < j {
            self.drop_span(i, j);
        }
    }

    /// Drop frames `i..j` and, for video, the frames after them up to the next keyframe.
    fn drop_span(&mut self, i: usize, mut j: usize) {
        if self.video {
            while self.frames.get(j).is_some_and(|f| !f.key) {
                j += 1;
            }
        }
        for f in self.frames.drain(i..j) {
            self.bytes -= f.data.len();
        }
    }

    /// `SourceBuffer.remove(a, b)`: the frames that start in `[a, b)`, and (video) the frames that
    /// depend on them.
    pub fn remove(&mut self, a: i64, b: i64) {
        if b <= a {
            return;
        }
        let i = self.frames.partition_point(|x| x.pts < a);
        let j = self.frames.partition_point(|x| x.pts < b);
        if i < j {
            self.drop_span(i, j);
        }
    }

    /// Forget what was played before `t`: frames that end before it, keeping (video) everything
    /// from the last keyframe at or before `t` so `t` can still be decoded. Returns the bytes freed.
    pub fn evict_before(&mut self, t: i64) -> usize {
        let mut n = self.frames.partition_point(|f| f.pts + f.dur <= t);
        if self.video {
            // Back to a keyframe: the frames from it on are needed to show `t`.
            while n > 0 && !self.frames.get(n).is_none_or(|f| f.key) {
                n -= 1;
            }
        }
        let before = self.bytes;
        for f in self.frames.drain(..n) {
            self.bytes -= f.data.len();
        }
        before - self.bytes
    }

    /// The buffered time (gaps up to [`RANGE_SLACK_NS`] closed).
    pub fn buffered(&self) -> Ranges {
        let mut r = Ranges::new();
        let mut run: Option<(i64, i64)> = None;
        for f in &self.frames {
            let (s, e) = (f.pts, f.pts + f.dur);
            run = match run {
                Some((a, b)) if s <= b + RANGE_SLACK_NS => Some((a, b.max(e))),
                Some((a, b)) => {
                    r.add(a, b);
                    Some((s, e))
                }
                None => Some((s, e)),
            };
        }
        if let Some((a, b)) = run {
            r.add(a, b);
        }
        r
    }

    /// The first frame to decode to show time `t`: for video the last keyframe at or before the
    /// frame that holds `t`, for audio that frame. `None` while `t` is not buffered (a keyframe
    /// starting up to [`SEEK_SLACK_NS`] after `t` also serves).
    pub fn seek_index(&self, t: i64) -> Option<usize> {
        // The frame holding t: the last one starting at or before it, if it lasts until t.
        let after = self.frames.partition_point(|f| f.pts <= t);
        let holder = after
            .checked_sub(1)
            .filter(|&i| self.frames[i].pts + self.frames[i].dur > t);
        match holder {
            Some(h) if !self.video => Some(h),
            Some(h) => {
                // Back to its keyframe through frames without holes.
                let mut i = h;
                loop {
                    if self.frames[i].key {
                        return Some(i);
                    }
                    if i == 0 {
                        break;
                    }
                    let (p, q) = (&self.frames[i - 1], &self.frames[i]);
                    if q.pts - (p.pts + p.dur) > RANGE_SLACK_NS {
                        break;
                    }
                    i -= 1;
                }
                // No keyframe before it: the next one, if it is close.
                self.key_from(h + 1, t)
            }
            None => self.key_from(after, t),
        }
    }

    /// The first keyframe at index `i` or later that starts within [`SEEK_SLACK_NS`] of `t`.
    fn key_from(&self, i: usize, t: i64) -> Option<usize> {
        (i..self.frames.len())
            .take_while(|&j| self.frames[j].pts <= t + SEEK_SLACK_NS)
            .find(|&j| self.frames[j].key)
    }

    /// The next frame for a decoder at `c`: after a seek, [`TrackBuffer::seek_index`]; otherwise the
    /// first frame after the last one decoded. A video frame that does not follow the decoded one
    /// (something was removed or replaced in between) cannot be decoded: the next keyframe is
    /// taken instead. `None`: nothing to decode yet.
    pub fn next(&self, c: &Cursor) -> Option<usize> {
        if let Some(t) = c.target {
            return self.seek_index(t);
        }
        let Some((pts, seq)) = c.last else {
            // From the start: the first frame (video: the first keyframe).
            return (0..self.frames.len()).find(|&j| !self.video || self.frames[j].key);
        };
        let i = self.frames.partition_point(|f| f.pts <= pts);
        let f = self.frames.get(i)?;
        let follows = i > 0 && self.frames[i - 1].seq == seq;
        if f.key || follows {
            return Some(i);
        }
        (i..self.frames.len()).find(|&j| self.frames[j].key)
    }

    /// Everything after the decoder at `c` has been decoded (the end of a stream that ended).
    pub fn done(&self, c: &Cursor) -> bool {
        c.target.is_none() && self.next(c).is_none()
    }
}

/// How long an Opus packet plays (RFC 6716 §3.1: the TOC byte and the frame count), in ns; 0 for a
/// malformed one.
pub fn opus_packet_ns(packet: &[u8]) -> i64 {
    let Some(&toc) = packet.first() else {
        return 0;
    };
    let config = toc >> 3;
    // Frame size in units of 2.5 ms.
    let units: i64 = match config {
        0..=11 => [4, 8, 16, 24][usize::from(config % 4)],
        12..=15 => [4, 8][usize::from(config % 2)],
        _ => [1, 2, 4, 8][usize::from(config % 4)],
    };
    let frames: i64 = match toc & 3 {
        0 => 1,
        1 | 2 => 2,
        _ => packet.get(1).map_or(0, |b| i64::from(b & 0x3F)),
    };
    // At most 120 ms per packet.
    (units * frames * 2_500_000).min(120_000_000)
}
