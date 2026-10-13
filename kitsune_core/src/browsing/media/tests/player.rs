use crate::browsing::media::clock::{AudioSink, Clock, frames_ns, ns_frames};
use crate::browsing::media::player::*;
use crate::browsing::media::proto::Header;
use alloc::vec;
use alloc::vec::Vec;

const MS: i64 = 1_000_000;

/// A sound device stream: a queue of `cap` frames, played `latency` frames after being consumed.
struct Sink {
    queued: u64,
    cap: u64,
    consumed: u64,
    latency: u64,
    paused: bool,
    volume: u8,
    flushes: u32,
    /// Every frame value written, for checking gaps (only the first sample of each frame).
    first_samples: Vec<i16>,
}

impl Sink {
    fn new() -> Sink {
        Sink {
            queued: 0,
            cap: 48_000,
            consumed: 0,
            latency: 0,
            paused: false,
            volume: 100,
            flushes: 0,
            first_samples: Vec::new(),
        }
    }

    /// The device runs for `frames` frames.
    fn run(&mut self, frames: u64) {
        if !self.paused {
            let k = frames.min(self.queued);
            self.queued -= k;
            self.consumed += k;
        }
    }
}

impl AudioSink for Sink {
    fn write(&mut self, bytes: &[u8]) -> usize {
        let n = ((bytes.len() / 4) as u64).min(self.cap - self.queued);
        for f in 0..n as usize {
            self.first_samples
                .push(i16::from_le_bytes([bytes[f * 4], bytes[f * 4 + 1]]));
        }
        self.queued += n;
        n as usize
    }
    fn played(&self) -> u64 {
        self.consumed.saturating_sub(self.latency)
    }
    fn consumed(&self) -> u64 {
        self.consumed
    }
    fn queued(&self) -> u64 {
        self.queued
    }
    fn set_paused(&mut self, p: bool) {
        self.paused = p;
    }
    fn flush(&mut self) {
        self.queued = 0;
        self.flushes += 1;
    }
    fn set_volume(&mut self, v: u8) {
        self.volume = v;
    }
}

fn header(video: bool, audio: bool) -> Header {
    Header {
        duration_ns: 3_000_000_000,
        width: 640,
        height: 360,
        video,
        audio,
        channels: 2,
        src: 0,
    }
}

/// `ms` of sound starting at `pts`, every sample = `value`.
fn pcm(ms: i64, value: i16) -> Vec<u8> {
    let frames = ns_frames(ms * MS) as usize;
    let mut v = Vec::with_capacity(frames * 4);
    for _ in 0..frames * 2 {
        v.extend_from_slice(&value.to_le_bytes());
    }
    v
}

fn names(p: &mut Player<u32>) -> Vec<&'static str> {
    p.take_events().iter().map(|e| e.name()).collect()
}

/// Video 30 fps frames numbered by their index, and 20 ms sound packets, for `ms` from `from_ms`.
fn feed(p: &mut Player<u32>, from_ms: i64, ms: i64) {
    let mut t = from_ms;
    let mut next_frame = (from_ms * 30 + 999) / 1000;
    while t < from_ms + ms {
        while next_frame * 1000 / 30 < t + 20 {
            p.frame(next_frame * 1_000_000_000 / 30, 1000, next_frame as u32);
            next_frame += 1;
        }
        p.audio(t * MS, pcm(20, 1), 3840);
        t += 20;
    }
}

#[test]
fn metadata_data_and_play_events_in_order() {
    let mut p: Player<u32> = Player::new(Flow::default(), true);
    let mut s = Sink::new();
    p.step(0, Some(&mut s));
    assert_eq!(p.state().ready, HAVE_NOTHING);
    p.header(header(true, true));
    p.step(0, Some(&mut s));
    assert_eq!(names(&mut p), ["durationchange", "loadedmetadata"]);
    assert_eq!(p.state().duration, Some(3_000_000_000));
    feed(&mut p, 0, 500);
    p.step(1, Some(&mut s));
    assert_eq!(names(&mut p), ["loadeddata", "canplay", "canplaythrough"]);
    assert_eq!(p.state().ready, HAVE_ENOUGH_DATA);
    assert_eq!(
        p.shown(),
        Some(&0),
        "the first picture is shown before playing"
    );
    assert!(s.paused, "the device waits while paused");
    p.play();
    p.step(2, Some(&mut s));
    assert_eq!(names(&mut p), ["play", "playing"]);
    assert!(!s.paused);
}

