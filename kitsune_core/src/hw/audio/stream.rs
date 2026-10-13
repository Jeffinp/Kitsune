//! One stream: its queue, the resampler to the device rate, its volume, and the clock.
//!
//! **Resampling** is linear interpolation with a 32.32 fixed-point position: each output frame is
//! `a + (b - a) * frac` of the two source frames around the position, which then advances by
//! `rate / DEVICE_RATE`. At 48 kHz the step is exactly one frame and the samples pass unchanged.
//! Linear interpolation is cheap and good enough for speech, beeps and music from 44.1 kHz (its
//! images sit above 20 kHz for most content); a better filter can replace [`Stream::render`]
//! without touching anything else.
//!
//! **The clock.** Frames leave the queue when they are mixed, but they are *heard* later: the
//! device has its DMA buffers ahead of the speaker. So each mix records a segment "device frames
//! `d0..d1` carried this stream's frames `c0..c1`", and [`Stream::played`] maps the device's
//! play position back through those segments. That is what a player uses to keep the picture in
//! step with the sound, and it stays right across underruns (no device time passes while the
//! device is starved) and pauses.

use super::{DEVICE_RATE, Frame, SampleRing, gain_q16};

/// Segments kept for the clock: enough for the device's whole lead (the driver keeps ~12 blocks).
const SEGMENTS: usize = 64;

#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
struct Segment {
    d0: u64,
    d1: u64,
    c0: u64,
    c1: u64,
}

/// What `audio_status` reports about a stream.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct StreamStatus {
    /// Bytes waiting in the queue (in the writer's format).
    pub queued_bytes: u64,
    /// Bytes that still fit.
    pub free_bytes: u64,
    /// Frames of this stream (at its own rate) the device has played: the player's clock.
    pub played: u64,
    /// Frames taken from the queue by the mixer (played, or in the device's buffers).
    pub consumed: u64,
    /// Frames ever written.
    pub written: u64,
    /// Times the stream ran dry while playing.
    pub underruns: u32,
    pub rate: u32,
    pub channels: u8,
    pub volume: u8,
    pub paused: bool,
    /// Closed with drain: plays what is queued, then goes away.
    pub draining: bool,
}

/// One stream of the mixer.
pub struct Stream {
    pub(super) owner: u32,
    pub(super) id: u32,
    rate: u32,
    ring: SampleRing,
    /// Position in the queue, 32.32 frames past the oldest frame.
    pos: u64,
    step: u64,
    volume: u8,
    pub(super) paused: bool,
    pub(super) draining: bool,
    /// Was producing sound at the last mix.
    playing: bool,
    /// Ran dry while playing; it becomes an underrun if more samples come (a gap in the middle),
    /// not if the stream simply ended.
    starved: bool,
    underruns: u32,
    consumed: u64,
    written: u64,
    segs: [Segment; SEGMENTS],
    seg_head: usize,
    seg_len: usize,
    /// What is known to be played once every segment is behind the device.
    played_floor: u64,
}

impl Stream {
    /// A stream of `channels` (1 or 2) at `rate` holding up to `queue_bytes` of samples.
    pub(super) fn new(
        owner: u32,
        id: u32,
        rate: u32,
        channels: usize,
        queue_bytes: usize,
    ) -> Stream {
        let frames = queue_bytes / (2 * channels.clamp(1, 2));
        Stream {
            owner,
            id,
            rate,
            ring: SampleRing::new(channels, frames),
            pos: 0,
            step: (u64::from(rate) << 32) / u64::from(DEVICE_RATE),
            volume: super::VOLUME_MAX,
            paused: false,
            draining: false,
            playing: false,
            starved: false,
            underruns: 0,
            consumed: 0,
            written: 0,
            segs: [Segment::default(); SEGMENTS],
            seg_head: 0,
            seg_len: 0,
            played_floor: 0,
        }
    }

    fn frame_bytes(&self) -> usize {
        2 * self.ring.channels()
    }

    /// Queue little-endian samples; returns the bytes taken (whole frames that fit).
    pub fn write(&mut self, bytes: &[u8]) -> usize {
        let n = self.ring.push_le_bytes(bytes);
        self.written += (n / self.frame_bytes()) as u64;
        self.note_data(n);
        n
    }

    /// New samples after the queue ran dry while playing: that was an underrun.
    fn note_data(&mut self, n: usize) {
        if n > 0 && self.starved {
            self.starved = false;
            self.underruns = self.underruns.saturating_add(1);
        }
    }

    /// Queue samples (`channels` per frame); returns the frames taken.
    pub fn write_samples(&mut self, samples: &[i16]) -> usize {
        let n = self.ring.push_samples(samples);
        self.written += n as u64;
        self.note_data(n);
        n
    }

    pub fn free_bytes(&self) -> usize {
        self.ring.room() * self.frame_bytes()
    }

