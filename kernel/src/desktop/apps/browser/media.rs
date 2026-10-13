//! `<video>` and `<audio>`: the players of a page.
//!
//! The page's script process (`media.js`) has the JavaScript API and fetches the bytes; each element
//! that plays gets a [`MediaPlayer`] here, with its decoder process (`mediadec --serve`, started with
//! `userproc::spawn_media`) and, when there is a sound device, a stream of the kernel's mixer
//! (`crate::audio`, used directly, owned by the decoder's pid so it closes with it). Every loop of the
//! compositor [`MediaSet::pump`] takes what the decoder sent, steps the player (its clock follows the
//! stream's "played" counter, see `kitsune_core::media::player`), returns the budget to the decoder
//! and tells the page what happened. The picture on screen is painted into the element's box by
//! `paint.rs`; the controls the browser draws are in `controls.rs`.
//!
//! The players live with the page's script process ([`super::script::JsProc`]): another page, a
//! closed tab or a dead process ends them (and their decoders). A tab in the background pauses them.

use super::script::JsProc;
use crate::desktop::*;
use alloc::collections::VecDeque;
use kitsune_core::media::clock::AudioSink;
use kitsune_core::media::player::{Flow, MEDIA_ERR_NETWORK, Player};
use kitsune_core::media::proto::{self, Field, Out, parse_out};
use kitsune_core::media::wire::{self, Command, Control};

mod controls;
mod direct;
mod paint;
mod stats;
pub(crate) use direct::NetCtx;
pub(crate) use paint::video_covers;

/// Players playing at once in a page (each is a decoder process with up to 128 MiB).
pub(crate) const MAX_PLAYERS: usize = 2;
/// Messages taken from one decoder per loop (the budget bounds what can be waiting anyway).
const MAX_TAKE: usize = 48;
/// How often the decoder hears the playback position (what it may forget behind it, and which
/// pictures are already late).
const TIME_EVERY_MS: u64 = 250;
/// How often the log hears how a playing player goes.
const LOG_EVERY_MS: u64 = 1000;
/// The budget of a player: pictures (a few frames of a 720p box) and sound (~2.7 s).
const FLOW: Flow = Flow {
    video: 8 << 20,
    audio: 512 << 10,
};

/// The milliseconds since boot.
pub(super) fn now_ms() -> u64 {
    crate::interrupts::ticks() * 1000 / u64::from(crate::interrupts::TIMER_HZ)
}

/// The players of one page.
#[derive(Default)]
pub(crate) struct MediaSet {
    pub players: Vec<MediaPlayer>,
}

/// One element's player.
pub(crate) struct MediaPlayer {
    /// The element ([`kitsune_core::web::Element::uid`]).
    pub uid: u32,
    /// The decoder process.
    pub pid: u32,
    /// The page's script process (for the log's CPU figures).
    js: u32,
    pub player: Player<Vec<u8>>,
    /// The mixer stream (owner: the decoder's pid).
    stream: Option<u32>,
    /// The picture size the decoder was asked for.
    size: (u32, u32),
    /// Playback started once: the poster gives way to the picture.
    pub started: bool,
    last_time_ms: u64,
    last_log_ms: u64,
    /// The clock ran when the decoder last heard it.
    told_running: bool,
    /// Bytes for the decoder that did not fit in its queue yet.
    backlog: VecDeque<Vec<u8>>,
    /// Where the current stretch of playback began (for the log).
    mark: Option<stats::Mark>,
    /// The download of a `src` address.
    direct: Option<direct::Direct>,
}

impl Drop for MediaPlayer {
    fn drop(&mut self) {
        crate::fetch::forget_script_fetches(self.pid);
        if let Some(id) = self.stream.take() {
            crate::audio::with(|s| {
                let _ = s.mixer.close(self.pid, id, false);
            });
        }
        crate::userproc::kill(self.pid);
    }
}

/// The mixer stream of a player, as its clock and sound output.
struct MixerSink {
    owner: u32,
    id: u32,
}

impl MixerSink {
    fn status(&self) -> Option<kitsune_core::hw::audio::StreamStatus> {
        crate::audio::with(|s| s.mixer.status(self.owner, self.id).ok()).flatten()
    }
}

