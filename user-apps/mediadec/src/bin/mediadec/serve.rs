//! The message server (`mediadec --serve`): a player (the browser, later) sends WebM bytes and gets
//! back RGBA pictures and PCM. Each message is one `page_recv`/`page_send` (calls 56/57, the same
//! link as `jsrun --page`); the first byte says what it is, numbers are little endian. The protocol
//! is in `docs/design/midia.md`.
//!
//! Shared by `mediadec` and the host checker; [`Loopback`] and [`selftest`] run it in memory.

use super::pipeline::{Fatal, Media, Out};
use alloc::collections::VecDeque;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;
use core::fmt::Write as _;

/// To the server: WebM bytes (init segment, clusters...), in file order.
pub const IN_DATA: u8 = 0x01;
/// `u64` time (ns): answered with [`OUT_SEEKPOS`], the file offset of the cluster to send next
/// (from the cues; `u64::MAX` when there are none); output before that time is dropped.
pub const IN_SEEK: u8 = 0x02;
/// `u64` file offset, `i64` time (ns): the next bytes start at that offset (a cluster the player
/// found itself); output before the time is dropped.
pub const IN_RESTART: u8 = 0x03;
/// `u32` width, `u32` height: the size of the pictures sent (0, 0: the video's own size). At most
/// [`MAX_W`] x [`MAX_H`].
pub const IN_SIZE: u8 = 0x04;
/// No more data for now: everything decodable is sent, then [`OUT_END`].
pub const IN_END: u8 = 0x05;
/// Stop the server.
pub const IN_QUIT: u8 = 0x06;

/// `u64` duration (ns, 0 unknown), `u32` width, `u32` height, `u8` has video, `u8` has audio, `u8`
/// channels of the stream, `u8` 0, `u32` output sample rate (48000; the PCM is always stereo).
pub const OUT_HEADER: u8 = 0x81;
/// `i64` time (ns), `u32` width, `u32` height, then `width * height` pixels, 4 bytes each (R, G, B, 255).
pub const OUT_FRAME: u8 = 0x82;
/// `i64` time (ns), `u32` samples per channel, then the samples: s16 little endian, stereo
/// interleaved, 48 kHz.
pub const OUT_AUDIO: u8 = 0x83;
/// `u64` file offset (`u64::MAX`: no index).
pub const OUT_SEEKPOS: u8 = 0x84;
/// `u32` pictures, `u32` audio packets sent since the start.
pub const OUT_END: u8 = 0x85;
/// `u32` code ([`ERR_DEMUX`]...), then a UTF-8 text.
pub const OUT_ERROR: u8 = 0x8F;

/// The stream is not WebM or is broken: send [`IN_SEEK`]/[`IN_RESTART`] to go on.
pub const ERR_DEMUX: u32 = 1;
/// A video packet did not decode (the next keyframe recovers).
pub const ERR_VIDEO: u32 = 2;
/// An audio packet did not decode.
pub const ERR_AUDIO: u32 = 3;
/// A message the server does not know, or too short.
pub const ERR_MESSAGE: u32 = 4;
/// A decoder could not be made (memory).
pub const ERR_MEMORY: u32 = 5;

/// The largest picture sent (a message holds at most 4 MiB).
pub const MAX_W: u32 = 1280;
pub const MAX_H: u32 = 800;

/// The two directions of messages.
pub trait Link {
    /// Wait for the next message and put it in `buf`; its length, or `None` when the link is gone.
    fn recv(&mut self, buf: &mut [u8]) -> Option<usize>;
    /// Send one message; `false` when the other side is gone.
    fn send(&mut self, msg: &[u8]) -> bool;
}

/// The server's state.
pub struct Server {
    media: Media,
    size: (u32, u32),
    rgba: Vec<u32>,
    msg: Vec<u8>,
    pictures: u32,
    audio_packets: u32,
}

impl Default for Server {
    fn default() -> Self {
        Self::new()
    }
}

impl Server {
    pub fn new() -> Server {
        Server {
            media: Media::new(),
            size: (0, 0),
            rgba: Vec::new(),
            msg: Vec::new(),
            pictures: 0,
            audio_packets: 0,
        }
    }

