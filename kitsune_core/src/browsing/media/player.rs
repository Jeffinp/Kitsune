//! The player of one `<video>`/`<audio>` element: what the decoder sent, the clock, which picture is
//! on screen, the sound fed to the device, and the state the page sees (`paused`, `ended`,
//! `seeking`, `readyState`) with its events. Generic over the picture (`F`): the kernel keeps the
//! decoder's message, the tests a number.
//!
//! Driven by the browser: [`Player::header`], [`Player::frame`], [`Player::audio`], [`Player::mark`]
//! and [`Player::drained`] for what the decoder sends; [`Player::play`], [`Player::pause`],
//! [`Player::seek`] and the setters for what the page or the controls ask; [`Player::step`] every
//! loop of the compositor (it feeds the sound, moves the clock and picks the picture); then
//! [`Player::take_events`] for the page and [`Player::take_acks`] for the decoder.
//!
//! **Sync.** The sound is the master clock (see [`super::clock`]). A picture is shown when its time
//! has come: of the pictures due, the latest is shown and the others are dropped (late); when none is
//! due the one on screen stays. Without sound the system clock runs, and it stops when the pictures
//! run out (`waiting`).

use super::clock::{AudioSink, Clock, frames_ns, ns_frames};
use super::proto::Header;
use alloc::collections::VecDeque;
use alloc::vec::Vec;

mod feed;
mod state;
pub use state::*;

/// Sound queued ahead (device queue plus waiting here) before playback starts or resumes.
pub const START_AHEAD_NS: i64 = 300_000_000;
/// Gaps and overlaps in the sound shorter than this are played as they come.
const AUDIO_SLACK_NS: i64 = 40_000_000;
/// The longest silence put in for a gap in the sound.
const MAX_GAP_NS: i64 = 2_000_000_000;
/// `timeupdate` while playing, at most this often.
const TIMEUPDATE_MS: u64 = 250;

/// How much the decoder may have waiting for the player (it is told with `IN_FLOW`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Flow {
    /// Bytes of picture messages.
    pub video: u32,
    /// Bytes of sound messages.
    pub audio: u32,
}

impl Default for Flow {
    fn default() -> Self {
        Flow {
            video: 6 << 20,
            audio: 512 << 10,
        }
    }
}

/// A decoded picture waiting for its time.
struct Queued<F> {
    pts: i64,
    bytes: u32,
    frame: F,
}

/// A piece of decoded sound waiting for room in the device queue.
struct Pcm {
    pts: i64,
    data: Vec<u8>,
    /// Bytes of the decoder's message (returned when it is all written).
    bytes: u32,
}

/// The player.
pub struct Player<F> {
    pub(super) st: State,
    flow: Flow,
    /// The device takes sound (else the sound is dropped and the system clock runs).
    audio_out: bool,
    clock: Clock,
    /// The media time last computed by [`Player::step`].
    position: i64,
    frames: VecDeque<Queued<F>>,
    shown: Option<Queued<F>>,
    /// The next picture goes on screen whatever its time (after a seek, at the start).
    show_next: bool,
    /// Time between the last two pictures (how long the last one lasts).
    frame_dur: i64,
    held_video: u32,
    pcm: VecDeque<Pcm>,
    /// Bytes of the front piece already in the device queue.
    pcm_off: usize,
    held_audio: u32,
    /// Time of the first sample written since the last flush, and frames written since.
    audio_base: Option<i64>,
    audio_written: i64,
    /// Silence still owed to the device for a gap (frames).
    silence: i64,
    /// End time of the last sound and the last picture received.
    audio_end: i64,
    video_end: i64,
    sink_paused: bool,
    /// The device queue must be emptied (a seek).
    flush_sink: bool,
    volume_set: Option<u8>,
    ack_video: u32,
    ack_audio: u32,
    last_update_ms: u64,
    /// Pictures shown and dropped (late).
    pub presented: u32,
    pub dropped: u32,
}