    pub fn set_volume(&mut self, v: u8) {
        self.volume = v.min(super::VOLUME_MAX);
    }

    pub fn volume(&self) -> u8 {
        self.volume
    }

    /// Has something to play (not paused, queue not empty).
    pub fn audible(&self) -> bool {
        !self.paused && !self.ring.is_empty()
    }

    /// Closed with drain and everything taken from the queue.
    pub fn finished(&self) -> bool {
        self.draining && self.ring.is_empty()
    }

    /// Mix this stream into `acc` (stereo sums, Q0) starting at device frame `dev0`: adds up to
    /// `acc.len()` resampled frames with the stream's volume and returns how many it added. A stream
    /// that runs dry while playing and then gets more samples counts an underrun.
    pub fn render(&mut self, acc: &mut [[i32; 2]], dev0: u64) -> usize {
        if self.paused {
            return 0;
        }
        let gain = gain_q16(self.volume);
        let c0 = self.consumed;
        let mut k = 0;
        while k < acc.len() {
            let i = (self.pos >> 32) as usize;
            let Some(a) = self.ring.get(i) else { break };
            let frac = (self.pos & 0xFFFF_FFFF) as i64;
            // Without the next frame (the queue ends here) hold the last one.
            let s = match self.ring.get(i + 1) {
                Some(b) if frac != 0 => lerp(a, b, frac),
                _ => a,
            };
            acc[k][0] += ((i64::from(s[0]) * gain) >> 16) as i32;
            acc[k][1] += ((i64::from(s[1]) * gain) >> 16) as i32;
            k += 1;
            self.pos += self.step;
        }
        let whole = ((self.pos >> 32) as usize).min(self.ring.len());
        self.ring.drop_front(whole);
        self.pos -= (whole as u64) << 32;
        if self.ring.is_empty() {
            // Never carry a position past the end into frames that arrive later.
            self.pos &= 0xFFFF_FFFF;
        }
        self.consumed += whole as u64;
        if k > 0 {
            self.playing = true;
        }
        if k < acc.len() && !self.draining {
            if self.playing {
                self.starved = true;
            }
            self.playing = false;
        }
        if k > 0 {
            self.push_segment(Segment {
                d0: dev0,
                d1: dev0 + k as u64,
                c0,
                c1: self.consumed,
            });
        }
        k
    }

    fn push_segment(&mut self, s: Segment) {
        if self.seg_len == SEGMENTS {
            self.played_floor = self.segs[self.seg_head].c1;
            self.seg_head = (self.seg_head + 1) % SEGMENTS;
            self.seg_len -= 1;
        }
        self.segs[(self.seg_head + self.seg_len) % SEGMENTS] = s;
        self.seg_len += 1;
    }

    /// Forget the segments the device has played completely (position `dev_played`).
    pub fn advance(&mut self, dev_played: u64) {
        while self.seg_len > 0 && self.segs[self.seg_head].d1 <= dev_played {
            self.played_floor = self.segs[self.seg_head].c1;
            self.seg_head = (self.seg_head + 1) % SEGMENTS;
            self.seg_len -= 1;
        }
    }

    /// Frames of this stream the device has played when its own position is `dev_played`.
    pub fn played(&self, dev_played: u64) -> u64 {
        let mut v = self.played_floor;
        for j in 0..self.seg_len {
            let s = self.segs[(self.seg_head + j) % SEGMENTS];
            if dev_played >= s.d1 {
                v = s.c1;
            } else {
                if dev_played > s.d0 {
                    v = s.c0 + (s.c1 - s.c0) * (dev_played - s.d0) / (s.d1 - s.d0);
                }
                break;
            }
        }
        v
    }

    /// Everything written has been played.
    pub fn drained(&self, dev_played: u64) -> bool {
        self.ring.is_empty() && self.played(dev_played) >= self.consumed
    }

    /// Drop what is queued (a seek or a stop): the clock keeps what was played.
    pub fn flush(&mut self) {
        self.ring.clear();
        self.pos = 0;
        self.playing = false;
        self.starved = false;
    }

    pub fn status(&self, dev_played: u64) -> StreamStatus {
        StreamStatus {
            queued_bytes: (self.ring.len() * self.frame_bytes()) as u64,
            free_bytes: self.free_bytes() as u64,
            played: self.played(dev_played),
            consumed: self.consumed,
            written: self.written,
            underruns: self.underruns,
            rate: self.rate,
            channels: self.ring.channels() as u8,
            volume: self.volume,
            paused: self.paused,
            draining: self.draining,
        }
    }
}

/// `a + (b - a) * frac / 2^32`, per channel.
fn lerp(a: Frame, b: Frame, frac: i64) -> Frame {
    let one = |x: i16, y: i16| -> i16 {
        let (x, y) = (i64::from(x), i64::from(y));
        (x + (((y - x) * frac) >> 32)) as i16
    };
    [one(a[0], b[0]), one(a[1], b[1])]
}
