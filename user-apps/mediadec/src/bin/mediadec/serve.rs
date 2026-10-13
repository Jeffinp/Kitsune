//! The message server (`mediadec --serve`): the decoder of one `<video>`/`<audio>` element of the
//! browser. The player sends WebM bytes for up to four source buffers (Media Source Extensions: video
//! and audio usually come in separate ones) and gets back RGBA pictures and PCM. Each message is one
//! `page_recv`/`page_send` (calls 56/57, the link of `jsrun --page`); the protocol is
//! `kitsune_core::media::proto` and the table is in `docs/design/midia.md`.
//!
//! What arrives is demuxed at once into **track buffers** of coded frames (the MSE model): the
//! buffered ranges are known right away (the answer to each append), `remove` and eviction work on
//! frames, and a seek inside what is buffered needs nothing new. Decoding runs ahead of the player
//! within the budget it gives (`IN_FLOW`, paid back with `IN_ACK`): the sound first (it is small and
//! the player's clock), the pictures as far as the budget allows, in time order between the two.
//!
//! Shared by `mediadec` and the host checker; [`Loopback`] and [`selftest`] run it in memory.

use super::ffi::{OPUS_MAX_SAMPLES, Opus, Vp9};
use alloc::collections::VecDeque;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;
use core::fmt::Write as _;
use kitsune_core::media::proto::{self, Field};
use kitsune_core::media::track::{Cursor, TrackBuffer};
use kitsune_core::webm::{Demuxer, Event};

// Loaded with `#[path]` (by `mediadec` and the host checker): the children are named the same way.
#[path = "serve/decode.rs"]
mod decode;
#[path = "serve/selftest.rs"]
mod selftest;
#[path = "serve/sources.rs"]
mod sources;
pub use selftest::*;
use sources::Source;

/// Coded data kept for all sources before what was played long ago is forgotten.
const EVICT_BYTES: usize = 24 << 20;
/// Most coded data kept: an append beyond it (after eviction) is refused (`QuotaExceededError`).
const MAX_BYTES: usize = 48 << 20;
/// What eviction keeps behind the playback position.
const KEEP_BEHIND_NS: i64 = 10_000_000_000;
/// Opus needs this much sound before a seek target to settle (RFC 7845 §4.6: 80 ms).
const PRE_ROLL_NS: i64 = 80_000_000;

/// What a send did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Sent {
    Done,
    /// The player's queue is full: try again later.
    Full,
    /// The player is gone.
    Gone,
}

/// The two directions of messages.
pub trait Link {
    /// Wait up to `timeout_ms` (`u64::MAX`: no limit, 0: just look) for the next message and put it
    /// in `buf`. `Ok(Some(len))`, `Ok(None)` when nothing came, `Err(())` when the link is gone.
    fn recv(&mut self, buf: &mut [u8], timeout_ms: u64) -> Result<Option<usize>, ()>;
    /// Send one message.
    fn send(&mut self, msg: &[u8]) -> Sent;
    /// Milliseconds since some start (for the player's clock between its `IN_TIME`s).
    fn now_ms(&self) -> u64 {
        0
    }
}

/// The video decoder, bound to the source that has the video track.
struct VideoDec {
    src: usize,
    dec: Vp9,
    cursor: Cursor,
}

/// The sound decoder, bound to the source that has the sound track.
struct AudioDec {
    src: usize,
    dec: Opus,
    cursor: Cursor,
    /// Samples still to drop at the start of the stream (`pre-skip`).
    skip: usize,
    /// The pre-skip, as the time to take off every timestamp (Matroska `CodecDelay`).
    delay_ns: i64,
}

/// The server's state.
pub struct Server {
    sources: Vec<Source>,
    video: Option<VideoDec>,
    audio: Option<AudioDec>,
    size: (u32, u32),
    /// Pictures in B, G, R, A order.
    bgr: bool,
    /// The player's clock: its time, whether it runs, and when it was told (`Link::now_ms`).
    clock: (i64, bool, u64),
    /// Pictures decoded too late to show (not sent).
    pub late: u32,
    /// Budget (pictures, sound) in bytes and what is out unpaid.
    flow: Option<(u32, u32)>,
    out_video: u32,
    out_audio: u32,
    epoch: u32,
    /// Output before this time is not sent (a seek).
    target: i64,
    eos: bool,
    drained_sent: bool,
    /// The playback position the player last told.
    now: i64,
    /// Messages that did not fit in the player's queue yet.
    pending: VecDeque<(Msg, u8)>,
    pictures: u32,
    audio_packets: u32,
    pcm: Vec<i16>,
    quit: bool,
}