impl<F> Player<F> {
    /// A player; `audio_out`: there is a sound device to play on.
    pub fn new(flow: Flow, audio_out: bool) -> Player<F> {
        Player {
            st: State::default(),
            flow,
            audio_out,
            clock: Clock::default(),
            position: 0,
            frames: VecDeque::new(),
            shown: None,
            show_next: true,
            frame_dur: 33_333_333,
            held_video: 0,
            pcm: VecDeque::new(),
            pcm_off: 0,
            held_audio: 0,
            audio_base: None,
            audio_written: 0,
            silence: 0,
            audio_end: i64::MIN,
            video_end: i64::MIN,
            sink_paused: false,
            flush_sink: false,
            volume_set: None,
            ack_video: 0,
            ack_audio: 0,
            last_update_ms: 0,
            presented: 0,
            dropped: 0,
        }
    }

    pub fn flow(&self) -> Flow {
        self.flow
    }

    /// The media time (`currentTime`), ns.
    pub fn position(&self) -> i64 {
        self.position
    }

    /// The picture on screen.
    pub fn shown(&self) -> Option<&F> {
        self.shown.as_ref().map(|q| &q.frame)
    }

    /// Time of the picture on screen.
    pub fn shown_pts(&self) -> Option<i64> {
        self.shown.as_ref().map(|q| q.pts)
    }

    pub fn queued_frames(&self) -> usize {
        self.frames.len()
    }

    /// Does the clock follow the sound?
    pub fn audio_clock(&self) -> bool {
        self.clock.on_audio()
    }

    /// Bytes the decoder may send again (pictures, sound), since the last call.
    pub fn take_acks(&mut self) -> (u32, u32) {
        (
            core::mem::take(&mut self.ack_video),
            core::mem::take(&mut self.ack_audio),
        )
    }

    /// Plays sound on the device: a sound track and a device.
    fn sound(&self) -> bool {
        self.audio_out && self.st.meta.is_some_and(|m| m.audio)
    }

    fn has_video(&self) -> bool {
        self.st.meta.is_some_and(|m| m.video)
    }

    // ---- what the decoder sends ----

    /// A source's header arrived: the tracks and the size are known.
    pub fn header(&mut self, h: Header) {
        self.st.header(h);
    }

    /// A picture for time `pts` (`bytes`: the size of its message, returned when it is let go).
    pub fn frame(&mut self, pts: i64, bytes: u32, frame: F) {
        if !self.st.current() {
            self.ack_video += bytes;
            return;
        }
        if let Some(last) = self.frames.back().map(|q| q.pts).or(self.shown_pts())
            && pts > last
            && pts - last <= 1_000_000_000
        {
            self.frame_dur = pts - last;
        }
        self.video_end = self.video_end.max(pts + self.frame_dur);
        self.held_video += bytes;
        self.frames.push_back(Queued { pts, bytes, frame });
    }

    /// Sound for time `pts`: stereo s16le samples (`bytes`: the size of the message).
    pub fn audio(&mut self, pts: i64, data: Vec<u8>, bytes: u32) {
        if !self.st.current() || !self.sound() {
            self.ack_audio += bytes;
            if self.st.current() {
                self.audio_end = self.audio_end.max(pts + frames_ns((data.len() / 4) as i64));
            }
            return;
        }
        self.audio_end = self.audio_end.max(pts + frames_ns((data.len() / 4) as i64));
        self.held_audio += bytes;
        self.pcm.push_back(Pcm { pts, data, bytes });
    }

    /// The decoder starts sending for seek `epoch`.
    pub fn mark(&mut self, epoch: u32) {
        self.st.mark(epoch);
    }

    /// Everything up to the end of the stream was sent (for seek `epoch`).
    pub fn drained(&mut self, epoch: u32) {
        if epoch == self.st.epoch && self.st.current() {
            self.st.drained = true;
        }
    }

    /// The decoder failed for good (`MEDIA_ERR_DECODE` and friends).
    pub fn fail(&mut self, code: u8) {
        self.st.fail(code);
    }