#[test]
fn the_picture_follows_the_sound_clock() {
    let mut p: Player<u32> = Player::new(Flow::default(), true);
    let mut s = Sink::new();
    p.header(header(true, true));
    feed(&mut p, 0, 1000);
    p.play();
    p.step(0, Some(&mut s));
    assert!(p.audio_clock());
    // 100 ms of sound played: the frame of 0.1 s (frame 3) is on screen.
    s.run(4800);
    p.step(10, Some(&mut s));
    assert_eq!(p.position(), 100 * MS);
    assert_eq!(p.shown(), Some(&3));
    // The device is late by 200 ms: the clock is what was heard, not what was written.
    s.latency = 9600;
    s.run(9600);
    p.step(20, Some(&mut s));
    assert_eq!(p.position(), 100 * MS);
    assert_eq!(p.shown(), Some(&3));
    // Then the picture jumps ahead and the late ones are dropped.
    s.latency = 0;
    p.step(30, Some(&mut s));
    assert_eq!(p.position(), 300 * MS);
    assert_eq!(p.shown(), Some(&9));
    assert!(p.dropped >= 5);
    let (v, a) = p.take_acks();
    assert!(v >= 9 * 1000 && a > 0);
}

#[test]
fn a_thin_buffer_that_keeps_up_does_not_flap() {
    let mut p: Player<u32> = Player::new(Flow::default(), true);
    let mut s = Sink::new();
    p.header(header(true, true));
    feed(&mut p, 0, 600);
    p.play();
    p.step(0, Some(&mut s));
    p.take_events();
    // Play on with only a little ahead each time (less than "enough", never dry).
    for k in 0..20 {
        s.run(960);
        feed(&mut p, 600 + k * 20, 20);
        p.step(10 + k as u64, Some(&mut s));
    }
    let ev = names(&mut p);
    assert!(
        !ev.contains(&"canplay") && !ev.contains(&"waiting"),
        "{ev:?}"
    );
    assert!(p.state().ready >= HAVE_FUTURE_DATA);
}

#[test]
fn pause_stops_the_clock_and_play_resumes() {
    let mut p: Player<u32> = Player::new(Flow::default(), true);
    let mut s = Sink::new();
    p.header(header(true, true));
    feed(&mut p, 0, 1000);
    p.play();
    p.step(0, Some(&mut s));
    s.run(4800);
    p.step(10, Some(&mut s));
    p.pause();
    p.step(11, Some(&mut s));
    assert!(s.paused);
    s.run(48_000);
    p.step(1000, Some(&mut s));
    assert_eq!(p.position(), 100 * MS, "nothing played while paused");
    p.take_events();
    p.play();
    p.step(1001, Some(&mut s));
    assert!(!s.paused);
    assert_eq!(names(&mut p)[..2], ["play", "playing"]);
}

#[test]
fn seek_flushes_waits_for_the_mark_and_shows_the_new_place() {
    let mut p: Player<u32> = Player::new(Flow::default(), true);
    let mut s = Sink::new();
    p.header(header(true, true));
    feed(&mut p, 0, 600);
    p.play();
    p.step(0, Some(&mut s));
    s.run(4800);
    p.step(10, Some(&mut s));
    p.take_events();
    let epoch = p.seek(2000 * MS);
    assert_eq!(epoch, 1);
    p.step(11, Some(&mut s));
    assert_eq!(s.flushes, 1);
    assert_eq!(p.position(), 2000 * MS);
    assert!(p.state().seeking.is_some());
    // Old pictures still on their way are dropped (and returned to the decoder).
    p.take_acks();
    p.frame(400 * MS, 1000, 400);
    p.step(12, Some(&mut s));
    assert_eq!(p.take_acks().0, 1000);
    assert_ne!(p.shown(), Some(&400));
    // The decoder marks the new epoch, then sends the new place.
    p.mark(epoch);
    feed(&mut p, 2000, 600);
    p.step(13, Some(&mut s));
    assert_eq!(p.shown(), Some(&60));
    let ev = names(&mut p);
    assert_eq!(ev[..3], ["seeking", "timeupdate", "seeked"]);
    assert!(ev.contains(&"playing"));
    s.run(4800);
    p.step(20, Some(&mut s));
    assert_eq!(p.position(), 2100 * MS);
    assert_eq!(p.shown(), Some(&63));
}

#[test]
fn running_dry_fires_waiting_and_resumes_with_playing() {
    let mut p: Player<u32> = Player::new(Flow::default(), true);
    let mut s = Sink::new();
    p.header(header(true, true));
    feed(&mut p, 0, 400);
    p.play();
    p.step(0, Some(&mut s));
    p.take_events();
    s.run(48_000);
    p.step(500, Some(&mut s));
    assert!(p.state().waiting);
    assert!(names(&mut p).contains(&"waiting"));
    assert_eq!(
        p.position(),
        400 * MS,
        "the clock stops where the sound ran out"
    );
    // A little data is not enough to start again; enough is.
    feed(&mut p, 400, 100);
    p.step(501, Some(&mut s));
    assert!(p.state().waiting);
    feed(&mut p, 500, 400);
    p.step(502, Some(&mut s));
    assert!(!p.state().waiting);
    assert!(names(&mut p).contains(&"playing"));
}

