//! The mixer: up to [`MAX_STREAMS`] streams summed into one stereo output at [`DEVICE_RATE`].
//!
//! A stream id is `generation << 8 | slot`, so an id that was closed is never mistaken for the
//! stream that took its slot later. Every operation names the owner (a process id, `0` for the
//! kernel) and a stream answers only to its owner. The sum is done in 32 bits, then scaled by the
//! master volume and saturated to 16 bits (two loud streams clip instead of wrapping around).
//! Mute keeps the clock running (frames are consumed and played as silence), so a player that is
//! muted stays in step.

use super::{
    AudioErr, DEVICE_RATE, Frame, MAX_RATE, MAX_STREAMS, MAX_STREAMS_PER_OWNER, MIN_RATE,
    QUEUE_BYTES, Stream, StreamStatus, VOLUME_MAX, gain_q16,
};
use alloc::vec;
use alloc::vec::Vec;

/// Frames mixed per pass (the scratch buffer's size).
const CHUNK: usize = 512;

/// All the streams and the output state.
pub struct Mixer {
    slots: Vec<Option<Stream>>,
    gens: [u32; MAX_STREAMS],
    master: u8,
    muted: bool,
    /// Frames handed to the device so far, and frames it has played (device time).
    written: u64,
    played: u64,
    acc: Vec<[i32; 2]>,
    /// Output samples that hit the 16-bit limit (a sign that the volume is too high).
    pub clipped: u64,
}

impl Default for Mixer {
    fn default() -> Self {
        Self::new()
    }
}

impl Mixer {
    pub fn new() -> Mixer {
        Mixer {
            slots: (0..MAX_STREAMS).map(|_| None).collect(),
            gens: [0; MAX_STREAMS],
            master: VOLUME_MAX,
            muted: false,
            written: 0,
            played: 0,
            acc: vec![[0; 2]; CHUNK],
            clipped: 0,
        }
    }

    /// Open a stream of s16 samples at `rate` with `channels`, owned by `owner`. Returns its id.
    pub fn open(&mut self, owner: u32, rate: u32, channels: u32) -> Result<u32, AudioErr> {
        if !(MIN_RATE..=MAX_RATE).contains(&rate) {
            return Err(AudioErr::BadRate);
        }
        if !(1..=2).contains(&channels) {
            return Err(AudioErr::BadChannels);
        }
        if self.owned_by(owner) >= MAX_STREAMS_PER_OWNER {
            return Err(AudioErr::TooMany);
        }
        let slot = self
            .slots
            .iter()
            .position(Option::is_none)
            .ok_or(AudioErr::TooMany)?;
        self.gens[slot] = (self.gens[slot] + 1) & 0x7F_FFFF;
        let id = self.gens[slot] << 8 | slot as u32;
        self.slots[slot] = Some(Stream::new(owner, id, rate, channels as usize, QUEUE_BYTES));
        Ok(id)
    }

    /// Streams `owner` has open.
    pub fn owned_by(&self, owner: u32) -> usize {
        self.slots
            .iter()
            .flatten()
            .filter(|s| s.owner == owner)
            .count()
    }

    /// Streams open.
    pub fn open_count(&self) -> usize {
        self.slots.iter().flatten().count()
    }

    /// The stream `id` of `owner`.
    pub fn stream(&mut self, owner: u32, id: u32) -> Result<&mut Stream, AudioErr> {
        let slot = (id & 0xFF) as usize;
        match self.slots.get_mut(slot) {
            Some(Some(s)) if s.id == id && s.owner == owner => Ok(s),
            _ => Err(AudioErr::NoStream),
        }
    }

    /// Queue bytes on a stream: the bytes taken, or `Full` when not even one frame fits.
    pub fn write(&mut self, owner: u32, id: u32, bytes: &[u8]) -> Result<usize, AudioErr> {
        let s = self.stream(owner, id)?;
        if s.draining {
            return Err(AudioErr::NoStream);
        }
        let n = s.write(bytes);
        if n == 0 && !bytes.is_empty() && s.free_bytes() < bytes.len() {
            return Err(AudioErr::Full);
        }
        Ok(n)
    }

    pub fn status(&mut self, owner: u32, id: u32) -> Result<StreamStatus, AudioErr> {
        let played = self.played;
        Ok(self.stream(owner, id)?.status(played))
    }

    /// Everything written to `id` has been played.
    pub fn drained(&mut self, owner: u32, id: u32) -> Result<bool, AudioErr> {
        let played = self.played;
        Ok(self.stream(owner, id)?.drained(played))
    }