impl AudioSink for MixerSink {
    fn write(&mut self, bytes: &[u8]) -> usize {
        let n = crate::audio::with(|s| s.mixer.write(self.owner, self.id, bytes).unwrap_or(0))
            .unwrap_or(0);
        if n > 0 {
            crate::audio::kick();
        }
        n / 4
    }
    fn played(&self) -> u64 {
        self.status().map_or(0, |s| s.played)
    }
    fn consumed(&self) -> u64 {
        self.status().map_or(0, |s| s.consumed)
    }
    fn queued(&self) -> u64 {
        self.status().map_or(0, |s| s.queued_bytes / 4)
    }
    fn set_paused(&mut self, p: bool) {
        let _ = crate::audio::with(|s| s.mixer.set_paused(self.owner, self.id, p));
        crate::audio::kick();
    }
    fn flush(&mut self) {
        let _ = crate::audio::with(|s| s.mixer.flush(self.owner, self.id));
    }
    fn set_volume(&mut self, v: u8) {
        let _ = crate::audio::with(|s| s.mixer.set_volume(self.owner, self.id, u32::from(v)));
    }
}

/// What a pump changed.
#[derive(Default)]
pub(crate) struct Pumped {
    /// A picture changed: the page area must be painted again.
    pub redraw: bool,
    /// A video learnt its size: the page must be laid out again.
    pub relayout: bool,
}

impl MediaPlayer {
    fn post(&mut self, msg: Vec<u8>) {
        if !self.backlog.is_empty() {
            self.backlog.push_back(msg);
            return;
        }
        match crate::userproc::page_try_post(self.pid, msg) {
            Ok(()) => {}
            Err(crate::userproc::PostError::Full(m)) => self.backlog.push_back(m),
            Err(crate::userproc::PostError::Gone) => {}
        }
    }

    /// Send what waited for room in the decoder's queue.
    fn flush_backlog(&mut self) {
        while let Some(m) = self.backlog.pop_front() {
            match crate::userproc::page_try_post(self.pid, m) {
                Ok(()) => {}
                Err(crate::userproc::PostError::Full(m)) => {
                    self.backlog.push_front(m);
                    return;
                }
                Err(crate::userproc::PostError::Gone) => {
                    self.backlog.clear();
                    return;
                }
            }
        }
    }

    fn sink(&self) -> Option<MixerSink> {
        self.stream.map(|id| MixerSink {
            owner: self.pid,
            id,
        })
    }
}

impl MediaSet {
    fn find(&mut self, uid: u32) -> Option<&mut MediaPlayer> {
        self.players.iter_mut().find(|p| p.uid == uid)
    }

    /// A command from the page (`js`: its script process, for the answers).
    pub(crate) fn command(&mut self, js: u32, c: Command) {
        if c.control == Control::Open {
            self.open(js, c.id);
            return;
        }
        if c.control == Control::Close {
            self.players.retain(|p| p.uid != c.id);
            return;
        }
        let Some(p) = self.find(c.id) else { return };
        match c.control {
            Control::Open | Control::Close => {}
            Control::Src(url) => {
                crate::klog!(Info, "media: element {} plays {url}", p.uid);
                p.direct = Some(direct::Direct::new(url));
            }
            Control::Play => {
                p.player.play();
                p.started = true;
            }
            Control::Pause => p.player.pause(),
            Control::Seek(t) => {
                p.player.seek(t);
            }
            Control::Volume(v, m) => p.player.set_volume(v, m),
            Control::Loop(on) => p.player.set_loop(on),
            Control::EndOfStream(on) => {
                p.player.set_eos(on);
                p.post(proto::msg(proto::IN_EOS, &[Field::U8(u8::from(on))]));
            }
            Control::Duration(d) => p.player.set_duration(d),
            Control::Remove { src, a, b } => p.post(proto::msg(
                proto::IN_REMOVE,
                &[Field::U8(src), Field::I64(a), Field::I64(b)],
            )),
            Control::Reset { src, offset } => p.post(proto::msg(
                proto::IN_RESET,
                &[Field::U8(src), Field::U64(offset)],
            )),
            Control::TimestampOffset { src, ns } => p.post(proto::msg(
                proto::IN_TSOFF,
                &[Field::U8(src), Field::I64(ns)],
            )),
            Control::Cue { src, t } => {
                p.post(proto::msg(proto::IN_CUE, &[Field::U8(src), Field::I64(t)]))
            }
        }
    }

    /// WebM bytes for source `src` of player `id`.
    pub(crate) fn data(&mut self, js: u32, id: u32, src: u8, seq: u32, bytes: &[u8]) {
        match self.find(id) {
            Some(p) => {
                // MSE appends are few (a segment of seconds each, in pieces of 1 MiB at most):
                // each one in the log says what reached the decoder.
                crate::klog!(
                    Info,
                    "media: element {id} source {src}: {} bytes appended (piece {seq})",
                    bytes.len()
                );
                p.post(proto::data(src, seq, bytes))
            }
            None => {
                let m = wire::buffered_message(id, src, seq, proto::BUF_BROKEN, &[]);
                crate::userproc::page_post(js, m);
            }
        }
    }

