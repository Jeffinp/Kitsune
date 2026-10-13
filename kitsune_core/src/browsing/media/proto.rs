//! The messages between a player (the browser) and its decoder process (`mediadec --serve`), over
//! the same link as a page's script process (calls 56/57). One message per call; the first byte says
//! what it is; numbers are little endian. The table is in `docs/design/midia.md`.
//!
//! A `src` that is a URL is fetched by the browser's fetcher straight into the decoder: the answer
//! comes as the fetcher writes it for a script (`web::script::net`: the head and the end as JSON,
//! [`FETCH_DATA`] pieces), the bytes go to source 0, and the decoder tells the player how the
//! request went ([`OUT_FETCH`]). The page's script never sees those bytes (a media element reads
//! other origins without CORS, and only the decoder may read them).
//!
//! The decoder holds the coded frames (one [`super::track::TrackBuffer`] per track of each source
//! buffer), decodes ahead of the player within the budget the player gives ([`IN_FLOW`], returned
//! with [`IN_ACK`]) and sends pictures and sound; the player shows and plays them on its clock.

use alloc::vec::Vec;

/// `u8` source, `u32` sequence number, then WebM bytes for that source (init segment, clusters...).
/// Answered with [`OUT_BUFFERED`] carrying the same sequence number.
pub const IN_DATA: u8 = 0x10;
/// The fetcher's body piece of a request made for the decoder (`web::script::DATA_TAG`): `u32`
/// request id, then bytes, appended to source 0. A message starting with `{` is the fetcher's JSON
/// (the head, the end or an error of the request; see [`fetch_event`]).
pub const FETCH_DATA: u8 = 0x01;
/// `u32` width, `u32` height of the pictures sent (0, 0: the video's own size, within the limits),
/// and optionally `u32` 1 for pictures in B, G, R, A byte order (a framebuffer's).
pub const IN_SIZE: u8 = 0x04;
/// `u8` 1: no more data will come (the end of the stream: [`OUT_DRAINED`] once all is sent); 0: more
/// data comes after all.
pub const IN_EOS: u8 = 0x05;
/// Stop the server.
pub const IN_QUIT: u8 = 0x06;
/// `u32` epoch, `i64` time (ns): play from that time. Answered with [`OUT_MARK`] before anything of
/// the new position; output before the time is not sent.
pub const IN_SEEK: u8 = 0x07;
/// `u8` source, `i64` start, `i64` end: `SourceBuffer.remove`. Answered with [`OUT_BUFFERED`]
/// (sequence 0).
pub const IN_REMOVE: u8 = 0x08;
/// `u8` source, `u64` file offset: the next bytes of the source start at that offset (a cluster a
/// player found, or 0 for the file from its start; `SourceBuffer.abort` uses the current offset).
/// What was half read is dropped; the coded frames stay.
pub const IN_RESET: u8 = 0x0A;
/// `u8` source, `i64` ns added to the source's timestamps (`SourceBuffer.timestampOffset`).
pub const IN_TSOFF: u8 = 0x0B;
/// `i64` the playback position (ns), and optionally `u8` 1 when the clock runs: what was played long
/// ago may be forgotten, and a picture already late when decoded is not sent.
pub const IN_TIME: u8 = 0x0C;
/// `u32` bytes of pictures, `u32` bytes of sound the player may have waiting at most (0: no limit).
pub const IN_FLOW: u8 = 0x0D;
/// `u32` bytes of pictures, `u32` bytes of sound the player is done with.
pub const IN_ACK: u8 = 0x0E;
/// `u8` source, `i64` time: where in the file is the cluster for that time ([`OUT_CUE`]).
pub const IN_CUE: u8 = 0x0F;