    /// Handle one message; `false` to stop.
    pub fn handle(&mut self, m: &[u8], link: &mut dyn Link) -> bool {
        let Some((&kind, body)) = m.split_first() else {
            return error(link, ERR_MESSAGE, "empty message");
        };
        let u64_at = |i: usize| -> Option<u64> {
            Some(u64::from_le_bytes(body.get(i..i + 8)?.try_into().ok()?))
        };
        let u32_at = |i: usize| -> Option<u32> {
            Some(u32::from_le_bytes(body.get(i..i + 4)?.try_into().ok()?))
        };
        match kind {
            IN_DATA => match self.media.push(body) {
                Ok(()) => self.drain(link),
                Err(e) => error(link, ERR_DEMUX, &fatal_text(e)),
            },
            IN_SEEK => {
                let Some(t) = u64_at(0) else {
                    return error(link, ERR_MESSAGE, "short seek");
                };
                let off = self.media.seek(t).unwrap_or(u64::MAX);
                let mut out = vec![OUT_SEEKPOS];
                out.extend_from_slice(&off.to_le_bytes());
                link.send(&out)
            }
            IN_RESTART => {
                let (Some(off), Some(t)) = (u64_at(0), u64_at(8)) else {
                    return error(link, ERR_MESSAGE, "short restart");
                };
                self.media.restart_at(off, t as i64);
                true
            }
            IN_SIZE => {
                let (Some(w), Some(h)) = (u32_at(0), u32_at(4)) else {
                    return error(link, ERR_MESSAGE, "short size");
                };
                self.size = (w.min(MAX_W), h.min(MAX_H));
                true
            }
            IN_END => {
                if !self.drain(link) {
                    return false;
                }
                let mut out = vec![OUT_END];
                out.extend_from_slice(&self.pictures.to_le_bytes());
                out.extend_from_slice(&self.audio_packets.to_le_bytes());
                link.send(&out)
            }
            IN_QUIT => false,
            _ => error(link, ERR_MESSAGE, "unknown message"),
        }
    }

    /// Send everything the bytes so far decode to.
    fn drain(&mut self, link: &mut dyn Link) -> bool {
        loop {
            let out = match self.media.poll() {
                Ok(Some(o)) => o,
                Ok(None) => return true,
                Err(e @ Fatal::NoDecoder) => return error(link, ERR_MEMORY, &fatal_text(e)),
                Err(e) => return error(link, ERR_DEMUX, &fatal_text(e)),
            };
            self.msg.clear();
            match out {
                Out::Header => {
                    let s = self.media.summary();
                    self.msg.push(OUT_HEADER);
                    self.msg.extend_from_slice(&s.duration_ns.to_le_bytes());
                    self.msg.extend_from_slice(&s.width.to_le_bytes());
                    self.msg.extend_from_slice(&s.height.to_le_bytes());
                    self.msg.push(u8::from(s.video_track.is_some()));
                    self.msg.push(u8::from(s.audio_track.is_some()));
                    self.msg.push(s.channels);
                    self.msg.push(0);
                    self.msg.extend_from_slice(&48_000u32.to_le_bytes());
                }
                Out::Picture { ts_ns, picture } => {
                    let (w, h) = fit(self.size, picture.width(), picture.height());
                    let n = (w * h) as usize;
                    if self.rgba.len() < n {
                        self.rgba.resize(n, 0);
                    }
                    picture.to_rgba(&mut self.rgba, w, h);
                    self.msg.reserve(24 + n * 4);
                    self.msg.push(OUT_FRAME);
                    self.msg.extend_from_slice(&ts_ns.to_le_bytes());
                    self.msg.extend_from_slice(&w.to_le_bytes());
                    self.msg.extend_from_slice(&h.to_le_bytes());
                    for px in &self.rgba[..n] {
                        self.msg.extend_from_slice(&px.to_le_bytes());
                    }
                    self.pictures += 1;
                }
                Out::Audio { ts_ns, pcm } => {
                    self.msg.push(OUT_AUDIO);
                    self.msg.extend_from_slice(&ts_ns.to_le_bytes());
                    self.msg
                        .extend_from_slice(&((pcm.len() / 2) as u32).to_le_bytes());
                    for s in pcm {
                        self.msg.extend_from_slice(&s.to_le_bytes());
                    }
                    self.audio_packets += 1;
                }
                Out::DecodeError { audio, code } => {
                    let kind = if audio { ERR_AUDIO } else { ERR_VIDEO };
                    let mut text = String::new();
                    let _ = write!(text, "decoder error {code}");
                    if !error(link, kind, &text) {
                        return false;
                    }
                    continue;
                }
            }
            if !link.send(&self.msg) {
                return false;
            }
        }
    }
}

/// The picture size to send: the asked size, or the video's own scaled down to fit the limits.
fn fit(asked: (u32, u32), w: u32, h: u32) -> (u32, u32) {
    if asked.0 > 0 && asked.1 > 0 {
        return asked;
    }
    if w <= MAX_W && h <= MAX_H {
        return (w, h);
    }
    // Keep the aspect ratio: the larger reduction wins.
    let (sw, sh) = (u64::from(w), u64::from(h));
    if sw * u64::from(MAX_H) > sh * u64::from(MAX_W) {
        (MAX_W, ((sh * u64::from(MAX_W) / sw) as u32).max(1))
    } else {
        (((sw * u64::from(MAX_H) / sh) as u32).max(1), MAX_H)
    }
}

fn fatal_text(e: Fatal) -> String {
    let mut s = String::new();
    match e {
        Fatal::Demux(d) => {
            let _ = write!(s, "{d}");
        }
        Fatal::NoDecoder => s.push_str("cannot create a decoder"),
    }
    s
}