/// A message waiting to go: bytes, or a picture built as words (its pixels written in place).
enum Msg {
    Bytes(Vec<u8>),
    Words(Vec<u32>),
}

impl Msg {
    fn bytes(&self) -> &[u8] {
        match self {
            Msg::Bytes(b) => b,
            // SAFETY: `u32` has no padding and every bit pattern is a valid `u8`; the slice covers
            // exactly the vector's initialised words and borrows it, so it lives no longer.
            Msg::Words(w) => unsafe {
                core::slice::from_raw_parts(w.as_ptr().cast::<u8>(), w.len() * 4)
            },
        }
    }
}

/// What a pending message is, for the budget.
const KIND_OTHER: u8 = 0;
const KIND_VIDEO: u8 = 1;
const KIND_AUDIO: u8 = 2;

impl Default for Server {
    fn default() -> Self {
        Self::new()
    }
}

impl Server {
    pub fn new() -> Server {
        Server {
            sources: Vec::new(),
            video: None,
            audio: None,
            size: (0, 0),
            bgr: false,
            clock: (0, false, 0),
            late: 0,
            flow: None,
            out_video: 0,
            out_audio: 0,
            epoch: 0,
            target: i64::MIN,
            eos: false,
            drained_sent: false,
            now: 0,
            pending: VecDeque::new(),
            pictures: 0,
            audio_packets: 0,
            pcm: vec![0; OPUS_MAX_SAMPLES * 2],
            quit: false,
        }
    }

    /// Queue a message for the player.
    fn post(&mut self, msg: Vec<u8>, kind: u8) {
        self.pending.push_back((Msg::Bytes(msg), kind));
    }

    fn error(&mut self, code: u32, text: &str) {
        self.post(proto::error(code, text), KIND_OTHER);
    }

    /// Send what is queued; `false` when the link is gone.
    fn flush(&mut self, link: &mut dyn Link) -> bool {
        while let Some((m, kind)) = self.pending.front() {
            let m = m.bytes();
            match link.send(m) {
                Sent::Done => {
                    let n = m.len() as u32;
                    match *kind {
                        KIND_VIDEO => self.out_video = self.out_video.saturating_add(n),
                        KIND_AUDIO => self.out_audio = self.out_audio.saturating_add(n),
                        _ => {}
                    }
                    self.pending.pop_front();
                }
                Sent::Full => return true,
                Sent::Gone => return false,
            }
        }
        true
    }

    /// Handle one message from the player; `now_ms` is the link's clock.
    pub fn handle(&mut self, m: &[u8], now_ms: u64) {
        let Some((&kind, body)) = m.split_first() else {
            return self.error(proto::ERR_MESSAGE, "empty message");
        };
        let u8_at = |i: usize| body.get(i).copied();
        let u32_at = |i: usize| -> Option<u32> {
            Some(u32::from_le_bytes(body.get(i..i + 4)?.try_into().ok()?))
        };
        let i64_at = |i: usize| -> Option<i64> {
            Some(i64::from_le_bytes(body.get(i..i + 8)?.try_into().ok()?))
        };
        let src_at = |i: usize| u8_at(i).filter(|&s| (s as usize) < proto::MAX_SOURCES);
        let short = |s: &mut Server| s.error(proto::ERR_MESSAGE, "short message");
        match kind {
            proto::IN_DATA => match (src_at(0), u32_at(1)) {
                (Some(src), Some(seq)) => self.append(src as usize, seq, &body[5..]),
                _ => short(self),
            },
            proto::IN_SIZE => match (u32_at(0), u32_at(4)) {
                (Some(w), Some(h)) => {
                    self.size = (w, h);
                    self.bgr = u32_at(8) == Some(1);
                }
                _ => short(self),
            },
            proto::IN_EOS => {
                self.eos = u8_at(0) == Some(1);
                if !self.eos {
                    self.drained_sent = false;
                }
            }
            proto::IN_QUIT => self.quit = true,
            proto::IN_SEEK => match (u32_at(0), i64_at(4)) {
                (Some(epoch), Some(t)) => self.seek(epoch, t),
                _ => short(self),
            },
            proto::IN_REMOVE => match (src_at(0), i64_at(1), i64_at(9)) {
                (Some(src), Some(a), Some(b)) => {
                    if let Some(s) = self.sources.get_mut(src as usize) {
                        s.remove(a, b);
                    }
                    self.report(src as usize, 0, proto::BUF_OK);
                }
                _ => short(self),
            },
            proto::IN_RESET => match (src_at(0), i64_at(1)) {
                (Some(src), Some(off)) => {
                    let s = self.source(src as usize);
                    s.reset(off as u64);
                }
                _ => short(self),
            },
            proto::IN_TSOFF => match (src_at(0), i64_at(1)) {
                (Some(src), Some(ns)) => self.source(src as usize).ts_off = ns,
                _ => short(self),
            },
            proto::IN_TIME => {
                if let Some(t) = i64_at(0) {
                    self.now = t;
                    self.clock = (t, u8_at(8) == Some(1), now_ms);
                }
            }
            proto::IN_FLOW => match (u32_at(0), u32_at(4)) {
                (Some(v), Some(a)) => self.flow = (v > 0 || a > 0).then_some((v, a)),
                _ => short(self),
            },
            proto::IN_ACK => {
                if let (Some(v), Some(a)) = (u32_at(0), u32_at(4)) {
                    self.out_video = self.out_video.saturating_sub(v);
                    self.out_audio = self.out_audio.saturating_sub(a);
                }
            }
            proto::IN_CUE => match (src_at(0), i64_at(1)) {
                (Some(src), Some(t)) => {
                    let off = self
                        .sources
                        .get(src as usize)
                        .and_then(|s| s.demux.cluster_for(t.max(0) as u64))
                        .unwrap_or(u64::MAX);
                    let m = proto::msg(
                        proto::OUT_CUE,
                        &[Field::U8(src), Field::I64(t), Field::U64(off)],
                    );
                    self.post(m, KIND_OTHER);
                }
                _ => short(self),
            },
            _ => self.error(proto::ERR_MESSAGE, "unknown message"),
        }
    }