/// `u64` duration (ns, 0 unknown), `u32` width, `u32` height, `u8` has video, `u8` has audio, `u8`
/// channels of the stream, `u8` source, `u32` output rate (48000; the sound is always stereo).
pub const OUT_HEADER: u8 = 0x81;
/// `u8` 1 for B, G, R, A pixels (else R, G, B, A), `u16` pictures dropped since the last one sent
/// (decoded too late to show; the pixels start 4-aligned, at [`FRAME_HEAD`]), `i64` time (ns), `u32`
/// width, `u32` height, then `width * height` pixels, 4 bytes each.
pub const OUT_FRAME: u8 = 0x82;
/// Where the pixels of an [`OUT_FRAME`] start.
pub const FRAME_HEAD: usize = 20;
/// `i64` time (ns), `u32` samples per channel, then the samples: s16 little endian, stereo
/// interleaved, 48 kHz.
pub const OUT_AUDIO: u8 = 0x83;
/// `u8` source, `u32` sequence, `u8` status ([`BUF_OK`]...), `u32` count, then that many `i64`
/// start, `i64` end pairs: what the source buffer holds.
pub const OUT_BUFFERED: u8 = 0x84;
/// `u32` epoch, `u32` pictures, `u32` sound packets sent: everything up to the end of the stream
/// was sent.
pub const OUT_DRAINED: u8 = 0x85;
/// `u32` epoch: what follows belongs to the seek of that epoch.
pub const OUT_MARK: u8 = 0x86;
/// `u8` source, `i64` time, `u64` file offset of the cluster (`u64::MAX`: no index).
pub const OUT_CUE: u8 = 0x87;
/// `u32` request id, `u8` what happened ([`FETCH_HEAD`], [`FETCH_END`], [`FETCH_FAIL`]), `u32` HTTP
/// status, `u64` the file's size (`u64::MAX`: unknown), `u64` bytes of the answer so far.
pub const OUT_FETCH: u8 = 0x88;
/// `u32` code ([`ERR_DEMUX`]...), then a UTF-8 text.
pub const OUT_ERROR: u8 = 0x8F;

/// [`OUT_FETCH`]: the head arrived.
pub const FETCH_HEAD: u8 = 1;
/// The whole answer arrived.
pub const FETCH_END: u8 = 2;
/// The request failed.
pub const FETCH_FAIL: u8 = 3;

/// [`OUT_BUFFERED`] status: the bytes were taken.
pub const BUF_OK: u8 = 0;
/// The buffer is full even after forgetting what was played (`QuotaExceededError`).
pub const BUF_FULL: u8 = 1;
/// The bytes are not WebM, or are broken (the source stops until reset).
pub const BUF_BROKEN: u8 = 2;

/// The stream is not WebM or is broken.
pub const ERR_DEMUX: u32 = 1;
/// A video packet did not decode (the next keyframe recovers).
pub const ERR_VIDEO: u32 = 2;
/// An audio packet did not decode.
pub const ERR_AUDIO: u32 = 3;
/// A message the server does not know, or too short.
pub const ERR_MESSAGE: u32 = 4;
/// A decoder could not be made (memory).
pub const ERR_MEMORY: u32 = 5;
/// No codec this player can decode (not VP9 or Opus).
pub const ERR_CODEC: u32 = 6;

/// The largest picture sent (a message holds at most 4 MiB).
pub const MAX_W: u32 = 1280;
pub const MAX_H: u32 = 800;
/// Source buffers per player.
pub const MAX_SOURCES: usize = 4;

/// A message from the decoder, read ([`parse_out`]).
#[derive(Debug, PartialEq, Eq)]
pub enum Out<'a> {
    Header(Header),
    Frame {
        pts: i64,
        w: u32,
        h: u32,
        /// The pixels are B, G, R, A.
        bgr: bool,
        /// Pictures the decoder dropped (too late) since the previous one.
        skipped: u16,
        rgba: &'a [u8],
    },
    /// Stereo 48 kHz s16le samples (`frames` per channel).
    Audio {
        pts: i64,
        frames: u32,
        bytes: &'a [u8],
    },
    Buffered {
        src: u8,
        seq: u32,
        status: u8,
        ranges: Vec<(i64, i64)>,
    },
    Drained {
        epoch: u32,
    },
    Mark {
        epoch: u32,
    },
    Cue {
        src: u8,
        t: i64,
        offset: u64,
    },
    Error {
        code: u32,
        text: &'a str,
    },
    Fetch {
        id: u32,
        ev: u8,
        status: u32,
        total: Option<u64>,
        bytes: u64,
    },
}

/// What [`OUT_HEADER`] says.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Header {
    pub duration_ns: u64,
    pub width: u32,
    pub height: u32,
    pub video: bool,
    pub audio: bool,
    pub channels: u8,
    pub src: u8,
}

fn u32_at(b: &[u8], i: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(i..i + 4)?.try_into().ok()?))
}

fn u64_at(b: &[u8], i: usize) -> Option<u64> {
    Some(u64::from_le_bytes(b.get(i..i + 8)?.try_into().ok()?))
}

fn i64_at(b: &[u8], i: usize) -> Option<i64> {
    u64_at(b, i).map(|v| v as i64)
}