#[test]
fn the_end_fires_pause_and_ended_or_loops() {
    for looping in [false, true] {
        let mut p: Player<u32> = Player::new(Flow::default(), true);
        let mut s = Sink::new();
        p.header(header(true, true));
        p.set_loop(looping);
        feed(&mut p, 0, 400);
        p.set_eos(true);
        p.drained(0);
        p.play();
        p.step(0, Some(&mut s));
        p.take_events();
        s.run(48_000);
        p.step(500, Some(&mut s));
        let ev = names(&mut p);
        if looping {
            assert!(!p.state().ended);
            assert!(ev.contains(&"seeking"), "{ev:?}");
            assert_eq!(p.position(), 0);
        } else {
            assert!(p.state().ended && p.state().paused);
            assert_eq!(ev[ev.len() - 2..], ["pause", "ended"]);
            assert_eq!(p.position(), 400 * MS);
            // play() after the end starts over.
            p.play();
            assert_eq!(p.state().seeking, Some(0));
        }
    }
}

#[test]
fn gaps_in_the_sound_are_filled_with_silence_and_overlaps_cut() {
    let mut p: Player<u32> = Player::new(Flow::default(), true);
    let mut s = Sink::new();
    p.header(header(false, true));
    p.audio(0, pcm(100, 1), 1);
    p.audio(200 * MS, pcm(100, 2), 1); // 100 ms gap
    p.audio(250 * MS, pcm(100, 3), 1); // 50 ms overlap
    p.play();
    p.step(0, Some(&mut s));
    let n = |ms: i64| ns_frames(ms * MS) as usize;
    assert_eq!(s.first_samples.len(), n(350));
    assert!(s.first_samples[..n(100)].iter().all(|&v| v == 1));
    assert!(s.first_samples[n(100)..n(200)].iter().all(|&v| v == 0));
    assert!(s.first_samples[n(200)..n(300)].iter().all(|&v| v == 2));
    assert!(s.first_samples[n(300)..].iter().all(|&v| v == 3));
}

#[test]
fn without_a_device_the_system_clock_runs_and_stops_when_pictures_run_out() {
    let mut p: Player<u32> = Player::new(Flow::default(), false);
    p.header(header(true, true));
    feed(&mut p, 0, 1000);
    let (_, audio_acked) = p.take_acks();
    assert!(audio_acked > 0, "sound is dropped at once");
    p.play();
    p.step(1000, None);
    assert!(!p.audio_clock());
    p.step(1500, None);
    assert_eq!(p.position(), 500 * MS);
    assert_eq!(p.shown(), Some(&15));
    p.take_events();
    p.step(3000, None);
    assert!(p.state().waiting, "no pictures left");
    let pos = p.position();
    p.step(4000, None);
    assert_eq!(p.position(), pos, "the clock waits");
}

#[test]
fn volume_and_mute_go_to_the_device_and_mute_keeps_the_clock() {
    let mut p: Player<u32> = Player::new(Flow::default(), true);
    let mut s = Sink::new();
    p.header(header(false, true));
    p.audio(0, pcm(1000, 1), 1);
    p.set_volume(40, false);
    p.play();
    p.step(0, Some(&mut s));
    assert_eq!(s.volume, 40);
    p.set_volume(40, true);
    p.step(1, Some(&mut s));
    assert_eq!(s.volume, 0);
    s.run(4800);
    p.step(2, Some(&mut s));
    assert_eq!(p.position(), 100 * MS);
}

#[test]
fn the_page_duration_wins_and_errors_stop() {
    let mut p: Player<u32> = Player::new(Flow::default(), true);
    p.set_duration(Some(10_000 * MS));
    p.header(header(true, false));
    assert_eq!(p.state().duration, Some(10_000 * MS));
    p.set_duration(None);
    assert_eq!(p.state().duration, None);
    p.fail(MEDIA_ERR_DECODE);
    assert_eq!(p.state().error, Some(MEDIA_ERR_DECODE));
    assert!(names(&mut p).contains(&"error"));
}

#[test]
fn clock_arithmetic() {
    assert_eq!(frames_ns(48_000), 1_000_000_000);
    assert_eq!(frames_ns(24), 500_000);
    assert_eq!(ns_frames(1_000_000_000), 48_000);
    assert_eq!(ns_frames(frames_ns(123_456_789)), 123_456_789);
    let mut c = Clock::at(5);
    assert_eq!(c.now(100, 100), 5);
    c.start_system(1000, 10);
    assert_eq!(c.now(20, 0), 1000 + 10 * MS);
    c.follow_audio(2000, 480);
    assert_eq!(c.now(0, 480 + 48), 2000 + MS);
    assert_eq!(
        c.now(0, 100),
        2000,
        "before its anchor the sound clock waits"
    );
    let _ = vec![0u8];
}