    /// The source buffer `src`, made on first use.
    fn source(&mut self, src: usize) -> &mut Source {
        while self.sources.len() <= src {
            self.sources.push(Source::new());
        }
        &mut self.sources[src]
    }

    /// Play from `t`: the decoders start over there (the coded frames stay), and [`proto::OUT_MARK`]
    /// tells the player where the new output begins.
    fn seek(&mut self, epoch: u32, t: i64) {
        self.epoch = epoch;
        self.target = t;
        self.drained_sent = false;
        // What was waiting for the player belongs to the old place.
        self.pending.retain(|(_, k)| *k == KIND_OTHER);
        if let Some(v) = self.video.as_mut() {
            v.cursor = Cursor::seek(t);
        }
        if let Some(a) = self.audio.as_mut() {
            a.cursor = Cursor::seek(t.saturating_sub(PRE_ROLL_NS));
            a.dec.reset();
            a.skip = 0;
        }
        self.post(
            proto::msg(proto::OUT_MARK, &[Field::U32(epoch)]),
            KIND_OTHER,
        );
    }

    /// Tell the player what source `src` holds (after append `seq`).
    fn report(&mut self, src: usize, seq: u32, status: u8) {
        let ranges: Vec<(i64, i64)> = self
            .sources
            .get(src)
            .map(|s| s.buffered().iter().collect())
            .unwrap_or_default();
        self.post(
            proto::buffered(src as u8, seq, status, &ranges),
            KIND_OTHER,
        );
    }

    /// Total coded bytes held.
    fn held(&self) -> usize {
        self.sources.iter().map(Source::bytes).sum()
    }

    /// Forget what was played long ago (never what a decoder still needs).
    fn evict(&mut self) {
        let mut t = self.now - KEEP_BEHIND_NS;
        if let Some(v) = &self.video {
            t = t.min(v.cursor.last.map_or(i64::MIN, |l| l.0));
        }
        if let Some(a) = &self.audio {
            t = t.min(a.cursor.last.map_or(i64::MIN, |l| l.0));
        }
        if t == i64::MIN {
            return;
        }
        for s in &mut self.sources {
            s.evict_before(t);
        }
    }
}

/// Run the server on `link` until it is told to stop or the link goes.
pub fn serve(link: &mut dyn Link) {
    let mut server = Server::new();
    let mut buf = vec![0u8; MAX_MSG];
    loop {
        if !server.flush(link) {
            return;
        }
        // Work to do: only look at the mail; the player's queue full: wait a little for it (or an
        // acknowledgement); nothing to do: wait for the next message.
        let timeout = if server.can_work() {
            0
        } else if !server.pending.is_empty() {
            4
        } else {
            u64::MAX
        };
        match link.recv(&mut buf, timeout) {
            Err(()) => return,
            Ok(Some(n)) => {
                let now = link.now_ms();
                server.handle(&buf[..n.min(buf.len())], now);
                if server.quit {
                    return;
                }
                continue;
            }
            Ok(None) => {}
        }
        server.work(link.now_ms());
    }
}

/// The largest message (calls 56/57 carry at most 4 MiB).
pub const MAX_MSG: usize = 4 * 1024 * 1024;

fn text(e: impl core::fmt::Display) -> String {
    let mut s = String::new();
    let _ = write!(s, "{e}");
    s
}