    // ---- what the page and the controls ask ----

    pub fn play(&mut self) {
        if self.st.ended && self.st.seeking.is_none() {
            self.seek_to(0);
        }
        self.st.play();
    }

    pub fn pause(&mut self) {
        self.st.pause();
    }

    /// Seek to `t`; returns the epoch to tell the decoder (`IN_SEEK`).
    pub fn seek(&mut self, t: i64) -> u32 {
        self.seek_to(t.max(0))
    }

    fn seek_to(&mut self, t: i64) -> u32 {
        for q in self.frames.drain(..) {
            self.ack_video += q.bytes;
        }
        self.held_video = 0;
        self.release_audio();
        self.flush_sink = true;
        self.audio_base = None;
        self.audio_written = 0;
        self.silence = 0;
        self.audio_end = i64::MIN;
        self.video_end = i64::MIN;
        self.show_next = true;
        self.clock.freeze(t);
        self.position = t;
        self.st.seek(t)
    }

    fn release_audio(&mut self) {
        for p in self.pcm.drain(..) {
            self.ack_audio += p.bytes;
        }
        self.pcm_off = 0;
        self.held_audio = 0;
    }

    /// No more data will come (`MediaSource.endOfStream`, or the whole file is in), or more will.
    pub fn set_eos(&mut self, on: bool) {
        self.st.eos = on;
        if !on {
            self.st.drained = false;
        }
    }

    /// The duration the page set (`MediaSource.duration`); `None` = unknown (infinite).
    pub fn set_duration(&mut self, d: Option<i64>) {
        self.st.set_duration(d);
    }

    pub fn set_loop(&mut self, on: bool) {
        self.st.looping = on;
    }

    /// Volume 0–100 and mute (the sound keeps flowing when muted, so the clock does too).
    pub fn set_volume(&mut self, volume: u8, muted: bool) {
        self.st.volume = volume.min(100);
        self.st.muted = muted;
    }

    // ---- every loop ----

    /// Feed the sound, move the clock, pick the picture, update the state. `sink` is the device
    /// stream (`None` without sound). Returns whether the picture on screen changed.
    pub fn step(&mut self, now_ms: u64, mut sink: Option<&mut dyn AudioSink>) -> bool {
        let sound = self.sound() && sink.is_some();
        if let Some(s) = sink.as_deref_mut() {
            self.sync_sink(s);
            if sound {
                self.feed(s);
            }
        }
        let played = sink.as_deref().map_or(0, |s| s.played());
        let ahead = self.audio_ahead(sink.as_deref());
        self.sync_clock(sound, now_ms, played);
        let t = self.clock.now(now_ms, played);
        let changed = self.pick(t, self.st.running());
        if self.st.seeking.is_none() {
            self.position = t;
        }
        self.update_state(now_ms, ahead, sink.as_deref());
        // The device and the clock follow the new state at once.
        if let Some(s) = sink.as_deref_mut() {
            self.sync_sink(s);
        }
        self.sync_clock(sound, now_ms, played);
        changed
    }

    /// Without sound the system clock runs only while the player does (with sound the clock
    /// follows the stream, which is paused when the player is not running).
    fn sync_clock(&mut self, sound: bool, now_ms: u64, played: u64) {
        if sound {
            return;
        }
        match (self.st.running(), self.clock.on_system()) {
            (true, false) => self.clock.start_system(self.position, now_ms),
            (false, true) => {
                let t = self.clock.now(now_ms, played);
                self.clock.freeze(t);
            }
            _ => {}
        }
    }

    /// Sound queued ahead of the clock, ns.
    fn audio_ahead(&self, sink: Option<&dyn AudioSink>) -> i64 {
        let waiting: i64 = self
            .pcm
            .iter()
            .map(|p| (p.data.len() / 4) as i64)
            .sum::<i64>()
            - (self.pcm_off / 4) as i64;
        let queued = sink.map_or(0, |s| s.queued() as i64);
        frames_ns(waiting + queued + self.silence)
    }