    /// What `audio_wait` waits for: `want` free bytes in the queue of `id`, or (`want == 0`)
    /// everything written to it played. A stream that is gone is "ready" (the waiter must look).
    pub fn ready(&mut self, owner: u32, id: u32, want: u64) -> bool {
        let played = self.played;
        match self.stream(owner, id) {
            Err(_) => true,
            Ok(s) if want == 0 => s.drained(played),
            Ok(s) => s.free_bytes() as u64 >= want,
        }
    }

    /// Close a stream: at once (what is queued is dropped), or after it plays what it has
    /// (`drain`; the id stops accepting writes and goes away by itself).
    pub fn close(&mut self, owner: u32, id: u32, drain: bool) -> Result<(), AudioErr> {
        let s = self.stream(owner, id)?;
        if drain && !s.finished() {
            s.draining = true;
            s.paused = false;
        } else {
            self.slots[(id & 0xFF) as usize] = None;
        }
        Ok(())
    }

    /// Close every stream of `owner` at once (it ended).
    pub fn close_owner(&mut self, owner: u32) -> usize {
        let mut n = 0;
        for slot in self.slots.iter_mut() {
            if slot.as_ref().is_some_and(|s| s.owner == owner) {
                *slot = None;
                n += 1;
            }
        }
        n
    }

    pub fn set_volume(&mut self, owner: u32, id: u32, v: u32) -> Result<(), AudioErr> {
        let v = u8::try_from(v)
            .ok()
            .filter(|v| *v <= VOLUME_MAX)
            .ok_or(AudioErr::BadValue)?;
        self.stream(owner, id)?.set_volume(v);
        Ok(())
    }

    pub fn set_paused(&mut self, owner: u32, id: u32, paused: bool) -> Result<(), AudioErr> {
        self.stream(owner, id)?.paused = paused;
        Ok(())
    }

    /// Drop what `id` has queued.
    pub fn flush(&mut self, owner: u32, id: u32) -> Result<(), AudioErr> {
        self.stream(owner, id)?.flush();
        Ok(())
    }

    pub fn master(&self) -> u8 {
        self.master
    }

    pub fn set_master(&mut self, v: u32) -> Result<(), AudioErr> {
        self.master = u8::try_from(v)
            .ok()
            .filter(|v| *v <= VOLUME_MAX)
            .ok_or(AudioErr::BadValue)?;
        Ok(())
    }

    pub fn muted(&self) -> bool {
        self.muted
    }

    pub fn set_muted(&mut self, m: bool) {
        self.muted = m;
    }

    /// Some stream has sound waiting (the device should keep running).
    pub fn has_audio(&self) -> bool {
        self.slots.iter().flatten().any(Stream::audible)
    }

    /// Device time: frames handed out and frames played.
    pub fn device_written(&self) -> u64 {
        self.written
    }

    pub fn device_played(&self) -> u64 {
        self.played
    }

    /// The device reports its play position (frames since it started; never goes back).
    pub fn set_device_played(&mut self, played: u64) {
        let played = played.min(self.written);
        if played > self.played {
            self.played = played;
            for s in self.slots.iter_mut().flatten() {
                s.advance(played);
            }
        }
    }

    /// Fill `out` with the next frames of the mix (silence where no stream has sound). Returns
    /// `true` if some stream contributed. Streams closed with drain that are done go away.
    pub fn mix(&mut self, out: &mut [Frame]) -> bool {
        let mut any = false;
        for chunk in out.chunks_mut(CHUNK) {
            let n = chunk.len();
            let acc = &mut self.acc[..n];
            acc.fill([0, 0]);
            for s in self.slots.iter_mut().flatten() {
                if s.render(acc, self.written) > 0 {
                    any = true;
                }
            }
            let master = if self.muted { 0 } else { gain_q16(self.master) };
            for (o, a) in chunk.iter_mut().zip(acc.iter()) {
                for c in 0..2 {
                    let v = (i64::from(a[c]) * master) >> 16;
                    let clamped = v.clamp(i64::from(i16::MIN), i64::from(i16::MAX));
                    if clamped != v {
                        self.clipped += 1;
                    }
                    o[c] = clamped as i16;
                }
            }
            self.written += n as u64;
        }
        for slot in self.slots.iter_mut() {
            if slot.as_ref().is_some_and(Stream::finished) {
                *slot = None;
            }
        }
        any
    }

    /// The device stopped and restarted (or was reset): device time continues from what was
    /// handed out, nothing is pending in it any more.
    pub fn device_reset(&mut self) {
        self.set_device_played(self.written);
    }

    /// Ids of every open stream with its owner (for the task list and tests).
    pub fn streams(&self) -> impl Iterator<Item = (u32, u32)> + '_ {
        self.slots.iter().flatten().map(|s| (s.owner, s.id))
    }

    /// The rate the mixer produces.
    pub const fn rate(&self) -> u32 {
        DEVICE_RATE
    }
}