/// Read a message from the decoder. `None`: unknown or malformed (the process is not trusted).
pub fn parse_out(m: &[u8]) -> Option<Out<'_>> {
    let (&kind, b) = m.split_first()?;
    Some(match kind {
        OUT_HEADER => Out::Header(Header {
            duration_ns: u64_at(b, 0)?,
            width: u32_at(b, 8)?,
            height: u32_at(b, 12)?,
            video: *b.get(16)? != 0,
            audio: *b.get(17)? != 0,
            channels: *b.get(18)?,
            src: *b.get(19)?,
        }),
        OUT_FRAME => {
            let (w, h) = (u32_at(b, 11)?, u32_at(b, 15)?);
            let n = (w as usize).checked_mul(h as usize)?.checked_mul(4)?;
            if w == 0 || h == 0 || w > MAX_W || h > MAX_H || m.len() - FRAME_HEAD != n {
                return None;
            }
            Out::Frame {
                pts: i64_at(b, 3)?,
                w,
                h,
                bgr: b[0] == 1,
                skipped: u16::from_le_bytes([b[1], b[2]]),
                rgba: &m[FRAME_HEAD..],
            }
        }
        OUT_AUDIO => {
            let frames = u32_at(b, 8)?;
            let bytes = b.get(12..)?;
            if bytes.len() != frames as usize * 4 {
                return None;
            }
            Out::Audio {
                pts: i64_at(b, 0)?,
                frames,
                bytes,
            }
        }
        OUT_BUFFERED => {
            let src = *b.first()?;
            let seq = u32_at(b, 1)?;
            let status = *b.get(5)?;
            let n = u32_at(b, 6)? as usize;
            if n > 4096 || b.len() != 10 + n * 16 {
                return None;
            }
            let ranges = (0..n)
                .filter_map(|i| Some((i64_at(b, 10 + i * 16)?, i64_at(b, 18 + i * 16)?)))
                .collect();
            Out::Buffered {
                src,
                seq,
                status,
                ranges,
            }
        }
        OUT_DRAINED => Out::Drained {
            epoch: u32_at(b, 0)?,
        },
        OUT_MARK => Out::Mark {
            epoch: u32_at(b, 0)?,
        },
        OUT_CUE => Out::Cue {
            src: *b.first()?,
            t: i64_at(b, 1)?,
            offset: u64_at(b, 9)?,
        },
        OUT_FETCH => Out::Fetch {
            id: u32_at(b, 0)?,
            ev: *b.get(4)?,
            status: u32_at(b, 5)?,
            total: u64_at(b, 9).filter(|&t| t != u64::MAX),
            bytes: u64_at(b, 17)?,
        },
        OUT_ERROR => Out::Error {
            code: u32_at(b, 0)?,
            text: core::str::from_utf8(b.get(4..)?).unwrap_or("?"),
        },
        _ => return None,
    })
}

// ---- building messages (both sides) ----

/// [`IN_DATA`] for `bytes`.
pub fn data(src: u8, seq: u32, bytes: &[u8]) -> Vec<u8> {
    let mut m = Vec::with_capacity(6 + bytes.len());
    m.push(IN_DATA);
    m.push(src);
    m.extend_from_slice(&seq.to_le_bytes());
    m.extend_from_slice(bytes);
    m
}

/// A message of `kind` followed by the given numbers.
pub fn msg(kind: u8, fields: &[Field]) -> Vec<u8> {
    let mut m = Vec::with_capacity(1 + fields.len() * 8);
    m.push(kind);
    for f in fields {
        match *f {
            Field::U8(v) => m.push(v),
            Field::U32(v) => m.extend_from_slice(&v.to_le_bytes()),
            Field::I64(v) => m.extend_from_slice(&v.to_le_bytes()),
            Field::U64(v) => m.extend_from_slice(&v.to_le_bytes()),
        }
    }
    m
}

/// One number of a message.
#[derive(Clone, Copy, Debug)]
pub enum Field {
    U8(u8),
    U32(u32),
    I64(i64),
    U64(u64),
}

/// [`OUT_BUFFERED`].
pub fn buffered(src: u8, seq: u32, status: u8, ranges: &[(i64, i64)]) -> Vec<u8> {
    let n = ranges.len().min(4096);
    let mut m = Vec::with_capacity(11 + n * 16);
    m.push(OUT_BUFFERED);
    m.push(src);
    m.extend_from_slice(&seq.to_le_bytes());
    m.push(status);
    m.extend_from_slice(&(n as u32).to_le_bytes());
    for &(a, b) in &ranges[..n] {
        m.extend_from_slice(&a.to_le_bytes());
        m.extend_from_slice(&b.to_le_bytes());
    }
    m
}