    /// Show the picture whose time has come. Returns whether the picture changed.
    fn pick(&mut self, t: i64, running: bool) -> bool {
        let mut changed = false;
        if self.show_next && self.st.marked() {
            if let Some(q) = self.frames.pop_front() {
                self.held_video -= q.bytes;
                self.put_on_screen(q);
                self.show_next = false;
                changed = true;
            }
        }
        if !running {
            return changed;
        }
        while self.frames.front().is_some_and(|q| q.pts <= t) {
            let Some(q) = self.frames.pop_front() else {
                break;
            };
            self.held_video -= q.bytes;
            if self.frames.front().is_some_and(|n| n.pts <= t) {
                // A later one is due too: this one is late.
                self.ack_video += q.bytes;
                self.dropped += 1;
                continue;
            }
            self.put_on_screen(q);
            changed = true;
        }
        changed
    }

    fn put_on_screen(&mut self, q: Queued<F>) {
        if let Some(old) = self.shown.replace(q) {
            self.ack_video += old.bytes;
        }
        self.presented += 1;
    }

    /// The decoder stream ended and everything was shown and heard.
    fn finished(&self, t: i64, sink: Option<&dyn AudioSink>) -> bool {
        if !(self.st.eos && self.st.drained && self.frames.is_empty() && self.pcm.is_empty()) {
            return false;
        }
        if self.sound() && sink.is_some() {
            let s = sink.map_or(0, |s| s.queued());
            let quiet = s == 0 && self.silence == 0;
            // Heard to the end: the clock reached the last sample (or the stream stopped there).
            return quiet && (t >= self.audio_end - 5_000_000 || self.audio_written == 0);
        }
        let end = self.video_end.max(self.audio_end);
        end == i64::MIN || t >= end
    }

    /// The state after this step: waiting, playing again, ended, ready state, `timeupdate`.
    fn update_state(&mut self, now_ms: u64, ahead: i64, sink: Option<&dyn AudioSink>) {
        let t = self.position;
        let sound = self.sound() && sink.is_some();
        // A seek is done when the new place has a picture (or, without video, sound), or nothing
        // is left to come.
        if self.st.seeking.is_some() && self.st.marked() {
            let there = if self.has_video() {
                !self.show_next
            } else {
                ahead > 0
            };
            if there || self.st.drained || self.st.meta.is_none() {
                self.st.seek_done();
            }
        }
        let video_ok = !self.has_video()
            || self.st.drained
            || self.frames.len() >= 3
            || self.held_video.saturating_mul(4) >= self.flow.video.saturating_mul(3);
        let audio_ok = !sound
            || self.st.drained
            || ahead >= START_AHEAD_NS
            || self.held_audio.saturating_mul(4) >= self.flow.audio.saturating_mul(3);
        let current = if self.has_video() {
            self.shown.is_some() && self.st.seeking.is_none()
        } else {
            self.st.meta.is_some() && (ahead > 0 || self.st.drained)
        };
        let enough = current && video_ok && audio_ok;
        // Running dry while playing.
        let dry = if sound {
            ahead <= 0 && !self.st.drained
        } else if self.has_video() {
            self.frames.is_empty() && !self.st.drained && t >= self.video_end
        } else {
            false
        };
        let finished = self.st.seeking.is_none() && self.finished(t, sink);
        let restart = self.st.step(enough, current, dry, finished);
        if restart {
            // `loop`: back to the start.
            self.seek_to(0);
        }
        if self.st.running() && now_ms.saturating_sub(self.last_update_ms) >= TIMEUPDATE_MS {
            self.last_update_ms = now_ms;
            self.st.push(Event::TimeUpdate);
        }
    }

    pub fn take_events(&mut self) -> Vec<Event> {
        core::mem::take(&mut self.st.events)
    }

    /// What the page sees.
    pub fn state(&self) -> &State {
        &self.st
    }
}