    /// A new player for element `uid`.
    fn open(&mut self, js: u32, uid: u32) {
        if self.find(uid).is_some() {
            return;
        }
        if self.players.len() >= MAX_PLAYERS {
            crate::klog!(
                Info,
                "browser: no free player for a media element (at most {MAX_PLAYERS})"
            );
            let m = wire::error_message(uid, 4, "too many players playing at once");
            crate::userproc::page_post(js, m);
            return;
        }
        let pid = match crate::userproc::spawn_media(crate::desktop::services::accounts::cred()) {
            Ok(pid) => pid,
            Err(e) => {
                crate::klog!(Warn, "browser: no media decoder ({:?})", e);
                let m = wire::error_message(uid, 4, "no media decoder");
                crate::userproc::page_post(js, m);
                return;
            }
        };
        let audio_out = crate::audio::present();
        crate::klog!(
            Info,
            "browser: player for element {uid}: decoder pid {pid}, sound {}",
            if audio_out {
                "on"
            } else {
                "off (no device): system clock"
            }
        );
        let mut p = MediaPlayer {
            uid,
            pid,
            js,
            player: Player::new(FLOW, audio_out),
            stream: None,
            size: (0, 0),
            started: false,
            last_time_ms: 0,
            last_log_ms: 0,
            told_running: false,
            backlog: VecDeque::new(),
            mark: None,
            direct: None,
        };
        p.post(proto::msg(
            proto::IN_FLOW,
            &[Field::U32(FLOW.video), Field::U32(FLOW.audio)],
        ));
        // Small pictures until the element's box is known.
        p.size = (160, 90);
        p.post(proto::msg(
            proto::IN_SIZE,
            &[Field::U32(160), Field::U32(90)],
        ));
        self.players.push(p);
    }

    /// Pause every player (the tab went to the background).
    pub(crate) fn pause_all(&mut self) {
        for p in &mut self.players {
            p.player.pause();
        }
    }

    /// Take what the decoders sent, step the players, answer the page. `js` is the page's script
    /// process; `page` its layout (the boxes the pictures go to), `doc` its document (the videos'
    /// own sizes).
    pub(crate) fn pump(
        &mut self,
        js: u32,
        net: &mut NetCtx,
        page: Option<&kitsune_core::web::Page>,
        doc: Option<&mut kitsune_core::web::Doc>,
    ) -> Pumped {
        let mut out = Pumped::default();
        let now = now_ms();
        let mut doc = doc;
        // A decoder that died ends its player; the page hears it as a decode error.
        self.players.retain(|p| {
            let alive = crate::userproc::end(p.pid) == crate::userproc::End::Running;
            if !alive {
                crate::klog!(
                    Warn,
                    "browser: the media decoder of element {} ended",
                    p.uid
                );
                let m = wire::error_message(p.uid, 3, "the decoder ended");
                crate::userproc::page_post(js, m);
            }
            alive
        });
        for p in &mut self.players {
            p.flush_backlog();
            p.pump_direct(net);
            for _ in 0..MAX_TAKE {
                let Some(m) = crate::userproc::page_take(p.pid) else {
                    break;
                };
                if take(p, js, m, &mut doc) {
                    out.relayout = true;
                }
            }
            let mut sink = p.sink();
            let changed = p
                .player
                .step(now, sink.as_mut().map(|s| s as &mut dyn AudioSink));
            out.redraw |= changed;
            // A seek (the page's, the controls', play() after the end, `loop`) goes to the decoder.
            if let Some((epoch, t)) = p.player.take_seek() {
                p.post(proto::msg(
                    proto::IN_SEEK,
                    &[Field::U32(epoch), Field::I64(t)],
                ));
            }
            let (v, a) = p.player.take_acks();
            if v > 0 || a > 0 {
                p.post(proto::msg(proto::IN_ACK, &[Field::U32(v), Field::U32(a)]));
            }
            let running = p.player.state().running();
            if now.saturating_sub(p.last_time_ms) >= TIME_EVERY_MS || running != p.told_running {
                p.last_time_ms = now;
                p.told_running = running;
                let t = p.player.position();
                p.post(proto::msg(
                    proto::IN_TIME,
                    &[Field::I64(t), Field::U8(u8::from(running))],
                ));
            }
            if now.saturating_sub(p.last_log_ms) >= LOG_EVERY_MS {
                p.last_log_ms = now;
                // While it is supposed to play, how it goes (once a second).
                if !p.player.state().paused {
                    let sink = p.sink().map_or((0, 0), |s| (s.queued(), s.played()));
                    crate::klog!(
                        Info,
                        "media: element {} {} sink q={} played={} backlog={}",
                        p.uid,
                        p.player.describe(),
                        sink.0,
                        sink.1,
                        p.backlog.len()
                    );
                }
            }
            // The picture size: the element's box, the video's shape kept.
            if let Some(b) = page.and_then(|pg| pg.videos.iter().find(|v| v.uid == p.uid)) {
                let want = picture_size(p, b.w, b.h);
                if want != p.size && want.0 > 0 {
                    p.size = want;
                    p.post(proto::msg(
                        proto::IN_SIZE,
                        &[
                            Field::U32(want.0),
                            Field::U32(want.1),
                            Field::U32(u32::from(paint::fb_bgr())),
                        ],
                    ));
                }
            }
            let events = p.player.take_events();
            p.note(&events);
            if !events.is_empty() {
                let m = wire::state_message(
                    p.uid,
                    p.player.state(),
                    p.player.position(),
                    &events,
                    p.player.quality(),
                );
                crate::userproc::page_post(js, m);
            }
        }
        out
    }
}