/// [`OUT_HEADER`].
pub fn header(h: &Header) -> Vec<u8> {
    let mut m = Vec::with_capacity(25);
    m.push(OUT_HEADER);
    m.extend_from_slice(&h.duration_ns.to_le_bytes());
    m.extend_from_slice(&h.width.to_le_bytes());
    m.extend_from_slice(&h.height.to_le_bytes());
    m.push(u8::from(h.video));
    m.push(u8::from(h.audio));
    m.push(h.channels);
    m.push(h.src);
    m.extend_from_slice(&48_000u32.to_le_bytes());
    m
}

/// [`OUT_ERROR`].
pub fn error(code: u32, text: &str) -> Vec<u8> {
    let mut m = Vec::with_capacity(5 + text.len());
    m.push(OUT_ERROR);
    m.extend_from_slice(&code.to_le_bytes());
    m.extend_from_slice(text.as_bytes());
    m
}

/// The head of an [`OUT_FRAME`] ([`FRAME_HEAD`] bytes; the pixels follow).
pub fn frame_head(out: &mut Vec<u8>, pts: i64, w: u32, h: u32, bgr: bool, skipped: u16) {
    out.push(OUT_FRAME);
    out.push(u8::from(bgr));
    out.extend_from_slice(&skipped.to_le_bytes());
    out.extend_from_slice(&pts.to_le_bytes());
    out.extend_from_slice(&w.to_le_bytes());
    out.extend_from_slice(&h.to_le_bytes());
}

/// The head of an [`OUT_AUDIO`] (the samples follow).
pub fn audio_head(out: &mut Vec<u8>, pts: i64, frames: u32) {
    out.push(OUT_AUDIO);
    out.extend_from_slice(&pts.to_le_bytes());
    out.extend_from_slice(&frames.to_le_bytes());
}

/// The size of the pictures to send for a video of `w`x`h` when `asked` (0, 0: its own size) is
/// the player's wish: within [`MAX_W`]x[`MAX_H`], the aspect ratio kept when scaling down.
pub fn fit(asked: (u32, u32), w: u32, h: u32) -> (u32, u32) {
    if asked.0 > 0 && asked.1 > 0 {
        return (asked.0.min(MAX_W), asked.1.min(MAX_H));
    }
    if w <= MAX_W && h <= MAX_H {
        return (w.max(1), h.max(1));
    }
    let (sw, sh) = (u64::from(w), u64::from(h));
    if sw * u64::from(MAX_H) > sh * u64::from(MAX_W) {
        (MAX_W, ((sh * u64::from(MAX_W) / sw) as u32).max(1))
    } else {
        (((sw * u64::from(MAX_H) / sh) as u32).max(1), MAX_H)
    }
}

/// What a JSON message of the fetcher says about a request: its id, what happened
/// ([`FETCH_HEAD`], [`FETCH_END`], [`FETCH_FAIL`]), the status, and the file's size (from
/// `Content-Range`, else `Content-Length`). `None` for anything else.
pub fn fetch_event(json: &[u8]) -> Option<(u32, u8, u32, Option<u64>)> {
    let text = core::str::from_utf8(json).ok()?;
    if !text.starts_with("{\"t\":\"fetch\"") {
        return None;
    }
    let number = |key: &str| -> Option<u64> {
        let at = text.find(key)? + key.len();
        let digits: &str = &text[at..];
        let end = digits
            .find(|c: char| !c.is_ascii_digit())
            .unwrap_or(digits.len());
        digits[..end].parse().ok()
    };
    let id = u32::try_from(number("\"id\":")?).ok()?;
    let ev = if text.contains("\"ev\":\"head\"") {
        FETCH_HEAD
    } else if text.contains("\"ev\":\"end\"") {
        FETCH_END
    } else if text.contains("\"ev\":\"error\"") {
        FETCH_FAIL
    } else {
        return None;
    };
    let status = number("\"status\":").map_or(0, |s| s.min(999) as u32);
    let lower = text.to_ascii_lowercase();
    let header = |name: &str| -> Option<&str> {
        let key = alloc::format!("[\"{name}\",\"");
        let at = lower.find(&key)? + key.len();
        let end = text[at..].find('"')? + at;
        Some(&text[at..end])
    };
    let total = header("content-range")
        .and_then(|v| v.rsplit('/').next())
        .and_then(|t| t.trim().parse().ok())
        .or_else(|| {
            if status == 200 {
                header("content-length").and_then(|v| v.trim().parse().ok())
            } else {
                None
            }
        });
    Some((id, ev, status, total))
}
