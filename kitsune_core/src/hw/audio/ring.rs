//! The queue of one stream: interleaved signed 16-bit samples, one or two channels, fixed capacity
//! (allocated once when the stream opens, so a write never allocates).

use super::Frame;
use alloc::vec;
use alloc::vec::Vec;

/// A circular buffer of frames of `channels` samples each.
pub struct SampleRing {
    buf: Vec<i16>,
    channels: usize,
    /// First frame, as a sample index into `buf`.
    head: usize,
    /// Frames held.
    len: usize,
}

impl SampleRing {
    /// A ring that holds `frames` frames of `channels` (1 or 2) samples.
    pub fn new(channels: usize, frames: usize) -> SampleRing {
        let channels = channels.clamp(1, 2);
        SampleRing {
            buf: vec![0; frames.max(1) * channels],
            channels,
            head: 0,
            len: 0,
        }
    }

    pub fn channels(&self) -> usize {
        self.channels
    }

    /// Frames it can hold.
    pub fn capacity(&self) -> usize {
        self.buf.len() / self.channels
    }

    /// Frames held.
    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Frames that still fit.
    pub fn room(&self) -> usize {
        self.capacity() - self.len
    }

    /// Append whole frames from little-endian bytes (`2 * channels` per frame). Returns the bytes
    /// taken: as many whole frames as fit; a partial frame at the end is never taken.
    pub fn push_le_bytes(&mut self, bytes: &[u8]) -> usize {
        let fb = 2 * self.channels;
        let frames = (bytes.len() / fb).min(self.room());
        let cap = self.buf.len();
        let mut at = (self.head + self.len * self.channels) % cap;
        for s in bytes[..frames * fb].as_chunks::<2>().0 {
            self.buf[at] = i16::from_le_bytes([s[0], s[1]]);
            at += 1;
            if at == cap {
                at = 0;
            }
        }
        self.len += frames;
        frames * fb
    }

    /// Append frames of samples (`channels` per frame); returns the frames taken.
    pub fn push_samples(&mut self, samples: &[i16]) -> usize {
        let frames = (samples.len() / self.channels).min(self.room());
        let cap = self.buf.len();
        let mut at = (self.head + self.len * self.channels) % cap;
        for &s in &samples[..frames * self.channels] {
            self.buf[at] = s;
            at += 1;
            if at == cap {
                at = 0;
            }
        }
        self.len += frames;
        frames
    }

    /// Frame `i` (0 = the oldest) as stereo: a mono sample goes to both sides.
    pub fn get(&self, i: usize) -> Option<Frame> {
        if i >= self.len {
            return None;
        }
        let at = (self.head + i * self.channels) % self.buf.len();
        let l = self.buf[at];
        Some(if self.channels == 2 {
            [l, self.buf[(at + 1) % self.buf.len()]]
        } else {
            [l, l]
        })
    }

    /// Forget the `n` oldest frames (all of them if fewer are held).
    pub fn drop_front(&mut self, n: usize) {
        let n = n.min(self.len);
        self.head = (self.head + n * self.channels) % self.buf.len();
        self.len -= n;
    }

    pub fn clear(&mut self) {
        self.head = 0;
        self.len = 0;
    }
}