/// The size of the pictures for a box of `w` x `h`: the video fitted in it.
fn picture_size(p: &MediaPlayer, w: i32, h: i32) -> (u32, u32) {
    let (vw, vh) = p
        .player
        .state()
        .meta
        .map_or((0, 0), |m| (m.width, m.height));
    let a = kitsune_core::media::controls::contain(w, h, vw, vh);
    (
        (a.w.max(1) as u32).min(proto::MAX_W),
        (a.h.max(1) as u32).min(proto::MAX_H),
    )
}

/// One message from the decoder of `p`. Returns whether the page must be laid out again.
fn take(
    p: &mut MediaPlayer,
    js: u32,
    m: Vec<u8>,
    doc: &mut Option<&mut kitsune_core::web::Doc>,
) -> bool {
    let len = m.len() as u32;
    let Some(o) = parse_out(&m) else {
        return false;
    };
    match o {
        Out::Header(h) => {
            p.player.header(h);
            if h.audio && p.stream.is_none() && crate::audio::present() {
                p.stream = crate::audio::with(|s| s.mixer.open(p.pid, 48_000, 2).ok()).flatten();
            }
            if h.video
                && let Some(d) = doc.as_deref_mut()
            {
                return d.set_media_size(p.uid, h.width, h.height);
            }
        }
        Out::Frame { pts, skipped, .. } => {
            p.player.decoder_dropped(u32::from(skipped));
            p.player.frame(pts, len, m);
        }
        Out::Audio { pts, bytes, .. } => {
            let data = bytes.to_vec();
            p.player.audio(pts, data, len);
        }
        Out::Buffered {
            src,
            seq,
            status,
            ranges,
        } => {
            if src == 0
                && seq == 0
                && let Some(d) = p.direct.as_mut()
            {
                d.ranges.clone_from(&ranges);
            }
            let msg = wire::buffered_message(p.uid, src, seq, status, &ranges);
            crate::userproc::page_post(js, msg);
        }
        Out::Drained { epoch } => p.player.drained(epoch),
        Out::Mark { epoch } => p.player.mark(epoch),
        Out::Cue { src, t, offset } => {
            let msg = wire::cue_message(p.uid, src, t, (offset != u64::MAX).then_some(offset));
            crate::userproc::page_post(js, msg);
        }
        Out::Fetch {
            id,
            ev,
            status,
            total,
            bytes,
        } => p.fetched(id, ev, status, total, bytes),
        Out::Error { code, text } => {
            crate::klog!(Info, "browser: media decoder of element {}: {text}", p.uid);
            // No codec or no memory ends the player; a bad frame does not, and broken bytes are
            // the page's to handle (the append says so).
            if code == proto::ERR_CODEC || code == proto::ERR_MEMORY {
                p.player
                    .fail(kitsune_core::media::player::MEDIA_ERR_SRC_NOT_SUPPORTED);
            }
        }
    }
    false
}

impl JsProc {
    /// The player of element `uid`, if it plays.
    pub(crate) fn media_player(&self, uid: u32) -> Option<&MediaPlayer> {
        self.media.players.iter().find(|p| p.uid == uid)
    }
}
