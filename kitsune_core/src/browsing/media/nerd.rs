//! The statistics overlay of a playing video ("Estatísticas do vídeo", Shift+I over the video or
//! the page's context menu): what the browser measures of the playback, in the interface's
//! language, one line each. The browser fills [`NerdStats`] and draws [`NerdStats::lines`] over the
//! video; the person reads, on their own machine, the frame rate shown, how even it is, what was
//! dropped, the picture's lag behind the sound, the CPU and what is buffered.

use super::pacing::Spread;
use crate::i18n::Arg;
use crate::t;
use alloc::string::String;
use alloc::vec::Vec;

/// What the overlay shows. Times in µs, rates in thousandths.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct NerdStats {
    /// The video's own size and frame rate (from the times of its pictures).
    pub video_w: u32,
    pub video_h: u32,
    pub video_milli_fps: u32,
    /// The size the decoder converts the pictures to, and the box on screen.
    pub picture_w: u32,
    pub picture_h: u32,
    pub box_w: u32,
    pub box_h: u32,
    /// Pictures a second on screen over the last 2 s, and their intervals.
    pub shown_milli_fps: u32,
    pub spread: Spread,
    /// Pictures shown and dropped since the start (dropped by the decoder: part of `dropped`).
    pub shown: u32,
    pub dropped: u32,
    pub decoder_dropped: u32,
    /// Mean lag of the pictures behind the sound clock over the last 2 s (µs; negative: early).
    pub late_us: i32,
    /// CPU over the last second, percent: the decoder, the compositor, the page's scripts.
    pub cpu_decoder: u8,
    pub cpu_compositor: u8,
    pub cpu_page: u8,
    /// The decoding budget (pixels a second), 0 if not measured yet.
    pub budget: u64,
    /// Media buffered ahead of the position (ms) and pictures decoded and waiting.
    pub ahead_ms: u32,
    pub frames_ready: u32,
    /// The clock follows the sound (else the system clock).
    pub sound_clock: bool,
}

/// `v` thousandths with one decimal.
fn milli1(v: u32) -> Arg<'static> {
    Arg::Dec(i64::from(v / 100), 1)
}

/// `v` µs as ms with one decimal.
fn ms1(v: i64) -> Arg<'static> {
    Arg::Dec(v / 100, 1)
}

/// The name of a quality as YouTube gives it: the shorter side of the picture ("240p", "720p").
pub fn quality_name(w: u32, h: u32) -> String {
    alloc::format!("{}p", w.min(h))
}

impl NerdStats {
    /// The lines of the overlay, title first.
    pub fn lines(&self) -> Vec<String> {
        let s = &self.spread;
        let mut v = Vec::with_capacity(11);
        v.push(String::from(t!("media.stats.title")));
        v.push(t!(
            "media.stats.video",
            w = self.video_w,
            h = self.video_h,
            fps = milli1(self.video_milli_fps)
        ));
        v.push(t!(
            "media.stats.picture",
            w = self.picture_w,
            h = self.picture_h,
            bw = self.box_w,
            bh = self.box_h
        ));
        v.push(t!("media.stats.shown", fps = milli1(self.shown_milli_fps)));
        v.push(t!(
            "media.stats.interval",
            mean = ms1(i64::from(s.mean)),
            sd = ms1(i64::from(s.sd)),
            p95 = ms1(i64::from(s.p95)),
            max = ms1(i64::from(s.max))
        ));
        v.push(t!(
            "media.stats.dropped",
            n = Arg::Num(i64::from(self.dropped)),
            total = Arg::Num(i64::from(self.shown) + i64::from(self.dropped)),
            dec = Arg::Num(i64::from(self.decoder_dropped))
        ));
        v.push(t!("media.stats.sync", ms = ms1(i64::from(self.late_us))));
        v.push(t!(
            "media.stats.cpu",
            dec = self.cpu_decoder,
            comp = self.cpu_compositor,
            page = self.cpu_page
        ));
        let q = quality_name(self.video_w, self.video_h);
        v.push(if self.budget > 0 {
            t!(
                "media.stats.quality",
                q = &q,
                mpx = Arg::Dec((self.budget / 100_000) as i64, 1)
            )
        } else {
            t!("media.stats.quality_unmeasured", q = &q)
        });
        v.push(t!(
            "media.stats.buffer",
            s = Arg::Dec(i64::from(self.ahead_ms / 100), 1),
            frames = self.frames_ready
        ));
        v.push(String::from(if self.sound_clock {
            t!("media.stats.clock_sound")
        } else {
            t!("media.stats.clock_system")
        }));
        v
    }
}