/// Send an [`OUT_ERROR`]; `false` when the link is gone.
fn error(link: &mut dyn Link, code: u32, text: &str) -> bool {
    let mut out = vec![OUT_ERROR];
    out.extend_from_slice(&code.to_le_bytes());
    out.extend_from_slice(text.as_bytes());
    link.send(&out)
}

/// Run the server on `link` until it is told to stop or the link goes.
pub fn serve(link: &mut dyn Link) {
    let mut server = Server::new();
    let mut buf = vec![0u8; MAX_MSG];
    while let Some(n) = link.recv(&mut buf) {
        if !server.handle(&buf[..n.min(buf.len())], link) {
            return;
        }
    }
}

/// The largest message (calls 56/57 carry at most 4 MiB).
pub const MAX_MSG: usize = 4 * 1024 * 1024;

/// An in-memory link: what is in `inbox` comes in, what the server sends lands in `outbox`.
#[derive(Default)]
pub struct Loopback {
    pub inbox: VecDeque<Vec<u8>>,
    pub outbox: Vec<Vec<u8>>,
}

impl Link for Loopback {
    fn recv(&mut self, buf: &mut [u8]) -> Option<usize> {
        let m = self.inbox.pop_front()?;
        let n = m.len().min(buf.len());
        buf[..n].copy_from_slice(&m[..n]);
        Some(n)
    }

    fn send(&mut self, msg: &[u8]) -> bool {
        self.outbox.push(msg.to_vec());
        true
    }
}

/// What [`selftest`] saw.
#[derive(Debug, Default)]
pub struct Report {
    pub header: bool,
    pub pictures: u32,
    pub picture_size: (u32, u32),
    pub samples: u64,
    pub end_pictures: u32,
    pub seek_offset: u64,
    pub after_seek_pictures: u32,
    pub first_after_seek_ns: i64,
    pub errors: u32,
}

/// Drive the server in memory over the WebM file `file`: pictures of 160x90, the file in 32 KiB
/// messages, the end, then a seek to 1 s (when the file has cues) and the rest of the file again.
pub fn selftest(file: &[u8]) -> Report {
    let mut r = Report::default();
    let mut link = Loopback::default();
    let mut server = Server::new();
    let mut size = vec![IN_SIZE];
    size.extend_from_slice(&160u32.to_le_bytes());
    size.extend_from_slice(&90u32.to_le_bytes());
    link.inbox.push_back(size);
    for c in file.chunks(32 * 1024) {
        let mut m = vec![IN_DATA];
        m.extend_from_slice(c);
        link.inbox.push_back(m);
    }
    link.inbox.push_back(vec![IN_END]);
    let mut seek = vec![IN_SEEK];
    seek.extend_from_slice(&1_000_000_000u64.to_le_bytes());
    link.inbox.push_back(seek);
    let mut seen_end = false;
    let mut buf = vec![0u8; MAX_MSG];
    while let Some(n) = link.recv(&mut buf) {
        if !server.handle(&buf[..n], &mut link) {
            break;
        }
        for m in core::mem::take(&mut link.outbox) {
            match m.first() {
                Some(&OUT_HEADER) => r.header = true,
                Some(&OUT_FRAME) if m.len() >= 17 => {
                    let ts = i64::from_le_bytes(m[1..9].try_into().unwrap_or([0; 8]));
                    let w = u32::from_le_bytes(m[9..13].try_into().unwrap_or([0; 4]));
                    let h = u32::from_le_bytes(m[13..17].try_into().unwrap_or([0; 4]));
                    if (m.len() - 17) as u64 != u64::from(w) * u64::from(h) * 4 {
                        r.errors += 1;
                    }
                    if seen_end {
                        if r.after_seek_pictures == 0 {
                            r.first_after_seek_ns = ts;
                        }
                        r.after_seek_pictures += 1;
                    } else {
                        r.pictures += 1;
                        r.picture_size = (w, h);
                    }
                }
                Some(&OUT_AUDIO) if m.len() >= 13 && !seen_end => {
                    r.samples += u64::from(u32::from_le_bytes(m[9..13].try_into().unwrap_or([0; 4])));
                }
                Some(&OUT_END) if m.len() >= 9 && !seen_end => {
                    r.end_pictures = u32::from_le_bytes(m[1..5].try_into().unwrap_or([0; 4]));
                    seen_end = true;
                }
                Some(&OUT_SEEKPOS) if m.len() >= 9 => {
                    r.seek_offset = u64::from_le_bytes(m[1..9].try_into().unwrap_or([0; 8]));
                    if let Some(rest) = usize::try_from(r.seek_offset)
                        .ok()
                        .and_then(|o| file.get(o..))
                    {
                        for c in rest.chunks(32 * 1024) {
                            let mut d = vec![IN_DATA];
                            d.extend_from_slice(c);
                            link.inbox.push_back(d);
                        }
                        link.inbox.push_back(vec![IN_END]);
                    }
                    link.inbox.push_back(vec![IN_QUIT]);
                }
                Some(&OUT_ERROR) => r.errors += 1,
                _ => {}
            }
        }
    }
    r
}
